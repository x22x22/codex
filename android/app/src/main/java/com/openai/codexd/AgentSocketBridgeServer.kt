package com.openai.codexd

import android.content.Context
import android.net.LocalServerSocket
import android.net.LocalSocket
import android.net.LocalSocketAddress
import android.util.Log
import com.openai.codex.bridge.AgentSocketBridgeContract
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.nio.charset.StandardCharsets
import java.util.Collections
import java.util.concurrent.atomic.AtomicBoolean
import org.json.JSONObject

object AgentSocketBridgeServer {
    @Volatile
    private var runningServer: RunningServer? = null

    fun ensureStarted(context: Context) {
        synchronized(this) {
            if (runningServer != null) {
                return
            }
            runningServer = RunningServer(context.applicationContext).also(RunningServer::start)
        }
    }

    private class RunningServer(
        private val context: Context,
    ) : Closeable {
        companion object {
            private const val TAG = "AgentSocketBridge"
        }

        private val boundSocket = LocalSocket().apply {
            bind(
                LocalSocketAddress(
                    AgentSocketBridgeContract.SOCKET_NAME,
                    LocalSocketAddress.Namespace.ABSTRACT,
                ),
            )
        }
        private val serverSocket = LocalServerSocket(boundSocket.fileDescriptor)
        private val closed = AtomicBoolean(false)
        private val clientSockets = Collections.synchronizedSet(mutableSetOf<LocalSocket>())
        private val acceptThread = Thread(::acceptLoop, "AgentSocketBridge")

        fun start() {
            acceptThread.start()
            Log.i(TAG, "Listening on ${AgentSocketBridgeContract.SOCKET_PATH}")
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
            acceptThread.interrupt()
        }

        private fun acceptLoop() {
            while (!closed.get()) {
                val socket = try {
                    serverSocket.accept()
                } catch (err: IOException) {
                    if (!closed.get()) {
                        Log.w(TAG, "Failed to accept Agent socket bridge connection", err)
                    }
                    return
                }
                clientSockets += socket
                Thread(
                    { handleClient(socket) },
                    "AgentSocketBridgeClient",
                ).start()
            }
        }

        private fun handleClient(socket: LocalSocket) {
            socket.use { client ->
                try {
                    val request = readRequest(client.inputStream)
                    when {
                        request.method == "GET" && request.path == "/internal/runtime/status" -> {
                            writeJsonResponse(
                                output = client.outputStream,
                                statusCode = 200,
                                body = buildRuntimeStatusJson().toString(),
                            )
                        }
                        request.method == "POST" && request.path == "/v1/responses" -> {
                            AgentResponsesProxy.streamResponsesTo(
                                context = context,
                                requestBody = request.body.orEmpty(),
                                output = client.outputStream,
                            )
                        }
                        request.method != "POST" && request.path == "/v1/responses" -> {
                            writeTextResponse(
                                output = client.outputStream,
                                statusCode = 405,
                                body = "Unsupported socket bridge method: ${request.method}",
                            )
                        }
                        else -> {
                            writeTextResponse(
                                output = client.outputStream,
                                statusCode = 404,
                                body = "Unsupported socket bridge path: ${request.path}",
                            )
                        }
                    }
                } catch (err: Exception) {
                    if (!closed.get()) {
                        Log.w(TAG, "Agent socket bridge request failed", err)
                        runCatching {
                            writeTextResponse(
                                output = client.outputStream,
                                statusCode = 502,
                                body = err.message ?: err::class.java.simpleName,
                            )
                        }
                    }
                } finally {
                    clientSockets -= client
                }
            }
        }

        private fun buildRuntimeStatusJson(): JSONObject {
            val status = AgentCodexAppServerClient.readRuntimeStatus(context)
            return JSONObject()
                .put("authenticated", status.authenticated)
                .put("accountEmail", status.accountEmail)
                .put("clientCount", status.clientCount)
                .put("modelProviderId", status.modelProviderId)
                .put("configuredModel", status.configuredModel)
                .put("effectiveModel", status.effectiveModel)
                .put("upstreamBaseUrl", status.upstreamBaseUrl)
        }
    }

    private data class ParsedRequest(
        val method: String,
        val path: String,
        val body: String?,
    )

    private fun readRequest(input: InputStream): ParsedRequest {
        val headerBuffer = ByteArrayOutputStream()
        var matched = 0
        while (matched < 4) {
            val next = input.read()
            if (next == -1) {
                throw EOFException("unexpected EOF while reading Agent socket bridge request headers")
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
            ?: throw IOException("socket bridge request line missing")
        val requestParts = requestLine.split(" ", limit = 3)
        if (requestParts.size < 2) {
            throw IOException("invalid socket bridge request line: $requestLine")
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
            throw IOException("chunked socket bridge requests are unsupported")
        }

        val contentLength = headers["content-length"]?.toIntOrNull() ?: 0
        val bodyBytes = ByteArray(contentLength)
        var offset = 0
        while (offset < bodyBytes.size) {
            val read = input.read(bodyBytes, offset, bodyBytes.size - offset)
            if (read == -1) {
                throw EOFException("unexpected EOF while reading Agent socket bridge request body")
            }
            offset += read
        }

        return ParsedRequest(
            method = requestParts[0],
            path = requestParts[1],
            body = if (bodyBytes.isEmpty()) null else bodyBytes.toString(StandardCharsets.UTF_8),
        )
    }

    private fun writeJsonResponse(
        output: OutputStream,
        statusCode: Int,
        body: String,
    ) {
        writeResponse(
            output = output,
            statusCode = statusCode,
            body = body,
            contentType = "application/json; charset=utf-8",
        )
    }

    private fun writeTextResponse(
        output: OutputStream,
        statusCode: Int,
        body: String,
    ) {
        writeResponse(
            output = output,
            statusCode = statusCode,
            body = body,
            contentType = "text/plain; charset=utf-8",
        )
    }

    private fun writeResponse(
        output: OutputStream,
        statusCode: Int,
        body: String,
        contentType: String,
    ) {
        val bodyBytes = body.toByteArray(StandardCharsets.UTF_8)
        val headers = buildString {
            append("HTTP/1.1 $statusCode ${reasonPhrase(statusCode)}\r\n")
            append("Content-Type: $contentType\r\n")
            append("Content-Length: ${bodyBytes.size}\r\n")
            append("Connection: close\r\n")
            append("\r\n")
        }
        output.write(headers.toByteArray(StandardCharsets.US_ASCII))
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
