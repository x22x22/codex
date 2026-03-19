package com.openai.codex.genie

import org.json.JSONObject
import java.io.IOException

object CodexAgentBridge {
    private const val BRIDGE_REQUEST_PREFIX = "__codex_bridge__ "
    private const val BRIDGE_RESPONSE_PREFIX = "__codex_bridge_result__ "
    private const val METHOD_GET_AUTH_STATUS = "get_auth_status"

    fun buildAuthStatusRequest(requestId: String): String {
        val payload = JSONObject()
            .put("requestId", requestId)
            .put("method", METHOD_GET_AUTH_STATUS)
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

    fun parseAuthStatusResponse(response: String, requestId: String): AuthStatus {
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
        return AuthStatus(
            authenticated = data.optBoolean("authenticated", false),
            accountEmail = if (data.isNull("accountEmail")) null else data.optString("accountEmail"),
            clientCount = data.optInt("clientCount", 0),
        )
    }
}
