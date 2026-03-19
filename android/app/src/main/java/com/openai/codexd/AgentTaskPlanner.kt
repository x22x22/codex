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
    private val PLANNER_INSTRUCTIONS =
        """
        You are Codex acting as the Android Agent planner.
        The user interacts only with the Agent. Decide which installed Android packages should receive delegated Genie sessions.
        Choose the fewest packages needed to complete the request.
        Return exactly one JSON object with this shape:
        {"targets":[{"packageName":"com.example.app","objective":"free-form delegated objective"}],"reason":"short explanation"}
        Rules:
        - Use only package names from the provided installed-app list.
        - `targets` must be non-empty.
        - Each delegated `objective` should be written for the child Genie, not the user.
        - Do not include markdown or code fences.
        """.trimIndent()

    fun plan(
        context: Context,
        userObjective: String,
        targetPackageOverride: String?,
    ): AgentDelegationPlan {
        if (!targetPackageOverride.isNullOrBlank()) {
            return AgentDelegationPlan(
                originalObjective = userObjective,
                targets = listOf(
                    AgentDelegationTarget(
                        packageName = targetPackageOverride,
                        objective = userObjective,
                    ),
                ),
                rationale = "Using explicit target package override.",
                usedOverride = true,
            )
        }
        val launchableApps = AgentInstalledAppCatalog.listLaunchableApps(context)
            .take(MAX_LAUNCHABLE_APPS)
        if (launchableApps.isEmpty()) {
            throw IOException("No launchable apps available for planning")
        }
        val planText = AgentCodexAppServerClient.requestText(
            context = context,
            instructions = PLANNER_INSTRUCTIONS,
            prompt = buildPlannerPrompt(userObjective, launchableApps),
        )
        return parsePlanResponse(
            responseText = planText,
            userObjective = userObjective,
            allowedPackageNames = launchableApps.mapTo(linkedSetOf(), InstalledLaunchableApp::packageName),
        )
    }

    internal fun parsePlanResponse(
        responseText: String,
        userObjective: String,
        allowedPackageNames: Set<String>,
    ): AgentDelegationPlan {
        val responseJson = extractJsonObject(responseText)
        val targetsJson = responseJson.optJSONArray("targets")
            ?: throw IOException("Planner response missing targets")
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
        return AgentDelegationPlan(
            originalObjective = userObjective,
            targets = targets,
            rationale = responseJson.optString("reason").ifBlank { null },
            usedOverride = false,
        )
    }

    private fun buildPlannerPrompt(
        userObjective: String,
        launchableApps: List<InstalledLaunchableApp>,
    ): String {
        val appList = launchableApps.joinToString(separator = "\n") { app ->
            "- ${app.label} (${app.packageName})"
        }
        return """
            User objective:
            $userObjective

            Installed launchable apps:
            $appList
        """.trimIndent()
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
