package com.openai.codex.genie

data class TargetAppContext(
    val packageName: String,
    val applicationLabel: String?,
    val versionName: String?,
    val versionCode: Long?,
    val launchIntentAction: String?,
    val launchIntentComponent: String?,
    val requestedPermissions: List<String>,
) {
    fun displayName(): String {
        return applicationLabel?.takeIf(String::isNotBlank) ?: packageName
    }

    fun describeForTrace(): String {
        val versionSummary = when {
            versionName != null && versionCode != null -> "version=$versionName ($versionCode)"
            versionName != null -> "version=$versionName"
            versionCode != null -> "versionCode=$versionCode"
            else -> "version=unknown"
        }
        val launcherSummary = launchIntentComponent?.let { component ->
            val actionSuffix = launchIntentAction?.let { " action=$it" } ?: ""
            "launcher=$component$actionSuffix"
        } ?: "launcher=unavailable"
        val permissionSummary = summarizePermissions(maxVisible = 3)
        return "${displayName()} ($packageName), $versionSummary, $launcherSummary, permissions=$permissionSummary"
    }

    fun renderPromptSection(): String {
        val permissions = requestedPermissions.joinToString(separator = "\n") { "- $it" }
            .ifBlank { "- none declared or visible" }
        return """
            Target app inspection:
            - package: $packageName
            - label: ${displayName()}
            - versionName: ${versionName ?: "unknown"}
            - versionCode: ${versionCode ?: "unknown"}
            - launcherAction: ${launchIntentAction ?: "unavailable"}
            - launcherComponent: ${launchIntentComponent ?: "unavailable"}
            - requestedPermissions:
            $permissions
        """.trimIndent()
    }

    private fun summarizePermissions(maxVisible: Int): String {
        if (requestedPermissions.isEmpty()) {
            return "none"
        }
        val visible = requestedPermissions.take(maxVisible)
        val summary = visible.joinToString()
        val remaining = requestedPermissions.size - visible.size
        return if (remaining > 0) {
            "$summary (+$remaining more)"
        } else {
            summary
        }
    }
}
