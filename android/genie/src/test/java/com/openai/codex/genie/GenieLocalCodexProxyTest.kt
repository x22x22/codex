package com.openai.codex.genie

import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.OutputStreamWriter
import java.net.Socket
import java.net.URI
import java.nio.charset.StandardCharsets
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class GenieLocalCodexProxyTest {
    @Test
    fun forwardsLoopbackResponsesRequestsToAgentBridge() {
        val forwardedRequestBody = AtomicReference<String?>()
        val proxy = GenieLocalCodexProxy(
            sessionId = "session-1",
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

        proxy.use { localProxy ->
            localProxy.start()
            val uri = URI(localProxy.baseUrl)
            Socket(uri.host, uri.port).use { socket ->
                val body = """{"model":"gpt-5.3-codex"}"""
                val requestPath = "${uri.rawPath}/responses"
                val writer = OutputStreamWriter(socket.getOutputStream(), StandardCharsets.UTF_8)
                writer.write("POST $requestPath HTTP/1.1\r\n")
                writer.write("Host: ${uri.host}\r\n")
                writer.write("Content-Type: application/json\r\n")
                writer.write("Content-Length: ${body.toByteArray(StandardCharsets.UTF_8).size}\r\n")
                writer.write("\r\n")
                writer.write(body)
                writer.flush()
                socket.shutdownOutput()
                val responseText = BufferedReader(
                    InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8),
                ).readText()
                assertTrue(responseText.startsWith("HTTP/1.1 200 OK"))
                assertTrue(responseText.contains("""{"ok":true}"""))
            }
        }

        assertEquals("""{"model":"gpt-5.3-codex"}""", forwardedRequestBody.get())
    }
}
