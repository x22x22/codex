package com.openai.codex.genie

import android.app.agent.AgentSessionInfo
import android.app.agent.GenieRequest
import android.app.agent.GenieService
import android.util.Log
import java.util.concurrent.ConcurrentHashMap

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
        sessionControls[sessionId]?.answer = response
        sessionControls[sessionId]?.answerLatch = false
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
                "Agent-mediated Codex runtime transport is the next integration step; this service currently validates framework lifecycle and question flow.",
            )

            if (request.isDetachedModeAllowed) {
                callback.requestLaunchDetachedTargetHidden(sessionId)
                callback.publishTrace(sessionId, "Requested detached target launch for ${request.targetPackage}.")
            }

            callback.publishQuestion(
                sessionId,
                "Codex Genie scaffold is active for ${request.targetPackage}. Continue the placeholder flow?",
            )
            callback.updateState(sessionId, AgentSessionInfo.STATE_WAITING_FOR_USER)

            while (control.answerLatch && !control.cancelled) {
                Thread.sleep(100)
            }

            if (control.cancelled) {
                callback.publishError(sessionId, "Cancelled")
                callback.updateState(sessionId, AgentSessionInfo.STATE_CANCELLED)
                return
            }

            callback.updateState(sessionId, AgentSessionInfo.STATE_RUNNING)
            val answer = control.answer ?: ""
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

    private class SessionControl {
        @Volatile var answerLatch = true
        @Volatile var cancelled = false
        @Volatile var answer: String? = null
    }
}
