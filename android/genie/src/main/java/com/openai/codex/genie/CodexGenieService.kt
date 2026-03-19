package com.openai.codex.genie

import android.app.agent.AgentSessionInfo
import android.app.agent.GenieRequest
import android.app.agent.GenieService
import android.util.Log
import java.io.IOException
import java.util.ArrayDeque
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

class CodexGenieService : GenieService() {
    companion object {
        private const val TAG = "CodexGenieService"
        private const val MAX_MODEL_TURNS = 12
        private const val MAX_OBJECTIVE_PROMPT_CHARS = 240
        private const val MAX_AGENT_ANSWER_CHARS = 120
        private const val MAX_TOOL_OBSERVATIONS = 6
        private val GENIE_RESPONSE_INSTRUCTIONS =
            """
            You are Codex acting as an Android Genie.
            Reply with exactly one line that starts with TOOL:, QUESTION:, or RESULT:.
            Use TOOL: with a single JSON object on the same line, for example:
            TOOL: {"name":"android.intent.launch","arguments":{"packageName":"com.android.deskclock"}}
            Available tools:
            - android.package.inspect {packageName?}
            - android.intent.launch {packageName?, action?, component?}
            - android.target.show {}
            - android.target.hide {}
            - android.target.attach {}
            - android.target.close {}
            - android.target.capture_frame {}
            - android.ui.dump {}
            - android.input.tap {x, y}
            - android.input.text {text}
            - android.input.key {key}
            - android.wait {millis}
            Use QUESTION: only when you need another free-form answer from the Agent.
            Use RESULT: when you are ready to report the next concrete step or final outcome.
            Do not emit markdown or extra lines.
            """.trimIndent()
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
                "Codex Genie started for target=${request.targetPackage} prompt=${request.prompt}",
            )
            callback.publishTrace(
                sessionId,
                "Genie is headless, routes control/data traffic through the Agent-owned Binder bridge, and uses structured Android tools locally.",
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
                val toolExecutor = AndroidGenieToolExecutor(
                    context = this,
                    callback = callback,
                    sessionId = sessionId,
                    defaultTargetPackage = request.targetPackage,
                )
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
                val toolObservations = ArrayDeque<GenieToolObservation>()

                var answer = waitForAgentAnswer(
                    sessionId = sessionId,
                    callback = callback,
                    control = control,
                )
                Log.i(TAG, "Received Agent answer for $sessionId")
                callback.publishTrace(sessionId, "Received Agent answer: $answer")

                repeat(MAX_MODEL_TURNS) {
                    if (control.cancelled) {
                        return@repeat
                    }
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
                    val activeRuntime = requireNotNull(runtime)
                    callback.publishTrace(
                        sessionId,
                        "Requesting a streaming /v1/responses call through the Agent using ${activeRuntime.effectiveModel}.",
                    )
                    val modelResponse = runCatching {
                        requestModelNextStep(
                            request = request,
                            answer = answer,
                            runtimeStatus = activeRuntime,
                            targetAppContext = targetAppContext.getOrNull(),
                            toolObservations = toolObservations.toList(),
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

                    when (val turn = GenieModelTurnParser.parse(modelResponse.getOrThrow())) {
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
                        is GenieModelTurn.ToolCall -> {
                            val observation = runCatching {
                                toolExecutor.execute(turn)
                            }.getOrElse { err ->
                                GenieToolObservation(
                                    name = turn.name,
                                    summary = "Tool ${turn.name} failed: ${err.message}",
                                    promptDetails = "Tool ${turn.name} failed.\nError: ${err.message ?: err::class.java.simpleName}",
                                )
                            }
                            rememberToolObservation(toolObservations, observation)
                            callback.publishTrace(sessionId, observation.summary)
                        }
                    }
                }

                callback.publishResult(
                    sessionId,
                    "Genie stopped after reaching the current tool/model turn limit. Continue the session with more guidance or increase the loop budget in code.",
                )
                callback.updateState(sessionId, AgentSessionInfo.STATE_COMPLETED)
                return
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
        toolObservations: List<GenieToolObservation>,
        bridgeClient: AgentBridgeClient,
    ): String {
        val model = checkNotNull(runtimeStatus.effectiveModel) { "missing effective model" }
        val recentImageInputs = toolObservations
            .flatMap(GenieToolObservation::imageDataUrls)
            .takeLast(1)
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
                    toolObservations = toolObservations,
                ),
                imageDataUrls = recentImageInputs,
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
        toolObservations: List<GenieToolObservation>,
    ): String {
        val objective = abbreviate(request.prompt, MAX_OBJECTIVE_PROMPT_CHARS)
        val agentAnswer = abbreviate(answer, MAX_AGENT_ANSWER_CHARS)
        val targetSummary = targetAppContext?.renderPromptSection()
            ?: "Target app inspection:\n- unavailable"
        val toolSummary = toolObservations.joinToString(separator = "\n\n") { it.renderForPrompt() }
            .ifBlank { "No tool observations yet." }
        return """
            You are Codex acting as an Android Genie for the target package ${request.targetPackage}.
            Original objective: $objective
            The Agent answered your latest question with: $agentAnswer

            $targetSummary

            Recent tool observations:
            $toolSummary

            Emit exactly one line starting with TOOL:, QUESTION:, or RESULT:.
        """.trimIndent()
    }

    private fun buildAgentQuestion(
        request: GenieRequest,
        targetAppContext: TargetAppContext?,
    ): String {
        val displayName = targetAppContext?.displayName() ?: request.targetPackage
        return "Codex Genie is ready to drive $displayName. Reply with any extra constraints or answer 'continue' to let Genie proceed."
    }

    private fun abbreviate(value: String, maxChars: Int): String {
        if (value.length <= maxChars) {
            return value
        }
        return value.take(maxChars - 1) + "…"
    }

    private fun rememberToolObservation(
        toolObservations: ArrayDeque<GenieToolObservation>,
        observation: GenieToolObservation,
    ) {
        toolObservations.addLast(observation)
        while (toolObservations.size > MAX_TOOL_OBSERVATIONS) {
            toolObservations.removeFirst()
        }
    }

    private class SessionControl {
        @Volatile var cancelled = false
        val userResponses = LinkedBlockingQueue<String>()
    }
}
