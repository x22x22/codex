package com.openai.codex.agent

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ResolveInfo

data class InstalledApp(
    val packageName: String,
    val label: String,
)

object InstalledAppCatalog {
    private val excludedPackages = setOf(
        "com.openai.codex.agent",
        "com.openai.codex.genie",
        "com.openai.codexd",
    )

    fun listLaunchableApps(
        context: Context,
        sessionController: AgentSessionController,
    ): List<InstalledApp> {
        val pm = context.packageManager
        val launcherIntent = Intent(Intent.ACTION_MAIN)
            .addCategory(Intent.CATEGORY_LAUNCHER)
        val launchableActivities = pm.queryIntentActivities(launcherIntent, PackageManager.MATCH_ALL)
        val appsByPackage = linkedMapOf<String, InstalledApp>()
        launchableActivities.forEach { resolveInfo ->
            val packageName = resolveInfo.packageNameOrNull() ?: return@forEach
            if (packageName in excludedPackages || !sessionController.canStartSessionForTarget(packageName)) {
                return@forEach
            }
            val label = resolveInfo.loadLabel(pm)?.toString().orEmpty().ifBlank { packageName }
            appsByPackage[packageName] = InstalledApp(
                packageName = packageName,
                label = label,
            )
        }
        return appsByPackage.values.sortedWith(
            compareBy<InstalledApp>({ it.label.lowercase() }).thenBy { it.packageName },
        )
    }

    private fun ResolveInfo.packageNameOrNull(): String? {
        return activityInfo?.applicationInfo?.packageName?.takeIf(String::isNotBlank)
    }
}
