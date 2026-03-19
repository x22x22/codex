package com.openai.codex.genie

import org.json.JSONObject
import java.io.IOException

object CodexAgentBridge {
    private const val BRIDGE_REQUEST_PREFIX = "__codex_bridge__ "
    private const val BRIDGE_RESPONSE_PREFIX = "__codex_bridge_result__ "
    private const val METHOD_HTTP_REQUEST = "http_request"

    fun buildRuntimeStatusRequest(requestId: String): String {
        return buildHttpRequest(requestId, "GET", "/internal/runtime/status", null)
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

    fun buildResponsesRequest(
        requestId: String,
        model: String,
        prompt: String,
    ): String {
        val body = JSONObject()
            .put("model", model)
            .put("store", false)
            .put("stream", false)
            .put("input", prompt)
            .toString()
        return buildHttpRequest(requestId, "POST", "/v1/responses", body)
    }

    fun isBridgeResponse(message: String): Boolean {
        return message.startsWith(BRIDGE_RESPONSE_PREFIX)
    }

    data class RuntimeStatus(
        val authenticated: Boolean,
        val accountEmail: String?,
        val clientCount: Int,
        val modelProviderId: String,
        val configuredModel: String?,
        val effectiveModel: String?,
        val upstreamBaseUrl: String,
    )

    data class HttpResponse(
        val statusCode: Int,
        val body: String,
    )

    fun parseRuntimeStatusResponse(response: String, requestId: String): RuntimeStatus {
        val httpResponse = parseHttpResponse(response, requestId)
        if (httpResponse.statusCode != 200) {
            throw IOException("HTTP ${httpResponse.statusCode}: ${httpResponse.body}")
        }
        val data = JSONObject(httpResponse.body)
        return RuntimeStatus(
            authenticated = data.optBoolean("authenticated", false),
            accountEmail = if (data.isNull("accountEmail")) null else data.optString("accountEmail"),
            clientCount = data.optInt("clientCount", 0),
            modelProviderId = data.optString("modelProviderId"),
            configuredModel = if (data.isNull("configuredModel")) null else data.optString("configuredModel"),
            effectiveModel = if (data.isNull("effectiveModel")) null else data.optString("effectiveModel"),
            upstreamBaseUrl = data.optString("upstreamBaseUrl"),
        )
    }

    fun parseResponsesOutputText(response: String, requestId: String): String {
        val httpResponse = parseHttpResponse(response, requestId)
        if (httpResponse.statusCode != 200) {
            throw IOException("HTTP ${httpResponse.statusCode}: ${httpResponse.body}")
        }
        val data = JSONObject(httpResponse.body)
        val directOutput = data.optString("output_text")
        if (directOutput.isNotBlank()) {
            return directOutput
        }
        val output = data.optJSONArray("output")
            ?: throw IOException("Responses payload missing output")
        val combined = buildString {
            for (outputIndex in 0 until output.length()) {
                val item = output.optJSONObject(outputIndex) ?: continue
                val content = item.optJSONArray("content") ?: continue
                for (contentIndex in 0 until content.length()) {
                    val part = content.optJSONObject(contentIndex) ?: continue
                    if (part.optString("type") == "output_text") {
                        append(part.optString("text"))
                    }
                }
            }
        }
        if (combined.isBlank()) {
            throw IOException("Responses payload missing output_text content")
        }
        return combined
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
