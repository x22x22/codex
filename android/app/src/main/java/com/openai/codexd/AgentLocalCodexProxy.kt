package com.openai.codexd

import android.util.Log
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.EOFException
import java.io.IOException
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.nio.charset.StandardCharsets
import java.util.Collections
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

class AgentLocalCodexProxy(
    private val requestForwarder: (String) -> CodexdLocalClient.HttpResponse,
) : Closeable {
    companion object {
        private const val TAG = "AgentLocalProxy"
    }

    private val pathSecret = UUID.randomUUID().toString().replace("-", "")
    private val loopbackAddress = InetAddress.getByName("127.0.0.1")
    private val serverSocket = ServerSocket(0, 50, loopbackAddress)
    private val closed = AtomicBoolean(false)
    private val clientSockets = Collections.synchronizedSet(mutableSetOf<Socket>())
    private val acceptThread = Thread(::acceptLoop, "AgentLocalProxy")

    val baseUrl: String = "http://${loopbackAddress.hostAddress}:${serverSocket.localPort}/${pathSecret}/v1"

    fun start() {
        acceptThread.start()
        logInfo("Listening on $baseUrl")
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) {
            return
        }
        runCatching { serverSocket.close() }
        synchronized(clientSockets) {
            clientSockets.forEach { socket -> runCatching { socket.close() } }
            clientSockets.clear()
        }
        acceptThread.interrupt()
    }

    private fun acceptLoop() {
        while (!closed.get()) {
            val socket = try {
                serverSocket.accept()
            } catch (err: IOException) {
                if (!closed.get()) {
                    logWarn("Failed to accept local proxy connection", err)
                }
                return
            }
            clientSockets += socket
            Thread(
                { handleClient(socket) },
                "AgentLocalProxyClient",
            ).start()
        }
    }

    private fun handleClient(socket: Socket) {
        socket.use { client ->
            try {
                val request = readRequest(client)
                logInfo("Forwarding ${request.method} ${request.forwardPath}")
                val response = forwardResponsesRequest(request)
                writeResponse(
                    socket = client,
                    statusCode = response.statusCode,
                    body = response.body,
                    path = request.forwardPath,
                )
            } catch (err: Exception) {
                if (!closed.get()) {
                    logWarn("Local proxy request failed", err)
                    runCatching {
                        writeResponse(
                            socket = client,
                            statusCode = 502,
                            body = err.message ?: err::class.java.simpleName,
                            path = "/error",
                        )
                    }
                }
            } finally {
                clientSockets -= client
            }
        }
    }

    private fun forwardResponsesRequest(request: ParsedRequest): CodexdLocalClient.HttpResponse {
        if (request.method != "POST") {
            return CodexdLocalClient.HttpResponse(
                statusCode = 405,
                body = "Unsupported local proxy method: ${request.method}",
            )
        }
        if (request.forwardPath != "/v1/responses") {
            return CodexdLocalClient.HttpResponse(
                statusCode = 404,
                body = "Unsupported local proxy path: ${request.forwardPath}",
            )
        }
        return requestForwarder(request.body.orEmpty())
    }

    private fun readRequest(socket: Socket): ParsedRequest {
        val input = socket.getInputStream()
        val headerBuffer = ByteArrayOutputStream()
        var matched = 0
        while (matched < 4) {
            val next = input.read()
            if (next == -1) {
                throw EOFException("unexpected EOF while reading local proxy request headers")
            }
            headerBuffer.write(next)
            matched = when {
                matched == 0 && next == '\r'.code -> 1
                matched == 1 && next == '\n'.code -> 2
                matched == 2 && next == '\r'.code -> 3
                matched == 3 && next == '\n'.code -> 4
                next == '\r'.code -> 1
                else -> 0
            }
        }

        val headerBytes = headerBuffer.toByteArray()
        val headerText = headerBytes
            .copyOfRange(0, headerBytes.size - 4)
            .toString(StandardCharsets.US_ASCII)
        val lines = headerText.split("\r\n")
        val requestLine = lines.firstOrNull()
            ?: throw IOException("local proxy request line missing")
        val requestParts = requestLine.split(" ", limit = 3)
        if (requestParts.size < 2) {
            throw IOException("invalid local proxy request line: $requestLine")
        }

        val headers = mutableMapOf<String, String>()
        lines.drop(1).forEach { line ->
            val separatorIndex = line.indexOf(':')
            if (separatorIndex <= 0) {
                return@forEach
            }
            val name = line.substring(0, separatorIndex).trim().lowercase()
            val value = line.substring(separatorIndex + 1).trim()
            headers[name] = value
        }

        if (headers["transfer-encoding"]?.contains("chunked", ignoreCase = true) == true) {
            throw IOException("chunked local proxy requests are unsupported")
        }

        val contentLength = headers["content-length"]?.toIntOrNull() ?: 0
        val bodyBytes = ByteArray(contentLength)
        var offset = 0
        while (offset < bodyBytes.size) {
            val read = input.read(bodyBytes, offset, bodyBytes.size - offset)
            if (read == -1) {
                throw EOFException("unexpected EOF while reading local proxy request body")
            }
            offset += read
        }

        val rawPath = requestParts[1]
        val forwardPath = normalizeForwardPath(rawPath)
        return ParsedRequest(
            method = requestParts[0],
            forwardPath = forwardPath,
            body = if (bodyBytes.isEmpty()) null else bodyBytes.toString(StandardCharsets.UTF_8),
        )
    }

    private fun normalizeForwardPath(rawPath: String): String {
        val expectedPrefix = "/$pathSecret"
        if (!rawPath.startsWith(expectedPrefix)) {
            throw IOException("unexpected local proxy path: $rawPath")
        }
        val strippedPath = rawPath.removePrefix(expectedPrefix)
        return if (strippedPath.isBlank()) "/" else strippedPath
    }

    private fun writeResponse(
        socket: Socket,
        statusCode: Int,
        body: String,
        path: String,
    ) {
        val bodyBytes = body.toByteArray(StandardCharsets.UTF_8)
        val contentType = when {
            path.startsWith("/v1/responses") -> "text/event-stream; charset=utf-8"
            body.trimStart().startsWith("{") || body.trimStart().startsWith("[") -> {
                "application/json; charset=utf-8"
            }
            else -> "text/plain; charset=utf-8"
        }
        val responseHeaders = buildString {
            append("HTTP/1.1 $statusCode ${reasonPhrase(statusCode)}\r\n")
            append("Content-Type: $contentType\r\n")
            append("Content-Length: ${bodyBytes.size}\r\n")
            append("Connection: close\r\n")
            append("\r\n")
        }

        val output = socket.getOutputStream()
        output.write(responseHeaders.toByteArray(StandardCharsets.US_ASCII))
        output.write(bodyBytes)
        output.flush()
    }

    private fun reasonPhrase(statusCode: Int): String {
        return when (statusCode) {
            200 -> "OK"
            400 -> "Bad Request"
            401 -> "Unauthorized"
            403 -> "Forbidden"
            404 -> "Not Found"
            500 -> "Internal Server Error"
            502 -> "Bad Gateway"
            503 -> "Service Unavailable"
            else -> "Response"
        }
    }

    private fun logInfo(message: String) {
        runCatching { Log.i(TAG, message) }
    }

    private fun logWarn(
        message: String,
        err: Throwable,
    ) {
        runCatching { Log.w(TAG, message, err) }
    }

    private data class ParsedRequest(
        val method: String,
        val forwardPath: String,
        val body: String?,
    )
}
