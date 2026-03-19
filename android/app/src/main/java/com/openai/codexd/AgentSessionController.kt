package com.openai.codexd

import android.app.agent.AgentManager
import android.app.agent.AgentSessionEvent
import android.app.agent.AgentSessionInfo
import android.content.Context
import android.os.Binder
import android.os.Process
import java.util.concurrent.Executor

class AgentSessionController(context: Context) {
    companion object {
        private const val PREFERRED_GENIE_PACKAGE = "com.openai.codex.genie"
    }

    private val agentManager = context.getSystemService(AgentManager::class.java)

    fun isAvailable(): Boolean = agentManager != null

    fun registerSessionListener(
        executor: Executor,
        listener: AgentManager.SessionListener,
    ): Boolean {
        val manager = agentManager ?: return false
        manager.registerSessionListener(currentUserId(), executor, listener)
        return true
    }

    fun unregisterSessionListener(listener: AgentManager.SessionListener) {
        agentManager?.unregisterSessionListener(listener)
    }

    fun registerSessionUiLease(parentSessionId: String, token: Binder) {
        agentManager?.registerSessionUiLease(parentSessionId, token)
    }

    fun unregisterSessionUiLease(parentSessionId: String, token: Binder) {
        agentManager?.unregisterSessionUiLease(parentSessionId, token)
    }

    fun loadSnapshot(focusedSessionId: String?): AgentSnapshot {
        val manager = agentManager ?: return AgentSnapshot.unavailable
        val roleHolders = manager.getGenieRoleHolders(currentUserId())
        val selectedGeniePackage = selectGeniePackage(roleHolders)
        val sessionDetails = manager.getSessions(currentUserId()).map { session ->
            val events = manager.getSessionEvents(session.sessionId)
            AgentSessionDetails(
                sessionId = session.sessionId,
                parentSessionId = session.parentSessionId,
                targetPackage = session.targetPackage,
                anchor = session.anchor,
                state = session.state,
                stateLabel = stateToString(session.state),
                targetDetached = session.isTargetDetached,
                latestQuestion = findLastEventMessage(events, AgentSessionEvent.TYPE_QUESTION),
                latestResult = findLastEventMessage(events, AgentSessionEvent.TYPE_RESULT),
                latestError = findLastEventMessage(events, AgentSessionEvent.TYPE_ERROR),
                latestTrace = findLastEventMessage(events, AgentSessionEvent.TYPE_TRACE),
                timeline = renderTimeline(events),
            )
        }
        val selectedSession = chooseSelectedSession(sessionDetails, focusedSessionId)
        val parentSession = findParentSession(sessionDetails, selectedSession)
        val relatedSessions = if (parentSession == null) {
            selectedSession?.let(::listOf) ?: emptyList()
        } else {
            sessionDetails.filter { session ->
                session.sessionId == parentSession.sessionId ||
                    session.parentSessionId == parentSession.sessionId
            }.sortedWith(compareBy<AgentSessionDetails> { it.parentSessionId != null }.thenBy { it.sessionId })
        }
        return AgentSnapshot(
            available = true,
            roleHolders = roleHolders,
            selectedGeniePackage = selectedGeniePackage,
            sessions = sessionDetails,
            selectedSession = selectedSession,
            parentSession = parentSession,
            relatedSessions = relatedSessions,
        )
    }

    fun startDirectSession(
        targetPackage: String,
        prompt: String,
        allowDetachedMode: Boolean,
    ): SessionStartResult {
        val manager = requireAgentManager()
        val geniePackage = selectGeniePackage(manager.getGenieRoleHolders(currentUserId()))
            ?: throw IllegalStateException("No GENIE role holder configured")
        val parentSession = manager.createDirectSession(currentUserId())
        val childSessionIds = mutableListOf<String>()
        try {
            manager.publishTrace(
                parentSession.sessionId,
                "Starting Codex direct session for $targetPackage.",
            )
            val childSession = manager.createChildSession(parentSession.sessionId, targetPackage)
            childSessionIds += childSession.sessionId
            manager.publishTrace(
                parentSession.sessionId,
                "Created child session ${childSession.sessionId} for $targetPackage.",
            )
            manager.startGenieSession(
                childSession.sessionId,
                geniePackage,
                prompt,
                allowDetachedMode,
            )
            return SessionStartResult(
                parentSessionId = parentSession.sessionId,
                childSessionId = childSession.sessionId,
                geniePackage = geniePackage,
            )
        } catch (err: RuntimeException) {
            childSessionIds.forEach { childSessionId ->
                runCatching { manager.cancelSession(childSessionId) }
            }
            runCatching { manager.cancelSession(parentSession.sessionId) }
            throw err
        }
    }

    fun answerQuestion(sessionId: String, answer: String, parentSessionId: String?) {
        val manager = requireAgentManager()
        manager.answerQuestion(sessionId, answer)
        if (parentSessionId != null) {
            manager.publishTrace(parentSessionId, "Answered question for $sessionId: $answer")
        }
    }

    fun attachTarget(sessionId: String) {
        requireAgentManager().attachTarget(sessionId)
    }

    fun cancelSession(sessionId: String) {
        requireAgentManager().cancelSession(sessionId)
    }

    fun cancelActiveSessions(): CancelActiveSessionsResult {
        val manager = requireAgentManager()
        val activeSessions = manager.getSessions(currentUserId())
            .filterNot { isTerminalState(it.state) }
            .sortedWith(
                compareByDescending<AgentSessionInfo> { it.parentSessionId != null }
                    .thenBy { it.sessionId },
            )
        val cancelledSessionIds = mutableListOf<String>()
        val failedSessionIds = mutableMapOf<String, String>()
        activeSessions.forEach { session ->
            runCatching {
                manager.cancelSession(session.sessionId)
            }.onSuccess {
                cancelledSessionIds += session.sessionId
            }.onFailure { err ->
                failedSessionIds[session.sessionId] = err.message ?: err::class.java.simpleName
            }
        }
        return CancelActiveSessionsResult(
            cancelledSessionIds = cancelledSessionIds,
            failedSessionIds = failedSessionIds,
        )
    }

    private fun requireAgentManager(): AgentManager {
        return checkNotNull(agentManager) { "AgentManager unavailable" }
    }

    private fun chooseSelectedSession(
        sessions: List<AgentSessionDetails>,
        focusedSessionId: String?,
    ): AgentSessionDetails? {
        val sessionsById = sessions.associateBy(AgentSessionDetails::sessionId)
        val focusedSession = focusedSessionId?.let(sessionsById::get)
        if (focusedSession != null) {
            if (focusedSession.parentSessionId != null) {
                return focusedSession
            }
            val childCandidate = sessions.firstOrNull { session ->
                session.parentSessionId == focusedSession.sessionId &&
                    session.state == AgentSessionInfo.STATE_WAITING_FOR_USER
            } ?: sessions.firstOrNull { session ->
                session.parentSessionId == focusedSession.sessionId &&
                    !isTerminalState(session.state)
            }
            return childCandidate ?: focusedSession
        }
        return sessions.firstOrNull { session ->
            session.parentSessionId != null &&
                session.state == AgentSessionInfo.STATE_WAITING_FOR_USER
        } ?: sessions.firstOrNull { session ->
            session.parentSessionId != null && !isTerminalState(session.state)
        } ?: sessions.firstOrNull(::isDirectParentSession) ?: sessions.firstOrNull()
    }

    private fun findParentSession(
        sessions: List<AgentSessionDetails>,
        selectedSession: AgentSessionDetails?,
    ): AgentSessionDetails? {
        if (selectedSession == null) {
            return null
        }
        if (selectedSession.parentSessionId == null) {
            return if (isDirectParentSession(selectedSession)) {
                selectedSession
            } else {
                null
            }
        }
        return sessions.firstOrNull { it.sessionId == selectedSession.parentSessionId }
    }

    private fun selectGeniePackage(roleHolders: List<String>): String? {
        return when {
            roleHolders.contains(PREFERRED_GENIE_PACKAGE) -> PREFERRED_GENIE_PACKAGE
            else -> roleHolders.firstOrNull()
        }
    }

    private fun findLastEventMessage(events: List<AgentSessionEvent>, type: Int): String? {
        for (index in events.indices.reversed()) {
            val event = events[index]
            if (event.type == type && event.message != null) {
                return event.message
            }
        }
        return null
    }

    private fun renderTimeline(events: List<AgentSessionEvent>): String {
        if (events.isEmpty()) {
            return "No framework events yet."
        }
        return events.joinToString("\n") { event ->
            "${eventTypeToString(event.type)}: ${event.message ?: ""}"
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

    private fun isDirectParentSession(session: AgentSessionDetails): Boolean {
        return session.anchor == AgentSessionInfo.ANCHOR_AGENT &&
            session.parentSessionId == null &&
            session.targetPackage == null
    }

    private fun isTerminalState(state: Int): Boolean {
        return state == AgentSessionInfo.STATE_COMPLETED ||
            state == AgentSessionInfo.STATE_CANCELLED ||
            state == AgentSessionInfo.STATE_FAILED
    }

    private fun stateToString(state: Int): String {
        return when (state) {
            AgentSessionInfo.STATE_CREATED -> "CREATED"
            AgentSessionInfo.STATE_RUNNING -> "RUNNING"
            AgentSessionInfo.STATE_WAITING_FOR_USER -> "WAITING_FOR_USER"
            AgentSessionInfo.STATE_QUEUED -> "QUEUED"
            AgentSessionInfo.STATE_COMPLETED -> "COMPLETED"
            AgentSessionInfo.STATE_CANCELLED -> "CANCELLED"
            AgentSessionInfo.STATE_FAILED -> "FAILED"
            else -> state.toString()
        }
    }

    private fun currentUserId(): Int = Process.myUid() / 100000
}

data class AgentSnapshot(
    val available: Boolean,
    val roleHolders: List<String>,
    val selectedGeniePackage: String?,
    val sessions: List<AgentSessionDetails>,
    val selectedSession: AgentSessionDetails?,
    val parentSession: AgentSessionDetails?,
    val relatedSessions: List<AgentSessionDetails>,
) {
    companion object {
        val unavailable = AgentSnapshot(
            available = false,
            roleHolders = emptyList(),
            selectedGeniePackage = null,
            sessions = emptyList(),
            selectedSession = null,
            parentSession = null,
            relatedSessions = emptyList(),
        )
    }
}

data class AgentSessionDetails(
    val sessionId: String,
    val parentSessionId: String?,
    val targetPackage: String?,
    val anchor: Int,
    val state: Int,
    val stateLabel: String,
    val targetDetached: Boolean,
    val latestQuestion: String?,
    val latestResult: String?,
    val latestError: String?,
    val latestTrace: String?,
    val timeline: String,
)

data class SessionStartResult(
    val parentSessionId: String,
    val childSessionId: String,
    val geniePackage: String,
)

data class CancelActiveSessionsResult(
    val cancelledSessionIds: List<String>,
    val failedSessionIds: Map<String, String>,
)
