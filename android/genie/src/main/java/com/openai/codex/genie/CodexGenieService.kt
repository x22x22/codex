package com.openai.codex.genie

import android.app.agent.AgentSessionInfo
import android.app.agent.GenieRequest
import android.app.agent.GenieService
import android.util.Log
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

class CodexGenieService : GenieService() {
    companion object {
        private const val TAG = "CodexGenieService"
        private const val MAX_OBJECTIVE_PROMPT_CHARS = 240
        private const val MAX_AGENT_ANSWER_CHARS = 120
        private const val GENIE_RESPONSE_INSTRUCTIONS =
            "You are Codex acting as an Android Genie. Reply with exactly one line that starts with QUESTION: or RESULT:."
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
        sessionControls[sessionId]?.userResponses?.offer(response)
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
                "Genie is headless and routes control/data traffic through the Agent-owned Binder bridge.",
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

            AgentBridgeClient(this).use { bridgeClient ->
                val runtimeStatus = runCatching { bridgeClient.getRuntimeStatus() }
                runtimeStatus.onSuccess { status ->
                    val accountSuffix = status.accountEmail?.let { " (${it})" } ?: ""
                    callback.publishTrace(
                        sessionId,
                        "Reached Agent Binder bridge; authenticated=${status.authenticated}${accountSuffix}, provider=${status.modelProviderId}, model=${status.effectiveModel ?: "unknown"}, clients=${status.clientCount}.",
                    )
                }
                runtimeStatus.onFailure { err ->
                    callback.publishTrace(
                        sessionId,
                        "Agent Binder bridge probe failed: ${err.message}",
                    )
                }

                if (request.isDetachedModeAllowed) {
                    callback.requestLaunchDetachedTargetHidden(sessionId)
                    callback.publishTrace(sessionId, "Requested detached target launch for ${request.targetPackage}.")
                }

                callback.publishQuestion(
                    sessionId,
                    buildAgentQuestion(
                        request = request,
                        targetAppContext = targetAppContext.getOrNull(),
                    ),
                )
                callback.updateState(sessionId, AgentSessionInfo.STATE_WAITING_FOR_USER)

                if (control.cancelled) {
                    callback.publishError(sessionId, "Cancelled")
                    callback.updateState(sessionId, AgentSessionInfo.STATE_CANCELLED)
                    return
                }

                var runtime = runtimeStatus.getOrNull()

                var answer = waitForAgentAnswer(
                    sessionId = sessionId,
                    callback = callback,
                    control = control,
                )
                Log.i(TAG, "Received Agent answer for $sessionId")
                callback.publishTrace(sessionId, "Received Agent answer: $answer")

                while (!control.cancelled) {
                    if (runtime == null || !runtime.authenticated || runtime.effectiveModel.isNullOrBlank()) {
                        runtime = runCatching { bridgeClient.getRuntimeStatus() }
                            .onFailure { err ->
                                callback.publishTrace(
                                    sessionId,
                                    "Agent Binder runtime refresh failed: ${err.message}",
                                )
                            }
                            .getOrNull()
                    }
                    if (runtime == null) {
                        callback.publishResult(
                            sessionId,
                            "Reached the Agent bridge, but runtime status was unavailable. Replace this scaffold with a real Codex-driven Genie executor.",
                        )
                        callback.updateState(sessionId, AgentSessionInfo.STATE_COMPLETED)
                        return
                    }
                    if (!runtime.authenticated || runtime.effectiveModel.isNullOrBlank()) {
                        callback.publishResult(
                            sessionId,
                            "Reached the Agent bridge, but the Agent runtime was not authenticated or did not expose an effective model for ${request.targetPackage}.",
                        )
                        callback.updateState(sessionId, AgentSessionInfo.STATE_COMPLETED)
                        return
                    }
                    callback.publishTrace(
                        sessionId,
                        "Requesting a streaming /v1/responses call through the Agent using ${runtime.effectiveModel}.",
                    )
                    val modelResponse = runCatching {
                        requestModelNextStep(
                            request = request,
                            answer = answer,
                            runtimeStatus = runtime,
                            targetAppContext = targetAppContext.getOrNull(),
                            bridgeClient = bridgeClient,
                        )
                    }
                    if (modelResponse.isFailure) {
                        callback.publishTrace(
                            sessionId,
                            "Agent-mediated /v1/responses request failed: ${modelResponse.exceptionOrNull()?.message}",
                        )
                        callback.publishResult(
                            sessionId,
                            "Reached the Agent bridge for ${request.targetPackage}, but the proxied model request failed. Replace this scaffold with a real Codex-driven Genie executor.",
                        )
                        callback.updateState(sessionId, AgentSessionInfo.STATE_COMPLETED)
                        return
                    }

                    when (val turn = parseGenieModelTurn(modelResponse.getOrThrow())) {
                        is GenieModelTurn.Result -> {
                            Log.i(TAG, "Publishing Genie result for $sessionId")
                            callback.publishResult(sessionId, turn.text)
                            callback.updateState(sessionId, AgentSessionInfo.STATE_COMPLETED)
                            return
                        }
                        is GenieModelTurn.Question -> {
                            Log.i(TAG, "Publishing Genie follow-up question for $sessionId")
                            callback.publishTrace(sessionId, "Genie follow-up question: ${turn.text}")
                            callback.publishQuestion(sessionId, turn.text)
                            callback.updateState(sessionId, AgentSessionInfo.STATE_WAITING_FOR_USER)
                            answer = waitForAgentAnswer(
                                sessionId = sessionId,
                                callback = callback,
                                control = control,
                            )
                            Log.i(TAG, "Received follow-up Agent answer for $sessionId")
                            callback.publishTrace(sessionId, "Received Agent answer: $answer")
                        }
                    }
                }

                callback.publishError(sessionId, "Cancelled")
                callback.updateState(sessionId, AgentSessionInfo.STATE_CANCELLED)
            }
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

    private fun requestModelNextStep(
        request: GenieRequest,
        answer: String,
        runtimeStatus: CodexAgentBridge.RuntimeStatus,
        targetAppContext: TargetAppContext?,
        bridgeClient: AgentBridgeClient,
    ): String {
        val model = checkNotNull(runtimeStatus.effectiveModel) { "missing effective model" }
        val response = bridgeClient.sendHttpRequest(
            method = "POST",
            path = "/v1/responses",
            body = CodexAgentBridge.buildResponsesRequest(
                model = model,
                instructions = GENIE_RESPONSE_INSTRUCTIONS,
                prompt = buildModelPrompt(
                    request = request,
                    answer = answer,
                    targetAppContext = targetAppContext,
                ),
            ).toString(),
        )
        return CodexAgentBridge.parseResponsesOutputText(response)
    }

    private fun waitForAgentAnswer(
        sessionId: String,
        callback: Callback,
        control: SessionControl,
    ): String {
        val answer = waitForUserResponse(control)
        callback.updateState(sessionId, AgentSessionInfo.STATE_RUNNING)
        return answer
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
        val objective = abbreviate(request.prompt, MAX_OBJECTIVE_PROMPT_CHARS)
        val agentAnswer = abbreviate(answer, MAX_AGENT_ANSWER_CHARS)
        val targetSummary = targetAppContext?.renderPromptSection()
            ?: "Target app inspection:\n- unavailable"
        return """
            You are Codex acting as an Android Genie for the target package ${request.targetPackage}.
            Original objective: $objective
            The Agent answered your latest question with: $agentAnswer

            $targetSummary

            Emit exactly one line starting with QUESTION: or RESULT:.
            Use QUESTION: when you need another free-form answer from the Agent before you can proceed.
            Use RESULT: when you are ready to report the next concrete step or final outcome.
        """.trimIndent()
    }

    private fun buildAgentQuestion(
        request: GenieRequest,
        targetAppContext: TargetAppContext?,
    ): String {
        val displayName = targetAppContext?.displayName() ?: request.targetPackage
        return "Codex Genie is ready to drive $displayName. Reply with any extra constraints or answer 'continue' to let Genie proceed."
    }

    private fun parseGenieModelTurn(message: String): GenieModelTurn {
        val trimmed = message.trim()
        val question = stripTurnPrefix(trimmed, "QUESTION:")
        if (question != null) {
            return GenieModelTurn.Question(question)
        }
        val result = stripTurnPrefix(trimmed, "RESULT:")
        if (result != null) {
            return GenieModelTurn.Result(result)
        }
        return if (trimmed.endsWith("?")) {
            GenieModelTurn.Question(trimmed)
        } else {
            GenieModelTurn.Result(trimmed)
        }
    }

    private fun stripTurnPrefix(message: String, prefix: String): String? {
        if (!message.startsWith(prefix, ignoreCase = true)) {
            return null
        }
        return message.substring(prefix.length).trim().ifEmpty { "continue" }
    }

    private fun abbreviate(value: String, maxChars: Int): String {
        if (value.length <= maxChars) {
            return value
        }
        return value.take(maxChars - 1) + "…"
    }

    private class SessionControl {
        @Volatile var cancelled = false
        val userResponses = LinkedBlockingQueue<String>()
    }

    private sealed interface GenieModelTurn {
        data class Question(val text: String) : GenieModelTurn

        data class Result(val text: String) : GenieModelTurn
    }
}
