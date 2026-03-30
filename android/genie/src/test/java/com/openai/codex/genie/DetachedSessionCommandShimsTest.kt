package com.openai.codex.genie

import kotlin.io.path.createTempDirectory
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class DetachedSessionCommandShimsTest {
    @Test
    fun installsDetachedSessionCommandShimsAndPrependsPath() {
        val codexHome = createTempDirectory("detached-shims").toFile()
        val environment = mutableMapOf("PATH" to "/system/bin:/system/xbin")

        DetachedSessionCommandShims.installAndConfigureEnvironment(
            codexHome = codexHome,
            environment = environment,
            targetPackage = "com.example.target",
        )

        val shimDirectory = codexHome.resolve("bin")
        assertEquals("1", environment["CODEX_ANDROID_DETACHED_MODE_ALLOWED"])
        assertEquals("com.example.target", environment["CODEX_ANDROID_DETACHED_TARGET_PACKAGE"])
        assertTrue(environment.getValue("PATH").startsWith("${shimDirectory.absolutePath}:"))
        assertTrue(shimDirectory.resolve("am").canExecute())
        assertTrue(shimDirectory.resolve("cmd").canExecute())
        assertTrue(shimDirectory.resolve("monkey").canExecute())

        codexHome.deleteRecursively()
    }
}
