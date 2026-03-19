package com.openai.codex.genie

import org.junit.Assert.assertEquals
import org.junit.Test

class TargetAppContextTest {
    @Test
    fun describeForTraceUsesLabelAndTruncatesPermissions() {
        val context = TargetAppContext(
            packageName = "com.android.deskclock",
            applicationLabel = "Clock",
            versionName = "14",
            versionCode = 42,
            launchIntentAction = "android.intent.action.MAIN",
            launchIntentComponent = "com.android.deskclock/.DeskClock",
            requestedPermissions = listOf(
                "android.permission.POST_NOTIFICATIONS",
                "android.permission.SCHEDULE_EXACT_ALARM",
                "android.permission.SET_ALARM",
                "android.permission.WAKE_LOCK",
            ),
        )

        assertEquals(
            "Clock (com.android.deskclock), version=14 (42), launcher=com.android.deskclock/.DeskClock action=android.intent.action.MAIN, permissions=android.permission.POST_NOTIFICATIONS, android.permission.SCHEDULE_EXACT_ALARM, android.permission.SET_ALARM (+1 more)",
            context.describeForTrace(),
        )
    }

    @Test
    fun renderPromptSectionFallsBackWhenMetadataIsMissing() {
        val context = TargetAppContext(
            packageName = "com.example.target",
            applicationLabel = null,
            versionName = null,
            versionCode = null,
            launchIntentAction = null,
            launchIntentComponent = null,
            requestedPermissions = emptyList(),
        )

        assertEquals(
            """
            Target app inspection:
            - package: com.example.target
            - label: com.example.target
            - versionName: unknown
            - versionCode: unknown
            - launcherAction: unavailable
            - launcherComponent: unavailable
            - requestedPermissions:
            - none declared or visible
            """.trimIndent(),
            context.renderPromptSection(),
        )
    }
}
