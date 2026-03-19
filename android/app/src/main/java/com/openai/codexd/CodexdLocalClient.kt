package com.openai.codexd

import android.content.Context
import android.net.LocalSocket
import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.nio.charset.StandardCharsets
import org.json.JSONObject

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

    data class RuntimeStatus(
        val authenticated: Boolean,
        val accountEmail: String?,
        val clientCount: Int,
        val modelProviderId: String,
        val configuredModel: String?,
        val effectiveModel: String?,
        val upstreamBaseUrl: String,
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

    fun waitForRuntimeStatus(context: Context): RuntimeStatus {
        val response = waitForResponse(context, "GET", "/internal/runtime/status", null)
        if (response.statusCode != 200) {
            throw IOException("HTTP ${response.statusCode}: ${response.body}")
        }
        return parseRuntimeStatus(response.body)
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

        val splitIndex = responseBytes.indexOfHeaderBodySeparator()
        if (splitIndex == -1) {
            throw IOException("Invalid HTTP response")
        }
        val headerText = responseBytes
            .copyOfRange(0, splitIndex)
            .toString(StandardCharsets.UTF_8)
        val statusLine = headerText
            .lineSequence()
            .firstOrNull()
            .orEmpty()
        val statusCode = statusLine.split(" ").getOrNull(1)?.toIntOrNull()
            ?: throw IOException("Missing status code")
        val bodyBytes = responseBytes.copyOfRange(splitIndex + 4, responseBytes.size)
        val decodedBodyBytes = if (headerText.contains("Transfer-Encoding: chunked", ignoreCase = true)) {
            decodeChunkedBody(bodyBytes)
        } else {
            bodyBytes
        }
        return HttpResponse(
            statusCode = statusCode,
            body = decodedBodyBytes.toString(StandardCharsets.UTF_8),
        )
    }

    private fun ByteArray.indexOfHeaderBodySeparator(): Int {
        for (index in 0 until size - 3) {
            if (
                this[index] == '\r'.code.toByte() &&
                this[index + 1] == '\n'.code.toByte() &&
                this[index + 2] == '\r'.code.toByte() &&
                this[index + 3] == '\n'.code.toByte()
            ) {
                return index
            }
        }
        return -1
    }

    private fun decodeChunkedBody(bodyBytes: ByteArray): ByteArray {
        val output = ByteArrayOutputStream(bodyBytes.size)
        var cursor = 0
        while (cursor < bodyBytes.size) {
            val lineEnd = bodyBytes.indexOfCrlf(cursor)
            if (lineEnd == -1) {
                throw IOException("Invalid chunked response")
            }
            val sizeLine = bodyBytes
                .copyOfRange(cursor, lineEnd)
                .toString(StandardCharsets.US_ASCII)
                .substringBefore(';')
                .trim()
            val chunkSize = sizeLine.toIntOrNull(radix = 16)
                ?: throw IOException("Invalid chunk size: $sizeLine")
            cursor = lineEnd + 2
            if (chunkSize == 0) {
                break
            }
            val nextCursor = cursor + chunkSize
            if (nextCursor > bodyBytes.size) {
                throw IOException("Chunk exceeds body length")
            }
            output.write(bodyBytes, cursor, chunkSize)
            cursor = nextCursor
            if (
                cursor + 1 >= bodyBytes.size ||
                bodyBytes[cursor] != '\r'.code.toByte() ||
                bodyBytes[cursor + 1] != '\n'.code.toByte()
            ) {
                throw IOException("Invalid chunk terminator")
            }
            cursor += 2
        }
        return output.toByteArray()
    }

    private fun ByteArray.indexOfCrlf(startIndex: Int): Int {
        for (index in startIndex until size - 1) {
            if (this[index] == '\r'.code.toByte() && this[index + 1] == '\n'.code.toByte()) {
                return index
            }
        }
        return -1
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

    private fun parseRuntimeStatus(body: String): RuntimeStatus {
        val json = JSONObject(body)
        return RuntimeStatus(
            authenticated = json.optBoolean("authenticated", false),
            accountEmail = if (json.isNull("accountEmail")) null else json.optString("accountEmail"),
            clientCount = json.optInt("clientCount", 0),
            modelProviderId = json.optString("modelProviderId", "unknown"),
            configuredModel = if (json.isNull("configuredModel")) null else json.optString("configuredModel"),
            effectiveModel = if (json.isNull("effectiveModel")) null else json.optString("effectiveModel"),
            upstreamBaseUrl = json.optString("upstreamBaseUrl", "unknown"),
        )
    }
}
