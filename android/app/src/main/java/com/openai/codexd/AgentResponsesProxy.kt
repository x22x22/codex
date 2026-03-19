package com.openai.codexd

import android.content.Context
import android.util.Log
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.nio.charset.StandardCharsets
import org.json.JSONObject

object AgentResponsesProxy {
    private const val TAG = "AgentResponsesProxy"
    private const val CONNECT_TIMEOUT_MS = 30_000
    private const val READ_TIMEOUT_MS = 30_000
    private const val DEFAULT_OPENAI_BASE_URL = "https://api.openai.com/v1"
    private const val DEFAULT_CHATGPT_BASE_URL = "https://chatgpt.com/backend-api/codex"
    private const val DEFAULT_ORIGINATOR = "codex_cli_rs"
    private const val DEFAULT_USER_AGENT = "codex_cli_rs/android_agent_bridge"

    internal data class AuthSnapshot(
        val authMode: String,
        val bearerToken: String,
        val accountId: String?,
    )

    fun sendResponsesRequest(
        context: Context,
        requestBody: String,
    ): CodexdLocalClient.HttpResponse {
        val authSnapshot = loadAuthSnapshot(File(context.filesDir, "codex-home/auth.json"))
        val upstreamUrl = buildResponsesUrl(
            upstreamBaseUrl = "provider-default",
            authSnapshot.authMode,
        )
        Log.i(TAG, "Proxying /v1/responses -> $upstreamUrl (auth_mode=${authSnapshot.authMode})")
        return executeRequest(upstreamUrl, requestBody, authSnapshot)
    }

    internal fun buildResponsesUrl(
        upstreamBaseUrl: String,
        authMode: String,
    ): String {
        val normalizedBaseUrl = when {
            upstreamBaseUrl.isBlank() || upstreamBaseUrl == "provider-default" -> {
                if (authMode == "chatgpt") {
                    DEFAULT_CHATGPT_BASE_URL
                } else {
                    DEFAULT_OPENAI_BASE_URL
                }
            }
            else -> upstreamBaseUrl
        }
        return "${normalizedBaseUrl.trimEnd('/')}/responses"
    }

    internal fun loadAuthSnapshot(authFile: File): AuthSnapshot {
        if (!authFile.isFile) {
            throw IOException("Missing Agent auth file at ${authFile.absolutePath}")
        }
        val json = JSONObject(authFile.readText())
        val openAiApiKey = json.stringOrNull("OPENAI_API_KEY")
        val authMode = when (json.stringOrNull("auth_mode")) {
            "apiKey", "apikey", "api_key" -> "apiKey"
            "chatgpt", "chatgptAuthTokens", "chatgpt_auth_tokens" -> "chatgpt"
            null -> if (openAiApiKey != null) "apiKey" else "chatgpt"
            else -> if (openAiApiKey != null) "apiKey" else "chatgpt"
        }
        return if (authMode == "apiKey") {
            val apiKey = openAiApiKey
                ?: throw IOException("Agent auth file is missing OPENAI_API_KEY")
            AuthSnapshot(
                authMode = authMode,
                bearerToken = apiKey,
                accountId = null,
            )
        } else {
            val tokens = json.optJSONObject("tokens")
                ?: throw IOException("Agent auth file is missing chatgpt tokens")
            val accessToken = tokens.stringOrNull("access_token")
                ?: throw IOException("Agent auth file is missing access_token")
            AuthSnapshot(
                authMode = "chatgpt",
                bearerToken = accessToken,
                accountId = tokens.stringOrNull("account_id"),
            )
        }
    }

    private fun executeRequest(
        upstreamUrl: String,
        requestBody: String,
        authSnapshot: AuthSnapshot,
    ): CodexdLocalClient.HttpResponse {
        val connection = (URL(upstreamUrl).openConnection() as HttpURLConnection).apply {
            requestMethod = "POST"
            connectTimeout = CONNECT_TIMEOUT_MS
            readTimeout = READ_TIMEOUT_MS
            doInput = true
            doOutput = true
            instanceFollowRedirects = true
            setRequestProperty("Authorization", "Bearer ${authSnapshot.bearerToken}")
            setRequestProperty("Content-Type", "application/json")
            setRequestProperty("Accept", "text/event-stream")
            setRequestProperty("Accept-Encoding", "identity")
            setRequestProperty("originator", DEFAULT_ORIGINATOR)
            setRequestProperty("User-Agent", DEFAULT_USER_AGENT)
            if (authSnapshot.authMode == "chatgpt" && !authSnapshot.accountId.isNullOrBlank()) {
                setRequestProperty("ChatGPT-Account-ID", authSnapshot.accountId)
            }
        }
        return try {
            connection.outputStream.use { output ->
                output.write(requestBody.toByteArray(StandardCharsets.UTF_8))
                output.flush()
            }
            val statusCode = connection.responseCode
            val stream = if (statusCode >= 400) connection.errorStream else connection.inputStream
            val responseBody = stream?.bufferedReader(StandardCharsets.UTF_8)?.use { it.readText() }
                .orEmpty()
            CodexdLocalClient.HttpResponse(
                statusCode = statusCode,
                body = responseBody,
            )
        } finally {
            connection.disconnect()
        }
    }

    private fun JSONObject.stringOrNull(key: String): String? {
        if (!has(key) || isNull(key)) {
            return null
        }
        return optString(key).ifBlank { null }
    }
}
