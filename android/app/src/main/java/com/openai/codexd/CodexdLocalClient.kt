package com.openai.codexd

import android.content.Context
import android.net.LocalSocket
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.io.BufferedInputStream
import java.nio.charset.StandardCharsets

object CodexdLocalClient {
    data class AuthStatus(
        val authenticated: Boolean,
        val accountEmail: String?,
        val clientCount: Int,
    )

    fun waitForAuthStatus(context: Context): AuthStatus {
        context.startForegroundService(
            android.content.Intent(context, CodexdForegroundService::class.java).apply {
                action = CodexdForegroundService.ACTION_START
                putExtra(CodexdForegroundService.EXTRA_SOCKET_PATH, CodexSocketConfig.DEFAULT_SOCKET_PATH)
                putExtra(CodexdForegroundService.EXTRA_CODEX_HOME, File(context.filesDir, "codex-home").absolutePath)
            },
        )

        repeat(30) {
            fetchAuthStatus(CodexSocketConfig.DEFAULT_SOCKET_PATH)?.let { return it }
            Thread.sleep(100)
        }

        throw IOException("codexd unavailable")
    }

    fun fetchAuthStatus(socketPath: String): AuthStatus? {
        return try {
            val socket = LocalSocket()
            val address = CodexSocketConfig.toLocalSocketAddress(socketPath)
            socket.connect(address)
            val request = buildString {
                append("GET /internal/auth/status HTTP/1.1\r\n")
                append("Host: localhost\r\n")
                append("Connection: close\r\n")
                append("\r\n")
            }
            val output = socket.outputStream
            output.write(request.toByteArray(StandardCharsets.UTF_8))
            output.flush()

            val responseBytes = BufferedInputStream(socket.inputStream).use { it.readBytes() }
            socket.close()

            val responseText = responseBytes.toString(StandardCharsets.UTF_8)
            val splitIndex = responseText.indexOf("\r\n\r\n")
            if (splitIndex == -1) {
                return null
            }
            val statusLine = responseText.substring(0, splitIndex)
                .lineSequence()
                .firstOrNull()
                .orEmpty()
            val statusCode = statusLine.split(" ").getOrNull(1)?.toIntOrNull() ?: return null
            if (statusCode != 200) {
                return null
            }
            val body = responseText.substring(splitIndex + 4)
            val json = JSONObject(body)
            val accountEmail =
                if (json.isNull("accountEmail")) null else json.optString("accountEmail")
            val clientCount = if (json.has("clientCount")) {
                json.optInt("clientCount", 0)
            } else {
                json.optInt("client_count", 0)
            }
            AuthStatus(
                authenticated = json.optBoolean("authenticated", false),
                accountEmail = accountEmail,
                clientCount = clientCount,
            )
        } catch (_: Exception) {
            null
        }
    }
}
