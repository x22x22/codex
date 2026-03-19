package com.openai.codex.genie

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.IBinder
import android.os.SystemClock
import com.openai.codex.bridge.BridgeHttpRequest
import com.openai.codex.bridge.ICodexAgentBridgeService
import java.io.Closeable
import java.io.IOException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class AgentBridgeClient(
    private val context: Context,
) : Closeable {
    companion object {
        private const val AGENT_PACKAGE = "com.openai.codexd"
        private const val AGENT_BRIDGE_SERVICE = "com.openai.codexd.CodexAgentBridgeService"
        private const val CONNECT_TIMEOUT_MS = 5_000L
    }

    private val connectLatch = CountDownLatch(1)

    @Volatile
    private var bridgeService: ICodexAgentBridgeService? = null

    @Volatile
    private var bindFailure: String? = null

    @Volatile
    private var bound = false

    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, service: IBinder?) {
            bridgeService = ICodexAgentBridgeService.Stub.asInterface(service)
            connectLatch.countDown()
        }

        override fun onServiceDisconnected(name: ComponentName?) {
            bridgeService = null
        }

        override fun onNullBinding(name: ComponentName?) {
            bindFailure = "Agent bridge returned a null binding"
            connectLatch.countDown()
        }

        override fun onBindingDied(name: ComponentName?) {
            bindFailure = "Agent bridge binding died"
            bridgeService = null
            connectLatch.countDown()
        }
    }

    fun getRuntimeStatus(): CodexAgentBridge.RuntimeStatus {
        val service = requireService()
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

    fun sendHttpRequest(method: String, path: String, body: String?): CodexAgentBridge.HttpResponse {
        val service = requireService()
        val response = service.sendHttpRequest(BridgeHttpRequest(method, path, body))
        return CodexAgentBridge.HttpResponse(
            statusCode = response.statusCode,
            body = response.body,
        )
    }

    override fun close() {
        if (!bound) {
            return
        }
        context.unbindService(connection)
        bound = false
        bridgeService = null
    }

    private fun requireService(): ICodexAgentBridgeService {
        bridgeService?.let { return it }
        synchronized(this) {
            bridgeService?.let { return it }
            ensureBound()
            val deadline = SystemClock.elapsedRealtime() + CONNECT_TIMEOUT_MS
            while (bridgeService == null && bindFailure == null) {
                val remainingMs = deadline - SystemClock.elapsedRealtime()
                if (remainingMs <= 0L) {
                    break
                }
                connectLatch.await(remainingMs, TimeUnit.MILLISECONDS)
            }
            bridgeService?.let { return it }
            throw IOException(bindFailure ?: "Timed out waiting for Agent bridge")
        }
    }

    private fun ensureBound() {
        if (bound) {
            return
        }
        val boundNow = context.bindService(
            Intent().setClassName(AGENT_PACKAGE, AGENT_BRIDGE_SERVICE),
            connection,
            Context.BIND_AUTO_CREATE,
        )
        if (!boundNow) {
            bindFailure = "Failed to bind to Agent bridge"
            connectLatch.countDown()
            throw IOException(bindFailure)
        }
        bound = true
    }
}
