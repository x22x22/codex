package com.openai.codex.agent

import android.app.agent.AgentSessionInfo
import android.content.Context
import com.openai.codex.bridge.SessionExecutionSettings
import kotlin.concurrent.thread
import org.json.JSONArray
import org.json.JSONObject

data class LaunchSessionRequest(
    val prompt: String,
    val targetPackage: String?,
    val model: String?,
    val reasoningEffort: String?,
    val existingSessionId: String? = null,
)

object AgentSessionLauncher {
    fun startSessionAsync(
        context: Context,
        request: LaunchSessionRequest,
        sessionController: AgentSessionController,
        requestUserInputHandler: ((JSONArray) -> JSONObject)? = null,
    ): SessionStartResult {
        val executionSettings = SessionExecutionSettings(
            model = request.model?.trim()?.ifEmpty { null },
            reasoningEffort = request.reasoningEffort?.trim()?.ifEmpty { null },
        )
        val targetPackage = request.targetPackage?.trim()?.ifEmpty { null }
        val existingSessionId = request.existingSessionId?.trim()?.ifEmpty { null }
        if (targetPackage != null || existingSessionId != null) {
            return startSession(
                context = context,
                request = request,
                sessionController = sessionController,
                requestUserInputHandler = requestUserInputHandler,
            )
        }
        val pendingSession = sessionController.createPendingDirectSession(
            objective = request.prompt,
            executionSettings = executionSettings,
        )
        val applicationContext = context.applicationContext
        thread(name = "CodexAgentPlanner-${pendingSession.parentSessionId}") {
            runCatching {
                AgentTaskPlanner.planSession(
                    context = applicationContext,
                    userObjective = request.prompt,
                    executionSettings = executionSettings,
                    sessionController = sessionController,
                    requestUserInputHandler = null,
                )
            }.onFailure { err ->
                if (!sessionController.isTerminalSession(pendingSession.parentSessionId)) {
                    sessionController.failDirectSession(
                        pendingSession.parentSessionId,
                        "Planning failed: ${err.message ?: err::class.java.simpleName}",
                    )
                }
            }.onSuccess { plannedRequest ->
                if (!sessionController.isTerminalSession(pendingSession.parentSessionId)) {
                    runCatching {
                        sessionController.startDirectSessionChildren(
                            parentSessionId = pendingSession.parentSessionId,
                            geniePackage = pendingSession.geniePackage,
                            plan = plannedRequest.plan,
                            allowDetachedMode = plannedRequest.allowDetachedMode,
                            executionSettings = executionSettings,
                        )
                    }.onFailure { err ->
                        if (!sessionController.isTerminalSession(pendingSession.parentSessionId)) {
                            sessionController.failDirectSession(
                                pendingSession.parentSessionId,
                                "Failed to start planned child session: ${err.message ?: err::class.java.simpleName}",
                            )
                        }
                    }
                }
            }
        }
        return SessionStartResult(
            parentSessionId = pendingSession.parentSessionId,
            childSessionIds = emptyList(),
            plannedTargets = emptyList(),
            geniePackage = pendingSession.geniePackage,
            anchor = AgentSessionInfo.ANCHOR_AGENT,
        )
    }

    fun startSession(
        context: Context,
        request: LaunchSessionRequest,
        sessionController: AgentSessionController,
        requestUserInputHandler: ((JSONArray) -> JSONObject)? = null,
    ): SessionStartResult {
        val executionSettings = SessionExecutionSettings(
            model = request.model?.trim()?.ifEmpty { null },
            reasoningEffort = request.reasoningEffort?.trim()?.ifEmpty { null },
        )
        val targetPackage = request.targetPackage?.trim()?.ifEmpty { null }
        val existingSessionId = request.existingSessionId?.trim()?.ifEmpty { null }
        return if (targetPackage == null) {
            check(existingSessionId == null) {
                "Existing HOME sessions require a target package"
            }
            AgentTaskPlanner.startSession(
                context = context,
                userObjective = request.prompt,
                targetPackageOverride = null,
                allowDetachedMode = true,
                executionSettings = executionSettings,
                sessionController = sessionController,
                requestUserInputHandler = requestUserInputHandler,
            )
        } else {
            if (existingSessionId != null) {
                sessionController.startExistingHomeSession(
                    sessionId = existingSessionId,
                    targetPackage = targetPackage,
                    prompt = request.prompt,
                    allowDetachedMode = true,
                    finalPresentationPolicy = SessionFinalPresentationPolicy.AGENT_CHOICE,
                    executionSettings = executionSettings,
                )
            } else {
                sessionController.startHomeSession(
                    targetPackage = targetPackage,
                    prompt = request.prompt,
                    allowDetachedMode = true,
                    finalPresentationPolicy = SessionFinalPresentationPolicy.AGENT_CHOICE,
                    executionSettings = executionSettings,
                )
            }
        }
    }

    fun continueSessionInPlace(
        sourceTopLevelSession: AgentSessionDetails,
        selectedSession: AgentSessionDetails,
        prompt: String,
        sessionController: AgentSessionController,
    ): SessionStartResult {
        val executionSettings = sessionController.executionSettingsForSession(sourceTopLevelSession.sessionId)
        return when (sourceTopLevelSession.anchor) {
            AgentSessionInfo.ANCHOR_HOME -> {
                throw UnsupportedOperationException(
                    "In-place continuation is not supported for app-scoped HOME sessions on the current framework",
                )
            }

            else -> {
                val targetPackage = checkNotNull(selectedSession.targetPackage) {
                    "Select a target child session to continue"
                }
                sessionController.continueDirectSessionInPlace(
                    parentSessionId = sourceTopLevelSession.sessionId,
                    target = AgentDelegationTarget(
                        packageName = targetPackage,
                        objective = SessionContinuationPromptBuilder.build(
                            sourceTopLevelSession = sourceTopLevelSession,
                            selectedSession = selectedSession,
                            prompt = prompt,
                        ),
                        finalPresentationPolicy = selectedSession.requiredFinalPresentationPolicy
                            ?: SessionFinalPresentationPolicy.AGENT_CHOICE,
                    ),
                    executionSettings = executionSettings,
                )
            }
        }
    }
}
