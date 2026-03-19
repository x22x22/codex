package com.openai.codexd

import android.app.agent.AgentManager
import android.app.agent.AgentService
import android.app.agent.AgentSessionEvent
import android.app.agent.AgentSessionInfo
import android.util.Log
import org.json.JSONObject
import java.util.concurrent.ConcurrentHashMap
import kotlin.concurrent.thread

class CodexAgentService : AgentService() {
    companion object {
        private const val TAG = "CodexAgentService"
        private const val BRIDGE_REQUEST_PREFIX = "__codex_bridge__ "
        private const val BRIDGE_RESPONSE_PREFIX = "__codex_bridge_result__ "
        private const val METHOD_GET_AUTH_STATUS = "get_auth_status"
        private const val METHOD_HTTP_REQUEST = "http_request"
    }

    private val handledBridgeRequests = ConcurrentHashMap.newKeySet<String>()
    private val agentManager by lazy { getSystemService(AgentManager::class.java) }

    override fun onSessionChanged(session: AgentSessionInfo) {
        Log.i(TAG, "onSessionChanged $session")
        handleInternalBridgeQuestion(session.sessionId)
    }

    override fun onSessionRemoved(sessionId: String) {
        Log.i(TAG, "onSessionRemoved sessionId=$sessionId")
    }

    private fun handleInternalBridgeQuestion(sessionId: String) {
        val manager = agentManager ?: return
        val events = manager.getSessionEvents(sessionId)
        val question = events.lastOrNull { event ->
            event.type == AgentSessionEvent.TYPE_QUESTION && event.message != null
        }?.message ?: return
        if (!question.startsWith(BRIDGE_REQUEST_PREFIX)) {
            return
        }
        val requestJson = runCatching {
            JSONObject(question.removePrefix(BRIDGE_REQUEST_PREFIX))
        }.getOrElse { err ->
            Log.w(TAG, "Ignoring malformed bridge question for $sessionId", err)
            return
        }
        val requestId = requestJson.optString("requestId")
        val method = requestJson.optString("method")
        if (requestId.isBlank() || method.isBlank()) {
            return
        }
        val requestKey = "$sessionId:$requestId"
        if (hasAnswerForRequest(events, requestId) || !handledBridgeRequests.add(requestKey)) {
            return
        }

        thread(name = "CodexAgentBridge-$requestId") {
            val response = when (method) {
                METHOD_GET_AUTH_STATUS -> runCatching { CodexdLocalClient.waitForAuthStatus(this) }
                    .fold(
                        onSuccess = { status ->
                            JSONObject()
                                .put("requestId", requestId)
                                .put("ok", true)
                                .put("authenticated", status.authenticated)
                                .put("accountEmail", status.accountEmail)
                                .put("clientCount", status.clientCount)
                        },
                        onFailure = { err ->
                            JSONObject()
                                .put("requestId", requestId)
                                .put("ok", false)
                            .put("error", err.message ?: err::class.java.simpleName)
                    },
                )
                METHOD_HTTP_REQUEST -> {
                    val httpMethod = requestJson.optString("httpMethod")
                    val path = requestJson.optString("path")
                    val body = if (requestJson.isNull("body")) null else requestJson.optString("body")
                    if (httpMethod.isBlank() || path.isBlank()) {
                        JSONObject()
                            .put("requestId", requestId)
                            .put("ok", false)
                            .put("error", "Missing httpMethod or path")
                    } else {
                        runCatching {
                            CodexdLocalClient.waitForResponse(this, httpMethod, path, body)
                        }.fold(
                            onSuccess = { httpResponse ->
                                JSONObject()
                                    .put("requestId", requestId)
                                    .put("ok", true)
                                    .put("statusCode", httpResponse.statusCode)
                                    .put("body", httpResponse.body)
                            },
                            onFailure = { err ->
                                JSONObject()
                                    .put("requestId", requestId)
                                    .put("ok", false)
                                    .put("error", err.message ?: err::class.java.simpleName)
                            },
                        )
                    }
                }
                else -> JSONObject()
                    .put("requestId", requestId)
                    .put("ok", false)
                    .put("error", "Unknown bridge method: $method")
            }

            runCatching {
                manager.answerQuestion(sessionId, "$BRIDGE_RESPONSE_PREFIX$response")
            }.onFailure { err ->
                handledBridgeRequests.remove(requestKey)
                Log.w(TAG, "Failed to answer bridge question for $sessionId", err)
            }
        }
    }

    private fun hasAnswerForRequest(events: List<AgentSessionEvent>, requestId: String): Boolean {
        return events.any { event ->
            if (event.type != AgentSessionEvent.TYPE_ANSWER || event.message == null) {
                return@any false
            }
            val message = event.message
            if (!message.startsWith(BRIDGE_RESPONSE_PREFIX)) {
                return@any false
            }
            runCatching {
                JSONObject(message.removePrefix(BRIDGE_RESPONSE_PREFIX)).optString("requestId")
            }.getOrNull() == requestId
        }
    }
}
