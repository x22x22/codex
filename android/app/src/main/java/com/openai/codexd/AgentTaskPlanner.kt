package com.openai.codexd

import android.content.Context
import java.io.IOException
import org.json.JSONArray
import org.json.JSONObject

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
    private const val MAX_LAUNCHABLE_APPS = 80
    private const val LIST_LAUNCHABLE_APPS_TOOL = "android.apps.list_launchable"
    private const val START_GENIE_SESSIONS_TOOL = "android.agent.start_genie_sessions"
    private val PLANNER_INSTRUCTIONS =
        """
        You are Codex acting as the Android Agent orchestrator.
        The user interacts only with the Agent. Decide which installed Android packages should receive delegated Genie sessions.
        Use the available Android app-list tool before selecting targets.
        Choose the fewest packages needed to complete the request and then call the Genie-session launch tool exactly once.
        Rules:
        - Use only package names returned by the Android app-list tool.
        - The launch tool `targets` must be non-empty.
        - Each delegated `objective` should be written for the child Genie, not the user.
        - After the launch tool succeeds, reply with a short summary for the Agent UI.
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
        val launchableApps = AgentInstalledAppCatalog.listLaunchableApps(context)
            .take(MAX_LAUNCHABLE_APPS)
        if (launchableApps.isEmpty()) {
            throw IOException("No launchable apps available for planning")
        }
        var sessionStartResult: SessionStartResult? = null
        AgentCodexAppServerClient.requestText(
            context = context,
            instructions = PLANNER_INSTRUCTIONS,
            prompt = buildPlannerPrompt(userObjective),
            dynamicTools = buildDynamicToolSpecs(),
            toolCallHandler = { toolName, arguments ->
                handleToolCall(
                    toolName = toolName,
                    arguments = arguments,
                    launchableApps = launchableApps,
                    userObjective = userObjective,
                    allowDetachedMode = allowDetachedMode,
                    sessionController = sessionController,
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

    internal fun parsePlanResponse(
        responseText: String,
        userObjective: String,
        allowedPackageNames: Set<String>,
    ): AgentDelegationPlan {
        val responseJson = extractJsonObject(responseText)
        val targetsJson = responseJson.optJSONArray("targets")
            ?: throw IOException("Planner response missing targets")
        val targets = parseTargets(
            targetsJson = targetsJson,
            userObjective = userObjective,
            allowedPackageNames = allowedPackageNames,
        )
        return AgentDelegationPlan(
            originalObjective = userObjective,
            targets = targets,
            rationale = responseJson.optString("reason").ifBlank { null },
            usedOverride = false,
        )
    }

    private fun buildPlannerPrompt(userObjective: String): String {
        return """
            User objective:
            $userObjective
        """.trimIndent()
    }

    private fun buildDynamicToolSpecs(): JSONArray {
        val launchableAppsTool = JSONObject()
            .put("name", LIST_LAUNCHABLE_APPS_TOOL)
            .put(
                "description",
                "List the launchable Android packages currently installed on this device.",
            )
            .put(
                "inputSchema",
                JSONObject()
                    .put("type", "object")
                    .put("properties", JSONObject())
                    .put("additionalProperties", false),
            )
        val startGenieSessionsTool = JSONObject()
            .put("name", START_GENIE_SESSIONS_TOOL)
            .put(
                "description",
                "Start the child Genie sessions needed for the user objective.",
            )
            .put(
                "inputSchema",
                JSONObject()
                    .put("type", "object")
                    .put(
                        "properties",
                        JSONObject()
                            .put(
                                "targets",
                                JSONObject()
                                    .put("type", "array")
                                    .put(
                                        "items",
                                        JSONObject()
                                            .put("type", "object")
                                            .put(
                                                "properties",
                                                JSONObject()
                                                    .put("packageName", stringSchema("Installed Android package name."))
                                                    .put("objective", stringSchema("Delegated free-form objective for the child Genie.")),
                                            )
                                            .put("required", JSONArray().put("packageName"))
                                            .put("additionalProperties", false),
                                    ),
                            )
                            .put("reason", stringSchema("Short explanation for why these targets were selected.")),
                    )
                    .put("required", JSONArray().put("targets"))
                    .put("additionalProperties", false),
            )
        return JSONArray()
            .put(launchableAppsTool)
            .put(startGenieSessionsTool)
    }

    internal fun parseLaunchToolArguments(
        arguments: JSONObject,
        userObjective: String,
        allowedPackageNames: Set<String>,
    ): AgentDelegationPlan {
        val targetsJson = arguments.optJSONArray("targets")
            ?: throw IOException("Launch tool arguments missing targets")
        val targets = parseTargets(
            targetsJson = targetsJson,
            userObjective = userObjective,
            allowedPackageNames = allowedPackageNames,
        )
        return AgentDelegationPlan(
            originalObjective = userObjective,
            targets = targets,
            rationale = arguments.optString("reason").ifBlank { null },
            usedOverride = false,
        )
    }

    private fun handleToolCall(
        toolName: String,
        arguments: JSONObject,
        launchableApps: List<InstalledLaunchableApp>,
        userObjective: String,
        allowDetachedMode: Boolean,
        sessionController: AgentSessionController,
        onSessionStarted: (SessionStartResult) -> Unit,
    ): JSONObject {
        return when (toolName) {
            LIST_LAUNCHABLE_APPS_TOOL -> {
                val appList = launchableApps.joinToString(separator = "\n") { app ->
                    "- ${app.label} (${app.packageName})"
                }
                JSONObject()
                    .put("success", true)
                    .put(
                        "contentItems",
                        JSONArray().put(
                            JSONObject()
                                .put("type", "inputText")
                                .put("text", "Launchable Android apps:\n$appList"),
                        ),
                    )
            }
            START_GENIE_SESSIONS_TOOL -> {
                val allowedPackageNames = launchableApps
                    .mapTo(linkedSetOf(), InstalledLaunchableApp::packageName)
                val plan = parseLaunchToolArguments(
                    arguments = arguments,
                    userObjective = userObjective,
                    allowedPackageNames = allowedPackageNames,
                )
                val startedSession = sessionController.startDirectSession(
                    plan = plan,
                    allowDetachedMode = allowDetachedMode,
                )
                onSessionStarted(startedSession)
                JSONObject()
                    .put("success", true)
                    .put(
                        "contentItems",
                        JSONArray().put(
                            JSONObject()
                                .put("type", "inputText")
                                .put(
                                    "text",
                                    "Started parent session ${startedSession.parentSessionId} for ${startedSession.plannedTargets.joinToString(", ")} using ${startedSession.geniePackage}.",
                                ),
                        ),
                    )
            }
            else -> throw IOException("Unsupported Agent planning tool: $toolName")
        }
    }

    private fun parseTargets(
        targetsJson: JSONArray,
        userObjective: String,
        allowedPackageNames: Set<String>,
    ): List<AgentDelegationTarget> {
        val targets = buildList {
            for (index in 0 until targetsJson.length()) {
                val target = targetsJson.optJSONObject(index) ?: continue
                val packageName = target.optString("packageName").trim()
                if (packageName.isEmpty() || !allowedPackageNames.contains(packageName)) {
                    continue
                }
                val objective = target.optString("objective").trim().ifEmpty { userObjective }
                add(
                    AgentDelegationTarget(
                        packageName = packageName,
                        objective = objective,
                    ),
                )
            }
        }.distinctBy(AgentDelegationTarget::packageName)
        if (targets.isEmpty()) {
            throw IOException("Planner response did not select an installed package")
        }
        return targets
    }

    private fun stringSchema(description: String): JSONObject {
        return JSONObject()
            .put("type", "string")
            .put("description", description)
    }

    private fun extractJsonObject(responseText: String): JSONObject {
        val start = responseText.indexOf('{')
        val end = responseText.lastIndexOf('}')
        if (start == -1 || end == -1 || end <= start) {
            throw IOException("Planner response did not contain JSON")
        }
        return try {
            JSONObject(responseText.substring(start, end + 1))
        } catch (err: Exception) {
            throw IOException("Planner response was not valid JSON: ${err.message}", err)
        }
    }
}
