package com.openai.codexd

import android.app.agent.AgentSessionInfo

object AgentSessionStateValues {
    const val CREATED = AgentSessionInfo.STATE_CREATED
    const val RUNNING = AgentSessionInfo.STATE_RUNNING
    const val WAITING_FOR_USER = AgentSessionInfo.STATE_WAITING_FOR_USER
    const val COMPLETED = AgentSessionInfo.STATE_COMPLETED
    const val CANCELLED = AgentSessionInfo.STATE_CANCELLED
    const val FAILED = AgentSessionInfo.STATE_FAILED
    const val QUEUED = AgentSessionInfo.STATE_QUEUED
}

data class ParentSessionChildSummary(
    val sessionId: String,
    val targetPackage: String?,
    val state: Int,
    val latestResult: String?,
    val latestError: String?,
)

data class ParentSessionRollup(
    val state: Int,
    val resultMessage: String?,
    val errorMessage: String?,
)

object AgentParentSessionAggregator {
    fun rollup(childSessions: List<ParentSessionChildSummary>): ParentSessionRollup {
        val state = computeParentState(childSessions.map(ParentSessionChildSummary::state))
        return when (state) {
            AgentSessionInfo.STATE_COMPLETED -> ParentSessionRollup(
                state = state,
                resultMessage = buildParentResult(childSessions),
                errorMessage = null,
            )
            AgentSessionInfo.STATE_FAILED -> ParentSessionRollup(
                state = state,
                resultMessage = null,
                errorMessage = buildParentError(childSessions),
            )
            else -> ParentSessionRollup(
                state = state,
                resultMessage = null,
                errorMessage = null,
            )
        }
    }

    private fun computeParentState(childStates: List<Int>): Int {
        var anyWaiting = false
        var anyRunning = false
        var anyQueued = false
        var anyFailed = false
        var anyCancelled = false
        var anyCompleted = false
        childStates.forEach { state ->
            when (state) {
                AgentSessionInfo.STATE_WAITING_FOR_USER -> anyWaiting = true
                AgentSessionInfo.STATE_RUNNING -> anyRunning = true
                AgentSessionInfo.STATE_QUEUED -> anyQueued = true
                AgentSessionInfo.STATE_FAILED -> anyFailed = true
                AgentSessionInfo.STATE_CANCELLED -> anyCancelled = true
                AgentSessionInfo.STATE_COMPLETED -> anyCompleted = true
            }
        }
        return when {
            anyWaiting -> AgentSessionInfo.STATE_WAITING_FOR_USER
            anyRunning || anyQueued -> AgentSessionInfo.STATE_RUNNING
            anyFailed -> AgentSessionInfo.STATE_FAILED
            anyCompleted -> AgentSessionInfo.STATE_COMPLETED
            anyCancelled -> AgentSessionInfo.STATE_CANCELLED
            else -> AgentSessionInfo.STATE_CREATED
        }
    }

    private fun buildParentResult(childSessions: List<ParentSessionChildSummary>): String {
        return buildString {
            append("Completed delegated session")
            childSessions.forEach { childSession ->
                append("; ")
                append(childSession.targetPackage ?: childSession.sessionId)
                append(": ")
                append(
                    childSession.latestResult
                        ?: childSession.latestError
                        ?: stateToString(childSession.state),
                )
            }
        }
    }

    private fun buildParentError(childSessions: List<ParentSessionChildSummary>): String {
        return buildString {
            append("Delegated session failed")
            childSessions.forEach { childSession ->
                if (childSession.state != AgentSessionInfo.STATE_FAILED) {
                    return@forEach
                }
                append("; ")
                append(childSession.targetPackage ?: childSession.sessionId)
                append(": ")
                append(childSession.latestError ?: stateToString(childSession.state))
            }
        }
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
}
