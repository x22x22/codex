package com.openai.codex.genie

import android.app.agent.GenieService
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.util.Log
import com.openai.codex.bridge.FrameworkSessionTransportCompat
import com.openai.codex.bridge.SessionExecutionSettings
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.Closeable
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.IOException
import java.nio.charset.StandardCharsets
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import org.json.JSONObject

class AgentBridgeClient(
    callback: GenieService.Callback,
    private val sessionId: String,
) : Closeable {
    companion object {
        private const val TAG = "AgentBridgeClient"
        private const val OP_GET_RUNTIME_STATUS = "getRuntimeStatus"
        private const val OP_READ_INSTALLED_AGENTS_FILE = "readInstalledAgentsFile"
        private const val OP_READ_SESSION_EXECUTION_SETTINGS = "readSessionExecutionSettings"
        private const val OP_REGISTER_APP_SERVER_THREAD = "registerAppServerThread"
        private const val WRITE_CHUNK_BYTES = 4096
        private const val RESPONSES_METHOD = "POST"
        private const val DEFAULT_RESPONSES_PATH = "/responses"
        private const val HEADER_CONTENT_TYPE = "Content-Type"
        private const val HEADER_ACCEPT = "Accept"
        private const val HEADER_ACCEPT_ENCODING = "Accept-Encoding"
        private const val HEADER_VALUE_APPLICATION_JSON = "application/json"
        private const val HEADER_VALUE_TEXT_EVENT_STREAM = "text/event-stream"
        private const val HEADER_VALUE_IDENTITY = "identity"
        private const val BRIDGE_REQUEST_TIMEOUT_MS = 30_000L
        private const val KIND_REQUEST = "request"
        private const val KIND_RESPONSE = "response"
        private const val KIND_REMOTE_CLIENT_MESSAGE = "remoteAppServerClientMessage"
        private const val KIND_REMOTE_SERVER_MESSAGE = "remoteAppServerServerMessage"
        private const val KIND_REMOTE_CLOSED = "remoteAppServerClosed"
    }

    interface AppServerProxyHandler {
        fun onMessage(message: String)

        fun onClosed(reason: String?)
    }

    private val frameworkCallback = callback
    private val bridgeFd: ParcelFileDescriptor = callback.openSessionBridge(sessionId)
    private val input = DataInputStream(BufferedInputStream(FileInputStream(bridgeFd.fileDescriptor)))
    private val output = DataOutputStream(BufferedOutputStream(FileOutputStream(bridgeFd.fileDescriptor)))
    private val ioLock = Any()
    private val pendingResponses = ConcurrentHashMap<String, LinkedBlockingQueue<JSONObject>>()
    private val closed = AtomicBoolean(false)
    private val readThread = Thread(::readLoop, "AgentBridgeClient-$sessionId")
    private var frameworkResponsesPath: String = DEFAULT_RESPONSES_PATH
    @Volatile
    private var currentRemoteConnectionId: String? = null
    @Volatile
    private var appServerProxyHandler: AppServerProxyHandler? = null

    init {
        Log.i(TAG, "Using framework session bridge transport for $sessionId")
        Log.i(TAG, "Using framework-owned HTTP bridge for $sessionId")
        readThread.start()
    }

    fun getRuntimeStatus(): CodexAgentBridge.RuntimeStatus {
        val status = request(
            JSONObject().put("method", OP_GET_RUNTIME_STATUS),
        ).getJSONObject("runtimeStatus")
        frameworkResponsesPath = status.optString("frameworkResponsesPath").ifBlank { DEFAULT_RESPONSES_PATH }
        return CodexAgentBridge.RuntimeStatus(
            authenticated = status.getBoolean("authenticated"),
            accountEmail = status.optNullableString("accountEmail"),
            clientCount = status.optInt("clientCount"),
            modelProviderId = status.optString("modelProviderId"),
            configuredModel = status.optNullableString("configuredModel"),
            effectiveModel = status.optNullableString("effectiveModel"),
            upstreamBaseUrl = status.optString("upstreamBaseUrl"),
            frameworkResponsesPath = frameworkResponsesPath,
        )
    }

    fun readInstalledAgentsMarkdown(): String {
        return request(
            JSONObject().put("method", OP_READ_INSTALLED_AGENTS_FILE),
        ).getString("agentsMarkdown")
    }

    fun readSessionExecutionSettings(): SessionExecutionSettings {
        val settings = request(
            JSONObject().put("method", OP_READ_SESSION_EXECUTION_SETTINGS),
        ).getJSONObject("executionSettings")
        return SessionExecutionSettings(
            model = settings.optNullableString("model"),
            reasoningEffort = settings.optNullableString("reasoningEffort"),
        )
    }

    fun registerAppServerThread(threadId: String) {
        request(
            JSONObject()
                .put("method", OP_REGISTER_APP_SERVER_THREAD)
                .put("threadId", threadId),
        )
    }

    fun setAppServerProxyHandler(handler: AppServerProxyHandler?) {
        appServerProxyHandler = handler
    }

    fun currentRemoteConnectionId(): String? = currentRemoteConnectionId

    fun sendRemoteAppServerMessage(message: String) {
        val connectionId = currentRemoteConnectionId ?: return
        sendMessage(
            JSONObject()
                .put("kind", KIND_REMOTE_SERVER_MESSAGE)
                .put("connectionId", connectionId)
                .put("message", message),
        )
    }

    fun closeRemoteAppServer(reason: String?) {
        val connectionId = currentRemoteConnectionId ?: return
        currentRemoteConnectionId = null
        sendMessage(
            JSONObject()
                .put("kind", KIND_REMOTE_CLOSED)
                .put("connectionId", connectionId)
                .put("reason", reason),
        )
        appServerProxyHandler?.onClosed(reason)
    }

    fun sendResponsesRequest(body: String): AgentResponsesHttpResponse {
        val response = FrameworkSessionTransportCompat.executeStreamingRequest(
            callback = frameworkCallback,
            sessionId = sessionId,
            request = FrameworkSessionTransportCompat.HttpRequest(
                method = RESPONSES_METHOD,
                path = frameworkResponsesPath,
                headers = Bundle().apply {
                    putString(HEADER_CONTENT_TYPE, HEADER_VALUE_APPLICATION_JSON)
                    putString(HEADER_ACCEPT, HEADER_VALUE_TEXT_EVENT_STREAM)
                    putString(HEADER_ACCEPT_ENCODING, HEADER_VALUE_IDENTITY)
                },
                body = body.toByteArray(StandardCharsets.UTF_8),
            ),
        )
        return AgentResponsesHttpResponse(
            statusCode = response.statusCode,
            body = response.bodyString,
        )
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) {
            return
        }
        currentRemoteConnectionId = null
        runCatching { input.close() }
        runCatching { output.close() }
        runCatching { bridgeFd.close() }
        readThread.interrupt()
    }

    private fun request(request: JSONObject): JSONObject {
        val requestId = UUID.randomUUID().toString()
        val responseQueue = LinkedBlockingQueue<JSONObject>(1)
        pendingResponses[requestId] = responseQueue
        try {
            sendMessage(
                request
                    .put("kind", KIND_REQUEST)
                    .put("requestId", requestId),
            )
            val response = responseQueue.poll(BRIDGE_REQUEST_TIMEOUT_MS, TimeUnit.MILLISECONDS)
                ?: throw IOException("Timed out waiting for Agent bridge response")
            if (!response.optBoolean("ok")) {
                throw IOException(response.optString("error").ifBlank { "Agent bridge request failed" })
            }
            return response
        } finally {
            pendingResponses.remove(requestId)
        }
    }

    private fun readLoop() {
        while (!closed.get()) {
            val message = try {
                readMessage()
            } catch (err: IOException) {
                if (!closed.get()) {
                    Log.w(TAG, "Agent bridge read failed for $sessionId", err)
                    appServerProxyHandler?.onClosed(err.message ?: err::class.java.simpleName)
                }
                return
            }
            when (message.optString("kind", KIND_RESPONSE)) {
                KIND_RESPONSE -> {
                    pendingResponses[message.optString("requestId")]?.offer(message)
                }
                KIND_REMOTE_CLIENT_MESSAGE -> {
                    val connectionId = message.optString("connectionId")
                    currentRemoteConnectionId = connectionId
                    appServerProxyHandler?.onMessage(message.optString("message"))
                }
                KIND_REMOTE_CLOSED -> {
                    currentRemoteConnectionId = null
                    appServerProxyHandler?.onClosed(message.optString("reason").ifBlank { null })
                }
            }
        }
    }

    private fun sendMessage(message: JSONObject) {
        synchronized(ioLock) {
            writeMessage(message)
        }
    }

    private fun writeMessage(message: JSONObject) {
        val payload = message.toString().toByteArray(StandardCharsets.UTF_8)
        output.writeInt(payload.size)
        output.flush()
        var offset = 0
        while (offset < payload.size) {
            val chunkSize = minOf(WRITE_CHUNK_BYTES, payload.size - offset)
            output.write(payload, offset, chunkSize)
            output.flush()
            offset += chunkSize
        }
    }

    private fun readMessage(): JSONObject {
        val size = input.readInt()
        if (size <= 0) {
            throw IOException("Invalid Agent bridge message length: $size")
        }
        val payload = ByteArray(size)
        input.readFully(payload)
        return JSONObject(payload.toString(StandardCharsets.UTF_8))
    }

    private fun JSONObject.optNullableString(key: String): String? {
        if (!has(key) || isNull(key)) {
            return null
        }
        return optString(key).ifBlank { null }
    }

    data class AgentResponsesHttpResponse(
        val statusCode: Int,
        val body: String,
    )
}
