package com.openai.codexd

import android.Manifest
import android.app.Activity
import android.app.agent.AgentManager
import android.app.agent.AgentSessionInfo
import android.content.Intent
import android.content.pm.PackageManager
import android.net.LocalSocket
import android.os.Binder
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.View
import android.widget.Button
import android.widget.EditText
import android.widget.TableLayout
import android.widget.TableRow
import android.widget.TextView
import android.widget.Toast
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.File
import java.io.IOException
import java.nio.charset.StandardCharsets
import java.util.Locale
import kotlin.concurrent.thread

class MainActivity : Activity() {
    companion object {
        private const val STATUS_REFRESH_INTERVAL_MS = 2000L
    }

    @Volatile
    private var isAuthenticated = false
    @Volatile
    private var isServiceRunning = false
    @Volatile
    private var statusRefreshInFlight = false
    @Volatile
    private var agentRefreshInFlight = false

    private val refreshHandler = Handler(Looper.getMainLooper())
    private val agentSessionController by lazy { AgentSessionController(this) }
    private val sessionUiLeaseToken = Binder()
    private val refreshRunnable = object : Runnable {
        override fun run() {
            refreshAuthStatus()
            refreshAgentSessions()
            refreshHandler.postDelayed(this, STATUS_REFRESH_INTERVAL_MS)
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
        setIntent(intent)
        handleSessionIntent(intent)
        refreshAgentSessions()
    }

    override fun onResume() {
        super.onResume()
        registerSessionListenerIfNeeded()
        refreshHandler.removeCallbacks(refreshRunnable)
        refreshHandler.post(refreshRunnable)
    }

    override fun onPause() {
        refreshHandler.removeCallbacks(refreshRunnable)
        unregisterSessionListenerIfNeeded()
        updateSessionUiLease(null)
        super.onPause()
    }

    private fun updatePaths() {
        findViewById<TextView>(R.id.socket_path).text = defaultSocketPath()
        findViewById<TextView>(R.id.codex_home).text = defaultCodexHome()
        isServiceRunning = false
        updateAuthUi("Auth status: unknown", false, null, emptyList())
        updateAgentUi(AgentSnapshot.unavailable)
    }

    private fun handleSessionIntent(intent: Intent?) {
        val sessionId = intent?.getStringExtra(AgentManager.EXTRA_SESSION_ID)
        if (!sessionId.isNullOrEmpty()) {
            focusedFrameworkSessionId = sessionId
        }
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
        val targetPackage = findViewById<EditText>(R.id.agent_target_package).text.toString().trim()
        val prompt = findViewById<EditText>(R.id.agent_prompt).text.toString().trim()
        if (targetPackage.isEmpty()) {
            showToast("Enter a target package")
            return
        }
        if (prompt.isEmpty()) {
            showToast("Enter a prompt")
            return
        }
        ensureCodexdRunningForAgent()
        thread {
            val result = runCatching {
                agentSessionController.startDirectSession(
                    targetPackage = targetPackage,
                    prompt = prompt,
                    allowDetachedMode = true,
                )
            }
            result.onFailure { err ->
                showToast("Failed to start Agent session: ${err.message}")
                refreshAgentSessions()
            }
            result.onSuccess { sessionStart ->
                focusedFrameworkSessionId = sessionStart.childSessionId
                showToast("Started ${sessionStart.childSessionId} via ${sessionStart.geniePackage}")
                refreshAgentSessions()
            }
        }
    }

    fun refreshAgentSessionAction(@Suppress("UNUSED_PARAMETER") view: View) {
        refreshAgentSessions(force = true)
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

    fun toggleCodexd(@Suppress("UNUSED_PARAMETER") view: View) {
        val intent = Intent(this, CodexdForegroundService::class.java).apply {
            putExtra(CodexdForegroundService.EXTRA_SOCKET_PATH, defaultSocketPath())
            putExtra(CodexdForegroundService.EXTRA_CODEX_HOME, defaultCodexHome())
        }
        if (isServiceRunning) {
            intent.action = CodexdForegroundService.ACTION_STOP
            startService(intent)
            isServiceRunning = false
            updateAuthUi("Auth status: stopping service...", false, 0, emptyList())
            return
        }

        intent.action = CodexdForegroundService.ACTION_START
        startForegroundService(intent)
        isServiceRunning = true
        updateAuthUi("Auth status: starting service...", isAuthenticated, null, emptyList())
        refreshAuthStatus()
    }

    fun authAction(@Suppress("UNUSED_PARAMETER") view: View) {
        if (isAuthenticated) {
            startSignOut()
        } else {
            startDeviceAuth()
        }
    }

    private fun startDeviceAuth() {
        val intent = Intent(this, CodexdForegroundService::class.java).apply {
            action = CodexdForegroundService.ACTION_START
            putExtra(CodexdForegroundService.EXTRA_SOCKET_PATH, defaultSocketPath())
            putExtra(CodexdForegroundService.EXTRA_CODEX_HOME, defaultCodexHome())
        }
        startForegroundService(intent)
        isServiceRunning = true
        updateAuthUi("Auth status: requesting device code...", false, null, emptyList())
        thread {
            val socketPath = defaultSocketPath()
            val response = runCatching { postDeviceAuthWithRetry(socketPath) }
            response.onFailure { err ->
                isServiceRunning = false
                updateAuthUi("Auth status: failed (${err.message})", false, null, emptyList())
            }
            response.onSuccess { deviceResponse ->
                when (deviceResponse.status) {
                    "already_authenticated" -> {
                        updateAuthUi("Auth status: already authenticated", true, null, emptyList())
                        showToast("Already signed in")
                    }
                    "pending", "in_progress" -> {
                        val url = deviceResponse.verificationUrl.orEmpty()
                        val code = deviceResponse.userCode.orEmpty()
                        updateAuthUi(
                            "Auth status: open $url and enter code $code",
                            false,
                            null,
                            emptyList(),
                        )
                        pollForAuthSuccess(socketPath)
                    }
                    else -> updateAuthUi(
                        "Auth status: ${deviceResponse.status}",
                        false,
                        null,
                        emptyList(),
                    )
                }
            }
        }
    }

    private fun startSignOut() {
        updateAuthUi("Auth status: signing out...", false, null, emptyList())
        thread {
            val socketPath = defaultSocketPath()
            val result = runCatching { postLogoutWithRetry(socketPath) }
            result.onFailure { err ->
                updateAuthUi(
                    "Auth status: sign out failed (${err.message})",
                    false,
                    null,
                    emptyList(),
                )
            }
            result.onSuccess {
                showToast("Signed out")
                refreshAuthStatus()
            }
        }
    }

    private fun refreshAuthStatus() {
        if (statusRefreshInFlight) {
            return
        }
        statusRefreshInFlight = true
        thread {
            val socketPath = defaultSocketPath()
            val result = runCatching { fetchAuthStatusWithRetry(socketPath) }
            result.onFailure { err ->
                isServiceRunning = false
                updateAuthUi(
                    "Auth status: codexd stopped or unavailable (${err.message})",
                    false,
                    null,
                    emptyList(),
                )
            }
            result.onSuccess { status ->
                isServiceRunning = true
                val message = if (status.authenticated) {
                    val emailSuffix = status.accountEmail?.let { " ($it)" } ?: ""
                    "Auth status: signed in$emailSuffix"
                } else {
                    "Auth status: not signed in"
                }
                updateAuthUi(message, status.authenticated, status.clientCount, status.clients)
            }
            statusRefreshInFlight = false
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

    private fun pollForAuthSuccess(socketPath: String) {
        val deadline = System.currentTimeMillis() + 15 * 60 * 1000
        while (System.currentTimeMillis() < deadline) {
            val status = runCatching { fetchAuthStatusWithRetry(socketPath) }.getOrNull()
            if (status?.authenticated == true) {
                val emailSuffix = status.accountEmail?.let { " ($it)" } ?: ""
                updateAuthUi(
                    "Auth status: signed in$emailSuffix",
                    true,
                    status.clientCount,
                    status.clients,
                )
                showToast("Signed in")
                return
            }
            Thread.sleep(3000)
        }
    }

    private fun updateAgentUi(snapshot: AgentSnapshot, unavailableMessage: String? = null) {
        runOnUiThread {
            val statusView = findViewById<TextView>(R.id.agent_status)
            val genieView = findViewById<TextView>(R.id.agent_genie_package)
            val focusView = findViewById<TextView>(R.id.agent_session_focus)
            val groupView = findViewById<TextView>(R.id.agent_session_group)
            val questionLabel = findViewById<TextView>(R.id.agent_question_label)
            val questionView = findViewById<TextView>(R.id.agent_question)
            val answerInput = findViewById<EditText>(R.id.agent_answer_input)
            val answerButton = findViewById<Button>(R.id.agent_answer_button)
            val attachButton = findViewById<Button>(R.id.agent_attach_button)
            val cancelButton = findViewById<Button>(R.id.agent_cancel_button)
            val timelineView = findViewById<TextView>(R.id.agent_timeline)
            val startButton = findViewById<Button>(R.id.agent_start_button)
            val refreshButton = findViewById<Button>(R.id.agent_refresh_button)

            if (!snapshot.available) {
                statusView.text = unavailableMessage?.let {
                    "Agent framework unavailable ($it)"
                } ?: "Agent framework unavailable on this build"
                genieView.text = "No GENIE role holder configured"
                focusView.text = "No framework session selected"
                groupView.text = "No framework sessions available"
                questionLabel.visibility = View.GONE
                questionView.visibility = View.GONE
                answerInput.visibility = View.GONE
                answerButton.visibility = View.GONE
                attachButton.visibility = View.GONE
                cancelButton.visibility = View.GONE
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
            genieView.text = snapshot.selectedGeniePackage ?: "No GENIE role holder configured"
            focusView.text = renderSelectedSession(snapshot)
            groupView.text = renderSessionGroup(snapshot)
            timelineView.text = renderTimeline(snapshot)
            startButton.isEnabled = snapshot.selectedGeniePackage != null
            refreshButton.isEnabled = true

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

    private fun updateAuthUi(
        message: String,
        authenticated: Boolean,
        clientCount: Int?,
        clients: List<ClientStats>,
    ) {
        isAuthenticated = authenticated
        runOnUiThread {
            val statusView = findViewById<TextView>(R.id.auth_status)
            statusView.text = message
            val serviceButton = findViewById<Button>(R.id.service_toggle)
            serviceButton.text = if (isServiceRunning) "Stop codexd" else "Start codexd"
            val actionButton = findViewById<Button>(R.id.auth_action)
            actionButton.text = if (authenticated) "Sign out" else "Start sign-in"
            actionButton.isEnabled = isServiceRunning
            val headingView = findViewById<TextView>(R.id.connected_clients_heading)
            val countSuffix = clientCount?.let { " ($it)" } ?: " (unknown)"
            headingView.text = "Connected clients$countSuffix"
            renderClientsTable(clientCount, clients)
        }
    }

    private fun renderClientsTable(clientCount: Int?, clients: List<ClientStats>) {
        val clientsTable = findViewById<TableLayout>(R.id.clients_table)
        while (clientsTable.childCount > 1) {
            clientsTable.removeViewAt(1)
        }

        if (clientCount == null) {
            val row = TableRow(this)
            val idCell = TextView(this)
            idCell.text = "unavailable"
            val trafficCell = TextView(this)
            trafficCell.text = "n/a"
            row.addView(idCell)
            row.addView(trafficCell)
            clientsTable.addView(row)
            return
        }

        if (clients.isEmpty()) {
            val row = TableRow(this)
            val idCell = TextView(this)
            idCell.text = "none"
            val trafficCell = TextView(this)
            trafficCell.text = "Tx 0.0 / Rx 0.0"
            row.addView(idCell)
            row.addView(trafficCell)
            clientsTable.addView(row)
            return
        }

        clients.sortedBy { it.id }.forEach { client ->
            val row = TableRow(this)
            val idCell = TextView(this)
            idCell.text = if (client.activeConnections > 0) {
                client.id
            } else {
                "${client.id} (idle)"
            }
            idCell.setPadding(0, 6, 24, 6)
            val trafficCell = TextView(this)
            trafficCell.text = formatTrafficKb(client.bytesSent, client.bytesReceived)
            trafficCell.setPadding(0, 6, 0, 6)
            row.addView(idCell)
            row.addView(trafficCell)
            clientsTable.addView(row)
        }
    }

    private fun formatTrafficKb(bytesSent: Long, bytesReceived: Long): String {
        val sentKb = bytesSent.toDouble() / 1024.0
        val receivedKb = bytesReceived.toDouble() / 1024.0
        return String.format(Locale.US, "Tx %.1f / Rx %.1f", sentKb, receivedKb)
    }

    private fun showToast(message: String) {
        runOnUiThread {
            Toast.makeText(this, message, Toast.LENGTH_SHORT).show()
        }
    }

    private fun ensureCodexdRunningForAgent() {
        val intent = Intent(this, CodexdForegroundService::class.java).apply {
            action = CodexdForegroundService.ACTION_START
            putExtra(CodexdForegroundService.EXTRA_SOCKET_PATH, defaultSocketPath())
            putExtra(CodexdForegroundService.EXTRA_CODEX_HOME, defaultCodexHome())
        }
        startForegroundService(intent)
        isServiceRunning = true
    }

    private data class AuthStatus(
        val authenticated: Boolean,
        val accountEmail: String?,
        val clientCount: Int,
        val clients: List<ClientStats>,
    )

    private data class ClientStats(
        val id: String,
        val activeConnections: Int,
        val bytesSent: Long,
        val bytesReceived: Long,
    )

    private data class DeviceAuthResponse(
        val status: String,
        val verificationUrl: String?,
        val userCode: String?,
    )

    private data class HttpResponse(val statusCode: Int, val body: String)

    private fun postDeviceAuthWithRetry(socketPath: String): DeviceAuthResponse {
        val response = executeSocketRequestWithRetry(
            socketPath,
            "POST",
            "/internal/auth/device",
            null,
        )
        if (response.statusCode != 200) {
            throw IOException("HTTP ${response.statusCode}: ${response.body}")
        }
        val json = JSONObject(response.body)
        val verificationUrl =
            if (json.isNull("verification_url")) null else json.optString("verification_url")
        val userCode = if (json.isNull("user_code")) null else json.optString("user_code")
        return DeviceAuthResponse(
            status = json.optString("status"),
            verificationUrl = verificationUrl,
            userCode = userCode,
        )
    }

    private fun postLogoutWithRetry(socketPath: String) {
        val response = executeSocketRequestWithRetry(
            socketPath,
            "POST",
            "/internal/auth/logout",
            null,
        )
        if (response.statusCode != 200) {
            throw IOException("HTTP ${response.statusCode}: ${response.body}")
        }
    }

    private fun fetchAuthStatusWithRetry(socketPath: String): AuthStatus {
        val response = executeSocketRequestWithRetry(
            socketPath,
            "GET",
            "/internal/auth/status",
            null,
        )
        if (response.statusCode != 200) {
            throw IOException("HTTP ${response.statusCode}: ${response.body}")
        }
        val json = JSONObject(response.body)
        val accountEmail =
            if (json.isNull("accountEmail")) null else json.optString("accountEmail")
        val clientCount = if (json.has("clientCount")) {
            json.optInt("clientCount", 0)
        } else {
            json.optInt("client_count", 0)
        }
        val clients = parseClients(json.optJSONArray("clients"))
        return AuthStatus(
            authenticated = json.optBoolean("authenticated", false),
            accountEmail = accountEmail,
            clientCount = clientCount,
            clients = clients,
        )
    }

    private fun parseClients(clientsJson: JSONArray?): List<ClientStats> {
        if (clientsJson == null) {
            return emptyList()
        }
        val clients = mutableListOf<ClientStats>()
        for (index in 0 until clientsJson.length()) {
            val clientJson = clientsJson.optJSONObject(index) ?: continue
            val id = clientJson.optString("id", "unknown")
            val activeConnections = if (clientJson.has("activeConnections")) {
                clientJson.optInt("activeConnections", 0)
            } else {
                clientJson.optInt("active_connections", 0)
            }
            val bytesSent = if (clientJson.has("bytesSent")) {
                clientJson.optLong("bytesSent", 0)
            } else {
                clientJson.optLong("bytes_sent", 0)
            }
            val bytesReceived = if (clientJson.has("bytesReceived")) {
                clientJson.optLong("bytesReceived", 0)
            } else {
                clientJson.optLong("bytes_received", 0)
            }
            clients.add(
                ClientStats(
                    id = id,
                    activeConnections = activeConnections,
                    bytesSent = bytesSent,
                    bytesReceived = bytesReceived,
                ),
            )
        }
        return clients
    }

    private fun executeSocketRequestWithRetry(
        socketPath: String,
        method: String,
        path: String,
        body: String?,
    ): HttpResponse {
        var lastError: Exception? = null
        repeat(10) {
            try {
                return executeSocketRequest(socketPath, method, path, body)
            } catch (err: Exception) {
                lastError = err
                Thread.sleep(250)
            }
        }
        throw IOException("Failed to connect to codexd socket: ${lastError?.message}")
    }

    private fun executeSocketRequest(
        socketPath: String,
        method: String,
        path: String,
        body: String?,
    ): HttpResponse {
        val socket = LocalSocket()
        val address = CodexSocketConfig.toLocalSocketAddress(socketPath)
        socket.connect(address)

        val payload = body ?: ""
        val request = buildString {
            append("$method $path HTTP/1.1\r\n")
            append("Host: localhost\r\n")
            append("Connection: close\r\n")
            if (body != null) {
                append("Content-Type: application/json\r\n")
            }
            append("Content-Length: ${payload.toByteArray(StandardCharsets.UTF_8).size}\r\n")
            append("\r\n")
            append(payload)
        }
        val output = socket.outputStream
        output.write(request.toByteArray(StandardCharsets.UTF_8))
        output.flush()

        val responseBytes = BufferedInputStream(socket.inputStream).use { it.readBytes() }
        socket.close()

        val responseText = responseBytes.toString(StandardCharsets.UTF_8)
        val splitIndex = responseText.indexOf("\r\n\r\n")
        if (splitIndex == -1) {
            throw IOException("Invalid HTTP response")
        }
        val headers = responseText.substring(0, splitIndex)
        val statusLine = headers.lineSequence().firstOrNull().orEmpty()
        val statusCode = statusLine.split(" ").getOrNull(1)?.toIntOrNull()
            ?: throw IOException("Missing status code")
        val responseBody = responseText.substring(splitIndex + 4)
        return HttpResponse(statusCode, responseBody)
    }

    private fun defaultSocketPath(): String {
        return CodexSocketConfig.DEFAULT_SOCKET_PATH
    }

    private fun defaultCodexHome(): String {
        return File(filesDir, "codex-home").absolutePath
    }
}
