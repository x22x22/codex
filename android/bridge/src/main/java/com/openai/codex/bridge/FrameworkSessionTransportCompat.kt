package com.openai.codex.bridge

import android.app.agent.AgentManager
import android.app.agent.GenieService
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.util.Log
import java.lang.reflect.InvocationTargetException
import java.lang.reflect.Method
import java.nio.charset.StandardCharsets

object FrameworkSessionTransportCompat {
    private const val TAG = "FrameworkSessionCompat"
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

    private sealed interface RuntimeApiAvailability {
        data object Missing : RuntimeApiAvailability

        data class Available(
            val api: AvailableRuntimeApi,
        ) : RuntimeApiAvailability
    }

    private val runtimeApiAvailability: RuntimeApiAvailability by lazy(LazyThreadSafetyMode.SYNCHRONIZED) {
        resolveRuntimeApiAvailability()
    }

    fun setSessionNetworkConfig(
        agentManager: AgentManager,
        sessionId: String,
        config: SessionNetworkConfig,
    ): Boolean {
        val api = (runtimeApiAvailability as? RuntimeApiAvailability.Available)?.api ?: return false
        val platformConfig = invokeChecked {
            api.networkConfigConstructor.newInstance(
                config.baseUrl,
                Bundle(config.defaultHeaders),
                config.connectTimeoutMillis,
                config.readTimeoutMillis,
            )
        }
        invokeChecked {
            api.setSessionNetworkConfigMethod.invoke(agentManager, sessionId, platformConfig)
        }
        return true
    }

    fun openFrameworkSessionBridge(
        callback: GenieService.Callback,
        sessionId: String,
    ): ParcelFileDescriptor? {
        val api = runtimeApiAvailability
        if (api !is RuntimeApiAvailability.Available) {
            return null
        }
        val method = runCatching {
            callback.javaClass.getMethod(
                OPEN_FRAMEWORK_SESSION_BRIDGE_METHOD,
                String::class.java,
            )
        }.getOrElse { err ->
            if (err is NoSuchMethodException) {
                Log.i(
                    TAG,
                    "Framework session HTTP bridge callback is unavailable; falling back to Agent-owned transport",
                )
                return null
            }
            throw err
        }
        return invokeChecked {
            method.invoke(callback, sessionId) as ParcelFileDescriptor
        }
    }

    fun executeRequestAndReadFully(
        bridge: ParcelFileDescriptor,
        request: HttpRequest,
    ): HttpResponse {
        val api = (runtimeApiAvailability as? RuntimeApiAvailability.Available)?.api
            ?: throw IllegalStateException("Framework session HTTP bridge is unavailable")
        val requestObject = invokeChecked {
            api.httpRequestConstructor.newInstance(
                request.method,
                request.path,
                Bundle(request.headers),
                request.body,
            )
        }
        val responseObject = invokeChecked {
            api.executeRequestAndReadFullyMethod.invoke(null, bridge, requestObject)
        }
        val statusCode = invokeChecked {
            api.httpResponseGetStatusCodeMethod.invoke(responseObject) as Int
        }
        val headers = invokeChecked {
            api.httpResponseGetHeadersMethod.invoke(responseObject) as? Bundle
        } ?: Bundle.EMPTY
        val body = invokeChecked {
            api.httpResponseGetBodyMethod.invoke(responseObject) as? ByteArray
        } ?: ByteArray(0)
        val bodyString = invokeChecked {
            api.httpResponseGetBodyAsStringMethod.invoke(responseObject) as? String
        } ?: body.toString(StandardCharsets.UTF_8)
        return HttpResponse(
            statusCode = statusCode,
            headers = Bundle(headers),
            body = body,
            bodyString = bodyString,
        )
    }

    private fun resolveRuntimeApiAvailability(): RuntimeApiAvailability {
        return try {
            val networkConfigClass = Class.forName(NETWORK_CONFIG_CLASS_NAME)
            val httpBridgeClass = Class.forName(HTTP_BRIDGE_CLASS_NAME)
            val httpRequestClass = Class.forName(HTTP_REQUEST_CLASS_NAME)
            val httpResponseClass = Class.forName(HTTP_RESPONSE_CLASS_NAME)
            RuntimeApiAvailability.Available(
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
                ),
            )
        } catch (err: ReflectiveOperationException) {
            Log.i(
                TAG,
                "Framework-owned HTTP session transport APIs are unavailable; using Agent-owned fallback",
                err,
            )
            RuntimeApiAvailability.Missing
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
