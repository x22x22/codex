package com.openai.codex.genie

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DetachedSessionGuardTest {
    @Test
    fun instructionsBanTargetRelaunches() {
        val instructions = DetachedSessionGuard.instructions("com.aurora.store")

        assertTrue(instructions.contains("com.aurora.store"))
        assertTrue(instructions.contains("Do not relaunch"))
        assertTrue(instructions.contains("android_target_ensure_hidden"))
        assertTrue(instructions.contains("android_target_show"))
    }

    @Test
    fun detectsForbiddenAmTargetLaunch() {
        val forbidden = DetachedSessionGuard.isForbiddenTargetLaunchCommand(
            command = "/bin/sh -lc 'am start --user 0 -n com.aurora.store/.MainActivity'",
            targetPackage = "com.aurora.store",
        )

        assertTrue(forbidden)
    }

    @Test
    fun detectsForbiddenCmdActivityTargetLaunch() {
        val forbidden = DetachedSessionGuard.isForbiddenTargetLaunchCommand(
            command = "/bin/sh -lc 'cmd activity start-activity --user 0 -p com.aurora.store'",
            targetPackage = "com.aurora.store",
        )

        assertTrue(forbidden)
    }

    @Test
    fun allowsNonLaunchPackageInspectionCommands() {
        val forbidden = DetachedSessionGuard.isForbiddenTargetLaunchCommand(
            command = "/bin/sh -lc 'cmd package query-activities --user 0 -a android.intent.action.MAIN -c android.intent.category.LAUNCHER com.aurora.store'",
            targetPackage = "com.aurora.store",
        )

        assertFalse(forbidden)
    }
}
