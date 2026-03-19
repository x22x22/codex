package com.openai.codexd

import android.content.Context
import java.io.IOException

data class AgentDelegationTarget(
    val packageName: String,
    val objective: String,
)

data class AgentDelegationPlan(
    val originalObjective: String,
    val targets: List<AgentDelegationTarget>,
    val rationale: String?,
    val usedOverride: Boolean,
) {
    val primaryTargetPackage: String
        get() = targets.first().packageName
}

object AgentTaskPlanner {
    private val PLANNER_INSTRUCTIONS =
        """
        You are Codex acting as the Android Agent orchestrator.
        The user interacts only with the Agent. Decide which installed Android packages should receive delegated Genie sessions.
        Use the standard Android shell tools already available in this runtime, such as `cmd package`, `pm`, and `am`, to inspect installed packages and resolve the correct targets.
        After deciding on the target packages, call the framework session tool `${AgentFrameworkToolBridge.START_DIRECT_SESSION_TOOL}` exactly once.
        Rules:
        - Choose the fewest packages needed to complete the request.
        - The framework session tool `targets` must be non-empty.
        - Each delegated `objective` should be written for the child Genie, not the user.
        - After the framework session tool succeeds, reply with a short summary for the Agent UI.
        """.trimIndent()

    fun startSession(
        context: Context,
        userObjective: String,
        targetPackageOverride: String?,
        allowDetachedMode: Boolean,
        sessionController: AgentSessionController,
    ): SessionStartResult {
        if (!targetPackageOverride.isNullOrBlank()) {
            return sessionController.startDirectSession(
                plan = AgentDelegationPlan(
                    originalObjective = userObjective,
                    targets = listOf(
                        AgentDelegationTarget(
                            packageName = targetPackageOverride,
                            objective = userObjective,
                        ),
                    ),
                    rationale = "Using explicit target package override.",
                    usedOverride = true,
                ),
                allowDetachedMode = allowDetachedMode,
            )
        }
        var sessionStartResult: SessionStartResult? = null
        val frameworkToolBridge = AgentFrameworkToolBridge(context, sessionController)
        AgentCodexAppServerClient.requestText(
            context = context,
            instructions = PLANNER_INSTRUCTIONS,
            prompt = buildPlannerPrompt(userObjective),
            dynamicTools = frameworkToolBridge.buildPlanningToolSpecs(),
            toolCallHandler = { toolName, arguments ->
                frameworkToolBridge.handleToolCall(
                    toolName = toolName,
                    arguments = arguments,
                    userObjective = userObjective,
                    onSessionStarted = { startedSession ->
                        if (sessionStartResult != null) {
                            throw IOException("Agent runtime attempted to start multiple Genie batches")
                        }
                        sessionStartResult = startedSession
                    },
                )
            },
        )
        return sessionStartResult
            ?: throw IOException("Agent runtime did not launch any Genie sessions")
    }

    private fun buildPlannerPrompt(userObjective: String): String {
        return """
            User objective:
            $userObjective
        """.trimIndent()
    }
}
