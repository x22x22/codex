package com.openai.codex.genie

import android.app.agent.GenieService
import android.os.ParcelFileDescriptor
import android.util.Log
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
        private const val OP_SEND_RESPONSES_REQUEST = "sendResponsesRequest"
        private const val OP_READ_INSTALLED_AGENTS_FILE = "readInstalledAgentsFile"
        private const val OP_READ_SESSION_EXECUTION_SETTINGS = "readSessionExecutionSettings"
    }

    private val bridgeFd: ParcelFileDescriptor = callback.openSessionBridge(sessionId)
    private val input = DataInputStream(BufferedInputStream(FileInputStream(bridgeFd.fileDescriptor)))
    private val output = DataOutputStream(BufferedOutputStream(FileOutputStream(bridgeFd.fileDescriptor)))
    private val ioLock = Any()

    init {
        Log.i(TAG, "Using framework session bridge transport for $sessionId")
    }

    fun getRuntimeStatus(): CodexAgentBridge.RuntimeStatus {
        val status = request(
            JSONObject().put("method", OP_GET_RUNTIME_STATUS),
        ).getJSONObject("runtimeStatus")
        return CodexAgentBridge.RuntimeStatus(
            authenticated = status.getBoolean("authenticated"),
            accountEmail = status.optNullableString("accountEmail"),
            clientCount = status.optInt("clientCount"),
            modelProviderId = status.optString("modelProviderId"),
            configuredModel = status.optNullableString("configuredModel"),
            effectiveModel = status.optNullableString("effectiveModel"),
            upstreamBaseUrl = status.optString("upstreamBaseUrl"),
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
        val response = request(
            JSONObject()
                .put("method", OP_SEND_RESPONSES_REQUEST)
                .put("requestBody", body),
        ).getJSONObject("httpResponse")
        return AgentResponsesHttpResponse(
            statusCode = response.getInt("statusCode"),
            body = response.optString("body"),
        )
    }

    override fun close() {
        synchronized(ioLock) {
            runCatching { input.close() }
            runCatching { output.close() }
            runCatching { bridgeFd.close() }
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
        output.write(payload)
        output.flush()
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
