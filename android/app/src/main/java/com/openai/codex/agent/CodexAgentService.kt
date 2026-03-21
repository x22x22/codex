package com.openai.codex.agent

import android.app.agent.AgentManager
import android.app.agent.AgentService
import android.app.agent.AgentSessionEvent
import android.app.agent.AgentSessionInfo
import android.os.Process
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
        private val handledGenieQuestions = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
        private val pendingGenieQuestions = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
        private val pendingQuestionLoads = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
        private val handledBridgeRequests = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
        private val pendingParentRollups = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
    }

    private sealed class AutoAnswerResult {
        data object Answered : AutoAnswerResult()

        data class Escalate(
            val question: String,
        ) : AutoAnswerResult()
    }

    private val agentManager by lazy { getSystemService(AgentManager::class.java) }
    private val sessionController by lazy { AgentSessionController(this) }
    private val presentationPolicyStore by lazy { SessionPresentationPolicyStore(this) }

    override fun onCreate() {
        super.onCreate()
    }

    override fun onSessionChanged(session: AgentSessionInfo) {
        Log.i(TAG, "onSessionChanged $session")
        maybeRollUpParentSession(session)
        agentManager?.let { manager ->
            if (shouldServeSessionBridge(session)) {
                AgentSessionBridgeServer.ensureStarted(this, manager, session.sessionId)
            } else if (isTerminalSessionState(session.state)) {
                AgentSessionBridgeServer.closeSession(session.sessionId)
            }
        }
        if (session.state != AgentSessionInfo.STATE_WAITING_FOR_USER) {
            AgentQuestionNotifier.cancel(this, session.sessionId)
            return
        }
        if (!pendingQuestionLoads.add(session.sessionId)) {
            return
        }
        thread(name = "CodexAgentQuestionLoad-${session.sessionId}") {
            try {
                handleWaitingSession(session)
            } finally {
                pendingQuestionLoads.remove(session.sessionId)
            }
        }
    }

    override fun onSessionRemoved(sessionId: String) {
        Log.i(TAG, "onSessionRemoved sessionId=$sessionId")
        AgentSessionBridgeServer.closeSession(sessionId)
        AgentQuestionNotifier.cancel(this, sessionId)
        presentationPolicyStore.removePolicy(sessionId)
        handledGenieQuestions.removeIf { it.startsWith("$sessionId:") }
        handledBridgeRequests.removeIf { it.startsWith("$sessionId:") }
        pendingGenieQuestions.removeIf { it.startsWith("$sessionId:") }
    }

    private fun maybeRollUpParentSession(session: AgentSessionInfo) {
        val parentSessionId = when {
            !session.parentSessionId.isNullOrBlank() -> session.parentSessionId
            isDirectParentSession(session) -> session.sessionId
            else -> null
        } ?: return
        if (!pendingParentRollups.add(parentSessionId)) {
            return
        }
        thread(name = "CodexAgentParentRollup-$parentSessionId") {
            try {
                runCatching {
            rollUpParentSession(parentSessionId)
        }.onFailure { err ->
            Log.w(TAG, "Parent session roll-up failed for $parentSessionId", err)
                }
            } finally {
                pendingParentRollups.remove(parentSessionId)
            }
        }
    }

    private fun rollUpParentSession(parentSessionId: String) {
        val manager = agentManager ?: return
        val sessions = manager.getSessions(currentUserId())
        val parentSession = sessions.firstOrNull { it.sessionId == parentSessionId } ?: return
        if (!isDirectParentSession(parentSession)) {
            return
        }
        val childSessions = sessions.filter { it.parentSessionId == parentSessionId }
        if (childSessions.isEmpty()) {
            return
        }
        val rollup = AgentParentSessionAggregator.rollup(
            childSessions.map { childSession ->
                val events = manager.getSessionEvents(childSession.sessionId)
                ParentSessionChildSummary(
                    sessionId = childSession.sessionId,
                    targetPackage = childSession.targetPackage,
                    state = childSession.state,
                    targetPresentation = childSession.targetPresentation,
                    requiredFinalPresentationPolicy = presentationPolicyStore.getPolicy(childSession.sessionId),
                    latestResult = findLastEventMessage(events, AgentSessionEvent.TYPE_RESULT),
                    latestError = findLastEventMessage(events, AgentSessionEvent.TYPE_ERROR),
                )
            },
        )
        rollup.sessionsToAttach.forEach { childSessionId ->
            runCatching {
                manager.attachTarget(childSessionId)
                manager.publishTrace(
                    parentSessionId,
                    "Requested attach for $childSessionId to satisfy the required final presentation policy.",
                )
            }.onFailure { err ->
                Log.w(TAG, "Failed to attach target for $childSessionId", err)
            }
        }
        if (parentSession.state != rollup.state) {
            runCatching {
                manager.updateSessionState(parentSessionId, rollup.state)
            }.onFailure { err ->
                Log.w(TAG, "Failed to update parent session state for $parentSessionId", err)
            }
        }
        val parentEvents = if (rollup.resultMessage != null || rollup.errorMessage != null) {
            manager.getSessionEvents(parentSessionId)
        } else {
            emptyList()
        }
        if (rollup.resultMessage != null && findLastEventMessage(parentEvents, AgentSessionEvent.TYPE_RESULT) == null) {
            runCatching {
                manager.publishResult(parentSessionId, rollup.resultMessage)
            }.onFailure { err ->
                Log.w(TAG, "Failed to publish parent result for $parentSessionId", err)
            }
        }
        if (rollup.errorMessage != null && findLastEventMessage(parentEvents, AgentSessionEvent.TYPE_ERROR) == null) {
            runCatching {
                manager.publishError(parentSessionId, rollup.errorMessage)
            }.onFailure { err ->
                Log.w(TAG, "Failed to publish parent error for $parentSessionId", err)
            }
        }
    }

    private fun shouldServeSessionBridge(session: AgentSessionInfo): Boolean {
        if (session.targetPackage.isNullOrBlank()) {
            return false
        }
        return !isTerminalSessionState(session.state)
    }

    private fun isTerminalSessionState(state: Int): Boolean {
        return when (state) {
            AgentSessionInfo.STATE_COMPLETED,
            AgentSessionInfo.STATE_CANCELLED,
            AgentSessionInfo.STATE_FAILED,
            -> true
            else -> false
        }
    }

    private fun handleWaitingSession(session: AgentSessionInfo) {
        val manager = agentManager ?: return
        val events = manager.getSessionEvents(session.sessionId)
        val question = findLatestQuestion(events) ?: return
        updateQuestionNotification(session, question)
        maybeAutoAnswerGenieQuestion(session, question, events)
    }

    private fun maybeAutoAnswerGenieQuestion(
        session: AgentSessionInfo,
        question: String,
        events: List<AgentSessionEvent>,
    ) {
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

    private fun updateQuestionNotification(session: AgentSessionInfo, question: String) {
        if (question.isBlank()) {
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

    private fun findLastEventMessage(events: List<AgentSessionEvent>, type: Int): String? {
        return events.lastOrNull { event ->
            event.type == type && !event.message.isNullOrBlank()
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
        if (requestId.isNotBlank()) {
            val bridgeRequestKey = "${session.sessionId}:$requestId"
            if (!handledBridgeRequests.add(bridgeRequestKey)) {
                Log.i(
                    TAG,
                    "Skipping duplicate bridge question method=${request.optString("method")} requestId=$requestId session=${session.sessionId}",
                )
                return
            }
        }
        Log.i(
            TAG,
            "Answering bridge question method=${request.optString("method")} requestId=$requestId session=${session.sessionId}",
        )
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
            AgentSessionEvent.TYPE_DETACHED_ACTION -> "DetachedAction"
            AgentSessionEvent.TYPE_ANSWER -> "Answer"
            else -> "Event($type)"
        }
    }

    private fun genieQuestionKey(sessionId: String, question: String): String {
        if (isBridgeQuestion(question)) {
            val requestId = runCatching {
                JSONObject(question.removePrefix(BRIDGE_REQUEST_PREFIX)).optString("requestId").trim()
            }.getOrNull()
            if (!requestId.isNullOrEmpty()) {
                return "$sessionId:bridge:$requestId"
            }
        }
        return "$sessionId:$question"
    }

    private fun isDirectParentSession(session: AgentSessionInfo): Boolean {
        return session.anchor == AgentSessionInfo.ANCHOR_AGENT &&
            session.parentSessionId == null &&
            session.targetPackage == null
    }

    private fun currentUserId(): Int {
        return Process.myUid() / 100000
    }
}
