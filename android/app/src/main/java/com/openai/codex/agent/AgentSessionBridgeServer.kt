package com.openai.codex.agent

import android.app.agent.AgentManager
import android.content.Context
import android.os.ParcelFileDescriptor
import android.util.Log
import com.openai.codex.bridge.HostedCodexConfig
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.Closeable
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.EOFException
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.IOException
import java.io.File
import java.nio.charset.StandardCharsets
import java.util.concurrent.ConcurrentHashMap
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread
import org.json.JSONObject

object AgentSessionBridgeServer {
    private val runningBridges = ConcurrentHashMap<String, RunningBridge>()

    private const val TAG = "AgentSessionBridge"

    fun ensureStarted(
        context: Context,
        agentManager: AgentManager,
        sessionId: String,
    ) {
        runningBridges.computeIfAbsent(sessionId) {
            RunningBridge(
                context = context.applicationContext,
                agentManager = agentManager,
                sessionId = sessionId,
            ).also(RunningBridge::start)
        }
    }

    fun closeSession(sessionId: String) {
        runningBridges.remove(sessionId)?.close()
    }

    fun activeThreadId(sessionId: String): String? = runningBridges[sessionId]?.activeThreadId()

    fun openDesktopProxy(
        sessionId: String,
        onMessage: (String) -> Unit,
        onClosed: (String?) -> Unit,
    ): String? = runningBridges[sessionId]?.openDesktopProxy(onMessage, onClosed)

    fun sendDesktopProxyInput(
        sessionId: String,
        connectionId: String,
        message: String,
    ): Boolean = runningBridges[sessionId]?.sendDesktopProxyInput(connectionId, message) ?: false

    fun closeDesktopProxy(
        sessionId: String,
        connectionId: String,
        reason: String? = null,
    ) {
        runningBridges[sessionId]?.closeDesktopProxy(connectionId, reason)
    }

    private class RunningBridge(
        private val context: Context,
        private val agentManager: AgentManager,
        private val sessionId: String,
    ) : Closeable {
        companion object {
            private const val METHOD_GET_RUNTIME_STATUS = "getRuntimeStatus"
            private const val METHOD_READ_INSTALLED_AGENTS_FILE = "readInstalledAgentsFile"
            private const val METHOD_READ_SESSION_EXECUTION_SETTINGS = "readSessionExecutionSettings"
            private const val METHOD_REGISTER_APP_SERVER_THREAD = "registerAppServerThread"
            private const val WRITE_CHUNK_BYTES = 4096
            private const val KIND_REQUEST = "request"
            private const val KIND_RESPONSE = "response"
            private const val KIND_REMOTE_CLIENT_MESSAGE = "remoteAppServerClientMessage"
            private const val KIND_REMOTE_SERVER_MESSAGE = "remoteAppServerServerMessage"
            private const val KIND_REMOTE_CLOSED = "remoteAppServerClosed"
        }

        private data class DesktopProxy(
            val connectionId: String,
            val onMessage: (String) -> Unit,
            val onClosed: (String?) -> Unit,
        )

        private val closed = AtomicBoolean(false)
        private var bridgeFd: ParcelFileDescriptor? = null
        private var input: DataInputStream? = null
        private var output: DataOutputStream? = null
        private val executionSettingsStore = SessionExecutionSettingsStore(context)
        private val writerLock = Any()
        private val proxyLock = Any()
        @Volatile
        private var currentDesktopProxy: DesktopProxy? = null
        @Volatile
        private var currentThreadId: String? = null
        private val serveThread = thread(
            start = false,
            name = "AgentSessionBridge-$sessionId",
        ) {
            serveLoop()
        }

        fun start() {
            serveThread.start()
        }

        override fun close() {
            if (!closed.compareAndSet(false, true)) {
                return
            }
            val proxy = synchronized(proxyLock) {
                currentDesktopProxy.also {
                    currentDesktopProxy = null
                }
            }
            runCatching {
                proxy?.onClosed("Agent session bridge closed")
            }
            runCatching { input?.close() }
            runCatching { output?.close() }
            runCatching { bridgeFd?.close() }
            serveThread.interrupt()
        }

        fun activeThreadId(): String? = currentThreadId

        fun openDesktopProxy(
            onMessage: (String) -> Unit,
            onClosed: (String?) -> Unit,
        ): String? {
            val threadId = currentThreadId ?: return null
            val connectionId = UUID.randomUUID().toString()
            val replacement = synchronized(proxyLock) {
                currentDesktopProxy.also {
                    currentDesktopProxy = DesktopProxy(connectionId, onMessage, onClosed)
                }
            }
            runCatching {
                replacement?.onClosed("Replaced by a newer desktop attach")
            }
            return connectionId
        }

        fun sendDesktopProxyInput(
            connectionId: String,
            message: String,
        ): Boolean {
            val proxy = currentDesktopProxy
            if (proxy?.connectionId != connectionId) {
                return false
            }
            sendBridgeMessage(
                JSONObject()
                    .put("kind", KIND_REMOTE_CLIENT_MESSAGE)
                    .put("connectionId", connectionId)
                    .put("message", message),
            )
            return true
        }

        fun closeDesktopProxy(
            connectionId: String,
            reason: String? = null,
        ) {
            val proxy = synchronized(proxyLock) {
                currentDesktopProxy?.takeIf { it.connectionId == connectionId }?.also {
                    currentDesktopProxy = null
                }
            } ?: return
            sendBridgeMessage(
                JSONObject()
                    .put("kind", KIND_REMOTE_CLOSED)
                    .put("connectionId", connectionId)
                    .put("reason", reason),
            )
            runCatching {
                proxy.onClosed(reason)
            }
        }

        private fun serveLoop() {
            try {
                val fd = agentManager.openSessionBridge(sessionId)
                bridgeFd = fd
                input = DataInputStream(BufferedInputStream(FileInputStream(fd.fileDescriptor)))
                output = DataOutputStream(BufferedOutputStream(FileOutputStream(fd.fileDescriptor)))
                Log.i(TAG, "Opened framework session bridge for $sessionId")
                while (!closed.get()) {
                    val message = try {
                        readMessage(input ?: break)
                    } catch (_: EOFException) {
                        return
                    }
                    when (message.optString("kind", KIND_REQUEST)) {
                        KIND_REQUEST -> {
                            val response = handleRequest(message)
                            sendBridgeMessage(response)
                        }
                        KIND_REMOTE_SERVER_MESSAGE -> {
                            handleRemoteServerMessage(message)
                        }
                        KIND_REMOTE_CLOSED -> {
                            handleRemoteClosed(message)
                        }
                        else -> {
                            Log.w(TAG, "Ignoring unsupported Agent bridge message for $sessionId: $message")
                        }
                    }
                }
            } catch (err: Exception) {
                if (!closed.get() && !isExpectedSessionShutdown(err)) {
                    Log.w(TAG, "Session bridge failed for $sessionId", err)
                }
            } finally {
                runningBridges.remove(sessionId, this)
                close()
            }
        }

        private fun isExpectedSessionShutdown(err: Exception): Boolean {
            return err is IllegalStateException
                && err.message?.contains("No active Genie runtime for session") == true
        }

        private fun handleRequest(request: JSONObject): JSONObject {
            val requestId = request.optString("requestId")
            return runCatching {
                when (request.optString("method")) {
                    METHOD_GET_RUNTIME_STATUS -> {
                        val status = AgentCodexAppServerClient.readRuntimeStatus(context)
                        JSONObject()
                            .put("kind", KIND_RESPONSE)
                            .put("requestId", requestId)
                            .put("ok", true)
                            .put(
                                "runtimeStatus",
                                JSONObject()
                                    .put("authenticated", status.authenticated)
                                    .put("accountEmail", status.accountEmail)
                                    .put("clientCount", status.clientCount)
                                    .put("modelProviderId", status.modelProviderId)
                                    .put("configuredModel", status.configuredModel)
                                    .put("effectiveModel", status.effectiveModel)
                                    .put("upstreamBaseUrl", status.upstreamBaseUrl)
                                    .put("frameworkResponsesPath", status.frameworkResponsesPath),
                            )
                    }
                    METHOD_READ_INSTALLED_AGENTS_FILE -> {
                        val codexHome = File(context.filesDir, "codex-home")
                        HostedCodexConfig.installBundledAgentsFile(context, codexHome)
                        JSONObject()
                            .put("kind", KIND_RESPONSE)
                            .put("requestId", requestId)
                            .put("ok", true)
                            .put("agentsMarkdown", HostedCodexConfig.readInstalledAgentsMarkdown(codexHome))
                    }
                    METHOD_READ_SESSION_EXECUTION_SETTINGS -> {
                        JSONObject()
                            .put("kind", KIND_RESPONSE)
                            .put("requestId", requestId)
                            .put("ok", true)
                            .put("executionSettings", executionSettingsStore.toJson(sessionId))
                    }
                    METHOD_REGISTER_APP_SERVER_THREAD -> {
                        currentThreadId = request.optString("threadId").trim().ifEmpty { null }
                        JSONObject()
                            .put("kind", KIND_RESPONSE)
                            .put("requestId", requestId)
                            .put("ok", true)
                    }
                    else -> {
                        JSONObject()
                            .put("kind", KIND_RESPONSE)
                            .put("requestId", requestId)
                            .put("ok", false)
                            .put("error", "Unsupported bridge method: ${request.optString("method")}")
                    }
                }
            }.getOrElse { err ->
                JSONObject()
                    .put("kind", KIND_RESPONSE)
                    .put("requestId", requestId)
                    .put("ok", false)
                    .put("error", err.message ?: err::class.java.simpleName)
            }
        }

        private fun handleRemoteServerMessage(message: JSONObject) {
            val connectionId = message.optString("connectionId")
            val proxy = currentDesktopProxy
            if (proxy == null || proxy.connectionId != connectionId) {
                return
            }
            runCatching {
                proxy.onMessage(message.optString("message"))
            }.onFailure { err ->
                Log.w(TAG, "Desktop proxy delivery failed for $sessionId", err)
                closeDesktopProxy(connectionId, err.message ?: err::class.java.simpleName)
            }
        }

        private fun handleRemoteClosed(message: JSONObject) {
            val connectionId = message.optString("connectionId")
            val reason = message.optString("reason").ifBlank { null }
            val proxy = synchronized(proxyLock) {
                currentDesktopProxy?.takeIf { it.connectionId == connectionId }?.also {
                    currentDesktopProxy = null
                }
            } ?: return
            runCatching {
                proxy.onClosed(reason)
            }
        }

        private fun readMessage(input: DataInputStream): JSONObject {
            val size = input.readInt()
            if (size <= 0) {
                throw IOException("Invalid session bridge message length: $size")
            }
            val payload = ByteArray(size)
            input.readFully(payload)
            return JSONObject(payload.toString(StandardCharsets.UTF_8))
        }

        private fun writeMessage(
            output: DataOutputStream,
            message: JSONObject,
        ) {
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

        private fun sendBridgeMessage(message: JSONObject) {
            synchronized(writerLock) {
                writeMessage(output ?: throw IOException("Session bridge output unavailable"), message)
            }
        }
    }
}
