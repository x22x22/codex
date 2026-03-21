package com.openai.codex.agent

import android.Manifest
import android.app.Activity
import android.app.agent.AgentManager
import android.app.agent.AgentSessionInfo
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Binder
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.View
import android.widget.Button
import android.widget.EditText
import android.widget.TextView
import android.widget.Toast
import kotlin.concurrent.thread

class MainActivity : Activity() {
    companion object {
        private const val TAG = "CodexMainActivity"
        private const val ACTION_DEBUG_START_AGENT_SESSION =
            "com.openai.codex.agent.action.DEBUG_START_AGENT_SESSION"
        private const val ACTION_DEBUG_CANCEL_ALL_AGENT_SESSIONS =
            "com.openai.codex.agent.action.DEBUG_CANCEL_ALL_AGENT_SESSIONS"
        private const val EXTRA_DEBUG_PROMPT = "prompt"
        private const val EXTRA_DEBUG_TARGET_PACKAGE = "targetPackage"
    }

    @Volatile
    private var isAuthenticated = false
    @Volatile
    private var agentRefreshInFlight = false
    @Volatile
    private var latestAgentRuntimeStatus: AgentCodexAppServerClient.RuntimeStatus? = null
    @Volatile
    private var pendingAuthMessage: String? = null

    private val agentSessionController by lazy { AgentSessionController(this) }
    private val sessionUiLeaseToken = Binder()
    private val runtimeStatusListener = AgentCodexAppServerClient.RuntimeStatusListener { status ->
        latestAgentRuntimeStatus = status
        if (status != null) {
            pendingAuthMessage = null
        }
        runOnUiThread {
            findViewById<TextView>(R.id.agent_runtime_status).text = renderAgentRuntimeStatus()
            updateAuthUi(
                message = renderAuthStatus(),
                authenticated = status?.authenticated == true,
            )
        }
    }
    private val sessionListener = object : AgentManager.SessionListener {
        override fun onSessionChanged(session: AgentSessionInfo) {
            if (focusedFrameworkSessionId == null && session.parentSessionId != null) {
                focusedFrameworkSessionId = session.sessionId
            }
            refreshAgentSessions()
        }

        override fun onSessionRemoved(sessionId: String, userId: Int) {
            if (focusedFrameworkSessionId == sessionId) {
                focusedFrameworkSessionId = null
            }
            refreshAgentSessions()
        }
    }

    private var sessionListenerRegistered = false
    private var focusedFrameworkSessionId: String? = null
    private var leasedParentSessionId: String? = null
    private var latestAgentSnapshot: AgentSnapshot = AgentSnapshot.unavailable

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)
        updatePaths()
        handleSessionIntent(intent)
        requestNotificationPermissionIfNeeded()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        Log.i(TAG, "onNewIntent action=${intent.action}")
        setIntent(intent)
        handleSessionIntent(intent)
        refreshAgentSessions()
    }

    override fun onResume() {
        super.onResume()
        handleSessionIntent(intent)
        registerSessionListenerIfNeeded()
        AgentCodexAppServerClient.registerRuntimeStatusListener(runtimeStatusListener)
        AgentCodexAppServerClient.refreshRuntimeStatusAsync(this, refreshToken = true)
        refreshAgentSessions(force = true)
    }

    override fun onPause() {
        AgentCodexAppServerClient.unregisterRuntimeStatusListener(runtimeStatusListener)
        unregisterSessionListenerIfNeeded()
        updateSessionUiLease(null)
        super.onPause()
    }

    private fun updatePaths() {
        latestAgentRuntimeStatus = AgentCodexAppServerClient.currentRuntimeStatus()
        updateAuthUi(
            message = renderAuthStatus(),
            authenticated = latestAgentRuntimeStatus?.authenticated == true,
        )
        updateAgentUi(AgentSnapshot.unavailable)
    }

    private fun handleSessionIntent(intent: Intent?) {
        val sessionId = intent?.getStringExtra(AgentManager.EXTRA_SESSION_ID)
        if (!sessionId.isNullOrEmpty()) {
            focusedFrameworkSessionId = sessionId
        }
        maybeStartSessionFromIntent(intent)
    }

    private fun maybeStartSessionFromIntent(intent: Intent?) {
        if (intent?.action == ACTION_DEBUG_CANCEL_ALL_AGENT_SESSIONS) {
            Log.i(TAG, "Handling debug cancel-all Agent sessions intent")
            thread {
                val result = runCatching { agentSessionController.cancelActiveSessions() }
                result.onFailure { err ->
                    Log.w(TAG, "Failed to cancel Agent sessions from debug intent", err)
                    showToast("Failed to cancel active sessions: ${err.message}")
                }
                result.onSuccess { cancelResult ->
                    focusedFrameworkSessionId = null
                    val cancelledCount = cancelResult.cancelledSessionIds.size
                    val failedCount = cancelResult.failedSessionIds.size
                    showToast("Cancelled $cancelledCount sessions, $failedCount failed")
                    refreshAgentSessions(force = true)
                }
            }
            intent.action = null
            return
        }
        if (intent?.action != ACTION_DEBUG_START_AGENT_SESSION) {
            return
        }
        val prompt = intent.getStringExtra(EXTRA_DEBUG_PROMPT)?.trim().orEmpty()
        if (prompt.isEmpty()) {
            Log.w(TAG, "Ignoring debug start intent without prompt")
            intent.action = null
            return
        }
        val targetPackageOverride = intent.getStringExtra(EXTRA_DEBUG_TARGET_PACKAGE)?.trim()
        findViewById<EditText>(R.id.agent_prompt).setText(prompt)
        findViewById<EditText>(R.id.agent_target_package).setText(targetPackageOverride.orEmpty())
        Log.i(TAG, "Handling debug start intent override=$targetPackageOverride prompt=${prompt.take(160)}")
        startDirectAgentSession(findViewById(R.id.agent_start_button))
        intent.action = null
    }

    private fun registerSessionListenerIfNeeded() {
        if (sessionListenerRegistered || !agentSessionController.isAvailable()) {
            return
        }
        sessionListenerRegistered = runCatching {
            agentSessionController.registerSessionListener(mainExecutor, sessionListener)
        }.getOrDefault(false)
    }

    private fun unregisterSessionListenerIfNeeded() {
        if (!sessionListenerRegistered) {
            return
        }
        runCatching { agentSessionController.unregisterSessionListener(sessionListener) }
        sessionListenerRegistered = false
    }

    private fun updateSessionUiLease(parentSessionId: String?) {
        if (leasedParentSessionId == parentSessionId) {
            return
        }
        val previousParentSessionId = leasedParentSessionId
        if (previousParentSessionId != null) {
            runCatching {
                agentSessionController.unregisterSessionUiLease(previousParentSessionId, sessionUiLeaseToken)
            }
            leasedParentSessionId = null
        }
        if (parentSessionId != null) {
            val registered = runCatching {
                agentSessionController.registerSessionUiLease(parentSessionId, sessionUiLeaseToken)
            }
            if (registered.isSuccess) {
                leasedParentSessionId = parentSessionId
            }
        }
    }

    private fun requestNotificationPermissionIfNeeded() {
        if (Build.VERSION.SDK_INT < 33) {
            return
        }
        if (checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS)
            == PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1001)
    }

    fun startDirectAgentSession(@Suppress("UNUSED_PARAMETER") view: View) {
        val targetPackageOverride = findViewById<EditText>(R.id.agent_target_package).text.toString().trim()
        val prompt = findViewById<EditText>(R.id.agent_prompt).text.toString().trim()
        if (prompt.isEmpty()) {
            showToast("Enter a prompt")
            return
        }
        Log.i(TAG, "startDirectAgentSession override=$targetPackageOverride prompt=${prompt.take(160)}")
        thread {
            val result = runCatching {
                AgentTaskPlanner.startSession(
                    context = this,
                    userObjective = prompt,
                    targetPackageOverride = targetPackageOverride.ifBlank { null },
                    allowDetachedMode = true,
                    sessionController = agentSessionController,
                    requestUserInputHandler = { questions ->
                        AgentUserInputPrompter.promptForAnswers(this, questions)
                    },
                )
            }
            result.onFailure { err ->
                Log.w(TAG, "Failed to start Agent session", err)
                showToast("Failed to start Agent session: ${err.message}")
                refreshAgentSessions()
            }
            result.onSuccess { sessionStart ->
                Log.i(
                    TAG,
                    "Started Agent session parent=${sessionStart.parentSessionId} children=${sessionStart.childSessionIds}",
                )
                focusedFrameworkSessionId = sessionStart.childSessionIds.firstOrNull()
                val targetSummary = sessionStart.plannedTargets.joinToString(", ")
                showToast("Started ${sessionStart.childSessionIds.size} Genie session(s) for $targetSummary via ${sessionStart.geniePackage}")
                refreshAgentSessions()
            }
        }
    }

    fun refreshAgentSessionAction(@Suppress("UNUSED_PARAMETER") view: View) {
        refreshAgentSessions(force = true)
    }

    fun cancelAllAgentSessions(@Suppress("UNUSED_PARAMETER") view: View) {
        thread {
            val result = runCatching { agentSessionController.cancelActiveSessions() }
            result.onFailure { err ->
                showToast("Failed to cancel active sessions: ${err.message}")
            }
            result.onSuccess { cancelResult ->
                focusedFrameworkSessionId = null
                val cancelledCount = cancelResult.cancelledSessionIds.size
                val failedCount = cancelResult.failedSessionIds.size
                if (cancelledCount == 0 && failedCount == 0) {
                    showToast("No active framework sessions")
                } else if (failedCount == 0) {
                    showToast("Cancelled $cancelledCount active sessions")
                } else {
                    showToast("Cancelled $cancelledCount sessions, $failedCount failed")
                }
                refreshAgentSessions(force = true)
            }
        }
    }

    fun answerAgentQuestion(@Suppress("UNUSED_PARAMETER") view: View) {
        val selectedSession = latestAgentSnapshot.selectedSession
        if (selectedSession == null) {
            showToast("No active Genie session selected")
            return
        }
        val answerInput = findViewById<EditText>(R.id.agent_answer_input)
        val answer = answerInput.text.toString().trim()
        if (answer.isEmpty()) {
            showToast("Enter an answer")
            return
        }
        thread {
            val result = runCatching {
                agentSessionController.answerQuestion(
                    selectedSession.sessionId,
                    answer,
                    latestAgentSnapshot.parentSession?.sessionId,
                )
            }
            result.onFailure { err ->
                showToast("Failed to answer question: ${err.message}")
            }
            result.onSuccess {
                answerInput.post { answerInput.text.clear() }
                showToast("Answered ${selectedSession.sessionId}")
                refreshAgentSessions(force = true)
            }
        }
    }

    fun attachAgentTarget(@Suppress("UNUSED_PARAMETER") view: View) {
        val selectedSession = latestAgentSnapshot.selectedSession
        if (selectedSession == null) {
            showToast("No detached target available")
            return
        }
        thread {
            val result = runCatching {
                agentSessionController.attachTarget(selectedSession.sessionId)
            }
            result.onFailure { err ->
                showToast("Failed to attach target: ${err.message}")
            }
            result.onSuccess {
                showToast("Attached target for ${selectedSession.sessionId}")
                refreshAgentSessions(force = true)
            }
        }
    }

    fun cancelAgentSession(@Suppress("UNUSED_PARAMETER") view: View) {
        val selectedSession = latestAgentSnapshot.selectedSession
        if (selectedSession == null) {
            showToast("No framework session selected")
            return
        }
        val sessionIdToCancel = latestAgentSnapshot.parentSession?.sessionId ?: selectedSession.sessionId
        thread {
            val result = runCatching {
                agentSessionController.cancelSession(sessionIdToCancel)
            }
            result.onFailure { err ->
                showToast("Failed to cancel session: ${err.message}")
            }
            result.onSuccess {
                if (focusedFrameworkSessionId == selectedSession.sessionId) {
                    focusedFrameworkSessionId = null
                }
                showToast("Cancelled $sessionIdToCancel")
                refreshAgentSessions(force = true)
            }
        }
    }

    fun authAction(@Suppress("UNUSED_PARAMETER") view: View) {
        if (isAuthenticated) {
            signOutAgent()
        } else {
            startAgentSignIn()
        }
    }

    private fun startAgentSignIn() {
        pendingAuthMessage = "Agent auth: opening browser for sign-in..."
        updateAuthUi(pendingAuthMessage.orEmpty(), false)
        thread {
            val result = runCatching { AgentCodexAppServerClient.startChatGptLogin(this) }
            result.onFailure { err ->
                pendingAuthMessage = null
                updateAuthUi("Agent auth: sign-in failed (${err.message})", false)
            }
            result.onSuccess { loginSession ->
                pendingAuthMessage = "Agent auth: complete sign-in in the browser"
                updateAuthUi(pendingAuthMessage.orEmpty(), false)
                runOnUiThread {
                    val browserResult = runCatching {
                        startActivity(
                            Intent(Intent.ACTION_VIEW, Uri.parse(loginSession.authUrl)),
                        )
                    }
                    browserResult.onFailure { err ->
                        pendingAuthMessage = "Agent auth: open ${loginSession.authUrl}"
                        updateAuthUi(pendingAuthMessage.orEmpty(), false)
                        showToast("Failed to open browser: ${err.message}")
                    }
                    browserResult.onSuccess {
                        showToast("Complete sign-in in the browser")
                    }
                }
            }
        }
    }

    private fun signOutAgent() {
        pendingAuthMessage = "Agent auth: signing out..."
        updateAuthUi(pendingAuthMessage.orEmpty(), false)
        thread {
            val result = runCatching { AgentCodexAppServerClient.logoutAccount(this) }
            result.onFailure { err ->
                pendingAuthMessage = null
                updateAuthUi("Agent auth: sign out failed (${err.message})", isAuthenticated)
            }
            result.onSuccess {
                pendingAuthMessage = null
                AgentCodexAppServerClient.refreshRuntimeStatusAsync(this)
                showToast("Signed out")
            }
        }
    }

    private fun refreshAgentSessions(force: Boolean = false) {
        if (!force && agentRefreshInFlight) {
            return
        }
        agentRefreshInFlight = true
        thread {
            val result = runCatching { agentSessionController.loadSnapshot(focusedFrameworkSessionId) }
            result.onFailure { err ->
                latestAgentSnapshot = AgentSnapshot.unavailable
                runOnUiThread {
                    updateAgentUi(AgentSnapshot.unavailable, err.message)
                }
            }
            result.onSuccess { snapshot ->
                latestAgentSnapshot = snapshot
                focusedFrameworkSessionId = snapshot.selectedSession?.sessionId ?: focusedFrameworkSessionId
                updateAgentUi(snapshot)
            }
            agentRefreshInFlight = false
        }
    }

    private fun updateAgentUi(snapshot: AgentSnapshot, unavailableMessage: String? = null) {
        runOnUiThread {
            val statusView = findViewById<TextView>(R.id.agent_status)
            val runtimeStatusView = findViewById<TextView>(R.id.agent_runtime_status)
            val genieView = findViewById<TextView>(R.id.agent_genie_package)
            val focusView = findViewById<TextView>(R.id.agent_session_focus)
            val groupView = findViewById<TextView>(R.id.agent_session_group)
            val questionLabel = findViewById<TextView>(R.id.agent_question_label)
            val questionView = findViewById<TextView>(R.id.agent_question)
            val answerInput = findViewById<EditText>(R.id.agent_answer_input)
            val answerButton = findViewById<Button>(R.id.agent_answer_button)
            val attachButton = findViewById<Button>(R.id.agent_attach_button)
            val cancelButton = findViewById<Button>(R.id.agent_cancel_button)
            val cancelAllButton = findViewById<Button>(R.id.agent_cancel_all_button)
            val timelineView = findViewById<TextView>(R.id.agent_timeline)
            val startButton = findViewById<Button>(R.id.agent_start_button)
            val refreshButton = findViewById<Button>(R.id.agent_refresh_button)

            if (!snapshot.available) {
                statusView.text = unavailableMessage?.let {
                    "Agent framework unavailable ($it)"
                } ?: "Agent framework unavailable on this build"
                runtimeStatusView.text = renderAgentRuntimeStatus()
                genieView.text = "No GENIE role holder configured"
                focusView.text = "No framework session selected"
                groupView.text = "No framework sessions available"
                questionLabel.visibility = View.GONE
                questionView.visibility = View.GONE
                answerInput.visibility = View.GONE
                answerButton.visibility = View.GONE
                attachButton.visibility = View.GONE
                cancelButton.visibility = View.GONE
                cancelAllButton.isEnabled = false
                timelineView.text = "No framework events yet."
                startButton.isEnabled = false
                refreshButton.isEnabled = false
                updateSessionUiLease(null)
                return@runOnUiThread
            }

            val roleHolders = if (snapshot.roleHolders.isEmpty()) {
                "none"
            } else {
                snapshot.roleHolders.joinToString(", ")
            }
            statusView.text = "Agent framework active. Genie role holders: $roleHolders"
            runtimeStatusView.text = renderAgentRuntimeStatus()
            genieView.text = snapshot.selectedGeniePackage ?: "No GENIE role holder configured"
            focusView.text = renderSelectedSession(snapshot)
            groupView.text = renderSessionGroup(snapshot)
            timelineView.text = renderTimeline(snapshot)
            startButton.isEnabled = snapshot.selectedGeniePackage != null
            refreshButton.isEnabled = true
            cancelAllButton.isEnabled = snapshot.sessions.any { !isTerminalState(it.state) }

            val selectedSession = snapshot.selectedSession
            val waitingQuestion = selectedSession?.latestQuestion
            val isWaitingForUser = selectedSession?.state == AgentSessionInfo.STATE_WAITING_FOR_USER &&
                !waitingQuestion.isNullOrEmpty()
            questionLabel.visibility = if (isWaitingForUser) View.VISIBLE else View.GONE
            questionView.visibility = if (isWaitingForUser) View.VISIBLE else View.GONE
            answerInput.visibility = if (isWaitingForUser) View.VISIBLE else View.GONE
            answerButton.visibility = if (isWaitingForUser) View.VISIBLE else View.GONE
            questionView.text = waitingQuestion ?: ""

            val showAttachButton = selectedSession?.targetDetached == true
            attachButton.visibility = if (showAttachButton) View.VISIBLE else View.GONE
            attachButton.isEnabled = showAttachButton

            val showCancelButton = selectedSession != null
            cancelButton.visibility = if (showCancelButton) View.VISIBLE else View.GONE
            cancelButton.isEnabled = showCancelButton

            updateSessionUiLease(snapshot.parentSession?.sessionId)
        }
    }

    private fun isTerminalState(state: Int): Boolean {
        return state == AgentSessionInfo.STATE_COMPLETED ||
            state == AgentSessionInfo.STATE_CANCELLED ||
            state == AgentSessionInfo.STATE_FAILED
    }

    private fun renderSelectedSession(snapshot: AgentSnapshot): String {
        val selectedSession = snapshot.selectedSession ?: return "No framework session selected"
        return buildString {
            append("Session: ${selectedSession.sessionId}\n")
            append("State: ${selectedSession.stateLabel}\n")
            append("Target: ${selectedSession.targetPackage ?: "direct-agent"}\n")
            append("Detached target: ${selectedSession.targetDetached}\n")
            val parentSessionId = snapshot.parentSession?.sessionId
            if (parentSessionId != null) {
                append("Parent: $parentSessionId\n")
            }
            selectedSession.latestResult?.let { append("Result: $it\n") }
            selectedSession.latestError?.let { append("Error: $it\n") }
            if (selectedSession.latestResult == null && selectedSession.latestError == null) {
                selectedSession.latestTrace?.let { append("Trace: $it") }
            }
        }.trimEnd()
    }

    private fun renderSessionGroup(snapshot: AgentSnapshot): String {
        val sessions = snapshot.relatedSessions.ifEmpty { snapshot.sessions }
        if (sessions.isEmpty()) {
            return "No framework sessions yet"
        }
        return sessions.joinToString("\n") { session ->
            val role = if (session.parentSessionId == null) {
                if (session.targetPackage == null) "parent" else "standalone"
            } else {
                "child"
            }
            val marker = if (session.sessionId == snapshot.selectedSession?.sessionId) "*" else "-"
            val detail = session.latestQuestion ?: session.latestResult ?: session.latestError ?: session.latestTrace
            buildString {
                append("$marker $role ${session.stateLabel} ${session.targetPackage ?: "direct-agent"}")
                if (session.targetDetached) {
                    append(" [detached]")
                }
                append("\n  ${session.sessionId}")
                if (!detail.isNullOrEmpty()) {
                    append("\n  $detail")
                }
            }
        }
    }

    private fun renderTimeline(snapshot: AgentSnapshot): String {
        val selectedSession = snapshot.selectedSession ?: return "No framework events yet."
        val parentSession = snapshot.parentSession
        if (parentSession == null || parentSession.sessionId == selectedSession.sessionId) {
            return selectedSession.timeline
        }
        return buildString {
            append("Parent ${parentSession.sessionId}\n")
            append(parentSession.timeline)
            append("\n\nSelected child ${selectedSession.sessionId}\n")
            append(selectedSession.timeline)
        }
    }

    private fun renderAgentRuntimeStatus(): String {
        val runtimeStatus = latestAgentRuntimeStatus
        if (runtimeStatus == null) {
            return "Agent runtime: probing..."
        }
        val authSummary = if (runtimeStatus.authenticated) {
            runtimeStatus.accountEmail?.let { "signed in ($it)" } ?: "signed in"
        } else {
            "not signed in"
        }
        val configuredModelSuffix = runtimeStatus.configuredModel
            ?.takeIf { it != runtimeStatus.effectiveModel }
            ?.let { ", configured=$it" }
            ?: ""
        val effectiveModel = runtimeStatus.effectiveModel ?: "unknown"
        return "Agent runtime: $authSummary, provider=${runtimeStatus.modelProviderId}, effective=$effectiveModel$configuredModelSuffix, clients=${runtimeStatus.clientCount}, base=${runtimeStatus.upstreamBaseUrl}"
    }

    private fun renderAuthStatus(): String {
        pendingAuthMessage?.let { return it }
        val runtimeStatus = latestAgentRuntimeStatus
        if (runtimeStatus == null) {
            return "Agent auth: probing..."
        }
        if (!runtimeStatus.authenticated) {
            return "Agent auth: not signed in"
        }
        return runtimeStatus.accountEmail?.let { email ->
            "Agent auth: signed in ($email)"
        } ?: "Agent auth: signed in"
    }

    private fun updateAuthUi(
        message: String,
        authenticated: Boolean,
    ) {
        isAuthenticated = authenticated
        runOnUiThread {
            val statusView = findViewById<TextView>(R.id.auth_status)
            statusView.text = message
            val actionButton = findViewById<Button>(R.id.auth_action)
            actionButton.text = if (authenticated) "Sign out" else "Start sign-in"
            actionButton.isEnabled = true
        }
    }

    private fun showToast(message: String) {
        runOnUiThread {
            Toast.makeText(this, message, Toast.LENGTH_SHORT).show()
        }
    }
}
