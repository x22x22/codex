package com.openai.codex.genie

import android.app.agent.GenieService
import android.graphics.Bitmap
import android.util.Base64
import java.io.ByteArrayOutputStream
import java.io.IOException
import org.json.JSONObject

class AndroidGenieToolExecutor(
    private val callback: GenieService.Callback,
    private val sessionId: String,
) {
    fun execute(
        toolName: String,
        @Suppress("UNUSED_PARAMETER") arguments: JSONObject,
    ): GenieToolObservation {
        return when (toolName) {
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
            else -> throw IOException("Unknown tool: $toolName")
        }
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
}
