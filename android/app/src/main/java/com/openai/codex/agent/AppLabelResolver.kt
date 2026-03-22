package com.openai.codex.agent

import android.content.Context

object AppLabelResolver {
    fun loadAppLabel(
        context: Context,
        packageName: String?,
    ): String {
        if (packageName.isNullOrBlank()) {
            return "Agent"
        }
        val pm = context.packageManager
        return runCatching {
            val applicationInfo = pm.getApplicationInfo(packageName, 0)
            pm.getApplicationLabel(applicationInfo)?.toString().orEmpty().ifBlank { packageName }
        }.getOrDefault(packageName)
    }
}
