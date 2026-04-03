package com.openai.codex.agent

import android.app.Activity
import android.app.agent.AgentManager
import android.app.agent.AgentSessionInfo
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.view.View
import android.view.WindowManager
import android.widget.Button
import android.widget.EditText
import android.widget.TextView
import android.widget.Toast
import kotlin.concurrent.thread

class SessionPopupActivity : Activity() {
    companion object {
        const val EXTRA_SESSION_ID = "sessionId"

        fun intent(
            context: Context,
            sessionId: String,
        ): Intent {
            return Intent(context, SessionPopupActivity::class.java)
                .putExtra(EXTRA_SESSION_ID, sessionId)
        }
    }

    private val sessionController by lazy { AgentSessionController(this) }
    private var requestedSessionId: String? = null
    private var fallbackLaunched = false
    private var popupRendered = false
    private var refreshInFlight = false
    private var sessionListenerRegistered = false

    private val sessionListener = object : AgentManager.SessionListener {
        override fun onSessionChanged(session: AgentSessionInfo) {
            if (session.sessionId == requestedSessionId || session.parentSessionId == requestedSessionId) {
                refreshPopup(force = true)
            }
        }

        override fun onSessionRemoved(sessionId: String, userId: Int) {
            if (sessionId == requestedSessionId) {
                finish()
            } else {
                refreshPopup(force = true)
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestedSessionId = intent.getStringExtra(EXTRA_SESSION_ID)?.trim()?.ifEmpty { null }
        if (requestedSessionId == null) {
            finish()
            return
        }
        setFinishOnTouchOutside(false)
    }

    override fun onResume() {
        super.onResume()
        registerSessionListenerIfNeeded()
        refreshPopup(force = true)
    }

    override fun onPause() {
        unregisterSessionListenerIfNeeded()
        super.onPause()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        requestedSessionId = intent.getStringExtra(EXTRA_SESSION_ID)?.trim()?.ifEmpty { null }
        fallbackLaunched = false
        popupRendered = false
        refreshPopup(force = true)
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

    private fun refreshPopup(force: Boolean = false) {
        if (!force && refreshInFlight) {
            return
        }
        val sessionId = requestedSessionId ?: return
        refreshInFlight = true
        thread(name = "CodexSessionPopupLoad-$sessionId") {
            try {
                val session = runCatching {
                    resolvePopupSession(sessionController.loadSnapshot(sessionId), sessionId)
                }.getOrNull()
                runOnUiThread {
                    renderSession(session)
                }
            } finally {
                refreshInFlight = false
            }
        }
    }

    private fun resolvePopupSession(
        snapshot: AgentSnapshot,
        sessionId: String,
    ): AgentSessionDetails? {
        return snapshot.sessions.firstOrNull { session -> session.sessionId == sessionId }
            ?: snapshot.selectedSession?.takeIf { session -> session.sessionId == sessionId }
            ?: snapshot.parentSession?.takeIf { session -> session.sessionId == sessionId }
    }

    private fun renderSession(session: AgentSessionDetails?) {
        if (session == null) {
            finish()
            return
        }
        when {
            isQuestionSession(session) -> showQuestionPopup(session)
            isResultSession(session) -> showResultPopup(session)
            popupRendered || fallbackLaunched -> finish()
            else -> launchFallbackDetail(session.sessionId)
        }
    }

    private fun isQuestionSession(session: AgentSessionDetails): Boolean {
        return session.state == AgentSessionInfo.STATE_WAITING_FOR_USER &&
            !session.latestQuestion.isNullOrBlank()
    }

    private fun isResultSession(session: AgentSessionDetails): Boolean {
        return when (session.state) {
            AgentSessionInfo.STATE_COMPLETED,
            AgentSessionInfo.STATE_CANCELLED,
            AgentSessionInfo.STATE_FAILED,
            -> true
            else -> false
        }
    }

    private fun showQuestionPopup(session: AgentSessionDetails) {
        popupRendered = true
        setContentView(R.layout.activity_session_question_popup)
        findViewById<TextView>(R.id.session_popup_question_text).text = session.latestQuestion.orEmpty()
        val answerInput = findViewById<EditText>(R.id.session_popup_answer_input)
        findViewById<Button>(R.id.session_popup_cancel_button).setOnClickListener {
            finish()
        }
        findViewById<Button>(R.id.session_popup_submit_button).setOnClickListener {
            submitAnswer(session, answerInput)
        }
        answerInput.requestFocus()
        window?.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE)
    }

    private fun showResultPopup(session: AgentSessionDetails) {
        popupRendered = true
        setContentView(R.layout.activity_session_result_popup)
        findViewById<TextView>(R.id.session_popup_result_title).text = resultTitle(session)
        findViewById<TextView>(R.id.session_popup_result_text).text = resultBody(session)
        findViewById<Button>(R.id.session_popup_ok_button).setOnClickListener { buttonView ->
            dismissResultPopup(session, buttonView as Button)
        }
    }

    private fun resultTitle(session: AgentSessionDetails): String {
        return when (session.state) {
            AgentSessionInfo.STATE_COMPLETED -> "Codex Result"
            AgentSessionInfo.STATE_CANCELLED -> "Codex Session Cancelled"
            AgentSessionInfo.STATE_FAILED -> "Codex Session Failed"
            else -> "Codex Session"
        }
    }

    private fun resultBody(session: AgentSessionDetails): String {
        return when {
            !session.latestResult.isNullOrBlank() -> session.latestResult
            !session.latestError.isNullOrBlank() -> session.latestError
            session.state == AgentSessionInfo.STATE_CANCELLED -> "This session was cancelled."
            else -> "No final message was recorded for this session."
        }
    }

    private fun submitAnswer(
        session: AgentSessionDetails,
        answerInput: EditText,
    ) {
        val answer = answerInput.text.toString().trim()
        if (answer.isEmpty()) {
            answerInput.error = "Enter an answer"
            return
        }
        val button = findViewById<Button>(R.id.session_popup_submit_button)
        button.isEnabled = false
        thread(name = "CodexSessionPopupAnswer-${session.sessionId}") {
            runCatching {
                sessionController.answerQuestion(
                    session.sessionId,
                    answer,
                    session.parentSessionId,
                )
                SessionNotificationCoordinator.acknowledgeSessionTree(
                    context = this,
                    sessionController = sessionController,
                    topLevelSessionId = session.parentSessionId ?: session.sessionId,
                    sessionIds = listOf(session.sessionId),
                )
            }.onFailure { err ->
                runOnUiThread {
                    button.isEnabled = true
                    Toast.makeText(
                        this,
                        "Failed to answer question: ${err.message}",
                        Toast.LENGTH_SHORT,
                    ).show()
                }
            }.onSuccess {
                runOnUiThread {
                    finish()
                }
            }
        }
    }

    private fun dismissResultPopup(
        session: AgentSessionDetails,
        button: Button,
    ) {
        AgentQuestionNotifier.cancel(this, session.sessionId)
        if (!isTopLevelHomeSession(session)) {
            finish()
            return
        }
        button.isEnabled = false
        thread(name = "CodexSessionPopupConsume-${session.sessionId}") {
            runCatching {
                sessionController.consumeHomeSessionPresentation(session.sessionId)
                if (session.targetDetached) {
                    sessionController.closeDetachedTarget(session.sessionId)
                }
            }.onFailure { err ->
                runOnUiThread {
                    button.isEnabled = true
                    Toast.makeText(
                        this,
                        "Failed to clear result badge: ${err.message}",
                        Toast.LENGTH_SHORT,
                    ).show()
                }
            }.onSuccess {
                runOnUiThread {
                    finish()
                }
            }
        }
    }

    private fun isTopLevelHomeSession(session: AgentSessionDetails): Boolean {
        return session.anchor == AgentSessionInfo.ANCHOR_HOME &&
            session.parentSessionId == null
    }

    private fun launchFallbackDetail(sessionId: String) {
        fallbackLaunched = true
        startActivity(
            Intent(this, SessionDetailActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP)
                .putExtra(SessionDetailActivity.EXTRA_SESSION_ID, sessionId),
        )
        finish()
    }
}
