package com.openai.codexd

import android.app.agent.AgentManager
import android.app.agent.AgentService
import android.app.agent.AgentSessionEvent
import android.app.agent.AgentSessionInfo
import android.os.Process
import android.util.Log
import java.io.IOException
import kotlin.concurrent.thread

class CodexAgentService : AgentService() {
    companion object {
        private const val TAG = "CodexAgentService"
        private const val BRIDGE_ANSWER_RETRY_COUNT = 10
        private const val BRIDGE_ANSWER_RETRY_DELAY_MS = 50L
        private const val AUTO_ANSWER_INSTRUCTIONS =
            "You are Codex acting as the Android Agent supervising a Genie execution. Reply with the exact free-form answer that should be sent back to the Genie. Keep it short and actionable. If the Genie can proceed without extra constraints, reply with exactly: continue"
        private const val MAX_AUTO_ANSWER_CONTEXT_CHARS = 800
    }

    private val handledGenieQuestions = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
    private val pendingGenieQuestions = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
    private val agentManager by lazy { getSystemService(AgentManager::class.java) }

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
        val runtimeStatus = AgentCodexAppServerClient.readRuntimeStatus(this)
        if (!runtimeStatus.authenticated) {
            throw IOException("Agent runtime is not authenticated")
        }
        return AgentCodexAppServerClient.requestText(
            context = this,
            instructions = AUTO_ANSWER_INSTRUCTIONS,
            prompt = buildAutoAnswerPrompt(session, question, events),
        )
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
    private fun findVisibleQuestion(events: List<AgentSessionEvent>): String? {
        return events.lastOrNull { event ->
            event.type == AgentSessionEvent.TYPE_QUESTION &&
                !event.message.isNullOrBlank()
        }?.message
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
