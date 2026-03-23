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
import java.util.UUID
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
        private const val WRITE_CHUNK_BYTES = 4096
        private const val RESPONSES_METHOD = "POST"
        private const val DEFAULT_RESPONSES_PATH = "/responses"
        private const val HEADER_CONTENT_TYPE = "Content-Type"
        private const val HEADER_ACCEPT = "Accept"
        private const val HEADER_ACCEPT_ENCODING = "Accept-Encoding"
        private const val HEADER_VALUE_APPLICATION_JSON = "application/json"
        private const val HEADER_VALUE_TEXT_EVENT_STREAM = "text/event-stream"
        private const val HEADER_VALUE_IDENTITY = "identity"
    }

    private val bridgeFd: ParcelFileDescriptor = callback.openSessionBridge(sessionId)
    private val frameworkHttpBridgeFd: ParcelFileDescriptor =
        FrameworkSessionTransportCompat.openFrameworkSessionBridge(callback, sessionId)
    private val input = DataInputStream(BufferedInputStream(FileInputStream(bridgeFd.fileDescriptor)))
    private val output = DataOutputStream(BufferedOutputStream(FileOutputStream(bridgeFd.fileDescriptor)))
    private val ioLock = Any()
    private var frameworkResponsesPath: String = DEFAULT_RESPONSES_PATH

    init {
        Log.i(TAG, "Using framework session bridge transport for $sessionId")
        Log.i(TAG, "Using framework-owned HTTP bridge for $sessionId")
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

    fun sendResponsesRequest(body: String): AgentResponsesHttpResponse {
        val response = ParcelFileDescriptor.dup(frameworkHttpBridgeFd.fileDescriptor).use { requestBridge ->
            FrameworkSessionTransportCompat.executeRequestAndReadFully(
                bridge = requestBridge,
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
        }
        return AgentResponsesHttpResponse(
            statusCode = response.statusCode,
            body = response.bodyString,
        )
    }

    override fun close() {
        synchronized(ioLock) {
            runCatching { input.close() }
            runCatching { output.close() }
            runCatching { bridgeFd.close() }
            runCatching { frameworkHttpBridgeFd.close() }
        }
    }

    private fun request(request: JSONObject): JSONObject {
        val requestId = UUID.randomUUID().toString()
        synchronized(ioLock) {
            writeMessage(request.put("requestId", requestId))
            val response = readMessage()
            if (response.optString("requestId") != requestId) {
                throw IOException("Mismatched Agent bridge response id")
            }
            if (!response.optBoolean("ok")) {
                throw IOException(response.optString("error").ifBlank { "Agent bridge request failed" })
            }
            return response
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
