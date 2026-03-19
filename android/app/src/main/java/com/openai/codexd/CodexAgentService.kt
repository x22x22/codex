package com.openai.codexd

import android.app.agent.AgentManager
import android.app.agent.AgentService
import android.app.agent.AgentSessionEvent
import android.app.agent.AgentSessionInfo
import android.util.Log
import java.io.IOException
import kotlin.concurrent.thread
import org.json.JSONObject

class CodexAgentService : AgentService() {
    companion object {
        private const val TAG = "CodexAgentService"
        private const val BRIDGE_REQUEST_PREFIX = "__codex_bridge__ "
        private const val BRIDGE_RESPONSE_PREFIX = "__codex_bridge_result__ "
        private const val BRIDGE_METHOD_GET_RUNTIME_STATUS = "getRuntimeStatus"
        private const val BRIDGE_METHOD_SEND_RESPONSES_REQUEST = "sendResponsesRequest"
        private const val AUTO_ANSWER_ESCALATE_PREFIX = "ESCALATE:"
        private const val AUTO_ANSWER_INSTRUCTIONS =
            "You are Codex acting as the Android Agent supervising a Genie execution. If you can answer the current Genie question from the available session context, call the framework session tool `android.framework.sessions.answer_question` exactly once with a short free-form answer. You may inspect current framework state with `android.framework.sessions.list`. If user input is required, do not call any framework tool. Instead reply with `ESCALATE: ` followed by the exact question the Agent should ask the user."
        private const val MAX_AUTO_ANSWER_CONTEXT_CHARS = 800
    }

    private sealed class AutoAnswerResult {
        data object Answered : AutoAnswerResult()

        data class Escalate(
            val question: String,
        ) : AutoAnswerResult()
    }

    private val handledGenieQuestions = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
    private val pendingGenieQuestions = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
    private val agentManager by lazy { getSystemService(AgentManager::class.java) }
    private val sessionController by lazy { AgentSessionController(this) }

    override fun onSessionChanged(session: AgentSessionInfo) {
        Log.i(TAG, "onSessionChanged $session")
        maybeAutoAnswerGenieQuestion(session)
        updateQuestionNotification(session)
    }

    override fun onSessionRemoved(sessionId: String) {
        Log.i(TAG, "onSessionRemoved sessionId=$sessionId")
        AgentQuestionNotifier.cancel(this, sessionId)
        handledGenieQuestions.removeIf { it.startsWith("$sessionId:") }
        pendingGenieQuestions.removeIf { it.startsWith("$sessionId:") }
    }

    private fun maybeAutoAnswerGenieQuestion(session: AgentSessionInfo) {
        if (session.state != AgentSessionInfo.STATE_WAITING_FOR_USER) {
            return
        }
        val manager = agentManager ?: return
        val events = manager.getSessionEvents(session.sessionId)
        val question = findLatestQuestion(events) ?: return
        val questionKey = genieQuestionKey(session.sessionId, question)
        if (handledGenieQuestions.contains(questionKey) || !pendingGenieQuestions.add(questionKey)) {
            return
        }
        thread(name = "CodexAgentAutoAnswer-${session.sessionId}") {
            Log.i(TAG, "Attempting Agent auto-answer for ${session.sessionId}")
            runCatching {
                if (isBridgeQuestion(question)) {
                    answerBridgeQuestion(session, question)
                    handledGenieQuestions.add(questionKey)
                    AgentQuestionNotifier.cancel(this, session.sessionId)
                    Log.i(TAG, "Answered bridge question for ${session.sessionId}")
                } else {
                    when (val result = requestGenieAutoAnswer(session, question, events)) {
                        AutoAnswerResult.Answered -> {
                            handledGenieQuestions.add(questionKey)
                            AgentQuestionNotifier.cancel(this, session.sessionId)
                            Log.i(TAG, "Auto-answered Genie question for ${session.sessionId}")
                        }
                        is AutoAnswerResult.Escalate -> {
                            if (sessionController.isSessionWaitingForUser(session.sessionId)) {
                                AgentQuestionNotifier.showQuestion(
                                    context = this,
                                    sessionId = session.sessionId,
                                    targetPackage = session.targetPackage,
                                    question = result.question,
                                )
                            }
                        }
                    }
                }
            }.onFailure { err ->
                Log.i(TAG, "Agent auto-answer unavailable for ${session.sessionId}: ${err.message}")
                if (!isBridgeQuestion(question) && sessionController.isSessionWaitingForUser(session.sessionId)) {
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
        val question = findLatestQuestion(manager.getSessionEvents(session.sessionId))
        if (question.isNullOrBlank()) {
            AgentQuestionNotifier.cancel(this, session.sessionId)
            return
        }
        if (isBridgeQuestion(question)) {
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
    ): AutoAnswerResult {
        val runtimeStatus = AgentCodexAppServerClient.readRuntimeStatus(this)
        if (!runtimeStatus.authenticated) {
            throw IOException("Agent runtime is not authenticated")
        }
        val frameworkToolBridge = AgentFrameworkToolBridge(this, sessionController)
        var answered = false
        val response = AgentCodexAppServerClient.requestText(
            context = this,
            instructions = AUTO_ANSWER_INSTRUCTIONS,
            prompt = buildAutoAnswerPrompt(session, question, events),
            dynamicTools = frameworkToolBridge.buildQuestionResolutionToolSpecs(),
            toolCallHandler = { toolName, arguments ->
                if (
                    toolName == AgentFrameworkToolBridge.ANSWER_QUESTION_TOOL &&
                    arguments.optString("sessionId").trim().isEmpty()
                ) {
                    arguments.put("sessionId", session.sessionId)
                }
                if (
                    toolName == AgentFrameworkToolBridge.ANSWER_QUESTION_TOOL &&
                    arguments.optString("parentSessionId").trim().isEmpty() &&
                    !session.parentSessionId.isNullOrBlank()
                ) {
                    arguments.put("parentSessionId", session.parentSessionId)
                }
                val toolResult = frameworkToolBridge.handleToolCall(
                    toolName = toolName,
                    arguments = arguments,
                    userObjective = question,
                    focusedSessionId = session.sessionId,
                )
                if (toolName == AgentFrameworkToolBridge.ANSWER_QUESTION_TOOL) {
                    answered = true
                }
                toolResult
            },
        ).trim()
        if (answered) {
            return AutoAnswerResult.Answered
        }
        if (response.startsWith(AUTO_ANSWER_ESCALATE_PREFIX, ignoreCase = true)) {
            val escalateQuestion = response.substringAfter(':').trim().ifEmpty { question }
            return AutoAnswerResult.Escalate(escalateQuestion)
        }
        if (response.isNotBlank()) {
            sessionController.answerQuestion(session.sessionId, response, session.parentSessionId)
            return AutoAnswerResult.Answered
        }
        throw IOException("Agent runtime did not return an answer")
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
            .takeLast(6)
            .joinToString("\n") { event ->
                "${eventTypeToString(event.type)}: ${event.message ?: ""}"
            }
        if (context.length <= MAX_AUTO_ANSWER_CONTEXT_CHARS) {
            return context.ifBlank { "No prior Genie context." }
        }
        return context.takeLast(MAX_AUTO_ANSWER_CONTEXT_CHARS)
    }

    private fun findLatestQuestion(events: List<AgentSessionEvent>): String? {
        return events.lastOrNull { event ->
            event.type == AgentSessionEvent.TYPE_QUESTION &&
                !event.message.isNullOrBlank()
        }?.message
    }

    private fun isBridgeQuestion(question: String): Boolean {
        return question.startsWith(BRIDGE_REQUEST_PREFIX)
    }

    private fun answerBridgeQuestion(
        session: AgentSessionInfo,
        question: String,
    ) {
        val request = JSONObject(question.removePrefix(BRIDGE_REQUEST_PREFIX))
        val requestId = request.optString("requestId")
        val response: JSONObject = runCatching {
            when (request.optString("method")) {
                BRIDGE_METHOD_GET_RUNTIME_STATUS -> {
                    val status = AgentCodexAppServerClient.readRuntimeStatus(this)
                    JSONObject()
                        .put("requestId", requestId)
                        .put("ok", true)
                        .put(
                            "runtimeStatus",
                            JSONObject()
                                .put("authenticated", status.authenticated)
                                .put("accountEmail", status.accountEmail)
                                .put("clientCount", status.clientCount)
                                .put("modelProviderId", status.modelProviderId)
                                .put("configuredModel", status.configuredModel)
                                .put("effectiveModel", status.effectiveModel)
                                .put("upstreamBaseUrl", status.upstreamBaseUrl),
                        )
                }
                BRIDGE_METHOD_SEND_RESPONSES_REQUEST -> {
                    val httpResponse = AgentResponsesProxy.sendResponsesRequest(
                        this,
                        request.optString("requestBody"),
                    )
                    JSONObject()
                        .put("requestId", requestId)
                        .put("ok", true)
                        .put(
                            "httpResponse",
                            JSONObject()
                                .put("statusCode", httpResponse.statusCode)
                                .put("body", httpResponse.body),
                        )
                }
                else -> JSONObject()
                    .put("requestId", requestId)
                    .put("ok", false)
                    .put("error", "Unsupported bridge method: ${request.optString("method")}")
            }
        }.getOrElse { err ->
            JSONObject()
                .put("requestId", requestId)
                .put("ok", false)
                .put("error", err.message ?: err::class.java.simpleName)
        }
        sessionController.answerQuestion(
            session.sessionId,
            BRIDGE_RESPONSE_PREFIX + response.toString(),
            session.parentSessionId,
        )
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
