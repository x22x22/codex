package com.openai.codex.genie

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test

class CodexAgentBridgeTest {
    @Test
    fun buildResponsesRequestUsesListInputPayload() {
        val request = CodexAgentBridge.buildResponsesRequest(
            model = "gpt-5.1-codex",
            instructions = "reply",
            prompt = "inspect the target app",
            imageDataUrls = listOf("data:image/jpeg;base64,AAA"),
        )

        assertEquals("gpt-5.1-codex", request.getString("model"))
        assertFalse(request.getBoolean("store"))
        assertEquals(true, request.getBoolean("stream"))
        val input = request.getJSONArray("input")
        assertEquals(1, input.length())
        val message = input.getJSONObject(0)
        assertEquals("user", message.getString("role"))
        val content = message.getJSONArray("content")
        assertEquals(2, content.length())
        assertEquals("input_text", content.getJSONObject(0).getString("type"))
        assertEquals("inspect the target app", content.getJSONObject(0).getString("text"))
        assertEquals("input_image", content.getJSONObject(1).getString("type"))
        assertEquals("data:image/jpeg;base64,AAA", content.getJSONObject(1).getString("image_url"))
    }

    @Test
    fun parseResponsesOutputTextCombinesOutputItems() {
        val response = CodexAgentBridge.HttpResponse(
            statusCode = 200,
            body = JSONObject()
                .put(
                    "output",
                    org.json.JSONArray().put(
                        JSONObject()
                            .put("type", "message")
                            .put("role", "assistant")
                            .put(
                                "content",
                                org.json.JSONArray()
                                    .put(
                                        JSONObject()
                                            .put("type", "output_text")
                                            .put("text", "Open the clock app. "),
                                    )
                                    .put(
                                        JSONObject()
                                            .put("type", "output_text")
                                            .put("text", "Set the requested timer."),
                                    ),
                            ),
                    ),
                )
                .toString(),
        )

        val outputText = CodexAgentBridge.parseResponsesOutputText(response)

        assertEquals("Open the clock app. Set the requested timer.", outputText)
    }

    @Test
    fun parseResponsesOutputTextReadsSseDeltaPayloads() {
        val response = CodexAgentBridge.HttpResponse(
            statusCode = 200,
            body = """
                event: response.output_text.delta
                data: {"type":"response.output_text.delta","delta":"Open Clock. "}

                event: response.output_text.delta
                data: {"type":"response.output_text.delta","delta":"Start the timer."}

                event: response.completed
                data: {"type":"response.completed","response":{"id":"resp-1"}}

                data: [DONE]
            """.trimIndent(),
        )

        val outputText = CodexAgentBridge.parseResponsesOutputText(response)

        assertEquals("Open Clock. Start the timer.", outputText)
    }

    @Test
    fun optNullableStringTreatsJsonNullAsNull() {
        val json = JSONObject()
            .put("present", "gpt-5.3-codex")
            .put("blank", "")
            .put("missingViaNull", JSONObject.NULL)

        assertEquals("gpt-5.3-codex", json.optNullableString("present"))
        assertNull(json.optNullableString("blank"))
        assertNull(json.optNullableString("missingViaNull"))
        assertNull(json.optNullableString("missing"))
    }
}
