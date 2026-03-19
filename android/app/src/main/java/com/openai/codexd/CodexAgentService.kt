package com.openai.codexd

import android.app.agent.AgentManager
import android.app.agent.AgentService
import android.app.agent.AgentSessionEvent
import android.app.agent.AgentSessionInfo
import android.os.Process
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import kotlin.concurrent.thread

class CodexAgentService : AgentService() {
    companion object {
        private const val TAG = "CodexAgentService"
        private const val BRIDGE_ANSWER_RETRY_COUNT = 10
        private const val BRIDGE_ANSWER_RETRY_DELAY_MS = 50L
        private const val BRIDGE_REQUEST_PREFIX = "__codex_bridge__ "
        private const val BRIDGE_RESPONSE_PREFIX = "__codex_bridge_result__ "
        private const val METHOD_GET_AUTH_STATUS = "get_auth_status"
        private const val METHOD_HTTP_REQUEST = "http_request"
        private const val AUTO_ANSWER_INSTRUCTIONS =
            "You are Codex acting as the Android Agent supervising a Genie execution. Reply with the exact free-form answer that should be sent back to the Genie. Keep it short and actionable. If the Genie can proceed without extra constraints, reply with exactly: continue"
        private const val MAX_AUTO_ANSWER_CONTEXT_CHARS = 800
    }

    private val handledBridgeRequests = ConcurrentHashMap.newKeySet<String>()
    private val handledGenieQuestions = ConcurrentHashMap.newKeySet<String>()
    private val pendingGenieQuestions = ConcurrentHashMap.newKeySet<String>()
    private val agentManager by lazy { getSystemService(AgentManager::class.java) }

    override fun onSessionChanged(session: AgentSessionInfo) {
        Log.i(TAG, "onSessionChanged $session")
        handleInternalBridgeQuestion(session.sessionId)
        maybeAutoAnswerGenieQuestion(session)
        updateQuestionNotification(session)
    }

    override fun onSessionRemoved(sessionId: String) {
        Log.i(TAG, "onSessionRemoved sessionId=$sessionId")
        AgentQuestionNotifier.cancel(this, sessionId)
        handledGenieQuestions.removeIf { it.startsWith("$sessionId:") }
        pendingGenieQuestions.removeIf { it.startsWith("$sessionId:") }
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
                answerQuestionWithRetry(manager, sessionId, "$BRIDGE_RESPONSE_PREFIX$response")
            }.onFailure { err ->
                handledBridgeRequests.remove(requestKey)
                Log.w(TAG, "Failed to answer bridge question for $sessionId", err)
            }
        }
    }

    private fun answerQuestionWithRetry(manager: AgentManager, sessionId: String, response: String) {
        repeat(BRIDGE_ANSWER_RETRY_COUNT) { attempt ->
            runCatching {
                manager.answerQuestion(sessionId, response)
            }.onSuccess {
                return
            }.onFailure { err ->
                if (attempt == BRIDGE_ANSWER_RETRY_COUNT - 1 || !isBridgeQuestionPending(manager, sessionId, err)) {
                    throw err
                }
                Thread.sleep(BRIDGE_ANSWER_RETRY_DELAY_MS)
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

    private fun isSessionWaitingForUser(manager: AgentManager, sessionId: String): Boolean {
        return manager.getSessions(Process.myUid() / 100000).any { session ->
            session.sessionId == sessionId &&
                session.state == AgentSessionInfo.STATE_WAITING_FOR_USER
        }
    }

    private fun isBridgeQuestionPending(
        manager: AgentManager,
        sessionId: String,
        err: Throwable,
    ): Boolean {
        return err.message?.contains("not waiting for user input", ignoreCase = true) == true ||
            !isSessionWaitingForUser(manager, sessionId)
    }

    private fun maybeAutoAnswerGenieQuestion(session: AgentSessionInfo) {
        if (session.state != AgentSessionInfo.STATE_WAITING_FOR_USER) {
            return
        }
        val manager = agentManager ?: return
        val events = manager.getSessionEvents(session.sessionId)
        val question = findVisibleQuestion(events) ?: return
        val questionKey = genieQuestionKey(session.sessionId, question)
        if (handledGenieQuestions.contains(questionKey) || !pendingGenieQuestions.add(questionKey)) {
            return
        }
        thread(name = "CodexAgentAutoAnswer-${session.sessionId}") {
            Log.i(TAG, "Attempting Agent auto-answer for ${session.sessionId}")
            runCatching {
                val answer = requestGenieAutoAnswer(session, question, events)
                answerQuestionWithRetry(manager, session.sessionId, answer)
                handledGenieQuestions.add(questionKey)
                AgentQuestionNotifier.cancel(this, session.sessionId)
                Log.i(TAG, "Auto-answered Genie question for ${session.sessionId}")
            }.onFailure { err ->
                Log.i(TAG, "Agent auto-answer unavailable for ${session.sessionId}: ${err.message}")
                if (isSessionWaitingForUser(manager, session.sessionId)) {
                    AgentQuestionNotifier.showQuestion(
                        context = this,
                        sessionId = session.sessionId,
                        targetPackage = session.targetPackage,
                        question = question,
                    )
                }
            }
            pendingGenieQuestions.remove(questionKey)
        }
    }

    private fun updateQuestionNotification(session: AgentSessionInfo) {
        if (session.state != AgentSessionInfo.STATE_WAITING_FOR_USER) {
            AgentQuestionNotifier.cancel(this, session.sessionId)
            return
        }
        val manager = agentManager ?: return
        val question = findVisibleQuestion(manager.getSessionEvents(session.sessionId))
        if (question.isNullOrBlank()) {
            AgentQuestionNotifier.cancel(this, session.sessionId)
            return
        }
        if (pendingGenieQuestions.contains(genieQuestionKey(session.sessionId, question))) {
            return
        }
        AgentQuestionNotifier.showQuestion(
            context = this,
            sessionId = session.sessionId,
            targetPackage = session.targetPackage,
            question = question,
        )
    }

    private fun requestGenieAutoAnswer(
        session: AgentSessionInfo,
        question: String,
        events: List<AgentSessionEvent>,
    ): String {
        val runtimeStatus = CodexdLocalClient.waitForRuntimeStatus(this)
        if (!runtimeStatus.authenticated) {
            throw IOException("codexd is not authenticated")
        }
        val model = runtimeStatus.effectiveModel ?: throw IOException("codexd effective model unavailable")
        val requestBody = JSONObject()
            .put("model", model)
            .put("store", false)
            .put("stream", false)
            .put("instructions", AUTO_ANSWER_INSTRUCTIONS)
            .put(
                "input",
                JSONArray().put(
                    JSONObject()
                        .put("role", "user")
                        .put(
                            "content",
                            JSONArray().put(
                                JSONObject()
                                    .put("type", "input_text")
                                    .put("text", buildAutoAnswerPrompt(session, question, events)),
                            ),
                        ),
                ),
            )
            .toString()
        val response = CodexdLocalClient.waitForResponse(this, "POST", "/v1/responses", requestBody)
        if (response.statusCode != 200) {
            throw IOException("HTTP ${response.statusCode}: ${response.body}")
        }
        return parseResponsesOutputText(response.body)
    }

    private fun buildAutoAnswerPrompt(
        session: AgentSessionInfo,
        question: String,
        events: List<AgentSessionEvent>,
    ): String {
        val recentContext = renderRecentContext(events)
        return """
            Target package: ${session.targetPackage ?: "unknown"}
            Current Genie question: $question

            Recent session context:
            $recentContext
        """.trimIndent()
    }

    private fun renderRecentContext(events: List<AgentSessionEvent>): String {
        val context = events
            .filterNot(::isInternalBridgeEvent)
            .takeLast(6)
            .joinToString("\n") { event ->
                "${eventTypeToString(event.type)}: ${event.message ?: ""}"
            }
        if (context.length <= MAX_AUTO_ANSWER_CONTEXT_CHARS) {
            return context.ifBlank { "No prior Genie context." }
        }
        return context.takeLast(MAX_AUTO_ANSWER_CONTEXT_CHARS)
    }

    private fun parseResponsesOutputText(body: String): String {
        val data = JSONObject(body)
        val directOutput = data.optString("output_text")
        if (directOutput.isNotBlank()) {
            return directOutput
        }
        val output = data.optJSONArray("output")
            ?: throw IOException("Responses payload missing output")
        val combined = buildString {
            for (outputIndex in 0 until output.length()) {
                val item = output.optJSONObject(outputIndex) ?: continue
                val content = item.optJSONArray("content") ?: continue
                for (contentIndex in 0 until content.length()) {
                    val part = content.optJSONObject(contentIndex) ?: continue
                    if (part.optString("type") == "output_text") {
                        append(part.optString("text"))
                    }
                }
            }
        }
        if (combined.isBlank()) {
            throw IOException("Responses payload missing output_text content")
        }
        return combined
    }

    private fun findVisibleQuestion(events: List<AgentSessionEvent>): String? {
        return events.lastOrNull { event ->
            event.type == AgentSessionEvent.TYPE_QUESTION &&
                !event.message.isNullOrBlank() &&
                !isInternalBridgeEvent(event)
        }?.message
    }

    private fun isInternalBridgeEvent(event: AgentSessionEvent): Boolean {
        val message = event.message ?: return false
        return when (event.type) {
            AgentSessionEvent.TYPE_QUESTION -> message.startsWith(BRIDGE_REQUEST_PREFIX)
            AgentSessionEvent.TYPE_ANSWER -> message.startsWith(BRIDGE_RESPONSE_PREFIX)
            else -> false
        }
    }

    private fun eventTypeToString(type: Int): String {
        return when (type) {
            AgentSessionEvent.TYPE_TRACE -> "Trace"
            AgentSessionEvent.TYPE_QUESTION -> "Question"
            AgentSessionEvent.TYPE_RESULT -> "Result"
            AgentSessionEvent.TYPE_ERROR -> "Error"
            AgentSessionEvent.TYPE_POLICY -> "Policy"
            AgentSessionEvent.TYPE_ANSWER -> "Answer"
            else -> "Event($type)"
        }
    }

    private fun genieQuestionKey(sessionId: String, question: String): String {
        return "$sessionId:$question"
    }
}
