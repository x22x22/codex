package com.openai.codex.genie

import android.app.agent.GenieService
import android.graphics.Bitmap
import android.util.Base64
import com.openai.codex.bridge.DetachedTargetCompat
import java.io.ByteArrayOutputStream
import java.io.IOException
import kotlin.math.max
import kotlin.math.roundToInt
import org.json.JSONObject

class AndroidGenieToolExecutor(
    private val callback: GenieService.Callback,
    private val sessionId: String,
) {
    companion object {
        private const val MAX_CAPTURE_LONG_EDGE = 480
        private const val MAX_CAPTURE_JPEG_BYTES = 48 * 1024
        private const val INITIAL_JPEG_QUALITY = 65
        private const val MIN_CAPTURE_JPEG_QUALITY = 38

        const val ENSURE_HIDDEN_TARGET_TOOL = "android_target_ensure_hidden"
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
            ENSURE_HIDDEN_TARGET_TOOL -> requestTargetVisibility(
                action = "ensure hidden",
                request = {
                    DetachedTargetCompat.ensureDetachedTargetHidden(callback, sessionId)
                },
                attemptRecovery = false,
            )
            SHOW_TARGET_TOOL -> requestTargetVisibility(
                action = "show",
                request = {
                    DetachedTargetCompat.showDetachedTarget(callback, sessionId)
                },
            )
            HIDE_TARGET_TOOL -> requestTargetVisibility(
                action = "hide",
                request = {
                    DetachedTargetCompat.hideDetachedTarget(callback, sessionId)
                },
            )
            ATTACH_TARGET_TOOL -> requestTargetVisibility(
                action = "attach",
                request = {
                    DetachedTargetCompat.attachDetachedTarget(callback, sessionId)
                },
            )
            CLOSE_TARGET_TOOL -> requestTargetVisibility(
                action = "close",
                request = {
                    DetachedTargetCompat.closeDetachedTarget(callback, sessionId)
                },
                attemptRecovery = false,
            )
            CAPTURE_TARGET_FRAME_TOOL -> captureDetachedTargetFrame()
            else -> throw IOException("Unknown tool: $toolName")
        }
    }

    private fun requestTargetVisibility(
        action: String,
        request: () -> DetachedTargetCompat.DetachedTargetControlResult,
        attemptRecovery: Boolean = true,
    ): GenieToolObservation {
        val recoveryDetails = mutableListOf<String>()
        var result = request()
        if (attemptRecovery && result.needsRecovery()) {
            val recovery = DetachedTargetCompat.ensureDetachedTargetHidden(callback, sessionId)
            recoveryDetails += recovery.summary("ensure hidden")
            if (recovery.isOk()) {
                result = request()
            } else {
                throw IOException(
                    "${result.summary(action)} Recovery failed: ${recovery.summary("ensure hidden")}",
                )
            }
        }
        if (!result.isOk()) {
            throw IOException(result.summary(action))
        }
        val promptDetails = buildString {
            append(result.summary(action))
            recoveryDetails.forEach { detail ->
                append("\n")
                append(detail)
            }
        }
        return GenieToolObservation(
            name = "android_target_" + action.replace(' ', '_'),
            summary = promptDetails.lineSequence().first(),
            promptDetails = promptDetails,
        )
    }

    private fun captureDetachedTargetFrame(): GenieToolObservation {
        val recoveryDetails = mutableListOf<String>()
        var capture = DetachedTargetCompat.captureDetachedTargetFrameResult(callback, sessionId)
        if (capture.needsRecovery()) {
            val recovery = DetachedTargetCompat.ensureDetachedTargetHidden(callback, sessionId)
            recoveryDetails += recovery.summary("ensure hidden")
            if (recovery.isOk()) {
                capture = DetachedTargetCompat.captureDetachedTargetFrameResult(callback, sessionId)
            } else {
                throw IOException("${capture.summary()} Recovery failed: ${recovery.summary("ensure hidden")}")
            }
        }
        if (!capture.isOk()) {
            throw IOException(capture.summary())
        }
        val result = checkNotNull(capture.captureResult)
        val hardwareBuffer = result.hardwareBuffer ?: throw IOException("Detached frame missing hardware buffer")
        val bitmap = Bitmap.wrapHardwareBuffer(hardwareBuffer, result.colorSpace)
            ?: throw IOException("Failed to wrap detached frame")
        val copy = bitmap.copy(Bitmap.Config.ARGB_8888, false)
            ?: throw IOException("Failed to copy detached frame")
        val (encodedBitmap, jpeg) = encodeDetachedFrame(copy)
        return GenieToolObservation(
            name = CAPTURE_TARGET_FRAME_TOOL,
            summary = "Captured detached target frame ${encodedBitmap.width}x${encodedBitmap.height} (${capture.targetRuntime.label}).",
            promptDetails = buildString {
                append(
                    "Captured detached target frame ${encodedBitmap.width}x${encodedBitmap.height}. Runtime=${capture.targetRuntime.label}. JPEG=${jpeg.size} bytes.",
                )
                recoveryDetails.forEach { detail ->
                    append("\n")
                    append(detail)
                }
                append("\nUse the attached image to inspect the current UI.")
            },
            imageDataUrls = listOf(
                "data:image/jpeg;base64," + Base64.encodeToString(jpeg, Base64.NO_WRAP),
            ),
        )
    }

    private fun encodeDetachedFrame(bitmap: Bitmap): Pair<Bitmap, ByteArray> {
        var encodedBitmap = bitmap.downscaleIfNeeded(MAX_CAPTURE_LONG_EDGE)
        var quality = INITIAL_JPEG_QUALITY
        var jpeg = encodedBitmap.encodeJpeg(quality)
        while (jpeg.size > MAX_CAPTURE_JPEG_BYTES && quality > MIN_CAPTURE_JPEG_QUALITY) {
            quality -= 7
            jpeg = encodedBitmap.encodeJpeg(quality)
        }
        while (jpeg.size > MAX_CAPTURE_JPEG_BYTES) {
            val nextWidth = max((encodedBitmap.width * 0.8f).roundToInt(), 1)
            val nextHeight = max((encodedBitmap.height * 0.8f).roundToInt(), 1)
            if (nextWidth == encodedBitmap.width && nextHeight == encodedBitmap.height) {
                break
            }
            val scaled = Bitmap.createScaledBitmap(encodedBitmap, nextWidth, nextHeight, true)
            if (encodedBitmap !== bitmap) {
                encodedBitmap.recycle()
            }
            encodedBitmap = scaled
            quality = INITIAL_JPEG_QUALITY
            jpeg = encodedBitmap.encodeJpeg(quality)
            while (jpeg.size > MAX_CAPTURE_JPEG_BYTES && quality > MIN_CAPTURE_JPEG_QUALITY) {
                quality -= 7
                jpeg = encodedBitmap.encodeJpeg(quality)
            }
        }
        return encodedBitmap to jpeg
    }

    private fun Bitmap.downscaleIfNeeded(maxLongEdge: Int): Bitmap {
        val longEdge = max(width, height)
        if (longEdge <= maxLongEdge) {
            return this
        }
        val scale = maxLongEdge.toFloat() / longEdge.toFloat()
        val scaledWidth = max((width * scale).roundToInt(), 1)
        val scaledHeight = max((height * scale).roundToInt(), 1)
        return Bitmap.createScaledBitmap(this, scaledWidth, scaledHeight, true)
    }

    private fun Bitmap.encodeJpeg(quality: Int): ByteArray {
        return ByteArrayOutputStream().use { output ->
            if (!compress(Bitmap.CompressFormat.JPEG, quality, output)) {
                throw IOException("Failed to encode detached frame")
            }
            output.toByteArray()
        }
    }
}
