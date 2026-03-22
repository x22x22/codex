package com.openai.codex.agent

import android.app.agent.AgentSessionInfo
import android.content.Context
import com.openai.codex.bridge.SessionExecutionSettings
import org.json.JSONArray
import org.json.JSONObject

data class LaunchSessionRequest(
    val prompt: String,
    val targetPackage: String?,
    val model: String?,
    val reasoningEffort: String?,
)

object AgentSessionLauncher {
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
        return if (targetPackage == null) {
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
            sessionController.startHomeSession(
                targetPackage = targetPackage,
                prompt = request.prompt,
                allowDetachedMode = true,
                finalPresentationPolicy = SessionFinalPresentationPolicy.AGENT_CHOICE,
                executionSettings = executionSettings,
            )
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
                        objective = prompt,
                        finalPresentationPolicy = selectedSession.requiredFinalPresentationPolicy
                            ?: SessionFinalPresentationPolicy.AGENT_CHOICE,
                    ),
                    executionSettings = executionSettings,
                )
            }
        }
    }
}
