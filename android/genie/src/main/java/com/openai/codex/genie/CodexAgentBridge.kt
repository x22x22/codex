package com.openai.codex.genie

import org.json.JSONArray
import org.json.JSONObject
import java.io.IOException

object CodexAgentBridge {
    fun buildResponsesRequest(
        model: String,
        instructions: String,
        prompt: String,
        imageDataUrls: List<String> = emptyList(),
    ): JSONObject {
        val content = JSONArray().put(
            JSONObject()
                .put("type", "input_text")
                .put("text", prompt),
        )
        imageDataUrls.forEach { imageDataUrl ->
            content.put(
                JSONObject()
                    .put("type", "input_image")
                    .put("image_url", imageDataUrl),
            )
        }
        return JSONObject()
            .put("model", model)
            .put("store", false)
            .put("stream", true)
            .put("instructions", instructions)
            .put(
                "input",
                JSONArray().put(
                    JSONObject()
                        .put("role", "user")
                        .put("content", content),
                ),
            )
    }

    data class RuntimeStatus(
        val authenticated: Boolean,
        val accountEmail: String?,
        val clientCount: Int,
        val modelProviderId: String,
        val configuredModel: String?,
        val effectiveModel: String?,
        val upstreamBaseUrl: String,
        val frameworkResponsesPath: String,
    )

    data class HttpResponse(
        val statusCode: Int,
        val body: String,
    )

    fun parseResponsesOutputText(httpResponse: HttpResponse): String {
        if (httpResponse.statusCode != 200) {
            throw IOException("HTTP ${httpResponse.statusCode}: ${httpResponse.body}")
        }
        val body = httpResponse.body.trim()
        if (body.startsWith("event:") || body.startsWith("data:")) {
            return parseResponsesStreamOutputText(body)
        }
        val data = JSONObject(body)
        return parseResponsesJsonOutputText(data)
    }

    private fun parseResponsesJsonOutputText(data: JSONObject): String {
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

    private fun parseResponsesStreamOutputText(body: String): String {
        val deltaText = StringBuilder()
        val completedItems = mutableListOf<String>()
        body.split("\n\n").forEach { rawEvent ->
            val lines = rawEvent.lineSequence().map(String::trimEnd).toList()
            if (lines.isEmpty()) {
                return@forEach
            }
            val dataPayload = lines
                .filter { it.startsWith("data:") }
                .joinToString("\n") { it.removePrefix("data:").trimStart() }
                .trim()
            if (dataPayload.isEmpty() || dataPayload == "[DONE]") {
                return@forEach
            }
            val event = JSONObject(dataPayload)
            when (event.optString("type")) {
                "response.output_text.delta" -> deltaText.append(event.optString("delta"))
                "response.output_item.done" -> {
                    val item = event.optJSONObject("item") ?: return@forEach
                    val content = item.optJSONArray("content") ?: return@forEach
                    val text = buildString {
                        for (index in 0 until content.length()) {
                            val part = content.optJSONObject(index) ?: continue
                            if (part.optString("type") == "output_text") {
                                append(part.optString("text"))
                            }
                        }
                    }
                    if (text.isNotBlank()) {
                        completedItems += text
                    }
                }
                "response.failed" -> {
                    throw IOException(event.toString())
                }
            }
        }
        if (deltaText.isNotBlank()) {
            return deltaText.toString()
        }
        val completedText = completedItems.joinToString("")
        if (completedText.isNotBlank()) {
            return completedText
        }
        throw IOException("Responses stream missing output_text content")
    }
}

internal fun JSONObject.optNullableString(name: String): String? = when {
    isNull(name) -> null
    else -> optString(name).ifBlank { null }
}
