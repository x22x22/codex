package com.openai.codex.genie

import org.json.JSONObject
import java.io.IOException

object CodexAgentBridge {
    private const val BRIDGE_REQUEST_PREFIX = "__codex_bridge__ "
    private const val BRIDGE_RESPONSE_PREFIX = "__codex_bridge_result__ "
    private const val METHOD_HTTP_REQUEST = "http_request"

    fun buildAuthStatusRequest(requestId: String): String {
        return buildHttpRequest(requestId, "GET", "/internal/auth/status", null)
    }

    fun buildHttpRequest(
        requestId: String,
        httpMethod: String,
        path: String,
        body: String?,
    ): String {
        val payload = JSONObject()
            .put("requestId", requestId)
            .put("method", METHOD_HTTP_REQUEST)
            .put("httpMethod", httpMethod)
            .put("path", path)
        if (body == null) {
            payload.put("body", JSONObject.NULL)
        } else {
            payload.put("body", body)
        }
        return "$BRIDGE_REQUEST_PREFIX$payload"
    }

    fun isBridgeResponse(message: String): Boolean {
        return message.startsWith(BRIDGE_RESPONSE_PREFIX)
    }

    data class AuthStatus(
        val authenticated: Boolean,
        val accountEmail: String?,
        val clientCount: Int,
    )

    data class HttpResponse(
        val statusCode: Int,
        val body: String,
    )

    fun parseAuthStatusResponse(response: String, requestId: String): AuthStatus {
        val httpResponse = parseHttpResponse(response, requestId)
        if (httpResponse.statusCode != 200) {
            throw IOException("HTTP ${httpResponse.statusCode}: ${httpResponse.body}")
        }
        val data = JSONObject(httpResponse.body)
        return AuthStatus(
            authenticated = data.optBoolean("authenticated", false),
            accountEmail = if (data.isNull("accountEmail")) null else data.optString("accountEmail"),
            clientCount = data.optInt("clientCount", 0),
        )
    }

    private fun parseHttpResponse(response: String, requestId: String): HttpResponse {
        if (!response.startsWith(BRIDGE_RESPONSE_PREFIX)) {
            throw IOException("Unexpected bridge response format")
        }
        val data = JSONObject(response.removePrefix(BRIDGE_RESPONSE_PREFIX))
        if (data.optString("requestId") != requestId) {
            throw IOException("Mismatched bridge response id")
        }
        if (!data.optBoolean("ok", false)) {
            throw IOException(data.optString("error", "Agent bridge request failed"))
        }
        return HttpResponse(
            statusCode = data.optInt("statusCode", 200),
            body = data.optString("body"),
        )
    }
}
