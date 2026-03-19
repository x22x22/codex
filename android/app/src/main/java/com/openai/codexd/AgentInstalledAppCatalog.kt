package com.openai.codexd

import android.content.Context
import android.content.Intent

data class InstalledLaunchableApp(
    val packageName: String,
    val label: String,
)

object AgentInstalledAppCatalog {
    fun listLaunchableApps(context: Context): List<InstalledLaunchableApp> {
        val packageManager = context.packageManager
        return packageManager.queryIntentActivities(
            Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER),
            0,
        )
            .map { resolveInfo ->
                val packageName = resolveInfo.activityInfo.packageName
                InstalledLaunchableApp(
                    packageName = packageName,
                    label = resolveInfo.loadLabel(packageManager)?.toString().orEmpty().ifBlank { packageName },
                )
            }
            .distinctBy(InstalledLaunchableApp::packageName)
            .sortedWith(compareBy(InstalledLaunchableApp::label, InstalledLaunchableApp::packageName))
    }
}
