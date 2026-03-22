package com.openai.codex.agent

import android.app.agent.AgentSessionInfo
import android.content.Context

object SessionUiFormatter {
    private const val MAX_LIST_DETAIL_CHARS = 96

    fun topLevelSessions(snapshot: AgentSnapshot): List<AgentSessionDetails> {
        return snapshot.sessions.filter { it.parentSessionId == null }
    }

    fun listRowTitle(
        context: Context,
        session: AgentSessionDetails,
    ): String {
        return when (session.anchor) {
            AgentSessionInfo.ANCHOR_HOME -> AppLabelResolver.loadAppLabel(context, session.targetPackage)
            AgentSessionInfo.ANCHOR_AGENT -> "Agent Session"
            else -> session.targetPackage ?: session.sessionId
        }
    }

    fun listRowSubtitle(
        context: Context,
        session: AgentSessionDetails,
    ): String {
        val detail = summarizeListDetail(
            session.latestQuestion ?: session.latestResult ?: session.latestError ?: session.latestTrace,
        )
        return buildString {
            append(anchorLabel(session.anchor))
            append(" • ")
            append(session.stateLabel)
            append(" • ")
            append(session.targetPresentationLabel)
            detail?.let {
                append(" • ")
                append(it)
            }
        }
    }

    fun detailSummary(
        context: Context,
        session: AgentSessionDetails,
        parentSession: AgentSessionDetails?,
    ): String {
        return buildString {
            append("Session: ${session.sessionId}\n")
            append("Anchor: ${anchorLabel(session.anchor)}\n")
            append("Target: ${AppLabelResolver.loadAppLabel(context, session.targetPackage)}")
            session.targetPackage?.let { append(" ($it)") }
            append("\nState: ${session.stateLabel}\n")
            append("Target presentation: ${session.targetPresentationLabel}\n")
            session.requiredFinalPresentationPolicy?.let { policy ->
                append("Required final presentation: ${policy.wireValue}\n")
            }
            parentSession?.takeIf { it.sessionId != session.sessionId }?.let {
                append("Parent: ${it.sessionId}\n")
            }
            val detail = session.latestQuestion ?: session.latestResult ?: session.latestError ?: session.latestTrace
            detail?.takeIf(String::isNotBlank)?.let {
                append("Latest: $it")
            }
        }.trimEnd()
    }

    fun relatedSessionTitle(
        context: Context,
        session: AgentSessionDetails,
    ): String {
        val targetLabel = AppLabelResolver.loadAppLabel(context, session.targetPackage)
        return buildString {
            append(anchorLabel(session.anchor))
            append(" • ")
            append(session.stateLabel)
            append(" • ")
            append(targetLabel)
            session.targetPackage?.let { append(" ($it)") }
        }
    }

    fun relatedSessionSubtitle(session: AgentSessionDetails): String {
        val detail = summarizeListDetail(
            session.latestQuestion ?: session.latestResult ?: session.latestError ?: session.latestTrace,
        )
        return buildString {
            append(session.targetPresentationLabel)
            detail?.let {
                append(" • ")
                append(it)
            }
        }
    }

    fun relatedSessionsText(
        context: Context,
        sessions: List<AgentSessionDetails>,
        selectedSessionId: String?,
    ): String {
        if (sessions.isEmpty()) {
            return "No related sessions"
        }
        return sessions.joinToString("\n") { session ->
            val marker = if (session.sessionId == selectedSessionId) "*" else "-"
            buildString {
                append(marker)
                append(" ")
                append(relatedSessionTitle(context, session))
                append(" [")
                append(relatedSessionSubtitle(session))
                append("]")
            }
        }
    }

    private fun anchorLabel(anchor: Int): String {
        return when (anchor) {
            AgentSessionInfo.ANCHOR_HOME -> "HOME"
            AgentSessionInfo.ANCHOR_AGENT -> "AGENT"
            else -> anchor.toString()
        }
    }

    private fun summarizeListDetail(detail: String?): String? {
        val trimmed = detail?.trim()?.takeIf(String::isNotEmpty) ?: return null
        return if (trimmed.length <= MAX_LIST_DETAIL_CHARS) {
            trimmed
        } else {
            trimmed.take(MAX_LIST_DETAIL_CHARS) + "…"
        }
    }
}
