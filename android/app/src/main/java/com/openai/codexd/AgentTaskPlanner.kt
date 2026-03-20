package com.openai.codexd

import android.content.Context
import android.util.Log
import java.io.IOException
import org.json.JSONArray
import org.json.JSONObject
import org.json.JSONTokener

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
    private const val TAG = "AgentTaskPlanner"
    private const val PLANNER_ATTEMPTS = 2

    private val PLANNER_INSTRUCTIONS =
        """
        You are Codex acting as the Android Agent orchestrator.
        The user interacts only with the Agent. Decide which installed Android packages should receive delegated Genie sessions.
        Use the standard Android shell tools already available in this runtime, such as `cmd package`, `pm`, and `am`, to inspect installed packages and resolve the correct targets.
        Return exactly one JSON object and nothing else. Do not wrap it in markdown fences.
        JSON schema:
        {
          "targets": [
            {
              "packageName": "installed.package",
              "objective": "free-form delegated objective for the child Genie"
            }
          ],
          "reason": "short rationale",
          "allowDetachedMode": true
        }
        Rules:
        - Choose the fewest packages needed to complete the request.
        - `targets` must be non-empty.
        - Each delegated `objective` should be written for the child Genie, not the user.
        - Stop after at most 6 shell commands.
        - Prefer direct package-manager commands over grepping large package lists.
        - Verify each chosen package by inspecting its package dump or query-activities output before returning it.
        - Only choose packages that directly own the requested app behavior. Never choose helper packages such as `com.android.shell`, `com.android.systemui`, or the Codex Agent/Genie packages unless the user explicitly asked for them.
        - For intent resolution commands, include `--user 0`.
        - If the user objective already names a specific installed package, use it directly after verification.
        - `cmd package list packages PACKAGE_NAME` alone is not sufficient verification.
        - Prefer focused verification commands such as `cmd package dump PACKAGE | sed -n '1,120p'`, `cmd package query-activities --brief --user 0 -p PACKAGE -a android.intent.action.MAIN`, and `cmd package query-activities --brief --user 0 -p PACKAGE -a RELEVANT_ACTION`.
        - Do not enumerate every launcher activity on the device. Query specific candidate packages instead.
        """.trimIndent()
    private val PLANNER_OUTPUT_SCHEMA =
        JSONObject()
            .put("type", "object")
            .put(
                "properties",
                JSONObject()
                    .put(
                        "targets",
                        JSONObject()
                            .put("type", "array")
                            .put("minItems", 1)
                            .put(
                                "items",
                                JSONObject()
                                    .put("type", "object")
                                    .put(
                                        "properties",
                                        JSONObject()
                                            .put("packageName", JSONObject().put("type", "string"))
                                            .put("objective", JSONObject().put("type", "string")),
                                    )
                                    .put("required", JSONArray().put("packageName").put("objective"))
                                    .put("additionalProperties", false),
                            ),
                    )
                    .put("reason", JSONObject().put("type", "string"))
                    .put("allowDetachedMode", JSONObject().put("type", "boolean")),
            )
            .put("required", JSONArray().put("targets").put("reason").put("allowDetachedMode"))
            .put("additionalProperties", false)

    fun startSession(
        context: Context,
        userObjective: String,
        targetPackageOverride: String?,
        allowDetachedMode: Boolean,
        sessionController: AgentSessionController,
        requestUserInputHandler: ((JSONArray) -> JSONObject)? = null,
    ): SessionStartResult {
        if (!targetPackageOverride.isNullOrBlank()) {
            Log.i(TAG, "Using explicit target override $targetPackageOverride")
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
        Log.i(TAG, "Planning Agent session for objective=${userObjective.take(160)}")
        val isEligibleTargetPackage = { packageName: String ->
            runCatching { context.packageManager.getApplicationInfo(packageName, 0) }.isSuccess &&
                packageName !in setOf(
                    "com.android.shell",
                    "com.android.systemui",
                    "com.openai.codexd",
                    "com.openai.codex.genie",
                )
        }
        var previousPlannerResponse: String? = null
        var plannerRequest: AgentFrameworkToolBridge.StartDirectSessionRequest? = null
        var lastPlannerError: IOException? = null
        for (attemptIndex in 0 until PLANNER_ATTEMPTS) {
            val plannerResponse = AgentCodexAppServerClient.requestText(
                context = context,
                instructions = PLANNER_INSTRUCTIONS,
                prompt = buildPlannerPrompt(
                    userObjective = userObjective,
                    previousPlannerResponse = previousPlannerResponse,
                    previousPlannerError = lastPlannerError?.message,
                ),
                outputSchema = PLANNER_OUTPUT_SCHEMA,
                requestUserInputHandler = requestUserInputHandler,
            )
            Log.i(TAG, "Planner response=${plannerResponse.take(400)}")
            previousPlannerResponse = plannerResponse
            val parsedRequest = runCatching {
                parsePlannerResponse(
                    responseText = plannerResponse,
                    userObjective = userObjective,
                    isEligibleTargetPackage = isEligibleTargetPackage,
                )
            }.getOrElse { err ->
                if (err is IOException && attemptIndex < PLANNER_ATTEMPTS - 1) {
                    Log.w(TAG, "Planner response rejected: ${err.message}")
                    lastPlannerError = err
                    continue
                }
                throw err
            }
            plannerRequest = parsedRequest
            break
        }
        val request = plannerRequest ?: throw (lastPlannerError
            ?: IOException("Planner did not return a valid session plan"))
        val sessionStartResult = sessionController.startDirectSession(
            plan = request.plan,
            allowDetachedMode = allowDetachedMode && request.allowDetachedMode,
        )
        Log.i(TAG, "Planner sessionStartResult=$sessionStartResult")
        return sessionStartResult
    }

    private fun buildPlannerPrompt(
        userObjective: String,
        previousPlannerResponse: String?,
        previousPlannerError: String?,
    ): String {
        return buildString {
            appendLine("User objective:")
            appendLine(userObjective)
            if (!previousPlannerError.isNullOrBlank()) {
                appendLine()
                appendLine("Previous candidate plan was rejected by host validation:")
                appendLine(previousPlannerError)
                appendLine("Choose a different installed target package and verify it with focused package commands.")
            }
            if (!previousPlannerResponse.isNullOrBlank()) {
                appendLine()
                appendLine("Previous invalid planner response:")
                appendLine(previousPlannerResponse)
            }
        }.trim()
    }

    internal fun parsePlannerResponse(
        responseText: String,
        userObjective: String,
        isEligibleTargetPackage: (String) -> Boolean,
    ): AgentFrameworkToolBridge.StartDirectSessionRequest {
        val plannerJson = extractPlannerJson(responseText)
        return AgentFrameworkToolBridge.parseStartDirectSessionArguments(
            arguments = plannerJson,
            userObjective = userObjective,
            isEligibleTargetPackage = isEligibleTargetPackage,
        )
    }

    private fun extractPlannerJson(responseText: String): JSONObject {
        val trimmed = responseText.trim()
        parseJsonObject(trimmed)?.let { return it }
        val unfenced = trimmed
            .removePrefix("```json")
            .removePrefix("```")
            .removeSuffix("```")
            .trim()
        parseJsonObject(unfenced)?.let { return it }
        val firstBrace = trimmed.indexOf('{')
        val lastBrace = trimmed.lastIndexOf('}')
        if (firstBrace >= 0 && lastBrace > firstBrace) {
            parseJsonObject(trimmed.substring(firstBrace, lastBrace + 1))?.let { return it }
        }
        throw IOException("Planner did not return a valid JSON object")
    }

    private fun parseJsonObject(text: String): JSONObject? {
        return runCatching {
            val tokener = JSONTokener(text)
            val value = tokener.nextValue()
            value as? JSONObject
        }.getOrNull()
    }
}
