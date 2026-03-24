package com.openai.codex.genie

internal object DetachedSessionGuard {
    fun instructions(
        targetPackage: String,
    ): String {
        return """
            Detached-session contract for $targetPackage:
            - The framework already launched $targetPackage hidden before your turn started.
            - Do not relaunch $targetPackage with `am start`, `cmd activity start-activity`, `monkey -p`, or similar shell launch surfaces. That bypasses detached hosting and can be blocked by Android background-activity-launch policy.
            - To surface the running target, use `android_target_show`.
            - If the detached target disappears or the framework reports it missing, use `android_target_ensure_hidden` to request framework-owned recovery.
            - To inspect the running detached target, use `android_target_capture_frame` and UI-inspection commands such as `uiautomator dump`.
            - Do not infer missing-target state from a blank launcher badge or a null frame alone. Use framework target controls first; if they still do not expose a usable target, report the framework-state problem instead of guessing.
        """.trimIndent()
    }

    fun isForbiddenTargetLaunchCommand(
        command: String,
        targetPackage: String,
    ): Boolean {
        val normalized = command.trim()
        val launchPatterns = listOf(
            "/bin/sh -lc 'am start",
            "/bin/sh -lc 'am start-activity",
            "/bin/sh -lc 'cmd activity start-activity",
            "/bin/sh -lc 'monkey ",
        )
        if (launchPatterns.none(normalized::startsWith)) {
            return false
        }
        return normalized.contains("-n $targetPackage/")
            || normalized.contains("-p $targetPackage")
            || normalized.contains("--package $targetPackage")
    }

    fun violationMessage(
        targetPackage: String,
        command: String,
    ): String {
        return "Detached session contract violated: attempted to relaunch $targetPackage with shell command `$command`. The framework already launched the target hidden; use android_target_ensure_hidden/android_target_show/android_target_capture_frame plus UI inspection/input instead."
    }
}
