package com.openai.codex.agent

import java.io.File
import java.net.SocketException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AgentResponsesProxyTest {
    @Test
    fun buildResponsesUrlUsesChatgptDefaultForProviderDefault() {
        assertEquals(
            "https://chatgpt.com/backend-api/codex/responses",
            AgentResponsesProxy.buildResponsesUrl(
                upstreamBaseUrl = "provider-default",
                authMode = "chatgpt",
            ),
        )
    }

    @Test
    fun buildResponsesUrlAppendsResponsesToConfiguredBase() {
        assertEquals(
            "https://api.openai.com/v1/responses",
            AgentResponsesProxy.buildResponsesUrl(
                upstreamBaseUrl = "https://api.openai.com/v1/",
                authMode = "apiKey",
            ),
        )
    }

    @Test
    fun loadAuthSnapshotReadsChatgptTokens() {
        val authFile = writeTempAuthJson(
            """
            {
              "auth_mode": "chatgpt",
              "OPENAI_API_KEY": null,
              "tokens": {
                "id_token": "header.payload.signature",
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "account_id": "acct-123"
              },
              "last_refresh": "2026-03-19T00:00:00Z"
            }
            """.trimIndent(),
        )

        val snapshot = AgentResponsesProxy.loadAuthSnapshot(authFile)

        assertEquals("chatgpt", snapshot.authMode)
        assertEquals("access-token", snapshot.bearerToken)
        assertEquals("acct-123", snapshot.accountId)
    }

    @Test
    fun loadAuthSnapshotFallsBackToApiKeyModeWhenAuthModeIsMissing() {
        val authFile = writeTempAuthJson(
            """
            {
              "OPENAI_API_KEY": "sk-test-key",
              "tokens": null,
              "last_refresh": null
            }
            """.trimIndent(),
        )

        val snapshot = AgentResponsesProxy.loadAuthSnapshot(authFile)

        assertEquals("apiKey", snapshot.authMode)
        assertEquals("sk-test-key", snapshot.bearerToken)
        assertNull(snapshot.accountId)
    }

    @Test
    fun describeRequestFailureIncludesPhaseUrlAndCause() {
        val message = AgentResponsesProxy.describeRequestFailure(
            phase = "read response body",
            upstreamUrl = "https://chatgpt.com/backend-api/codex/responses",
            err = SocketException("Software caused connection abort"),
        )

        assertEquals(
            "Responses proxy failed during read response body for https://chatgpt.com/backend-api/codex/responses: SocketException: Software caused connection abort",
            message,
        )
    }

    private fun writeTempAuthJson(contents: String): File {
        return File.createTempFile("agent-auth", ".json").apply {
            writeText(contents)
            deleteOnExit()
        }
    }
}
