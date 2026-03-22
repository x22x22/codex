package com.openai.codex.agent

import android.app.Activity
import android.app.agent.AgentManager
import android.app.agent.AgentSessionInfo
import android.content.Intent
import android.graphics.Typeface
import android.os.Binder
import android.os.Bundle
import android.util.Log
import android.view.View
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import kotlin.concurrent.thread

class SessionDetailActivity : Activity() {
    companion object {
        private const val TAG = "CodexSessionDetail"
        const val EXTRA_SESSION_ID = "sessionId"
        private const val ACTION_DEBUG_CONTINUE_SESSION =
            "com.openai.codex.agent.action.DEBUG_CONTINUE_SESSION"
        private const val EXTRA_DEBUG_PROMPT = "prompt"
    }

    private data class SessionViewState(
        val topLevelSession: AgentSessionDetails,
        val childSessions: List<AgentSessionDetails>,
        val selectedChildSession: AgentSessionDetails?,
    )

    private val sessionController by lazy { AgentSessionController(this) }
    private val dismissedSessionStore by lazy { DismissedSessionStore(this) }
    private val sessionUiLeaseToken = Binder()
    private var leasedSessionId: String? = null
    private var requestedSessionId: String? = null
    private var topLevelSessionId: String? = null
    private var selectedChildSessionId: String? = null
    private var latestSnapshot: AgentSnapshot = AgentSnapshot.unavailable
    private var refreshInFlight = false

    private val sessionListener = object : AgentManager.SessionListener {
        override fun onSessionChanged(session: AgentSessionInfo) {
            refreshSnapshot()
        }

        override fun onSessionRemoved(sessionId: String, userId: Int) {
            refreshSnapshot()
        }
    }

    private var sessionListenerRegistered = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_session_detail)
        requestedSessionId = intent.getStringExtra(EXTRA_SESSION_ID)
        setupViews()
        maybeHandleDebugIntent(intent)
    }

    override fun onResume() {
        super.onResume()
        registerSessionListenerIfNeeded()
        refreshSnapshot(force = true)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        requestedSessionId = intent.getStringExtra(EXTRA_SESSION_ID)
        topLevelSessionId = null
        selectedChildSessionId = null
        maybeHandleDebugIntent(intent)
        refreshSnapshot(force = true)
    }

    override fun onPause() {
        unregisterSessionListenerIfNeeded()
        updateSessionUiLease(null)
        super.onPause()
    }

    private fun setupViews() {
        findViewById<Button>(R.id.session_detail_cancel_button).setOnClickListener {
            cancelSession()
        }
        findViewById<Button>(R.id.session_detail_delete_button).setOnClickListener {
            deleteSession()
        }
        findViewById<Button>(R.id.session_detail_child_cancel_button).setOnClickListener {
            cancelSelectedChildSession()
        }
        findViewById<Button>(R.id.session_detail_child_delete_button).setOnClickListener {
            deleteSelectedChildSession()
        }
        findViewById<Button>(R.id.session_detail_attach_button).setOnClickListener {
            attachTarget()
        }
        findViewById<Button>(R.id.session_detail_answer_button).setOnClickListener {
            answerQuestion()
        }
        findViewById<Button>(R.id.session_detail_follow_up_button).setOnClickListener {
            sendFollowUpPrompt()
        }
    }

    private fun maybeHandleDebugIntent(intent: Intent?) {
        if (intent?.action != ACTION_DEBUG_CONTINUE_SESSION) {
            return
        }
        val prompt = intent.getStringExtra(EXTRA_DEBUG_PROMPT)?.trim().orEmpty()
        val sessionId = intent.getStringExtra(EXTRA_SESSION_ID)?.trim().orEmpty()
        if (prompt.isEmpty()) {
            intent.action = null
            return
        }
        Log.i(TAG, "Handling debug continuation for sessionId=$sessionId")
        thread {
            runCatching {
                val snapshot = sessionController.loadSnapshot(sessionId.ifEmpty { requestedSessionId })
                val viewState = resolveViewState(snapshot) ?: error("Session not found")
                Log.i(TAG, "Loaded snapshot for continuation topLevel=${viewState.topLevelSession.sessionId} child=${viewState.selectedChildSession?.sessionId}")
                continueSessionInPlaceOnce(prompt, snapshot, viewState)
            }.onFailure { err ->
                Log.w(TAG, "Debug continuation failed", err)
                showToast("Failed to continue session: ${err.message}")
            }.onSuccess { result ->
                Log.i(TAG, "Debug continuation reused topLevel=${result.parentSessionId}")
                showToast("Continued session in place")
                runOnUiThread {
                    startActivity(intentForSession(result.parentSessionId))
                }
            }
        }
        intent.action = null
    }

    private fun registerSessionListenerIfNeeded() {
        if (sessionListenerRegistered || !sessionController.isAvailable()) {
            return
        }
        sessionListenerRegistered = runCatching {
            sessionController.registerSessionListener(mainExecutor, sessionListener)
        }.getOrDefault(false)
    }

    private fun unregisterSessionListenerIfNeeded() {
        if (!sessionListenerRegistered) {
            return
        }
        runCatching { sessionController.unregisterSessionListener(sessionListener) }
        sessionListenerRegistered = false
    }

    private fun refreshSnapshot(force: Boolean = false) {
        if (!force && refreshInFlight) {
            return
        }
        refreshInFlight = true
        thread {
            try {
                val snapshot = runCatching {
                    sessionController.loadSnapshot(requestedSessionId ?: selectedChildSessionId ?: topLevelSessionId)
                }
                    .getOrElse {
                        runOnUiThread {
                            findViewById<TextView>(R.id.session_detail_summary).text =
                                "Failed to load session: ${it.message}"
                        }
                        return@thread
                    }
                latestSnapshot = snapshot
                runOnUiThread {
                    updateUi(snapshot)
                }
            } finally {
                refreshInFlight = false
            }
        }
    }

    private fun updateUi(snapshot: AgentSnapshot) {
        val viewState = resolveViewState(snapshot)
        if (viewState == null) {
            findViewById<TextView>(R.id.session_detail_summary).text = "Session not found"
            findViewById<TextView>(R.id.session_detail_child_summary).text = "Session not found"
            updateSessionUiLease(null)
            return
        }
        val topLevelSession = viewState.topLevelSession
        val selectedChildSession = viewState.selectedChildSession
        val actionableSession = selectedChildSession ?: topLevelSession
        val executionSettings = sessionController.executionSettingsForSession(topLevelSession.sessionId)
        val summary = buildString {
            append(
                SessionUiFormatter.detailSummary(
                    context = this@SessionDetailActivity,
                    session = topLevelSession,
                    parentSession = null,
                ),
            )
            if (!executionSettings.model.isNullOrBlank()) {
                append("\nModel: ${executionSettings.model}")
            }
            if (!executionSettings.reasoningEffort.isNullOrBlank()) {
                append("\nThinking depth: ${executionSettings.reasoningEffort}")
            }
        }
        findViewById<TextView>(R.id.session_detail_summary).text = summary.trimEnd()
        renderChildSessions(viewState.childSessions, selectedChildSession?.sessionId)
        findViewById<TextView>(R.id.session_detail_child_summary).text =
            selectedChildSession?.let { child ->
                SessionUiFormatter.detailSummary(
                    context = this,
                    session = child,
                    parentSession = topLevelSession,
                )
            } ?: "Select a child session to inspect it."
        findViewById<TextView>(R.id.session_detail_timeline).text = renderTimeline(topLevelSession, selectedChildSession)

        val isWaitingForUser = actionableSession.state == AgentSessionInfo.STATE_WAITING_FOR_USER &&
            !actionableSession.latestQuestion.isNullOrBlank()
        findViewById<TextView>(R.id.session_detail_question_label).visibility =
            if (isWaitingForUser) View.VISIBLE else View.GONE
        findViewById<TextView>(R.id.session_detail_question).visibility =
            if (isWaitingForUser) View.VISIBLE else View.GONE
        findViewById<EditText>(R.id.session_detail_answer_input).visibility =
            if (isWaitingForUser) View.VISIBLE else View.GONE
        findViewById<Button>(R.id.session_detail_answer_button).visibility =
            if (isWaitingForUser) View.VISIBLE else View.GONE
        findViewById<TextView>(R.id.session_detail_question).text =
            actionableSession.latestQuestion.orEmpty()

        val isTopLevelActive = !isTerminalState(topLevelSession.state)
        val topLevelActionNote = findViewById<TextView>(R.id.session_detail_top_level_action_note)
        findViewById<Button>(R.id.session_detail_cancel_button).apply {
            visibility = if (isTopLevelActive) View.VISIBLE else View.GONE
            text = if (topLevelSession.anchor == AgentSessionInfo.ANCHOR_AGENT) {
                "Cancel Child Sessions"
            } else {
                "Cancel Session"
            }
        }
        findViewById<Button>(R.id.session_detail_delete_button).visibility =
            if (isTopLevelActive) View.GONE else View.VISIBLE
        findViewById<Button>(R.id.session_detail_delete_button).text = "Delete Session"
        topLevelActionNote.visibility = View.VISIBLE
        topLevelActionNote.text = if (topLevelSession.anchor == AgentSessionInfo.ANCHOR_AGENT) {
            if (isTopLevelActive) {
                "Cancelling the top-level session cancels all active child sessions."
            } else {
                "Deleting the top-level session removes it and its child sessions from the Agent UI."
            }
        } else {
            if (isTopLevelActive) {
                "This app-scoped session is still active."
            } else {
                "Deleting this app-scoped session consumes its framework presentation and removes it from the Agent UI."
            }
        }
        val childIsSelected = selectedChildSession != null
        val isSelectedChildActive = selectedChildSession?.let { !isTerminalState(it.state) } == true
        findViewById<LinearLayout>(R.id.session_detail_child_actions).visibility =
            if (childIsSelected) View.VISIBLE else View.GONE
        findViewById<Button>(R.id.session_detail_child_cancel_button).visibility =
            if (isSelectedChildActive) View.VISIBLE else View.GONE
        findViewById<Button>(R.id.session_detail_child_delete_button).visibility =
            if (childIsSelected && !isSelectedChildActive) View.VISIBLE else View.GONE
        val canAttach = childIsSelected &&
            actionableSession.targetPresentation != AgentSessionInfo.TARGET_PRESENTATION_ATTACHED
        findViewById<Button>(R.id.session_detail_attach_button).visibility =
            if (canAttach) View.VISIBLE else View.GONE
        val supportsInPlaceContinuation = topLevelSession.anchor == AgentSessionInfo.ANCHOR_AGENT
        val continueVisibility =
            if (!isTopLevelActive && supportsInPlaceContinuation) View.VISIBLE else View.GONE
        findViewById<TextView>(R.id.session_detail_follow_up_label).apply {
            visibility = continueVisibility
            text = "Continue Direct Session"
        }
        findViewById<EditText>(R.id.session_detail_follow_up_prompt).visibility = continueVisibility
        findViewById<Button>(R.id.session_detail_follow_up_button).apply {
            visibility = continueVisibility
            text = "Send In-Place Prompt"
        }
        findViewById<TextView>(R.id.session_detail_follow_up_note).visibility =
            if (!isTopLevelActive && !supportsInPlaceContinuation) View.VISIBLE else View.GONE

        updateSessionUiLease(topLevelSession.sessionId)
    }

    private fun renderChildSessions(
        sessions: List<AgentSessionDetails>,
        selectedSessionId: String?,
    ) {
        val container = findViewById<LinearLayout>(R.id.session_detail_children_container)
        val emptyView = findViewById<TextView>(R.id.session_detail_children_empty)
        container.removeAllViews()
        emptyView.visibility = if (sessions.isEmpty()) View.VISIBLE else View.GONE
        sessions.forEach { session ->
            val row = LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                setPadding(0, dp(8), 0, dp(8))
                isClickable = true
                isFocusable = true
                setBackgroundResource(android.R.drawable.list_selector_background)
                setOnClickListener {
                    if (session.sessionId != selectedChildSessionId) {
                        selectedChildSessionId = session.sessionId
                        requestedSessionId = topLevelSessionId
                        updateUi(latestSnapshot)
                    }
                }
            }
            val title = TextView(this).apply {
                text = SessionUiFormatter.relatedSessionTitle(this@SessionDetailActivity, session)
                setTypeface(typeface, if (session.sessionId == selectedSessionId) Typeface.BOLD else Typeface.NORMAL)
            }
            val subtitle = TextView(this).apply {
                text = SessionUiFormatter.relatedSessionSubtitle(session)
            }
            row.addView(title)
            row.addView(subtitle)
            container.addView(row)
        }
    }

    private fun renderTimeline(
        topLevelSession: AgentSessionDetails,
        selectedChildSession: AgentSessionDetails?,
    ): String {
        return if (selectedChildSession == null) {
            topLevelSession.timeline
        } else {
            buildString {
                append("Top-level ${topLevelSession.sessionId}\n")
                append(topLevelSession.timeline)
                append("\n\nSelected child ${selectedChildSession.sessionId}\n")
                append(selectedChildSession.timeline)
            }
        }
    }

    private fun answerQuestion() {
        val selectedSession = currentActionableSession(latestSnapshot) ?: return
        val answerInput = findViewById<EditText>(R.id.session_detail_answer_input)
        val answer = answerInput.text.toString().trim()
        if (answer.isEmpty()) {
            answerInput.error = "Enter an answer"
            return
        }
        thread {
            runCatching {
                sessionController.answerQuestion(
                    selectedSession.sessionId,
                    answer,
                    topLevelSession(latestSnapshot)?.sessionId,
                )
            }.onFailure { err ->
                showToast("Failed to answer question: ${err.message}")
            }.onSuccess {
                answerInput.post { answerInput.text.clear() }
                showToast("Answered ${selectedSession.sessionId}")
                refreshSnapshot(force = true)
            }
        }
    }

    private fun attachTarget() {
        val selectedSession = selectedChildSession(latestSnapshot) ?: return
        thread {
            runCatching {
                sessionController.attachTarget(selectedSession.sessionId)
            }.onFailure { err ->
                showToast("Failed to attach target: ${err.message}")
            }.onSuccess {
                showToast("Attached target for ${selectedSession.sessionId}")
                refreshSnapshot(force = true)
            }
        }
    }

    private fun cancelSession() {
        val topLevelSession = topLevelSession(latestSnapshot) ?: return
        thread {
            runCatching {
                if (topLevelSession.anchor == AgentSessionInfo.ANCHOR_AGENT) {
                    val activeChildren = childSessions(latestSnapshot)
                        .filterNot { isTerminalState(it.state) }
                    activeChildren.forEach { childSession ->
                        sessionController.cancelSession(childSession.sessionId)
                    }
                } else {
                    sessionController.cancelSession(topLevelSession.sessionId)
                }
            }.onFailure { err ->
                showToast("Failed to cancel session: ${err.message}")
            }.onSuccess {
                showToast(
                    if (topLevelSession.anchor == AgentSessionInfo.ANCHOR_AGENT) {
                        "Cancelled active child sessions"
                    } else {
                        "Cancelled ${topLevelSession.sessionId}"
                    },
                )
                refreshSnapshot(force = true)
            }
        }
    }

    private fun deleteSession() {
        val topLevelSession = topLevelSession(latestSnapshot) ?: return
        thread {
            runCatching {
                if (topLevelSession.anchor == AgentSessionInfo.ANCHOR_HOME) {
                    if (topLevelSession.state == AgentSessionInfo.STATE_COMPLETED) {
                        sessionController.consumeCompletedHomeSession(topLevelSession.sessionId)
                    } else {
                        sessionController.consumeHomeSessionPresentation(topLevelSession.sessionId)
                    }
                }
                dismissedSessionStore.dismiss(topLevelSession.sessionId)
                childSessions(latestSnapshot).forEach { childSession ->
                    dismissedSessionStore.dismiss(childSession.sessionId)
                }
            }.onFailure { err ->
                showToast("Failed to delete session: ${err.message}")
            }.onSuccess {
                showToast("Deleted session")
                finish()
            }
        }
    }

    private fun cancelSelectedChildSession() {
        val selectedChildSession = selectedChildSession(latestSnapshot) ?: return
        thread {
            runCatching {
                sessionController.cancelSession(selectedChildSession.sessionId)
            }.onFailure { err ->
                showToast("Failed to cancel child session: ${err.message}")
            }.onSuccess {
                showToast("Cancelled ${selectedChildSession.sessionId}")
                refreshSnapshot(force = true)
            }
        }
    }

    private fun deleteSelectedChildSession() {
        val selectedChildSession = selectedChildSession(latestSnapshot) ?: return
        thread {
            runCatching {
                dismissedSessionStore.dismiss(selectedChildSession.sessionId)
            }.onFailure { err ->
                showToast("Failed to delete child session: ${err.message}")
            }.onSuccess {
                selectedChildSessionId = null
                showToast("Deleted child session")
                refreshSnapshot(force = true)
            }
        }
    }

    private fun sendFollowUpPrompt() {
        val promptInput = findViewById<EditText>(R.id.session_detail_follow_up_prompt)
        val prompt = promptInput.text.toString().trim()
        if (prompt.isEmpty()) {
            promptInput.error = "Enter a follow-up prompt"
            return
        }
        promptInput.text.clear()
        continueSessionInPlaceAsync(prompt, latestSnapshot)
    }

    private fun continueSessionInPlaceAsync(
        prompt: String,
        snapshot: AgentSnapshot,
    ) {
        thread {
            runCatching {
                continueSessionInPlaceOnce(prompt, snapshot)
            }.onFailure { err ->
                showToast("Failed to continue session: ${err.message}")
            }.onSuccess { result ->
                showToast("Continued session in place")
                runOnUiThread {
                    startActivity(
                        intentForSession(result.parentSessionId),
                    )
                }
            }
        }
    }

    private fun continueSessionInPlaceOnce(
        prompt: String,
        snapshot: AgentSnapshot,
        viewState: SessionViewState = resolveViewState(snapshot) ?: error("Session not found"),
    ): SessionStartResult {
        val topLevelSession = viewState.topLevelSession
        val selectedSession = viewState.selectedChildSession
            ?: viewState.childSessions.lastOrNull()
            ?: topLevelSession
        Log.i(
            TAG,
            "Continuing session topLevel=${topLevelSession.sessionId} selected=${selectedSession.sessionId} anchor=${topLevelSession.anchor}",
        )
        return AgentSessionLauncher.continueSessionInPlace(
            sourceTopLevelSession = topLevelSession,
            selectedSession = selectedSession,
            prompt = prompt,
            sessionController = sessionController,
        )
    }

    private fun topLevelSession(snapshot: AgentSnapshot): AgentSessionDetails? {
        return resolveViewState(snapshot)?.topLevelSession
    }

    private fun childSessions(snapshot: AgentSnapshot): List<AgentSessionDetails> {
        return resolveViewState(snapshot)?.childSessions.orEmpty()
    }

    private fun selectedChildSession(snapshot: AgentSnapshot): AgentSessionDetails? {
        return resolveViewState(snapshot)?.selectedChildSession
    }

    private fun currentActionableSession(snapshot: AgentSnapshot): AgentSessionDetails? {
        val viewState = resolveViewState(snapshot) ?: return null
        return viewState.selectedChildSession ?: viewState.topLevelSession
    }

    private fun resolveViewState(snapshot: AgentSnapshot): SessionViewState? {
        val sessionsById = snapshot.sessions.associateBy(AgentSessionDetails::sessionId)
        val requestedSession = requestedSessionId?.let(sessionsById::get)
        val resolvedTopLevelSession = topLevelSessionId?.let(sessionsById::get)
            ?: requestedSession?.let { session ->
                if (session.parentSessionId == null) {
                    session
                } else {
                    sessionsById[session.parentSessionId]
                }
            }
            ?: snapshot.parentSession
            ?: snapshot.selectedSession?.takeIf { it.parentSessionId == null }
            ?: SessionUiFormatter.topLevelSessions(snapshot).firstOrNull()
            ?: return null
        topLevelSessionId = resolvedTopLevelSession.sessionId
        requestedSessionId = resolvedTopLevelSession.sessionId
        val visibleChildSessions = snapshot.sessions
            .filter { session ->
                session.parentSessionId == resolvedTopLevelSession.sessionId &&
                    !dismissedSessionStore.isDismissed(session.sessionId)
            }
            .sortedBy(AgentSessionDetails::sessionId)
        val requestedChildSession = requestedSession?.takeIf { session ->
            session.parentSessionId == resolvedTopLevelSession.sessionId &&
                !dismissedSessionStore.isDismissed(session.sessionId)
        }
        val resolvedSelectedChildSession = selectedChildSessionId?.let(sessionsById::get)?.takeIf { session ->
            session.parentSessionId == resolvedTopLevelSession.sessionId &&
                !dismissedSessionStore.isDismissed(session.sessionId)
        } ?: requestedChildSession
        selectedChildSessionId = resolvedSelectedChildSession?.sessionId
        return SessionViewState(
            topLevelSession = resolvedTopLevelSession,
            childSessions = visibleChildSessions,
            selectedChildSession = resolvedSelectedChildSession,
        )
    }

    private fun updateSessionUiLease(sessionId: String?) {
        if (leasedSessionId == sessionId) {
            return
        }
        leasedSessionId?.let { previous ->
            runCatching {
                sessionController.unregisterSessionUiLease(previous, sessionUiLeaseToken)
            }
            leasedSessionId = null
        }
        sessionId?.let { current ->
            val registered = runCatching {
                sessionController.registerSessionUiLease(current, sessionUiLeaseToken)
            }
            if (registered.isSuccess) {
                leasedSessionId = current
            }
        }
    }

    private fun intentForSession(sessionId: String) =
        android.content.Intent(this, SessionDetailActivity::class.java)
            .putExtra(EXTRA_SESSION_ID, sessionId)

    private fun isTerminalState(state: Int): Boolean {
        return state == AgentSessionInfo.STATE_COMPLETED ||
            state == AgentSessionInfo.STATE_CANCELLED ||
            state == AgentSessionInfo.STATE_FAILED
    }

    private fun showToast(message: String) {
        runOnUiThread {
            Toast.makeText(this, message, Toast.LENGTH_SHORT).show()
        }
    }

    private fun dp(value: Int): Int {
        return (value * resources.displayMetrics.density).toInt()
    }
}
