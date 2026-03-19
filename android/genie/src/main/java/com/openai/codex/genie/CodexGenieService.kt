package com.openai.codex.genie

import android.app.agent.AgentSessionInfo
import android.app.agent.GenieRequest
import android.app.agent.GenieService
import android.util.Log
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.UUID

class CodexGenieService : GenieService() {
    companion object {
        private const val TAG = "CodexGenieService"
    }

    private val sessionControls = ConcurrentHashMap<String, SessionControl>()

    override fun onStartGenieSession(request: GenieRequest, callback: Callback) {
        val control = SessionControl()
        sessionControls[request.sessionId] = control
        Thread {
            runSession(request, callback, control)
        }.apply {
            name = "CodexGenie-${request.sessionId}"
            start()
        }
    }

    override fun onCancelGenieSession(sessionId: String) {
        sessionControls.remove(sessionId)?.cancelled = true
        Log.i(TAG, "Cancelled session $sessionId")
    }

    override fun onUserResponse(sessionId: String, response: String) {
        val control = sessionControls[sessionId] ?: return
        if (CodexAgentBridge.isBridgeResponse(response)) {
            control.bridgeResponses.offer(response)
        } else {
            control.userResponses.offer(response)
        }
        Log.i(TAG, "Received user response for $sessionId")
    }

    private fun runSession(request: GenieRequest, callback: Callback, control: SessionControl) {
        val sessionId = request.sessionId
        try {
            callback.updateState(sessionId, AgentSessionInfo.STATE_RUNNING)
            callback.publishTrace(
                sessionId,
                "Codex Genie scaffold started for target=${request.targetPackage} prompt=${request.prompt}",
            )
            callback.publishTrace(
                sessionId,
                "Genie is headless and uses the Agent-owned bridge for auth/network reachability checks.",
            )
            val bridgeStatus = runCatching { requestAgentAuthStatus(sessionId, callback, control) }
            bridgeStatus.onSuccess { status ->
                val accountSuffix = status.accountEmail?.let { " (${it})" } ?: ""
                callback.publishTrace(
                    sessionId,
                    "Reached Agent bridge through framework orchestration; authenticated=${status.authenticated}${accountSuffix}, clients=${status.clientCount}.",
                )
            }
            bridgeStatus.onFailure { err ->
                callback.publishTrace(
                    sessionId,
                    "Agent bridge probe failed: ${err.message}",
                )
            }

            if (request.isDetachedModeAllowed) {
                callback.requestLaunchDetachedTargetHidden(sessionId)
                callback.publishTrace(sessionId, "Requested detached target launch for ${request.targetPackage}.")
            }

            callback.publishQuestion(
                sessionId,
                "Codex Genie scaffold is active for ${request.targetPackage}. Continue the placeholder flow?",
            )
            callback.updateState(sessionId, AgentSessionInfo.STATE_WAITING_FOR_USER)

            if (control.cancelled) {
                callback.publishError(sessionId, "Cancelled")
                callback.updateState(sessionId, AgentSessionInfo.STATE_CANCELLED)
                return
            }

            val answer = waitForUserResponse(control)
            callback.updateState(sessionId, AgentSessionInfo.STATE_RUNNING)
            callback.publishTrace(sessionId, "Received user response: $answer")
            callback.publishResult(
                sessionId,
                "Placeholder Genie result for ${request.targetPackage}. Replace this with Codex-driven Android tool execution.",
            )
            callback.updateState(sessionId, AgentSessionInfo.STATE_COMPLETED)
        } catch (err: InterruptedException) {
            Thread.currentThread().interrupt()
            callback.publishError(sessionId, "Interrupted: ${err.message}")
            callback.updateState(sessionId, AgentSessionInfo.STATE_FAILED)
        } catch (err: RuntimeException) {
            callback.publishError(sessionId, "${err::class.java.simpleName}: ${err.message}")
            callback.updateState(sessionId, AgentSessionInfo.STATE_FAILED)
        } finally {
            sessionControls.remove(sessionId)
        }
    }

    private fun requestAgentAuthStatus(
        sessionId: String,
        callback: Callback,
        control: SessionControl,
    ): CodexAgentBridge.AuthStatus {
        val requestId = UUID.randomUUID().toString()
        callback.publishQuestion(sessionId, CodexAgentBridge.buildAuthStatusRequest(requestId))
        callback.updateState(sessionId, AgentSessionInfo.STATE_WAITING_FOR_USER)
        val response = waitForBridgeResponse(control, requestId)
        callback.updateState(sessionId, AgentSessionInfo.STATE_RUNNING)
        return CodexAgentBridge.parseAuthStatusResponse(response, requestId)
    }

    private fun waitForBridgeResponse(control: SessionControl, requestId: String): String {
        val deadlineNanos = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
        while (!control.cancelled) {
            val remainingNanos = deadlineNanos - System.nanoTime()
            if (remainingNanos <= 0) {
                throw IOException("Timed out waiting for Agent bridge response")
            }
            val response = control.bridgeResponses.poll(remainingNanos, TimeUnit.NANOSECONDS)
            if (response != null) {
                return response
            }
        }
        throw IOException("Cancelled while waiting for Agent bridge response $requestId")
    }

    private fun waitForUserResponse(control: SessionControl): String {
        while (!control.cancelled) {
            val response = control.userResponses.poll(100, TimeUnit.MILLISECONDS)
            if (response != null) {
                return response
            }
        }
        throw IOException("Cancelled while waiting for user response")
    }

    private class SessionControl {
        @Volatile var cancelled = false
        val bridgeResponses = LinkedBlockingQueue<String>()
        val userResponses = LinkedBlockingQueue<String>()
    }
}
