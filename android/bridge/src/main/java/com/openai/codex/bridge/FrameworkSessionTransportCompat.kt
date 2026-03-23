package com.openai.codex.bridge

import android.app.agent.AgentManager
import android.app.agent.GenieService
import android.os.Bundle
import android.os.ParcelFileDescriptor
import java.lang.reflect.InvocationTargetException
import java.lang.reflect.Method
import java.nio.charset.StandardCharsets

object FrameworkSessionTransportCompat {
    private const val NETWORK_CONFIG_CLASS_NAME = "android.app.agent.AgentSessionNetworkConfig"
    private const val HTTP_BRIDGE_CLASS_NAME = "android.app.agent.FrameworkSessionHttpBridge"
    private const val HTTP_REQUEST_CLASS_NAME = "android.app.agent.FrameworkSessionHttpBridge\$HttpRequest"
    private const val HTTP_RESPONSE_CLASS_NAME = "android.app.agent.FrameworkSessionHttpBridge\$HttpResponse"
    private const val OPEN_FRAMEWORK_SESSION_BRIDGE_METHOD = "openFrameworkSessionBridge"
    private const val SET_SESSION_NETWORK_CONFIG_METHOD = "setSessionNetworkConfig"
    private const val EXECUTE_REQUEST_AND_READ_FULLY_METHOD = "executeRequestAndReadFully"

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

    private data class AvailableRuntimeApi(
        val setSessionNetworkConfigMethod: Method,
        val networkConfigConstructor: java.lang.reflect.Constructor<*>,
        val executeRequestAndReadFullyMethod: Method,
        val httpRequestConstructor: java.lang.reflect.Constructor<*>,
        val httpResponseGetStatusCodeMethod: Method,
        val httpResponseGetHeadersMethod: Method,
        val httpResponseGetBodyMethod: Method,
        val httpResponseGetBodyAsStringMethod: Method,
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

    fun openFrameworkSessionBridge(
        callback: GenieService.Callback,
        sessionId: String,
    ): ParcelFileDescriptor {
        resolveRuntimeApi()
        val method = try {
            callback.javaClass.getMethod(
                OPEN_FRAMEWORK_SESSION_BRIDGE_METHOD,
                String::class.java,
            )
        } catch (err: NoSuchMethodException) {
            throw IllegalStateException(
                "Framework session HTTP bridge callback is unavailable. The device runtime and AgentSDK are out of sync.",
                err,
            )
        }
        return invokeChecked {
            method.invoke(callback, sessionId) as ParcelFileDescriptor
        }
    }

    fun executeRequestAndReadFully(
        bridge: ParcelFileDescriptor,
        request: HttpRequest,
    ): HttpResponse {
        val requestObject = invokeChecked {
            runtimeApi.httpRequestConstructor.newInstance(
                request.method,
                request.path,
                Bundle(request.headers),
                request.body,
            )
        }
        val responseObject = invokeChecked {
            runtimeApi.executeRequestAndReadFullyMethod.invoke(null, bridge, requestObject)
        }
        val statusCode = invokeChecked {
            runtimeApi.httpResponseGetStatusCodeMethod.invoke(responseObject) as Int
        }
        val headers = invokeChecked {
            runtimeApi.httpResponseGetHeadersMethod.invoke(responseObject) as? Bundle
        } ?: Bundle.EMPTY
        val body = invokeChecked {
            runtimeApi.httpResponseGetBodyMethod.invoke(responseObject) as? ByteArray
        } ?: ByteArray(0)
        val bodyString = invokeChecked {
            runtimeApi.httpResponseGetBodyAsStringMethod.invoke(responseObject) as? String
        } ?: body.toString(StandardCharsets.UTF_8)
        return HttpResponse(
            statusCode = statusCode,
            headers = Bundle(headers),
            body = body,
            bodyString = bodyString,
        )
    }

    private fun resolveRuntimeApi(): AvailableRuntimeApi {
        return try {
            val networkConfigClass = Class.forName(NETWORK_CONFIG_CLASS_NAME)
            val httpBridgeClass = Class.forName(HTTP_BRIDGE_CLASS_NAME)
            val httpRequestClass = Class.forName(HTTP_REQUEST_CLASS_NAME)
            val httpResponseClass = Class.forName(HTTP_RESPONSE_CLASS_NAME)
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
                executeRequestAndReadFullyMethod = httpBridgeClass.getMethod(
                    EXECUTE_REQUEST_AND_READ_FULLY_METHOD,
                    ParcelFileDescriptor::class.java,
                    httpRequestClass,
                ),
                httpRequestConstructor = httpRequestClass.getConstructor(
                    String::class.java,
                    String::class.java,
                    Bundle::class.java,
                    ByteArray::class.java,
                ),
                httpResponseGetStatusCodeMethod = httpResponseClass.getMethod("getStatusCode"),
                httpResponseGetHeadersMethod = httpResponseClass.getMethod("getHeaders"),
                httpResponseGetBodyMethod = httpResponseClass.getMethod("getBody"),
                httpResponseGetBodyAsStringMethod = httpResponseClass.getMethod("getBodyAsString"),
            )
        } catch (err: ReflectiveOperationException) {
            throw IllegalStateException(
                "Framework-owned HTTP session transport APIs are unavailable. The device runtime and AgentSDK are out of sync.",
                err,
            )
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
