package com.openai.codex.agent

import android.content.Context
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager

data class InstalledApp(
    val packageName: String,
    val label: String,
    val eligibleTarget: Boolean,
)

object InstalledAppCatalog {
    private val excludedPackages = setOf(
        "com.openai.codex.agent",
        "com.openai.codex.genie",
        "com.openai.codexd",
    )

    fun listInstalledApps(
        context: Context,
        sessionController: AgentSessionController,
    ): List<InstalledApp> {
        val pm = context.packageManager
        val installedApplications = pm.getInstalledApplications(PackageManager.MATCH_ALL)
        val appsByPackage = linkedMapOf<String, InstalledApp>()
        installedApplications.forEach { applicationInfo ->
            val packageName = applicationInfo.packageName.takeIf(String::isNotBlank) ?: return@forEach
            if (packageName in excludedPackages) {
                return@forEach
            }
            val label = applicationInfo.loadLabel(pm)?.toString().orEmpty().ifBlank { packageName }
            appsByPackage[packageName] = InstalledApp(
                packageName = packageName,
                label = label,
                eligibleTarget = sessionController.canStartSessionForTarget(packageName),
            )
        }
        return appsByPackage.values.sortedWith(
            compareBy<InstalledApp>({ it.label.lowercase() }).thenBy { it.packageName },
        )
    }
}
