package com.openai.codex.agent

import android.app.agent.AgentSessionInfo
import android.content.Context
import android.os.Binder
import android.os.SystemClock
import android.util.Log
import java.net.InetAddress
import java.net.InetSocketAddress
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import kotlin.concurrent.thread
import org.java_websocket.WebSocket
import org.java_websocket.handshake.ClientHandshake
import org.java_websocket.server.WebSocketServer
import org.json.JSONArray
import org.json.JSONObject

object DesktopBridgeServer {
    private const val TAG = "DesktopBridgeServer"
    private const val LISTEN_PORT = 48765
    private const val CONTROL_PATH = "/control"
    private const val SESSION_PATH_PREFIX = "/session/"
    private const val DEFAULT_MODEL = "gpt-5.3-codex-spark"
    private const val DEFAULT_REASONING_EFFORT = "low"
    private const val ATTACH_TOKEN_TTL_MS = 60_000L
    private const val ATTACH_THREAD_WAIT_MS = 5_000L
    private const val ATTACH_THREAD_POLL_MS = 100L

    private val authorizedTokens = ConcurrentHashMap.newKeySet<String>()
    private val attachTokens = ConcurrentHashMap<String, AttachedSessionTarget>()
    private val createdHomeSessionUiLeases = ConcurrentHashMap<String, Binder>()
    @Volatile
    private var server: AgentDesktopBridgeSocketServer? = null

    private data class AttachedSessionTarget(
        val sessionId: String,
        val expiresAtElapsedRealtimeMs: Long,
    )

    fun ensureStarted(
        context: Context,
        authToken: String,
    ) {
        authorizedTokens += authToken
        val existing = server
        if (existing != null) {
            return
        }
        synchronized(this) {
            if (server != null) {
                return
            }
            server = AgentDesktopBridgeSocketServer(context.applicationContext).also {
                it.start()
                Log.i(TAG, "Desktop bridge listening on ws://127.0.0.1:$LISTEN_PORT$CONTROL_PATH")
            }
        }
    }

    private class AgentDesktopBridgeSocketServer(
        private val context: Context,
    ) : WebSocketServer(InetSocketAddress(InetAddress.getByName("127.0.0.1"), LISTEN_PORT)) {
        private val sessionController = AgentSessionController(context)

        override fun onOpen(
            conn: WebSocket,
            handshake: ClientHandshake,
        ) {
            val authHeader = handshake.getFieldValue("Authorization")
            val bearerToken = parseBearerToken(authHeader)
            if (bearerToken == null || !authorizedTokens.contains(bearerToken)) {
                conn.close(1008, "Unauthorized")
                return
            }
            val path = handshake.resourceDescriptor ?: CONTROL_PATH
            if (path == CONTROL_PATH) {
                return
            }
            if (path.startsWith(SESSION_PATH_PREFIX)) {
                val attachToken = path.removePrefix(SESSION_PATH_PREFIX)
                val target = attachTokens[attachToken]
                if (target == null) {
                    conn.close(1008, "Unknown attach token")
                    return
                }
                if (target.expiresAtElapsedRealtimeMs <= SystemClock.elapsedRealtime()) {
                    attachTokens.remove(attachToken, target)
                    conn.close(1008, "Expired attach token")
                    return
                }
                val connectionId = openSessionProxy(
                    sessionId = target.sessionId,
                    onMessage = { message ->
                        runCatching { conn.send(message) }
                            .onFailure { conn.close(1011, it.message ?: "Desktop send failed") }
                    },
                    onClosed = { reason ->
                        conn.close(1000, reason ?: "Session proxy closed")
                    },
                )
                if (connectionId == null) {
                    conn.close(1011, "Session is not attachable")
                    return
                }
                conn.setAttachment(SessionProxyConnection(target.sessionId, connectionId))
                return
            }
            conn.close(1008, "Unsupported path")
        }

        override fun onClose(
            conn: WebSocket,
            code: Int,
            reason: String,
            remote: Boolean,
        ) {
            val attachment = conn.getAttachment<SessionProxyConnection>()
            if (attachment != null) {
                closeSessionProxy(
                    sessionId = attachment.sessionId,
                    connectionId = attachment.connectionId,
                    reason = reason.ifBlank { null },
                )
            }
        }

        override fun onMessage(
            conn: WebSocket,
            message: String,
        ) {
            val attachment = conn.getAttachment<SessionProxyConnection>()
            if (attachment != null) {
                if (!sendSessionProxyInput(
                        sessionId = attachment.sessionId,
                        connectionId = attachment.connectionId,
                        message = message,
                    )
                ) {
                    conn.close(1008, "Session proxy is no longer active")
                }
                return
            }
            thread(name = "DesktopBridgeControl") {
                handleControlMessage(conn, message)
            }
        }

        override fun onError(
            conn: WebSocket?,
            ex: Exception,
        ) {
            Log.w(TAG, "Desktop bridge websocket failed", ex)
        }

        override fun onStart() {
            connectionLostTimeout = 30
        }

        private fun handleControlMessage(
            conn: WebSocket,
            message: String,
        ) {
            val request = runCatching { JSONObject(message) }
                .getOrElse { err ->
                    sendError(conn, null, -32700, err.message ?: "Invalid JSON")
                    return
                }
            val requestId = request.opt("id")
            val method = request.optString("method")
            val params = request.optJSONObject("params")
            pruneCreatedHomeSessionUiLeases()
            runCatching {
                when (method) {
                    "androidSession/list" -> listSessions()
                    "androidSession/read" -> readSession(params)
                    "androidSession/create" -> createSession(params)
                    "androidSession/start" -> startSession(params)
                    "androidSession/answer" -> answerQuestion(params)
                    "androidSession/cancel" -> cancelSession(params)
                    "androidSession/attachTarget" -> attachTarget(params)
                    "androidSession/attach" -> attachSession(params)
                    else -> {
                        sendError(
                            conn = conn,
                            requestId = requestId,
                            code = -32601,
                            message = "Unsupported desktop bridge method: $method",
                        )
                        return
                    }
                }
            }.onSuccess { result ->
                sendResult(conn, requestId, result)
            }.onFailure { err ->
                val code = when (err) {
                    is IllegalArgumentException -> -32602
                    is IllegalStateException -> -32000
                    else -> -32603
                }
                sendError(
                    conn = conn,
                    requestId = requestId,
                    code = code,
                    message = err.message ?: err::class.java.simpleName,
                )
            }
        }

        private fun listSessions(): JSONObject {
            val snapshot = sessionController.loadSnapshot(null)
            val data = JSONArray()
            snapshot.sessions.forEach { session ->
                data.put(sessionJson(session))
            }
            return JSONObject().put("data", data)
        }

        private fun readSession(params: JSONObject?): JSONObject {
            val sessionId = params.requireString("sessionId")
            return sessionJson(requireSession(sessionId), includeTimeline = true)
        }

        private fun createSession(params: JSONObject?): JSONObject {
            val targetPackage = params.optNullableString("targetPackage")
            val model = params.optNullableString("model") ?: DEFAULT_MODEL
            val reasoningEffort = params.optNullableString("reasoningEffort") ?: DEFAULT_REASONING_EFFORT
            val result = AgentSessionLauncher.createSessionDraft(
                request = CreateSessionRequest(
                    targetPackage = targetPackage,
                    model = model,
                    reasoningEffort = reasoningEffort,
                ),
                sessionController = sessionController,
            )
            if (result.anchor == AgentSessionInfo.ANCHOR_HOME) {
                registerCreatedHomeSessionUiLease(result.sessionId)
            }
            return sessionJson(requireSession(result.sessionId), includeTimeline = true)
        }

        private fun startSession(params: JSONObject?): JSONObject {
            val sessionId = params.requireString("sessionId")
            val prompt = params.requireString("prompt")
            val result = AgentSessionLauncher.startSessionDraftAsync(
                context = context,
                request = StartSessionRequest(
                    sessionId = sessionId,
                    prompt = prompt,
                ),
                sessionController = sessionController,
                requestUserInputHandler = null,
            )
            unregisterCreatedHomeSessionUiLease(sessionId)
            return sessionJson(requireSession(result.parentSessionId), includeTimeline = true)
                .put("geniePackage", result.geniePackage)
                .put("plannedTargets", JSONArray(result.plannedTargets))
                .put("childSessionIds", JSONArray(result.childSessionIds))
        }

        private fun answerQuestion(params: JSONObject?): JSONObject {
            val sessionId = params.requireString("sessionId")
            val answer = params.requireString("answer")
            val snapshot = sessionController.loadSnapshot(sessionId)
            val session = snapshot.sessions.firstOrNull { it.sessionId == sessionId }
                ?: throw IllegalArgumentException("Unknown session: $sessionId")
            sessionController.answerQuestion(sessionId, answer, session.parentSessionId)
            return JSONObject().put("ok", true)
        }

        private fun cancelSession(params: JSONObject?): JSONObject {
            val sessionId = params.requireString("sessionId")
            sessionController.cancelSession(sessionId)
            unregisterCreatedHomeSessionUiLease(sessionId)
            return JSONObject().put("ok", true)
        }

        private fun attachTarget(params: JSONObject?): JSONObject {
            val sessionId = params.requireString("sessionId")
            sessionController.attachTarget(sessionId)
            return JSONObject().put("ok", true)
        }

        private fun attachSession(params: JSONObject?): JSONObject {
            val sessionId = params.requireString("sessionId")
            val session = requireSession(sessionId)
            ensureDraftSessionAttachable(session)
            val threadId = activeThreadId(session)
                ?: throw IllegalStateException("Session $sessionId is not attachable")
            pruneExpiredAttachTokens()
            val attachToken = UUID.randomUUID().toString().replace("-", "")
            attachTokens[attachToken] = AttachedSessionTarget(
                sessionId = sessionId,
                expiresAtElapsedRealtimeMs = SystemClock.elapsedRealtime() + ATTACH_TOKEN_TTL_MS,
            )
            return JSONObject()
                .put("sessionId", sessionId)
                .put("threadId", threadId)
                .put("websocketPath", "$SESSION_PATH_PREFIX$attachToken")
        }

        private fun pruneExpiredAttachTokens() {
            val now = SystemClock.elapsedRealtime()
            attachTokens.entries.removeIf { (_, target) ->
                target.expiresAtElapsedRealtimeMs <= now
            }
        }

        private fun sessionJson(
            session: AgentSessionDetails,
            includeTimeline: Boolean = false,
        ): JSONObject {
            val threadId = activeThreadId(session)
            val executionSettings = sessionController.executionSettingsForSession(session.sessionId)
            return JSONObject()
                .put("sessionId", session.sessionId)
                .put("parentSessionId", session.parentSessionId)
                .put("kind", sessionKind(session))
                .put("anchor", session.anchor)
                .put("state", session.state)
                .put("stateLabel", session.stateLabel)
                .put("targetPackage", session.targetPackage)
                .put("targetPresentation", session.targetPresentationLabel)
                .put("targetRuntime", session.targetRuntimeLabel)
                .put("latestQuestion", session.latestQuestion)
                .put("latestResult", session.latestResult)
                .put("latestError", session.latestError)
                .put("latestTrace", session.latestTrace)
                .put("model", executionSettings.model)
                .put("reasoningEffort", executionSettings.reasoningEffort)
                .put("threadId", threadId)
                .put("attachable", !threadId.isNullOrBlank())
                .apply {
                    if (includeTimeline) {
                        put("timeline", session.timeline)
                    }
                }
        }

        private fun activeThreadId(session: AgentSessionDetails): String? {
            return AgentSessionBridgeServer.activeThreadId(session.sessionId)
                ?: AgentPlannerRuntimeManager.activeThreadId(session.sessionId)
        }

        private fun requireSession(sessionId: String): AgentSessionDetails {
            val snapshot = sessionController.loadSnapshot(sessionId)
            return snapshot.sessions.firstOrNull { it.sessionId == sessionId }
                ?: throw IllegalArgumentException("Unknown session: $sessionId")
        }

        private fun ensureDraftSessionAttachable(session: AgentSessionDetails) {
            if (!activeThreadId(session).isNullOrBlank()) {
                return
            }
            if (session.state != AgentSessionInfo.STATE_CREATED) {
                return
            }
            when {
                session.anchor == AgentSessionInfo.ANCHOR_HOME &&
                    session.parentSessionId == null &&
                    !session.targetPackage.isNullOrBlank() -> {
                    sessionController.startExistingHomeSessionIdle(
                        sessionId = session.sessionId,
                        targetPackage = checkNotNull(session.targetPackage),
                        allowDetachedMode = true,
                        finalPresentationPolicy = session.requiredFinalPresentationPolicy
                            ?: SessionFinalPresentationPolicy.AGENT_CHOICE,
                        executionSettings = sessionController.executionSettingsForSession(session.sessionId),
                    )
                    unregisterCreatedHomeSessionUiLease(session.sessionId)
                }
                session.anchor == AgentSessionInfo.ANCHOR_AGENT &&
                    session.parentSessionId == null &&
                    session.targetPackage == null -> {
                    AgentPlannerRuntimeManager.ensureIdleDesktopSession(
                        context = context,
                        sessionController = sessionController,
                        sessionId = session.sessionId,
                    )
                }
                else -> return
            }
            waitForAttachableThread(session)
        }

        private fun waitForAttachableThread(session: AgentSessionDetails) {
            val deadline = SystemClock.elapsedRealtime() + ATTACH_THREAD_WAIT_MS
            while (SystemClock.elapsedRealtime() < deadline) {
                if (!activeThreadId(session).isNullOrBlank()) {
                    return
                }
                Thread.sleep(ATTACH_THREAD_POLL_MS)
            }
            throw IllegalStateException("Session ${session.sessionId} did not expose an attachable thread in time")
        }

        private fun registerCreatedHomeSessionUiLease(sessionId: String) {
            createdHomeSessionUiLeases.computeIfAbsent(sessionId) {
                Binder().also { token ->
                    sessionController.registerSessionUiLease(sessionId, token)
                }
            }
        }

        private fun unregisterCreatedHomeSessionUiLease(sessionId: String) {
            val token = createdHomeSessionUiLeases.remove(sessionId) ?: return
            runCatching {
                sessionController.unregisterSessionUiLease(sessionId, token)
            }
        }

        private fun pruneCreatedHomeSessionUiLeases() {
            if (createdHomeSessionUiLeases.isEmpty()) {
                return
            }
            val sessionsById = sessionController.loadSnapshot(null).sessions.associateBy(AgentSessionDetails::sessionId)
            createdHomeSessionUiLeases.keys.forEach { sessionId ->
                val session = sessionsById[sessionId]
                if (
                    session == null ||
                    session.anchor != AgentSessionInfo.ANCHOR_HOME ||
                    session.parentSessionId != null ||
                    session.state != AgentSessionInfo.STATE_CREATED
                ) {
                    unregisterCreatedHomeSessionUiLease(sessionId)
                }
            }
        }

        private fun openSessionProxy(
            sessionId: String,
            onMessage: (String) -> Unit,
            onClosed: (String?) -> Unit,
        ): String? {
            return AgentSessionBridgeServer.openDesktopProxy(
                sessionId = sessionId,
                onMessage = onMessage,
                onClosed = onClosed,
            ) ?: AgentPlannerRuntimeManager.openDesktopProxy(
                sessionId = sessionId,
                onMessage = onMessage,
                onClosed = onClosed,
            )
        }

        private fun sendSessionProxyInput(
            sessionId: String,
            connectionId: String,
            message: String,
        ): Boolean {
            return AgentSessionBridgeServer.sendDesktopProxyInput(
                sessionId = sessionId,
                connectionId = connectionId,
                message = message,
            ) || AgentPlannerRuntimeManager.sendDesktopProxyInput(
                sessionId = sessionId,
                connectionId = connectionId,
                message = message,
            )
        }

        private fun closeSessionProxy(
            sessionId: String,
            connectionId: String,
            reason: String? = null,
        ) {
            AgentSessionBridgeServer.closeDesktopProxy(sessionId, connectionId, reason)
            AgentPlannerRuntimeManager.closeDesktopProxy(sessionId, connectionId, reason)
        }

        private fun sendResult(
            conn: WebSocket,
            requestId: Any?,
            result: JSONObject,
        ) {
            conn.send(
                JSONObject()
                    .put("id", requestId)
                    .put("result", result)
                    .toString(),
            )
        }

        private fun sendError(
            conn: WebSocket,
            requestId: Any?,
            code: Int,
            message: String,
        ) {
            conn.send(
                JSONObject()
                    .put("id", requestId)
                    .put(
                        "error",
                        JSONObject()
                            .put("code", code)
                            .put("message", message),
                    )
                    .toString(),
            )
        }

        private fun sessionKind(session: AgentSessionDetails): String {
            return when {
                session.anchor == AgentSessionInfo.ANCHOR_AGENT &&
                    session.parentSessionId == null &&
                    session.targetPackage == null -> "agent_parent"
                session.parentSessionId != null -> "genie_child"
                else -> "home_session"
            }
        }

        private fun JSONObject?.requireString(key: String): String {
            val value = this?.optString(key)?.trim().orEmpty()
            require(value.isNotEmpty()) { "Missing required parameter: $key" }
            return value
        }

        private fun JSONObject?.optNullableString(key: String): String? {
            if (this == null || !has(key) || isNull(key)) {
                return null
            }
            return optString(key).trim().ifEmpty { null }
        }
    }

    private data class SessionProxyConnection(
        val sessionId: String,
        val connectionId: String,
    )

    private fun parseBearerToken(header: String?): String? {
        if (header.isNullOrBlank()) {
            return null
        }
        val prefix = "Bearer "
        if (!header.startsWith(prefix, ignoreCase = true)) {
            return null
        }
        return header.substring(prefix.length).trim().ifEmpty { null }
    }
}
