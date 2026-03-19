package com.openai.codex.genie

import android.app.agent.AgentSessionInfo
import android.app.agent.GenieService
import java.io.Closeable
import java.io.IOException
import java.util.UUID
import org.json.JSONObject

interface CodexResponsesRequestForwarder {
    fun sendResponsesRequest(body: String): CodexAgentBridge.HttpResponse
}

class AgentBridgeClient(
    private val sessionId: String,
    private val callback: GenieService.Callback,
    private val control: GenieSessionControl,
) : Closeable, CodexResponsesRequestForwarder {
    companion object {
        private const val BRIDGE_REQUEST_PREFIX = "__codex_bridge__ "
        private const val BRIDGE_RESPONSE_PREFIX = "__codex_bridge_result__ "
        private const val OP_GET_RUNTIME_STATUS = "getRuntimeStatus"
        private const val OP_SEND_RESPONSES_REQUEST = "sendResponsesRequest"
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

    override fun sendResponsesRequest(body: String): CodexAgentBridge.HttpResponse {
        val response = request(
            JSONObject()
                .put("method", OP_SEND_RESPONSES_REQUEST)
                .put("requestBody", body),
        ).getJSONObject("httpResponse")
        return CodexAgentBridge.HttpResponse(
            statusCode = response.getInt("statusCode"),
            body = response.optString("body"),
        )
    }

    override fun close() = Unit

    private fun request(request: JSONObject): JSONObject {
        val requestId = UUID.randomUUID().toString()
        callback.publishQuestion(
            sessionId,
            BRIDGE_REQUEST_PREFIX + request.put("requestId", requestId).toString(),
        )
        callback.updateState(sessionId, AgentSessionInfo.STATE_WAITING_FOR_USER)
        val answer = try {
            control.waitForBridgeResponse(requestId)
        } finally {
            if (!control.cancelled) {
                callback.updateState(sessionId, AgentSessionInfo.STATE_RUNNING)
            }
        }
        if (!answer.startsWith(BRIDGE_RESPONSE_PREFIX)) {
            throw IOException("Unexpected Agent bridge response: $answer")
        }
        val response = JSONObject(answer.removePrefix(BRIDGE_RESPONSE_PREFIX))
        if (response.optString("requestId") != requestId) {
            throw IOException("Mismatched Agent bridge response id")
        }
        if (!response.optBoolean("ok")) {
            throw IOException(response.optString("error").ifBlank { "Agent bridge request failed" })
        }
        return response
    }
}

internal fun JSONObject.optNullableString(name: String): String? = when {
    isNull(name) -> null
    else -> optString(name).ifBlank { null }
}
