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
        private const val MAX_BRIDGE_PROMPT_CHARS = 240
        private const val MAX_BRIDGE_ANSWER_CHARS = 120
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
            Log.i(TAG, "Received bridge response for $sessionId")
        } else {
            control.userResponses.offer(response)
            Log.i(TAG, "Received user response for $sessionId")
        }
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
                "Genie is headless and routes model/backend traffic through the Agent-owned bridge.",
            )
            val targetAppContext = runCatching { TargetAppInspector.inspect(this, request.targetPackage) }
            targetAppContext.onSuccess { targetApp ->
                callback.publishTrace(
                    sessionId,
                    "Inspected target app inside the paired sandbox: ${targetApp.describeForTrace()}",
                )
            }
            targetAppContext.onFailure { err ->
                callback.publishTrace(
                    sessionId,
                    "Target app inspection failed for ${request.targetPackage}: ${err.message}",
                )
            }
            val runtimeStatus = runCatching { requestAgentRuntimeStatus(sessionId, callback, control) }
            runtimeStatus.onSuccess { status ->
                val accountSuffix = status.accountEmail?.let { " (${it})" } ?: ""
                callback.publishTrace(
                    sessionId,
                    "Reached Agent bridge through framework orchestration; authenticated=${status.authenticated}${accountSuffix}, provider=${status.modelProviderId}, model=${status.effectiveModel ?: "unknown"}, clients=${status.clientCount}.",
                )
            }
            runtimeStatus.onFailure { err ->
                callback.publishTrace(
                    sessionId,
                    "Agent runtime probe failed: ${err.message}",
                )
            }

            if (request.isDetachedModeAllowed) {
                callback.requestLaunchDetachedTargetHidden(sessionId)
                callback.publishTrace(sessionId, "Requested detached target launch for ${request.targetPackage}.")
            }

            callback.publishQuestion(
                sessionId,
                "Codex Genie is active for ${targetAppContext.getOrNull()?.displayName() ?: request.targetPackage}. Continue with the Agent-bridged next-step synthesis?",
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
            val runtime = runtimeStatus.getOrNull()
            val modelResponse = runtime?.takeIf { status ->
                status.authenticated && !status.effectiveModel.isNullOrBlank()
            }?.let { status ->
                callback.publishTrace(
                    sessionId,
                    "Requesting a non-streaming /v1/responses call through the Agent using ${status.effectiveModel}.",
                )
                runCatching {
                    requestModelNextStep(
                        sessionId = sessionId,
                        request = request,
                        answer = answer,
                        runtimeStatus = status,
                        targetAppContext = targetAppContext.getOrNull(),
                        callback = callback,
                        control = control,
                    )
                }
            }

            when {
                modelResponse == null && runtime == null -> {
                    callback.publishResult(
                        sessionId,
                        "Reached the framework-managed Agent bridge, but runtime status was unavailable. Replace this scaffold with a real Codex-driven Genie executor.",
                    )
                }
                modelResponse == null -> {
                    callback.publishResult(
                        sessionId,
                        "Reached the Agent bridge, but the Agent runtime was not authenticated or did not expose an effective model for ${request.targetPackage}.",
                    )
                }
                modelResponse.isSuccess -> {
                    callback.publishResult(
                        sessionId,
                        modelResponse.getOrThrow(),
                    )
                }
                else -> {
                    callback.publishTrace(
                        sessionId,
                        "Agent-mediated /v1/responses request failed: ${modelResponse.exceptionOrNull()?.message}",
                    )
                    callback.publishResult(
                        sessionId,
                        "Reached the Agent bridge for ${request.targetPackage}, but the proxied model request failed. Replace this scaffold with a real Codex-driven Genie executor.",
                    )
                }
            }
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

    private fun requestAgentRuntimeStatus(
        sessionId: String,
        callback: Callback,
        control: SessionControl,
    ): CodexAgentBridge.RuntimeStatus {
        val requestId = UUID.randomUUID().toString()
        callback.publishQuestion(sessionId, CodexAgentBridge.buildRuntimeStatusRequest(requestId))
        callback.updateState(sessionId, AgentSessionInfo.STATE_WAITING_FOR_USER)
        val response = waitForBridgeResponse(control, requestId)
        callback.updateState(sessionId, AgentSessionInfo.STATE_RUNNING)
        return CodexAgentBridge.parseRuntimeStatusResponse(response, requestId)
    }

    private fun requestModelNextStep(
        sessionId: String,
        request: GenieRequest,
        answer: String,
        runtimeStatus: CodexAgentBridge.RuntimeStatus,
        targetAppContext: TargetAppContext?,
        callback: Callback,
        control: SessionControl,
    ): String {
        val model = checkNotNull(runtimeStatus.effectiveModel) { "missing effective model" }
        val requestId = UUID.randomUUID().toString()
        callback.publishQuestion(
            sessionId,
            CodexAgentBridge.buildResponsesRequest(
                requestId = requestId,
                model = model,
                prompt = buildModelPrompt(
                    request = request,
                    answer = answer,
                    targetAppContext = targetAppContext,
                ),
            ),
        )
        callback.updateState(sessionId, AgentSessionInfo.STATE_WAITING_FOR_USER)
        val response = waitForBridgeResponse(control, requestId)
        callback.updateState(sessionId, AgentSessionInfo.STATE_RUNNING)
        return CodexAgentBridge.parseResponsesOutputText(response, requestId)
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

    private fun buildModelPrompt(
        request: GenieRequest,
        answer: String,
        targetAppContext: TargetAppContext?,
    ): String {
        val objective = abbreviate(request.prompt, MAX_BRIDGE_PROMPT_CHARS)
        val userAnswer = abbreviate(answer, MAX_BRIDGE_ANSWER_CHARS)
        val targetSummary = targetAppContext?.renderPromptSection()
            ?: "Target app inspection:\n- unavailable"
        return """
            You are Codex acting as an Android Genie for the target package ${request.targetPackage}.
            Original objective: $objective
            The user answered the current question with: $userAnswer
            
            $targetSummary

            Reply with one short sentence describing the next automation step you would take in the target app.
        """.trimIndent()
    }

    private fun abbreviate(value: String, maxChars: Int): String {
        if (value.length <= maxChars) {
            return value
        }
        return value.take(maxChars - 1) + "…"
    }

    private class SessionControl {
        @Volatile var cancelled = false
        val bridgeResponses = LinkedBlockingQueue<String>()
        val userResponses = LinkedBlockingQueue<String>()
    }
}
