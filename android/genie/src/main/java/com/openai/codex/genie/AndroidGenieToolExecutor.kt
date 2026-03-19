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
    companion object {
        const val SHOW_TARGET_TOOL = "android_target_show"
        const val HIDE_TARGET_TOOL = "android_target_hide"
        const val ATTACH_TARGET_TOOL = "android_target_attach"
        const val CLOSE_TARGET_TOOL = "android_target_close"
        const val CAPTURE_TARGET_FRAME_TOOL = "android_target_capture_frame"
    }

    fun execute(
        toolName: String,
        @Suppress("UNUSED_PARAMETER") arguments: JSONObject,
    ): GenieToolObservation {
        return when (toolName) {
            SHOW_TARGET_TOOL -> requestTargetVisibility(
                action = "show",
                request = callback::requestShowDetachedTarget,
            )
            HIDE_TARGET_TOOL -> requestTargetVisibility(
                action = "hide",
                request = callback::requestHideDetachedTarget,
            )
            ATTACH_TARGET_TOOL -> requestTargetVisibility(
                action = "attach",
                request = callback::requestAttachTarget,
            )
            CLOSE_TARGET_TOOL -> requestTargetVisibility(
                action = "close",
                request = callback::requestCloseDetachedTarget,
            )
            CAPTURE_TARGET_FRAME_TOOL -> captureDetachedTargetFrame()
            else -> throw IOException("Unknown tool: $toolName")
        }
    }

    private fun requestTargetVisibility(
        action: String,
        request: (String) -> Unit,
    ): GenieToolObservation {
        request(sessionId)
        return GenieToolObservation(
            name = "android_target_$action",
            summary = "Requested detached target $action.",
            promptDetails = "Requested framework action android_target_$action for session $sessionId.",
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
            name = CAPTURE_TARGET_FRAME_TOOL,
            summary = "Captured detached target frame ${copy.width}x${copy.height}.",
            promptDetails = "Captured detached target frame ${copy.width}x${copy.height}. Use the attached image to inspect the current UI.",
            imageDataUrls = listOf(
                "data:image/jpeg;base64," + Base64.encodeToString(jpeg, Base64.NO_WRAP),
            ),
        )
    }
}
