package com.openai.codex.bridge

import android.app.agent.AgentManager
import android.app.agent.GenieService
import android.os.Bundle
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.lang.reflect.Constructor
import java.lang.reflect.InvocationTargetException
import java.lang.reflect.Method
import java.lang.reflect.Modifier
import java.nio.charset.StandardCharsets

object FrameworkSessionTransportCompat {
    private const val NETWORK_CONFIG_CLASS_NAME = "android.app.agent.AgentSessionNetworkConfig"
    private const val HTTP_BRIDGE_CLASS_NAME = "android.app.agent.FrameworkSessionHttpBridge"
    private const val HTTP_EXCHANGE_CLASS_NAME = "android.app.agent.FrameworkHttpExchange"
    private const val HTTP_REQUEST_HEAD_CLASS_NAME = "android.app.agent.FrameworkHttpRequestHead"
    private const val HTTP_RESPONSE_HEAD_CLASS_NAME = "android.app.agent.FrameworkHttpResponseHead"
    private const val HTTP_RESPONSE_HEAD_RESULT_CLASS_NAME = "android.app.agent.FrameworkHttpResponseHeadResult"
    private const val OPEN_EXCHANGE_METHOD = "openExchange"
    private const val OPEN_REQUEST_BODY_OUTPUT_STREAM_METHOD = "openRequestBodyOutputStream"
    private const val AWAIT_RESPONSE_HEAD_METHOD = "awaitResponseHead"
    private const val OPEN_RESPONSE_BODY_INPUT_STREAM_METHOD = "openResponseBodyInputStream"
    private const val CANCEL_METHOD = "cancel"
    private const val SET_SESSION_NETWORK_CONFIG_METHOD = "setSessionNetworkConfig"
    private const val STATUS_OK_FIELD_NAME = "STATUS_OK"
    private const val READ_BUFFER_BYTES = 8192
    private const val WRITE_BUFFER_BYTES = 8192

    data class SessionNetworkConfig(
        val baseUrl: String,
        val defaultHeaders: Bundle,
        val connectTimeoutMillis: Int,
        val readTimeoutMillis: Int,
    )

    data class HttpRequest(
        val method: String,
        val path: String,
        val headers: Bundle,
        val body: ByteArray,
    )

    data class HttpResponse(
        val statusCode: Int,
        val headers: Bundle,
        val body: ByteArray,
        val bodyString: String,
    )

    private data class HttpExchange(
        val runtimeValue: Any,
    )

    private data class HttpResponseHead(
        val statusCode: Int,
        val headers: Bundle,
    )

    private data class HttpResponseHeadResult(
        val status: Int,
        val statusName: String,
        val responseHead: HttpResponseHead?,
        val message: String?,
    )

    private data class AvailableRuntimeApi(
        val setSessionNetworkConfigMethod: Method,
        val networkConfigConstructor: Constructor<*>,
        val requestHeadConstructor: Constructor<*>,
        val openExchangeMethod: Method,
        val openRequestBodyOutputStreamMethod: Method,
        val awaitResponseHeadMethod: Method,
        val openResponseBodyInputStreamMethod: Method,
        val cancelMethod: Method,
        val responseHeadResultGetStatusMethod: Method,
        val responseHeadResultGetResponseHeadMethod: Method,
        val responseHeadResultGetMessageMethod: Method?,
        val responseHeadGetStatusCodeMethod: Method,
        val responseHeadGetHeadersMethod: Method,
        val statusNamesByValue: Map<Int, String>,
        val okStatus: Int,
    )

    private val runtimeApi: AvailableRuntimeApi by lazy(LazyThreadSafetyMode.SYNCHRONIZED, ::resolveRuntimeApi)

    fun setSessionNetworkConfig(
        agentManager: AgentManager,
        sessionId: String,
        config: SessionNetworkConfig,
    ) {
        val platformConfig = invokeChecked {
            runtimeApi.networkConfigConstructor.newInstance(
                config.baseUrl,
                Bundle(config.defaultHeaders),
                config.connectTimeoutMillis,
                config.readTimeoutMillis,
            )
        }
        invokeChecked {
            runtimeApi.setSessionNetworkConfigMethod.invoke(agentManager, sessionId, platformConfig)
        }
    }

    fun executeStreamingRequest(
        callback: GenieService.Callback,
        sessionId: String,
        request: HttpRequest,
    ): HttpResponse {
        val exchange = openExchange(callback, sessionId, request)
        var cancelExchange = true
        try {
            invokeChecked {
                runtimeApi.openRequestBodyOutputStreamMethod.invoke(null, exchange.runtimeValue) as OutputStream
            }.use { requestBody ->
                writeAll(requestBody, request.body)
            }
            val responseHeadResult = awaitResponseHead(callback, sessionId, exchange)
            if (responseHeadResult.status != runtimeApi.okStatus) {
                val details = responseHeadResult.message?.takeIf(String::isNotBlank)
                val suffix = if (details == null) "" else ": $details"
                throw IOException(
                    "Framework HTTP exchange failed with ${responseHeadResult.statusName}$suffix",
                )
            }
            val responseHead = responseHeadResult.responseHead
                ?: throw IOException("Framework HTTP exchange succeeded without a response head")
            val responseBody = invokeChecked {
                runtimeApi.openResponseBodyInputStreamMethod.invoke(null, exchange.runtimeValue) as InputStream
            }.use(::readFully)
            cancelExchange = false
            return HttpResponse(
                statusCode = responseHead.statusCode,
                headers = responseHead.headers,
                body = responseBody,
                bodyString = responseBody.toString(StandardCharsets.UTF_8),
            )
        } finally {
            if (cancelExchange) {
                runCatching {
                    invokeChecked {
                        runtimeApi.cancelMethod.invoke(null, callback, sessionId, exchange.runtimeValue)
                    }
                }
            }
        }
    }

    private fun openExchange(
        callback: GenieService.Callback,
        sessionId: String,
        request: HttpRequest,
    ): HttpExchange {
        val requestHead = invokeChecked {
            runtimeApi.requestHeadConstructor.newInstance(
                request.method,
                request.path,
                Bundle(request.headers),
            )
        }
        val runtimeExchange = invokeChecked {
            runtimeApi.openExchangeMethod.invoke(null, callback, sessionId, requestHead)
                ?: throw IOException("Framework HTTP exchange opened with no exchange handle")
        }
        return HttpExchange(runtimeExchange)
    }

    private fun awaitResponseHead(
        callback: GenieService.Callback,
        sessionId: String,
        exchange: HttpExchange,
    ): HttpResponseHeadResult {
        val resultObject = invokeChecked {
            runtimeApi.awaitResponseHeadMethod.invoke(null, callback, sessionId, exchange.runtimeValue)
        }
        val status = invokeChecked {
            runtimeApi.responseHeadResultGetStatusMethod.invoke(resultObject) as Int
        }
        val responseHeadObject = invokeChecked {
            runtimeApi.responseHeadResultGetResponseHeadMethod.invoke(resultObject)
        }
        val responseHead = if (responseHeadObject == null) {
            null
        } else {
            val statusCode = invokeChecked {
                runtimeApi.responseHeadGetStatusCodeMethod.invoke(responseHeadObject) as Int
            }
            val headers = invokeChecked {
                runtimeApi.responseHeadGetHeadersMethod.invoke(responseHeadObject) as? Bundle
            } ?: Bundle.EMPTY
            HttpResponseHead(
                statusCode = statusCode,
                headers = Bundle(headers),
            )
        }
        val message = runtimeApi.responseHeadResultGetMessageMethod?.let { method ->
            invokeChecked {
                method.invoke(resultObject) as? String
            }
        }?.ifBlank { null }
        return HttpResponseHeadResult(
            status = status,
            statusName = runtimeApi.statusNamesByValue[status] ?: "STATUS_$status",
            responseHead = responseHead,
            message = message,
        )
    }

    private fun resolveRuntimeApi(): AvailableRuntimeApi {
        return try {
            val networkConfigClass = Class.forName(NETWORK_CONFIG_CLASS_NAME)
            val httpBridgeClass = Class.forName(HTTP_BRIDGE_CLASS_NAME)
            val exchangeClass = Class.forName(HTTP_EXCHANGE_CLASS_NAME)
            val requestHeadClass = Class.forName(HTTP_REQUEST_HEAD_CLASS_NAME)
            val responseHeadClass = Class.forName(HTTP_RESPONSE_HEAD_CLASS_NAME)
            val responseHeadResultClass = Class.forName(HTTP_RESPONSE_HEAD_RESULT_CLASS_NAME)
            val statusNamesByValue = responseHeadResultClass.fields
                .filter { field ->
                    Modifier.isStatic(field.modifiers) &&
                        field.type == Int::class.javaPrimitiveType &&
                        field.name.startsWith("STATUS_")
                }
                .associate { field ->
                    field.getInt(null) to field.name
                }
            val okStatus = responseHeadResultClass.getField(STATUS_OK_FIELD_NAME).getInt(null)
            AvailableRuntimeApi(
                setSessionNetworkConfigMethod = AgentManager::class.java.getMethod(
                    SET_SESSION_NETWORK_CONFIG_METHOD,
                    String::class.java,
                    networkConfigClass,
                ),
                networkConfigConstructor = networkConfigClass.getConstructor(
                    String::class.java,
                    Bundle::class.java,
                    Int::class.javaPrimitiveType,
                    Int::class.javaPrimitiveType,
                ),
                requestHeadConstructor = requestHeadClass.getConstructor(
                    String::class.java,
                    String::class.java,
                    Bundle::class.java,
                ),
                openExchangeMethod = requireMethod(
                    owner = httpBridgeClass,
                    name = OPEN_EXCHANGE_METHOD,
                    GenieService.Callback::class.java,
                    String::class.java,
                    requestHeadClass,
                ),
                openRequestBodyOutputStreamMethod = requireMethod(
                    owner = httpBridgeClass,
                    name = OPEN_REQUEST_BODY_OUTPUT_STREAM_METHOD,
                    exchangeClass,
                ),
                awaitResponseHeadMethod = requireMethod(
                    owner = httpBridgeClass,
                    name = AWAIT_RESPONSE_HEAD_METHOD,
                    GenieService.Callback::class.java,
                    String::class.java,
                    exchangeClass,
                ),
                openResponseBodyInputStreamMethod = requireMethod(
                    owner = httpBridgeClass,
                    name = OPEN_RESPONSE_BODY_INPUT_STREAM_METHOD,
                    exchangeClass,
                ),
                cancelMethod = requireMethod(
                    owner = httpBridgeClass,
                    name = CANCEL_METHOD,
                    GenieService.Callback::class.java,
                    String::class.java,
                    exchangeClass,
                ),
                responseHeadResultGetStatusMethod = requireMethod(
                    owner = responseHeadResultClass,
                    name = "getStatus",
                ),
                responseHeadResultGetResponseHeadMethod = requireMethod(
                    owner = responseHeadResultClass,
                    name = "getResponseHead",
                ),
                responseHeadResultGetMessageMethod = responseHeadResultClass.methods.firstOrNull { method ->
                    method.name == "getMessage" && method.parameterCount == 0
                },
                responseHeadGetStatusCodeMethod = requireMethod(
                    owner = responseHeadClass,
                    name = "getStatusCode",
                ),
                responseHeadGetHeadersMethod = requireMethod(
                    owner = responseHeadClass,
                    name = "getHeaders",
                ),
                statusNamesByValue = statusNamesByValue,
                okStatus = okStatus,
            )
        } catch (err: ReflectiveOperationException) {
            throw IllegalStateException(
                "Framework-owned HTTP streaming APIs are unavailable. The device runtime and AgentSDK are out of sync.",
                err,
            )
        }
    }

    private fun requireMethod(
        owner: Class<*>,
        name: String,
        vararg parameterTypes: Class<*>,
    ): Method {
        return owner.methods.firstOrNull { method ->
            method.name == name &&
                method.parameterCount == parameterTypes.size &&
                method.parameterTypes.contentEquals(parameterTypes)
        } ?: throw NoSuchMethodException(
            "${owner.name}#$name(${parameterTypes.joinToString { it.name }})",
        )
    }

    private fun writeAll(
        output: OutputStream,
        bytes: ByteArray,
    ) {
        var offset = 0
        while (offset < bytes.size) {
            val chunkSize = minOf(WRITE_BUFFER_BYTES, bytes.size - offset)
            output.write(bytes, offset, chunkSize)
            offset += chunkSize
        }
        output.flush()
    }

    private fun readFully(input: InputStream): ByteArray {
        val buffer = ByteArray(READ_BUFFER_BYTES)
        val bytes = ByteArrayOutputStream()
        while (true) {
            val read = input.read(buffer)
            if (read == -1) {
                return bytes.toByteArray()
            }
            bytes.write(buffer, 0, read)
        }
    }

    private fun <T> invokeChecked(block: () -> T): T {
        try {
            return block()
        } catch (err: InvocationTargetException) {
            throw err.targetException ?: err
        }
    }
}
