package com.openai.codex.bridge

import android.app.agent.AgentSessionEvent
import org.json.JSONObject

object FrameworkEventBridge {
    const val THREAD_FRAMEWORK_EVENT_METHOD = "thread/frameworkEvent"

    private const val BRIDGE_REQUEST_PREFIX = "__codex_bridge__ "
    private const val BRIDGE_RESPONSE_PREFIX = "__codex_bridge_result__ "

    fun buildThreadFrameworkEventNotification(
        threadId: String,
        event: AgentSessionEvent,
    ): String? {
        val eventType = when (event.type) {
            AgentSessionEvent.TYPE_TRACE -> "trace"
            AgentSessionEvent.TYPE_QUESTION -> "question"
            AgentSessionEvent.TYPE_RESULT -> "result"
            AgentSessionEvent.TYPE_ERROR -> "error"
            else -> return null
        }
        val message = normalizeEventMessage(event.message) ?: return null
        return buildThreadFrameworkEventNotification(
            threadId = threadId,
            eventType = eventType,
            message = message,
        )
    }

    fun buildThreadFrameworkEventNotification(
        threadId: String,
        eventType: String,
        message: String,
    ): String? {
        if (message.isBlank()) {
            return null
        }
        if (eventType !in setOf("trace", "question", "result", "error")) {
            return null
        }
        return JSONObject()
            .put("method", THREAD_FRAMEWORK_EVENT_METHOD)
            .put(
                "params",
                JSONObject()
                    .put("threadId", threadId)
                    .put("eventType", eventType)
                    .put("message", message),
            ).toString()
    }

    private fun normalizeEventMessage(message: String?): String? {
        val trimmed = message?.trim()?.takeIf(String::isNotEmpty) ?: return null
        if (trimmed.startsWith(BRIDGE_REQUEST_PREFIX)) {
            return summarizeBridgeRequest(trimmed)
        }
        if (trimmed.startsWith(BRIDGE_RESPONSE_PREFIX)) {
            return summarizeBridgeResponse(trimmed)
        }
        return trimmed
    }

    private fun summarizeBridgeRequest(message: String): String {
        val request = runCatching {
            JSONObject(message.removePrefix(BRIDGE_REQUEST_PREFIX))
        }.getOrNull()
        val method = request?.optString("method")?.ifEmpty { "unknown" } ?: "unknown"
        val requestId = request?.optString("requestId")?.takeIf(String::isNotBlank)
        return buildString {
            append("Bridge request: ")
            append(method)
            requestId?.let {
                append(" (#")
                append(it)
                append(')')
            }
        }
    }

    private fun summarizeBridgeResponse(message: String): String {
        val response = runCatching {
            JSONObject(message.removePrefix(BRIDGE_RESPONSE_PREFIX))
        }.getOrNull()
        val requestId = response?.optString("requestId")?.takeIf(String::isNotBlank)
        val statusCode = response?.optJSONObject("httpResponse")?.optInt("statusCode")
        return buildString {
            append("Bridge response")
            requestId?.let {
                append(" (#")
                append(it)
                append(')')
            }
            statusCode?.let {
                append(" status=")
                append(it)
            }
        }
    }
}
