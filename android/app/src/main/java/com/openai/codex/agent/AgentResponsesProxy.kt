package com.openai.codex.agent

import android.content.Context
import android.os.Bundle
import android.util.Log
import com.openai.codex.bridge.FrameworkSessionTransportCompat
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.SocketException
import java.net.URL
import java.nio.charset.StandardCharsets
import org.json.JSONObject

object AgentResponsesProxy {
    private const val TAG = "AgentResponsesProxy"
    private const val CONNECT_TIMEOUT_MS = 30_000
    private const val READ_TIMEOUT_MS = 0
    private const val DEFAULT_OPENAI_BASE_URL = "https://api.openai.com/v1"
    private const val DEFAULT_CHATGPT_BASE_URL = "https://chatgpt.com/backend-api/codex"
    private const val DEFAULT_ORIGINATOR = "codex_cli_rs"
    private const val DEFAULT_USER_AGENT = "codex_cli_rs/android_agent_bridge"
    private const val HEADER_AUTHORIZATION = "Authorization"
    private const val HEADER_CONTENT_TYPE = "Content-Type"
    private const val HEADER_ACCEPT = "Accept"
    private const val HEADER_ACCEPT_ENCODING = "Accept-Encoding"
    private const val HEADER_CHATGPT_ACCOUNT_ID = "ChatGPT-Account-ID"
    private const val HEADER_ORIGINATOR = "originator"
    private const val HEADER_USER_AGENT = "User-Agent"
    private const val HEADER_VALUE_BEARER_PREFIX = "Bearer "
    private const val HEADER_VALUE_APPLICATION_JSON = "application/json"
    private const val HEADER_VALUE_TEXT_EVENT_STREAM = "text/event-stream"
    private const val HEADER_VALUE_IDENTITY = "identity"

    internal data class AuthSnapshot(
        val authMode: String,
        val bearerToken: String,
        val accountId: String?,
    )

    data class HttpResponse(
        val statusCode: Int,
        val body: String,
    )

    internal data class FrameworkTransportTarget(
        val baseUrl: String,
        val responsesPath: String,
    )

    fun sendResponsesRequest(
        context: Context,
        requestBody: String,
    ): HttpResponse {
        val authSnapshot = loadAuthSnapshot(File(context.filesDir, "codex-home/auth.json"))
        val upstreamUrl = buildResponsesUrl(upstreamBaseUrl = "provider-default", authMode = authSnapshot.authMode)
        val requestBodyBytes = requestBody.toByteArray(StandardCharsets.UTF_8)
        Log.i(
            TAG,
            "Proxying /v1/responses -> $upstreamUrl (auth_mode=${authSnapshot.authMode}, bytes=${requestBodyBytes.size})",
        )
        return executeRequest(upstreamUrl, requestBodyBytes, authSnapshot)
    }

    internal fun buildFrameworkSessionNetworkConfig(
        context: Context,
        upstreamBaseUrl: String,
    ): FrameworkSessionTransportCompat.SessionNetworkConfig {
        val authSnapshot = loadAuthSnapshot(File(context.filesDir, "codex-home/auth.json"))
        val transportTarget = buildFrameworkTransportTarget(
            buildResponsesBaseUrl(upstreamBaseUrl, authSnapshot.authMode),
        )
        return FrameworkSessionTransportCompat.SessionNetworkConfig(
            baseUrl = transportTarget.baseUrl,
            defaultHeaders = buildDefaultHeaders(authSnapshot),
            connectTimeoutMillis = CONNECT_TIMEOUT_MS,
            readTimeoutMillis = READ_TIMEOUT_MS,
        )
    }

    internal fun buildFrameworkResponsesPath(responsesBaseUrl: String): String {
        return buildFrameworkTransportTarget(responsesBaseUrl).responsesPath
    }

    internal fun buildResponsesBaseUrl(
        upstreamBaseUrl: String,
        authMode: String,
    ): String {
        val normalizedUpstreamBaseUrl = upstreamBaseUrl.trim()
        return when {
            normalizedUpstreamBaseUrl.isBlank() ||
                normalizedUpstreamBaseUrl == "provider-default" ||
                normalizedUpstreamBaseUrl == "null" -> {
                if (authMode == "chatgpt") {
                    DEFAULT_CHATGPT_BASE_URL
                } else {
                    DEFAULT_OPENAI_BASE_URL
                }
            }
            else -> normalizedUpstreamBaseUrl
        }.trimEnd('/')
    }

    internal fun buildResponsesUrl(
        upstreamBaseUrl: String,
        authMode: String,
    ): String {
        return "${buildResponsesBaseUrl(upstreamBaseUrl, authMode)}/responses"
    }

    internal fun buildFrameworkTransportTarget(responsesBaseUrl: String): FrameworkTransportTarget {
        val upstreamUrl = URL(responsesBaseUrl)
        val baseUrl = buildString {
            append(upstreamUrl.protocol)
            append("://")
            append(upstreamUrl.host)
            if (upstreamUrl.port != -1) {
                append(":")
                append(upstreamUrl.port)
            }
        }
        val normalizedPath = upstreamUrl.path.trimEnd('/').ifBlank { "/" }
        val responsesPath = if (normalizedPath == "/") {
            "/responses"
        } else {
            "$normalizedPath/responses"
        }
        return FrameworkTransportTarget(
            baseUrl = baseUrl,
            responsesPath = responsesPath,
        )
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
        requestBodyBytes: ByteArray,
        authSnapshot: AuthSnapshot,
    ): HttpResponse {
        val connection = openConnection(upstreamUrl, authSnapshot)
        return try {
            try {
                connection.outputStream.use { output ->
                    output.write(requestBodyBytes)
                    output.flush()
                }
            } catch (err: IOException) {
                throw wrapRequestFailure("write request body", upstreamUrl, err)
            }
            val statusCode = try {
                connection.responseCode
            } catch (err: IOException) {
                throw wrapRequestFailure("read response status", upstreamUrl, err)
            }
            val responseBody = try {
                val stream = if (statusCode >= 400) connection.errorStream else connection.inputStream
                stream?.bufferedReader(StandardCharsets.UTF_8)?.use { it.readText() }.orEmpty()
            } catch (err: IOException) {
                throw wrapRequestFailure("read response body", upstreamUrl, err)
            }
            Log.i(
                TAG,
                "Responses proxy completed status=$statusCode response_bytes=${responseBody.toByteArray(StandardCharsets.UTF_8).size}",
            )
            HttpResponse(
                statusCode = statusCode,
                body = responseBody,
            )
        } finally {
            connection.disconnect()
        }
    }

    private fun openConnection(
        upstreamUrl: String,
        authSnapshot: AuthSnapshot,
    ): HttpURLConnection {
        return try {
            (URL(upstreamUrl).openConnection() as HttpURLConnection).apply {
                requestMethod = "POST"
                connectTimeout = CONNECT_TIMEOUT_MS
                readTimeout = READ_TIMEOUT_MS
                doInput = true
                doOutput = true
                instanceFollowRedirects = true
                val defaultHeaders = buildDefaultHeaders(authSnapshot)
                defaultHeaders.keySet().forEach { key ->
                    defaultHeaders.getString(key)?.let { value ->
                        setRequestProperty(key, value)
                    }
                }
                val requestHeaders = buildResponsesRequestHeaders()
                requestHeaders.keySet().forEach { key ->
                    requestHeaders.getString(key)?.let { value ->
                        setRequestProperty(key, value)
                    }
                }
            }
        } catch (err: IOException) {
            throw wrapRequestFailure("open connection", upstreamUrl, err)
        }
    }

    internal fun buildDefaultHeaders(authSnapshot: AuthSnapshot): Bundle {
        return Bundle().apply {
            putString(HEADER_AUTHORIZATION, "$HEADER_VALUE_BEARER_PREFIX${authSnapshot.bearerToken}")
            putString(HEADER_ORIGINATOR, DEFAULT_ORIGINATOR)
            putString(HEADER_USER_AGENT, DEFAULT_USER_AGENT)
            if (authSnapshot.authMode == "chatgpt" && !authSnapshot.accountId.isNullOrBlank()) {
                putString(HEADER_CHATGPT_ACCOUNT_ID, authSnapshot.accountId)
            }
        }
    }

    internal fun buildResponsesRequestHeaders(): Bundle {
        return Bundle().apply {
            putString(HEADER_CONTENT_TYPE, HEADER_VALUE_APPLICATION_JSON)
            putString(HEADER_ACCEPT, HEADER_VALUE_TEXT_EVENT_STREAM)
            putString(HEADER_ACCEPT_ENCODING, HEADER_VALUE_IDENTITY)
        }
    }

    internal fun describeRequestFailure(
        phase: String,
        upstreamUrl: String,
        err: IOException,
    ): String {
        val reason = err.message?.ifBlank { err::class.java.simpleName } ?: err::class.java.simpleName
        return "Responses proxy failed during $phase for $upstreamUrl: ${err::class.java.simpleName}: $reason"
    }

    private fun wrapRequestFailure(
        phase: String,
        upstreamUrl: String,
        err: IOException,
    ): IOException {
        val wrapped = IOException(describeRequestFailure(phase, upstreamUrl, err), err)
        if (err is SocketException) {
            Log.w(TAG, wrapped.message, err)
        } else {
            Log.e(TAG, wrapped.message, err)
        }
        return wrapped
    }

    private fun JSONObject.stringOrNull(key: String): String? {
        if (!has(key) || isNull(key)) {
            return null
        }
        return optString(key).ifBlank { null }
    }
}
