package com.openai.codex.genie

import android.net.LocalServerSocket
import android.net.LocalSocket
import android.net.LocalSocketAddress
import android.util.Log
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.EOFException
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.nio.charset.StandardCharsets
import java.util.Collections
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

class GenieLocalCodexProxy(
    private val sessionId: String,
    socketDirectory: File,
    private val requestForwarder: CodexResponsesRequestForwarder,
) : Closeable {
    companion object {
        private const val TAG = "GenieLocalProxy"
    }

    private val socketFile = File(
        socketDirectory,
        "codex_${UUID.randomUUID().toString().replace("-", "").take(12)}.sock",
    )
    private val boundSocket = LocalSocket().apply {
        socketFile.delete()
        bind(LocalSocketAddress(socketFile.absolutePath, LocalSocketAddress.Namespace.FILESYSTEM))
    }
    private val serverSocket = LocalServerSocket(boundSocket.fileDescriptor)
    private val closed = AtomicBoolean(false)
    private val clientSockets = Collections.synchronizedSet(mutableSetOf<LocalSocket>())
    private val acceptThread = Thread(::acceptLoop, "GenieLocalProxy-$sessionId")

    val socketPath: String = socketFile.absolutePath

    fun start() {
        acceptThread.start()
        logInfo("Listening on $socketPath for $sessionId")
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) {
            return
        }
        runCatching { serverSocket.close() }
        runCatching { boundSocket.close() }
        synchronized(clientSockets) {
            clientSockets.forEach { socket -> runCatching { socket.close() } }
            clientSockets.clear()
        }
        runCatching { socketFile.delete() }
        acceptThread.interrupt()
    }

    private fun acceptLoop() {
        while (!closed.get()) {
            val socket = try {
                serverSocket.accept()
            } catch (err: IOException) {
                if (!closed.get()) {
                    logWarn("Failed to accept local proxy connection for $sessionId", err)
                }
                return
            }
            clientSockets += socket
            Thread(
                { handleClient(socket) },
                "GenieLocalProxyClient-$sessionId",
            ).start()
        }
    }

    private fun handleClient(socket: LocalSocket) {
        socket.use { client ->
            try {
                GenieLocalCodexHttpProxy.handleExchange(
                    input = client.inputStream,
                    output = client.outputStream,
                    requestForwarder = requestForwarder,
                    logRequest = { request ->
                        logInfo("Forwarding ${request.method} ${request.forwardPath} for $sessionId")
                    },
                )
            } catch (err: Exception) {
                if (!closed.get()) {
                    logWarn("Local proxy request failed for $sessionId", err)
                    runCatching { GenieLocalCodexHttpProxy.writeError(client.outputStream, err) }
                }
            } finally {
                clientSockets -= client
            }
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
}

internal object GenieLocalCodexHttpProxy {
    internal data class ParsedRequest(
        val method: String,
        val forwardPath: String,
        val body: String?,
    )

    fun handleExchange(
        input: InputStream,
        output: OutputStream,
        requestForwarder: CodexResponsesRequestForwarder,
        logRequest: (ParsedRequest) -> Unit = {},
    ) {
        val request = readRequest(input)
        logRequest(request)
        when {
            request.method != "POST" -> {
                writeResponse(
                    output = output,
                    statusCode = 405,
                    body = "Unsupported local proxy method: ${request.method}",
                    path = request.forwardPath,
                )
            }
            request.forwardPath != "/v1/responses" -> {
                writeResponse(
                    output = output,
                    statusCode = 404,
                    body = "Unsupported local proxy path: ${request.forwardPath}",
                    path = request.forwardPath,
                )
            }
            else -> {
                requestForwarder.openResponsesStream(request.body.orEmpty()).use { responseInput ->
                    responseInput.copyTo(output)
                }
                output.flush()
            }
        }
    }

    fun writeError(
        output: OutputStream,
        err: Throwable,
    ) {
        writeResponse(
            output = output,
            statusCode = 502,
            body = err.message ?: err::class.java.simpleName,
            path = "/error",
        )
    }

    private fun readRequest(input: InputStream): ParsedRequest {
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

        return ParsedRequest(
            method = requestParts[0],
            forwardPath = requestParts[1],
            body = if (bodyBytes.isEmpty()) null else bodyBytes.toString(StandardCharsets.UTF_8),
        )
    }

    private fun writeResponse(
        output: OutputStream,
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
}
