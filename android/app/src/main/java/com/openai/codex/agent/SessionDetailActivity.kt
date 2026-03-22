package com.openai.codex.agent

import android.app.Activity
import android.app.agent.AgentManager
import android.app.agent.AgentSessionInfo
import android.content.Intent
import android.os.Binder
import android.os.Bundle
import android.view.View
import android.widget.Button
import android.widget.EditText
import android.widget.TextView
import android.widget.Toast
import kotlin.concurrent.thread

class SessionDetailActivity : Activity() {
    companion object {
        const val EXTRA_SESSION_ID = "sessionId"
    }

    private val sessionController by lazy { AgentSessionController(this) }
    private val dismissedSessionStore by lazy { DismissedSessionStore(this) }
    private val sessionUiLeaseToken = Binder()
    private var leasedSessionId: String? = null
    private var focusedSessionId: String? = null
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
        focusedSessionId = intent.getStringExtra(EXTRA_SESSION_ID)
        setupViews()
    }

    override fun onResume() {
        super.onResume()
        registerSessionListenerIfNeeded()
        refreshSnapshot(force = true)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        focusedSessionId = intent.getStringExtra(EXTRA_SESSION_ID)
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
                val snapshot = runCatching { sessionController.loadSnapshot(focusedSessionId) }
                    .getOrElse {
                        runOnUiThread {
                            findViewById<TextView>(R.id.session_detail_summary).text =
                                "Failed to load session: ${it.message}"
                        }
                        return@thread
                    }
                latestSnapshot = snapshot
                val selectedSession = snapshot.selectedSession
                if (selectedSession != null) {
                    focusedSessionId = selectedSession.sessionId
                }
                runOnUiThread {
                    updateUi(snapshot)
                }
            } finally {
                refreshInFlight = false
            }
        }
    }

    private fun updateUi(snapshot: AgentSnapshot) {
        val topLevelSession = topLevelSession(snapshot)
        val selectedSession = snapshot.selectedSession
        if (topLevelSession == null || selectedSession == null) {
            findViewById<TextView>(R.id.session_detail_summary).text = "Session not found"
            updateSessionUiLease(null)
            return
        }
        val executionSettings = sessionController.executionSettingsForSession(topLevelSession.sessionId)
        val summary = buildString {
            append(
                SessionUiFormatter.detailSummary(
                    context = this@SessionDetailActivity,
                    session = selectedSession,
                    parentSession = snapshot.parentSession,
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
        findViewById<TextView>(R.id.session_detail_related_sessions).text =
            SessionUiFormatter.relatedSessionsText(this, snapshot.relatedSessions, selectedSession.sessionId)
        findViewById<TextView>(R.id.session_detail_timeline).text = renderTimeline(snapshot)

        val isWaitingForUser = selectedSession.state == AgentSessionInfo.STATE_WAITING_FOR_USER &&
            !selectedSession.latestQuestion.isNullOrBlank()
        findViewById<TextView>(R.id.session_detail_question_label).visibility =
            if (isWaitingForUser) View.VISIBLE else View.GONE
        findViewById<TextView>(R.id.session_detail_question).visibility =
            if (isWaitingForUser) View.VISIBLE else View.GONE
        findViewById<EditText>(R.id.session_detail_answer_input).visibility =
            if (isWaitingForUser) View.VISIBLE else View.GONE
        findViewById<Button>(R.id.session_detail_answer_button).visibility =
            if (isWaitingForUser) View.VISIBLE else View.GONE
        findViewById<TextView>(R.id.session_detail_question).text =
            selectedSession.latestQuestion.orEmpty()

        val isTopLevelActive = !isTerminalState(topLevelSession.state)
        findViewById<Button>(R.id.session_detail_cancel_button).visibility =
            if (isTopLevelActive) View.VISIBLE else View.GONE
        findViewById<Button>(R.id.session_detail_delete_button).visibility =
            if (isTopLevelActive) View.GONE else View.VISIBLE
        findViewById<Button>(R.id.session_detail_delete_button).text =
            if (topLevelSession.anchor == AgentSessionInfo.ANCHOR_HOME) {
                "Delete Session"
            } else {
                "Hide Session"
            }
        val canAttach = selectedSession.targetPresentation != AgentSessionInfo.TARGET_PRESENTATION_ATTACHED
        findViewById<Button>(R.id.session_detail_attach_button).visibility =
            if (canAttach) View.VISIBLE else View.GONE

        updateSessionUiLease(snapshot.parentSession?.sessionId ?: topLevelSession.sessionId)
    }

    private fun renderTimeline(snapshot: AgentSnapshot): String {
        val selectedSession = snapshot.selectedSession ?: return "No framework events yet."
        val parentSession = snapshot.parentSession
        return if (parentSession == null || parentSession.sessionId == selectedSession.sessionId) {
            selectedSession.timeline
        } else {
            buildString {
                append("Parent ${parentSession.sessionId}\n")
                append(parentSession.timeline)
                append("\n\nSelected child ${selectedSession.sessionId}\n")
                append(selectedSession.timeline)
            }
        }
    }

    private fun answerQuestion() {
        val selectedSession = latestSnapshot.selectedSession ?: return
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
                    latestSnapshot.parentSession?.sessionId,
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
        val selectedSession = latestSnapshot.selectedSession ?: return
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
                sessionController.cancelSession(topLevelSession.sessionId)
            }.onFailure { err ->
                showToast("Failed to cancel session: ${err.message}")
            }.onSuccess {
                showToast("Cancelled ${topLevelSession.sessionId}")
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
            }.onFailure { err ->
                showToast("Failed to delete session: ${err.message}")
            }.onSuccess {
                showToast(
                    if (topLevelSession.anchor == AgentSessionInfo.ANCHOR_HOME) {
                        "Deleted session"
                    } else {
                        "Hidden session from Agent UI"
                    },
                )
                finish()
            }
        }
    }

    private fun sendFollowUpPrompt() {
        val topLevelSession = topLevelSession(latestSnapshot) ?: return
        val promptInput = findViewById<EditText>(R.id.session_detail_follow_up_prompt)
        val prompt = promptInput.text.toString().trim()
        if (prompt.isEmpty()) {
            promptInput.error = "Enter a follow-up prompt"
            return
        }
        thread {
            runCatching {
                AgentSessionLauncher.startFollowUpSession(
                    context = this,
                    sourceTopLevelSession = topLevelSession,
                    prompt = prompt,
                    sessionController = sessionController,
                    requestUserInputHandler = { questions ->
                        AgentUserInputPrompter.promptForAnswers(this, questions)
                    },
                )
            }.onFailure { err ->
                showToast("Failed to continue session: ${err.message}")
            }.onSuccess { result ->
                promptInput.post { promptInput.text.clear() }
                showToast("Started follow-up session")
                runOnUiThread {
                    startActivity(
                        intentForSession(result.parentSessionId),
                    )
                }
            }
        }
    }

    private fun topLevelSession(snapshot: AgentSnapshot): AgentSessionDetails? {
        return snapshot.parentSession ?: snapshot.selectedSession?.takeIf { it.parentSessionId == null }
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
}
