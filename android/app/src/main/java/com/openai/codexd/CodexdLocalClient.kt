package com.openai.codexd

import android.content.Context
import android.net.LocalSocket
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.io.BufferedInputStream
import java.nio.charset.StandardCharsets

object CodexdLocalClient {
    data class HttpResponse(
        val statusCode: Int,
        val body: String,
    )

    data class AuthStatus(
        val authenticated: Boolean,
        val accountEmail: String?,
        val clientCount: Int,
    )

    fun waitForResponse(
        context: Context,
        method: String,
        path: String,
        body: String?,
    ): HttpResponse {
        context.startForegroundService(
            android.content.Intent(context, CodexdForegroundService::class.java).apply {
                action = CodexdForegroundService.ACTION_START
                putExtra(CodexdForegroundService.EXTRA_SOCKET_PATH, CodexSocketConfig.DEFAULT_SOCKET_PATH)
                putExtra(CodexdForegroundService.EXTRA_CODEX_HOME, File(context.filesDir, "codex-home").absolutePath)
            },
        )

        repeat(30) {
            runCatching {
                executeRequest(CodexSocketConfig.DEFAULT_SOCKET_PATH, method, path, body)
            }.getOrNull()?.let { return it }
            Thread.sleep(100)
        }

        throw IOException("codexd unavailable")
    }

    fun waitForAuthStatus(context: Context): AuthStatus {
        val response = waitForResponse(context, "GET", "/internal/auth/status", null)
        if (response.statusCode != 200) {
            throw IOException("HTTP ${response.statusCode}: ${response.body}")
        }
        return parseAuthStatus(response.body)
    }

    fun fetchAuthStatus(socketPath: String): AuthStatus? {
        return try {
            val response = executeRequest(socketPath, "GET", "/internal/auth/status", null)
            if (response.statusCode != 200) {
                return null
            }
            parseAuthStatus(response.body)
        } catch (_: Exception) {
            null
        }
    }

    fun executeRequest(
        socketPath: String,
        method: String,
        path: String,
        body: String?,
    ): HttpResponse {
        val socket = LocalSocket()
        val address = CodexSocketConfig.toLocalSocketAddress(socketPath)
        socket.connect(address)
        val payload = body ?: ""
        val request = buildString {
            append("$method $path HTTP/1.1\r\n")
            append("Host: localhost\r\n")
            append("Connection: close\r\n")
            if (body != null) {
                append("Content-Type: application/json\r\n")
            }
            append("Content-Length: ${payload.toByteArray(StandardCharsets.UTF_8).size}\r\n")
            append("\r\n")
            append(payload)
        }
        val output = socket.outputStream
        output.write(request.toByteArray(StandardCharsets.UTF_8))
        output.flush()

        val responseBytes = BufferedInputStream(socket.inputStream).use { it.readBytes() }
        socket.close()

        val responseText = responseBytes.toString(StandardCharsets.UTF_8)
        val splitIndex = responseText.indexOf("\r\n\r\n")
        if (splitIndex == -1) {
            throw IOException("Invalid HTTP response")
        }
        val statusLine = responseText.substring(0, splitIndex)
            .lineSequence()
            .firstOrNull()
            .orEmpty()
        val statusCode = statusLine.split(" ").getOrNull(1)?.toIntOrNull()
            ?: throw IOException("Missing status code")
        return HttpResponse(
            statusCode = statusCode,
            body = responseText.substring(splitIndex + 4),
        )
    }

    private fun parseAuthStatus(body: String): AuthStatus {
        val json = JSONObject(body)
        val accountEmail =
            if (json.isNull("accountEmail")) null else json.optString("accountEmail")
        val clientCount = if (json.has("clientCount")) {
            json.optInt("clientCount", 0)
        } else {
            json.optInt("client_count", 0)
        }
        return AuthStatus(
            authenticated = json.optBoolean("authenticated", false),
            accountEmail = accountEmail,
            clientCount = clientCount,
        )
    }
}
