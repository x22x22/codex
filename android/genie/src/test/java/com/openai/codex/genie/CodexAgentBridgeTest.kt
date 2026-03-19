package com.openai.codex.genie

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class CodexAgentBridgeTest {
    @Test
    fun runtimeStatusRequestUsesInternalRuntimeStatusEndpoint() {
        val request = CodexAgentBridge.buildRuntimeStatusRequest("req-1")

        assertTrue(request.startsWith("__codex_bridge__ "))
        assertTrue(request.contains("\"requestId\":\"req-1\""))
        assertTrue(request.contains("\"method\":\"http_request\""))
        assertTrue(request.contains("\"httpMethod\":\"GET\""))
        assertTrue(request.contains("\"path\":\"/internal/runtime/status\""))
    }

    @Test
    fun parseRuntimeStatusResponseExtractsModelFields() {
        val response = """
            __codex_bridge_result__ {"requestId":"req-1","ok":true,"statusCode":200,"body":"{\"authenticated\":true,\"accountEmail\":\"user@example.com\",\"clientCount\":2,\"modelProviderId\":\"openai\",\"configuredModel\":\"gpt-5.1-codex\",\"effectiveModel\":\"gpt-5.1-codex\",\"upstreamBaseUrl\":\"https://api.openai.com/v1\"}"}
        """.trimIndent()

        val status = CodexAgentBridge.parseRuntimeStatusResponse(response, "req-1")

        assertEquals(
            CodexAgentBridge.RuntimeStatus(
                authenticated = true,
                accountEmail = "user@example.com",
                clientCount = 2,
                modelProviderId = "openai",
                configuredModel = "gpt-5.1-codex",
                effectiveModel = "gpt-5.1-codex",
                upstreamBaseUrl = "https://api.openai.com/v1",
            ),
            status,
        )
    }

    @Test
    fun parseResponsesOutputTextCombinesOutputItems() {
        val response = """
            __codex_bridge_result__ {"requestId":"req-1","ok":true,"statusCode":200,"body":"{\"id\":\"resp-1\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Open the clock app. \"},{\"type\":\"output_text\",\"text\":\"Set the requested timer.\"}]}]}"}
        """.trimIndent()

        val outputText = CodexAgentBridge.parseResponsesOutputText(response, "req-1")

        assertEquals("Open the clock app. Set the requested timer.", outputText)
    }
}
