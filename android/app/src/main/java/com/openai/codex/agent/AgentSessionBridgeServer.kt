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
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread
import org.json.JSONObject

object AgentSessionBridgeServer {
    private val runningBridges = ConcurrentHashMap<String, RunningBridge>()

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

    private class RunningBridge(
        private val context: Context,
        private val agentManager: AgentManager,
        private val sessionId: String,
    ) : Closeable {
        companion object {
            private const val TAG = "AgentSessionBridge"
            private const val METHOD_GET_RUNTIME_STATUS = "getRuntimeStatus"
            private const val METHOD_READ_INSTALLED_AGENTS_FILE = "readInstalledAgentsFile"
            private const val METHOD_READ_SESSION_EXECUTION_SETTINGS = "readSessionExecutionSettings"
            private const val WRITE_CHUNK_BYTES = 4096
        }

        private val closed = AtomicBoolean(false)
        private var bridgeFd: ParcelFileDescriptor? = null
        private var input: DataInputStream? = null
        private var output: DataOutputStream? = null
        private val executionSettingsStore = SessionExecutionSettingsStore(context)
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
            runCatching { input?.close() }
            runCatching { output?.close() }
            runCatching { bridgeFd?.close() }
            serveThread.interrupt()
        }

        private fun serveLoop() {
            try {
                val fd = agentManager.openSessionBridge(sessionId)
                bridgeFd = fd
                input = DataInputStream(BufferedInputStream(FileInputStream(fd.fileDescriptor)))
                output = DataOutputStream(BufferedOutputStream(FileOutputStream(fd.fileDescriptor)))
                Log.i(TAG, "Opened framework session bridge for $sessionId")
                while (!closed.get()) {
                    val request = try {
                        readMessage(input ?: break)
                    } catch (_: EOFException) {
                        return
                    }
                    val response = handleRequest(request)
                    writeMessage(output ?: break, response)
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
                            .put("requestId", requestId)
                            .put("ok", true)
                            .put("agentsMarkdown", HostedCodexConfig.readInstalledAgentsMarkdown(codexHome))
                    }
                    METHOD_READ_SESSION_EXECUTION_SETTINGS -> {
                        JSONObject()
                            .put("requestId", requestId)
                            .put("ok", true)
                            .put("executionSettings", executionSettingsStore.toJson(sessionId))
                    }
                    else -> {
                        JSONObject()
                            .put("requestId", requestId)
                            .put("ok", false)
                            .put("error", "Unsupported bridge method: ${request.optString("method")}")
                    }
                }
            }.getOrElse { err ->
                JSONObject()
                    .put("requestId", requestId)
                    .put("ok", false)
                    .put("error", err.message ?: err::class.java.simpleName)
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
    }
}
