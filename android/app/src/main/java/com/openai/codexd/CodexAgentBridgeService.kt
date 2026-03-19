package com.openai.codexd

import android.app.Service
import android.content.Intent
import android.os.IBinder
import android.util.Log
import com.openai.codex.bridge.BridgeHttpResponse
import com.openai.codex.bridge.BridgeRuntimeStatus
import com.openai.codex.bridge.ICodexAgentBridgeService

class CodexAgentBridgeService : Service() {
    companion object {
        const val PERMISSION_BIND_AGENT_BRIDGE = "com.openai.codex.permission.BIND_AGENT_BRIDGE"
        private const val TAG = "CodexAgentBridgeSvc"
    }

    private val binder = object : ICodexAgentBridgeService.Stub() {
        override fun getRuntimeStatus(): BridgeRuntimeStatus {
            val status = runCatching {
                AgentCodexAppServerClient.readRuntimeStatus(this@CodexAgentBridgeService)
            }.getOrElse { err ->
                throw err.asBinderError("getRuntimeStatus")
            }
            Log.i(TAG, "Served runtime status")
            return BridgeRuntimeStatus(
                status.authenticated,
                status.accountEmail,
                status.clientCount,
                status.modelProviderId,
                status.configuredModel,
                status.effectiveModel,
                status.upstreamBaseUrl,
            )
        }

        override fun sendResponsesRequest(requestBody: String?): BridgeHttpResponse {
            val response = runCatching {
                AgentResponsesProxy.sendResponsesRequest(
                    this@CodexAgentBridgeService,
                    requestBody.orEmpty(),
                )
            }.getOrElse { err ->
                throw err.asBinderError("sendResponsesRequest")
            }
            Log.i(TAG, "Proxied /v1/responses")
            return BridgeHttpResponse(response.statusCode, response.body)
        }
    }

    override fun onBind(intent: Intent?): IBinder {
        return binder
    }

    private fun Throwable.asBinderError(operation: String): IllegalStateException {
        val detail = message ?: javaClass.simpleName
        val message = "$operation failed: $detail"
        Log.w(TAG, message, this)
        return IllegalStateException(message, this)
    }
}
