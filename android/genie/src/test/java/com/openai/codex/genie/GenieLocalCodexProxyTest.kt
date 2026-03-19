package com.openai.codex.genie

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.nio.charset.StandardCharsets
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class GenieLocalCodexProxyTest {
    @Test
    fun forwardsResponsesRequestsToAgentBridge() {
        val forwardedRequestBody = AtomicReference<String?>()
        val body = """{"model":"gpt-5.3-codex"}"""
        val request = buildString {
            append("POST /v1/responses HTTP/1.1\r\n")
            append("Host: localhost\r\n")
            append("Content-Type: application/json\r\n")
            append("Content-Length: ${body.toByteArray(StandardCharsets.UTF_8).size}\r\n")
            append("\r\n")
            append(body)
        }
        val responseBytes = ByteArrayOutputStream()

        GenieLocalCodexHttpProxy.handleExchange(
            input = ByteArrayInputStream(request.toByteArray(StandardCharsets.UTF_8)),
            output = responseBytes,
            requestForwarder = object : CodexResponsesRequestForwarder {
                override fun sendResponsesRequest(body: String): CodexAgentBridge.HttpResponse {
                    forwardedRequestBody.set(body)
                    return CodexAgentBridge.HttpResponse(
                        statusCode = 200,
                        body = """{"ok":true}""",
                    )
                }
            },
        )

        val responseText = responseBytes.toString(StandardCharsets.UTF_8)
        assertTrue(responseText.startsWith("HTTP/1.1 200 OK"))
        assertTrue(responseText.contains("""{"ok":true}"""))
        assertEquals("""{"model":"gpt-5.3-codex"}""", forwardedRequestBody.get())
    }
}
