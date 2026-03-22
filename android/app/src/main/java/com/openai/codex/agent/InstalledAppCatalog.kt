package com.openai.codex.agent

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.drawable.Drawable

data class InstalledApp(
    val packageName: String,
    val label: String,
    val icon: Drawable?,
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
        val launcherIntent = Intent(Intent.ACTION_MAIN)
            .addCategory(Intent.CATEGORY_LAUNCHER)
        val appsByPackage = linkedMapOf<String, InstalledApp>()
        pm.queryIntentActivities(launcherIntent, 0).forEach { resolveInfo ->
            val applicationInfo = resolveInfo.activityInfo?.applicationInfo ?: return@forEach
            val packageName = applicationInfo.packageName.takeIf(String::isNotBlank) ?: return@forEach
            if (packageName in excludedPackages) {
                return@forEach
            }
            if (packageName in appsByPackage) {
                return@forEach
            }
            val label = resolveInfo.loadLabel(pm)?.toString().orEmpty().ifBlank { packageName }
            appsByPackage[packageName] = InstalledApp(
                packageName = packageName,
                label = label,
                icon = resolveInfo.loadIcon(pm),
                eligibleTarget = sessionController.canStartSessionForTarget(packageName),
            )
        }
        return appsByPackage.values.sortedWith(
            compareBy<InstalledApp>({ it.label.lowercase() }).thenBy { it.packageName },
        )
    }
}
