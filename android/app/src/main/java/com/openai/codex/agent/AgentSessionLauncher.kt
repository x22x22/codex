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

    fun startFollowUpSession(
        context: Context,
        sourceTopLevelSession: AgentSessionDetails,
        prompt: String,
        sessionController: AgentSessionController,
        requestUserInputHandler: ((JSONArray) -> JSONObject)? = null,
    ): SessionStartResult {
        val executionSettings = sessionController.executionSettingsForSession(sourceTopLevelSession.sessionId)
        return when (sourceTopLevelSession.anchor) {
            AgentSessionInfo.ANCHOR_HOME -> {
                val targetPackage = checkNotNull(sourceTopLevelSession.targetPackage) {
                    "HOME-anchored session missing target package"
                }
                sessionController.startHomeSession(
                    targetPackage = targetPackage,
                    prompt = prompt,
                    allowDetachedMode = true,
                    finalPresentationPolicy = sourceTopLevelSession.requiredFinalPresentationPolicy
                        ?: SessionFinalPresentationPolicy.AGENT_CHOICE,
                    executionSettings = executionSettings,
                )
            }

            else -> AgentTaskPlanner.startSession(
                context = context,
                userObjective = prompt,
                targetPackageOverride = null,
                allowDetachedMode = true,
                executionSettings = executionSettings,
                sessionController = sessionController,
                requestUserInputHandler = requestUserInputHandler,
            )
        }
    }
}
