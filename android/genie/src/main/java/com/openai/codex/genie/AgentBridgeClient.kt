package com.openai.codex.genie

import android.app.agent.AgentSessionInfo
import android.app.agent.GenieService
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.net.LocalSocket
import android.net.LocalSocketAddress
import android.os.IBinder
import android.os.ParcelFileDescriptor
import android.util.Log
import com.openai.codex.bridge.AgentSocketBridgeContract
import com.openai.codex.bridge.ICodexAgentBridgeService
import java.io.BufferedInputStream
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.FilterInputStream
import java.io.IOException
import java.io.InputStream
import java.nio.charset.StandardCharsets
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.UUID
import org.json.JSONObject

interface CodexResponsesRequestForwarder {
    fun openResponsesStream(body: String): InputStream
}

class AgentBridgeClient(
    context: Context,
    private val sessionId: String,
    private val callback: GenieService.Callback,
    private val control: GenieSessionControl,
) : Closeable, CodexResponsesRequestForwarder {
    companion object {
        private const val TAG = "AgentBridgeClient"
        private const val AGENT_PACKAGE = "com.openai.codexd"
        private const val AGENT_BRIDGE_SERVICE = "com.openai.codexd.CodexAgentBridgeService"
        private const val BIND_TIMEOUT_MS = 5_000L
        private const val BRIDGE_REQUEST_PREFIX = "__codex_bridge__ "
        private const val BRIDGE_RESPONSE_PREFIX = "__codex_bridge_result__ "
        private const val OP_GET_RUNTIME_STATUS = "getRuntimeStatus"
        private const val OP_SEND_RESPONSES_REQUEST = "sendResponsesRequest"
    }

    private interface Transport : Closeable {
        fun getRuntimeStatus(): CodexAgentBridge.RuntimeStatus

        fun openResponsesStream(body: String): InputStream
    }

    private val appContext = context.applicationContext
    private val bindLatch = CountDownLatch(1)
    private var bound = false
    private var bridgeService: ICodexAgentBridgeService? = null
    private val connection = object : ServiceConnection {
        override fun onServiceConnected(
            name: ComponentName?,
            service: IBinder?,
        ) {
            bridgeService = ICodexAgentBridgeService.Stub.asInterface(service)
            bindLatch.countDown()
        }

        override fun onServiceDisconnected(name: ComponentName?) {
            bridgeService = null
            bindLatch.countDown()
        }
    }
    private val transport: Transport = bindTransport()

    fun getRuntimeStatus(): CodexAgentBridge.RuntimeStatus {
        return transport.getRuntimeStatus()
    }

    override fun openResponsesStream(body: String): InputStream {
        return transport.openResponsesStream(body)
    }

    override fun close() {
        transport.close()
    }

    private fun bindTransport(): Transport {
        tryBindBridgeService()?.let { service ->
            Log.i(TAG, "Using Binder Agent bridge transport")
            return BinderTransport(service)
        }
        return runCatching {
            SocketTransport().also {
                it.getRuntimeStatus()
                Log.i(TAG, "Using socket Agent bridge transport")
            }
        }.getOrElse { err ->
            Log.w(TAG, "Socket Agent bridge transport unavailable", err)
            Log.i(TAG, "Falling back to framework Agent bridge transport")
            FrameworkTransport()
        }
    }

    private fun tryBindBridgeService(): ICodexAgentBridgeService? {
        val intent = Intent().setClassName(AGENT_PACKAGE, AGENT_BRIDGE_SERVICE)
        val boundService = runCatching {
            appContext.bindService(intent, connection, Context.BIND_AUTO_CREATE)
        }.getOrElse { err ->
            Log.w(TAG, "Binder Agent bridge transport unavailable", err)
            return null
        }
        if (!boundService) {
            return null
        }
        bound = true
        if (!bindLatch.await(BIND_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
            unbindIfNeeded()
            return null
        }
        return bridgeService
    }

    private fun unbindIfNeeded() {
        if (!bound) {
            return
        }
        runCatching { appContext.unbindService(connection) }
        bound = false
    }

    private inner class BinderTransport(
        private val service: ICodexAgentBridgeService,
    ) : Transport {
        override fun getRuntimeStatus(): CodexAgentBridge.RuntimeStatus {
            val status = service.getRuntimeStatus()
            return CodexAgentBridge.RuntimeStatus(
                authenticated = status.authenticated,
                accountEmail = status.accountEmail,
                clientCount = status.clientCount,
                modelProviderId = status.modelProviderId,
                configuredModel = status.configuredModel,
                effectiveModel = status.effectiveModel,
                upstreamBaseUrl = status.upstreamBaseUrl,
            )
        }

        override fun openResponsesStream(body: String): InputStream {
            return ParcelFileDescriptor.AutoCloseInputStream(service.openResponsesStream(body))
        }

        override fun close() {
            unbindIfNeeded()
        }
    }

    private inner class SocketTransport : Transport {
        override fun getRuntimeStatus(): CodexAgentBridge.RuntimeStatus {
            val response = executeSocketRequest("GET", "/internal/runtime/status", null)
            if (response.statusCode != 200) {
                throw IOException("HTTP ${response.statusCode}: ${response.body}")
            }
            val json = JSONObject(response.body)
            return CodexAgentBridge.RuntimeStatus(
                authenticated = json.optBoolean("authenticated", false),
                accountEmail = json.optNullableString("accountEmail"),
                clientCount = json.optInt("clientCount", 0),
                modelProviderId = json.optString("modelProviderId", "unknown"),
                configuredModel = json.optNullableString("configuredModel"),
                effectiveModel = json.optNullableString("effectiveModel"),
                upstreamBaseUrl = json.optString("upstreamBaseUrl", "unknown"),
            )
        }

        override fun openResponsesStream(body: String): InputStream {
            val socket = openSocketConnection("POST", "/v1/responses", body)
            return object : FilterInputStream(socket.inputStream) {
                override fun close() {
                    try {
                        super.close()
                    } finally {
                        socket.close()
                    }
                }
            }
        }

        override fun close() = Unit
    }

    private inner class FrameworkTransport : Transport {
        override fun getRuntimeStatus(): CodexAgentBridge.RuntimeStatus {
            val status = request(
                JSONObject().put("method", OP_GET_RUNTIME_STATUS),
            ).getJSONObject("runtimeStatus")
            return CodexAgentBridge.RuntimeStatus(
                authenticated = status.getBoolean("authenticated"),
                accountEmail = status.optNullableString("accountEmail"),
                clientCount = status.optInt("clientCount"),
                modelProviderId = status.optString("modelProviderId"),
                configuredModel = status.optNullableString("configuredModel"),
                effectiveModel = status.optNullableString("effectiveModel"),
                upstreamBaseUrl = status.optString("upstreamBaseUrl"),
            )
        }

        override fun openResponsesStream(body: String): InputStream {
            val response = request(
                JSONObject()
                    .put("method", OP_SEND_RESPONSES_REQUEST)
                    .put("requestBody", body),
            ).getJSONObject("httpResponse")
            val statusCode = response.getInt("statusCode")
            val responseBody = response.optString("body")
            val httpResponse = buildString {
                append("HTTP/1.1 $statusCode ${reasonPhrase(statusCode)}\r\n")
                append("Content-Type: text/event-stream; charset=utf-8\r\n")
                append("Content-Length: ${responseBody.toByteArray(StandardCharsets.UTF_8).size}\r\n")
                append("Connection: close\r\n")
                append("\r\n")
                append(responseBody)
            }
            return ByteArrayInputStream(httpResponse.toByteArray(StandardCharsets.UTF_8))
        }

        override fun close() = Unit

        private fun request(request: JSONObject): JSONObject {
            val requestId = UUID.randomUUID().toString()
            callback.publishQuestion(
                sessionId,
                BRIDGE_REQUEST_PREFIX + request.put("requestId", requestId).toString(),
            )
            callback.updateState(sessionId, AgentSessionInfo.STATE_WAITING_FOR_USER)
            val answer = try {
                control.waitForBridgeResponse(requestId)
            } finally {
                if (!control.cancelled) {
                    callback.updateState(sessionId, AgentSessionInfo.STATE_RUNNING)
                }
            }
            if (!answer.startsWith(BRIDGE_RESPONSE_PREFIX)) {
                throw IOException("Unexpected Agent bridge response: $answer")
            }
            val response = JSONObject(answer.removePrefix(BRIDGE_RESPONSE_PREFIX))
            if (response.optString("requestId") != requestId) {
                throw IOException("Mismatched Agent bridge response id")
            }
            if (!response.optBoolean("ok")) {
                throw IOException(response.optString("error").ifBlank { "Agent bridge request failed" })
            }
            return response
        }
    }

    private fun executeSocketRequest(
        method: String,
        path: String,
        body: String?,
    ): CodexAgentBridge.HttpResponse {
        val socket = openSocketConnection(method, path, body)
        val responseBytes = BufferedInputStream(socket.inputStream).use { it.readBytes() }
        socket.close()
        val splitIndex = responseBytes.indexOfHeaderBodySeparator()
        if (splitIndex == -1) {
            throw IOException("Invalid Agent socket bridge response")
        }
        val headerText = responseBytes
            .copyOfRange(0, splitIndex)
            .toString(StandardCharsets.UTF_8)
        val statusLine = headerText.lineSequence().firstOrNull().orEmpty()
        val statusCode = statusLine.split(" ").getOrNull(1)?.toIntOrNull()
            ?: throw IOException("Missing Agent socket bridge status code")
        val bodyBytes = responseBytes.copyOfRange(splitIndex + 4, responseBytes.size)
        return CodexAgentBridge.HttpResponse(
            statusCode = statusCode,
            body = bodyBytes.toString(StandardCharsets.UTF_8),
        )
    }

    private fun openSocketConnection(
        method: String,
        path: String,
        body: String?,
    ): LocalSocket {
        val socket = LocalSocket()
        socket.connect(
            LocalSocketAddress(
                AgentSocketBridgeContract.SOCKET_NAME,
                LocalSocketAddress.Namespace.ABSTRACT,
            ),
        )
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
        return socket
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
