package com.openai.codex.genie

import android.app.agent.GenieService
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.util.Base64
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.util.concurrent.TimeUnit
import org.json.JSONObject

class AndroidGenieToolExecutor(
    private val context: Context,
    private val callback: GenieService.Callback,
    private val sessionId: String,
    private val defaultTargetPackage: String,
) {
    companion object {
        private const val INPUT_BIN = "/system/bin/input"
        private const val UIAUTOMATOR_BIN = "/system/bin/uiautomator"
        private const val SHELL_TIMEOUT_MS = 5_000L
        private const val MAX_UI_XML_CHARS = 8_000
    }

    fun execute(
        toolName: String,
        arguments: JSONObject,
    ): GenieToolObservation {
        return when (toolName) {
            "android.package.inspect" -> inspectPackage(arguments)
            "android.intent.launch" -> launchIntent(arguments)
            "android.target.show" -> requestTargetVisibility(
                action = "show",
                request = callback::requestShowDetachedTarget,
            )
            "android.target.hide" -> requestTargetVisibility(
                action = "hide",
                request = callback::requestHideDetachedTarget,
            )
            "android.target.attach" -> requestTargetVisibility(
                action = "attach",
                request = callback::requestAttachTarget,
            )
            "android.target.close" -> requestTargetVisibility(
                action = "close",
                request = callback::requestCloseDetachedTarget,
            )
            "android.target.capture_frame" -> captureDetachedTargetFrame()
            "android.ui.dump" -> dumpUiHierarchy()
            "android.input.tap" -> tap(arguments)
            "android.input.text" -> inputText(arguments)
            "android.input.key" -> inputKey(arguments)
            "android.wait" -> waitFor(arguments)
            else -> throw IOException("Unknown tool: $toolName")
        }
    }

    private fun inspectPackage(arguments: JSONObject): GenieToolObservation {
        val packageName = resolvePackageName(arguments)
        val targetApp = TargetAppInspector.inspect(context, packageName)
        return GenieToolObservation(
            name = "android.package.inspect",
            summary = "Inspected ${targetApp.displayName()} ($packageName).",
            promptDetails = targetApp.renderPromptSection(),
        )
    }

    private fun launchIntent(arguments: JSONObject): GenieToolObservation {
        val packageName = resolvePackageName(arguments)
        val componentName = arguments.optString("component").trim()
        val action = arguments.optString("action").trim()
        val intent = when {
            componentName.isNotEmpty() -> Intent().apply {
                component = ComponentName.unflattenFromString(componentName)
                    ?: throw IOException("Invalid component: $componentName")
            }
            action.isNotEmpty() -> Intent(action).apply {
                `package` = packageName
            }
            else -> context.packageManager.getLaunchIntentForPackage(packageName)
                ?: throw IOException("No launch intent for $packageName")
        }
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        context.startActivity(intent)
        return GenieToolObservation(
            name = "android.intent.launch",
            summary = "Launched target intent for $packageName.",
            promptDetails = buildString {
                appendLine("Launched target app.")
                appendLine("- package: $packageName")
                appendLine("- action: ${if (action.isNotEmpty()) action else intent.action ?: "default"}")
                append("- component: ${intent.component?.flattenToShortString() ?: "default launcher"}")
            },
        )
    }

    private fun requestTargetVisibility(
        action: String,
        request: (String) -> Unit,
    ): GenieToolObservation {
        request(sessionId)
        return GenieToolObservation(
            name = "android.target.$action",
            summary = "Requested detached target $action.",
            promptDetails = "Requested framework action android.target.$action for session $sessionId.",
        )
    }

    private fun captureDetachedTargetFrame(): GenieToolObservation {
        val result = callback.captureDetachedTargetFrame(sessionId)
            ?: throw IOException("captureDetachedTargetFrame returned null")
        val hardwareBuffer = result.hardwareBuffer ?: throw IOException("Detached frame missing hardware buffer")
        val bitmap = Bitmap.wrapHardwareBuffer(hardwareBuffer, result.colorSpace)
            ?: throw IOException("Failed to wrap detached frame")
        val copy = bitmap.copy(Bitmap.Config.ARGB_8888, false)
            ?: throw IOException("Failed to copy detached frame")
        val jpeg = ByteArrayOutputStream().use { output ->
            if (!copy.compress(Bitmap.CompressFormat.JPEG, 85, output)) {
                throw IOException("Failed to encode detached frame")
            }
            output.toByteArray()
        }
        return GenieToolObservation(
            name = "android.target.capture_frame",
            summary = "Captured detached target frame ${copy.width}x${copy.height}.",
            promptDetails = "Captured detached target frame ${copy.width}x${copy.height}. Use the attached image to inspect the current UI.",
            imageDataUrls = listOf(
                "data:image/jpeg;base64," + Base64.encodeToString(jpeg, Base64.NO_WRAP),
            ),
        )
    }

    private fun dumpUiHierarchy(): GenieToolObservation {
        val outputFile = File(context.cacheDir, "genie-ui-$sessionId.xml")
        val commandOutput = runCommand(listOf(UIAUTOMATOR_BIN, "dump", outputFile.absolutePath))
        val xml = outputFile.readText()
        val trimmedXml = if (xml.length > MAX_UI_XML_CHARS) {
            "${xml.take(MAX_UI_XML_CHARS)}\n...[truncated]"
        } else {
            xml
        }
        return GenieToolObservation(
            name = "android.ui.dump",
            summary = "Dumped UI hierarchy (${xml.length} chars).",
            promptDetails = buildString {
                appendLine("uiautomator dump output:")
                appendLine(commandOutput.ifBlank { "(no command output)" })
                appendLine()
                append(trimmedXml)
            },
        )
    }

    private fun tap(arguments: JSONObject): GenieToolObservation {
        val x = arguments.optInt("x", Int.MIN_VALUE)
        val y = arguments.optInt("y", Int.MIN_VALUE)
        if (x == Int.MIN_VALUE || y == Int.MIN_VALUE) {
            throw IOException("android.input.tap requires integer x and y")
        }
        val output = runCommand(listOf(INPUT_BIN, "tap", x.toString(), y.toString()))
        return GenieToolObservation(
            name = "android.input.tap",
            summary = "Sent tap at ($x, $y).",
            promptDetails = "Executed input tap at ($x, $y).\n${output.ifBlank { "Command output: (none)" }}",
        )
    }

    private fun inputText(arguments: JSONObject): GenieToolObservation {
        val text = arguments.optString("text").takeIf(String::isNotBlank)
            ?: throw IOException("android.input.text requires non-empty text")
        val escapedText = text.replace(" ", "%s")
        val output = runCommand(listOf(INPUT_BIN, "text", escapedText))
        return GenieToolObservation(
            name = "android.input.text",
            summary = "Sent text input (${text.length} chars).",
            promptDetails = "Executed input text for ${text.length} characters.\n${output.ifBlank { "Command output: (none)" }}",
        )
    }

    private fun inputKey(arguments: JSONObject): GenieToolObservation {
        val key = arguments.optString("key").takeIf(String::isNotBlank)
            ?: throw IOException("android.input.key requires key")
        val output = runCommand(listOf(INPUT_BIN, "keyevent", key))
        return GenieToolObservation(
            name = "android.input.key",
            summary = "Sent key event $key.",
            promptDetails = "Executed input keyevent $key.\n${output.ifBlank { "Command output: (none)" }}",
        )
    }

    private fun waitFor(arguments: JSONObject): GenieToolObservation {
        val millis = arguments.optLong("millis", -1L)
        if (millis <= 0L || millis > 10_000L) {
            throw IOException("android.wait requires millis in 1..10000")
        }
        Thread.sleep(millis)
        return GenieToolObservation(
            name = "android.wait",
            summary = "Waited ${millis}ms.",
            promptDetails = "Paused execution for ${millis}ms.",
        )
    }

    private fun resolvePackageName(arguments: JSONObject): String {
        return arguments.optString("packageName").takeIf(String::isNotBlank) ?: defaultTargetPackage
    }

    private fun runCommand(command: List<String>): String {
        val process = ProcessBuilder(command)
            .redirectErrorStream(true)
            .start()
        if (!process.waitFor(SHELL_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
            process.destroyForcibly()
            throw IOException("Timed out: ${command.joinToString(" ")}")
        }
        val output = process.inputStream.bufferedReader().use { it.readText() }.trim()
        if (process.exitValue() != 0) {
            val detail = output.ifBlank { "exit ${process.exitValue()}" }
            throw IOException("${command.joinToString(" ")} failed: $detail")
        }
        return output
    }
}
