package com.openai.codex.bridge

import android.app.agent.AgentSessionInfo
import android.app.agent.GenieService
import android.window.ScreenCapture
import java.lang.reflect.Field
import java.lang.reflect.InvocationTargetException
import java.lang.reflect.Method
import java.lang.reflect.Modifier

object DetachedTargetCompat {
    private const val METHOD_GET_TARGET_RUNTIME = "getTargetRuntime"
    private const val METHOD_ENSURE_DETACHED_TARGET_HIDDEN = "ensureDetachedTargetHidden"
    private const val METHOD_SHOW_DETACHED_TARGET = "showDetachedTarget"
    private const val METHOD_HIDE_DETACHED_TARGET = "hideDetachedTarget"
    private const val METHOD_ATTACH_DETACHED_TARGET = "attachDetachedTarget"
    private const val METHOD_CLOSE_DETACHED_TARGET = "closeDetachedTarget"
    private const val METHOD_CAPTURE_DETACHED_TARGET_FRAME_RESULT = "captureDetachedTargetFrameResult"
    private const val METHOD_GET_STATUS = "getStatus"
    private const val METHOD_GET_DETACHED_DISPLAY_ID = "getDetachedDisplayId"
    private const val METHOD_GET_MESSAGE = "getMessage"

    private const val TARGET_RUNTIME_NONE_LABEL = "TARGET_RUNTIME_NONE"
    private const val TARGET_RUNTIME_ATTACHED_LABEL = "TARGET_RUNTIME_ATTACHED"
    private const val TARGET_RUNTIME_DETACHED_LAUNCHING_LABEL = "TARGET_RUNTIME_DETACHED_LAUNCHING"
    private const val TARGET_RUNTIME_DETACHED_HIDDEN_LABEL = "TARGET_RUNTIME_DETACHED_HIDDEN"
    private const val TARGET_RUNTIME_DETACHED_SHOWN_LABEL = "TARGET_RUNTIME_DETACHED_SHOWN"
    private const val TARGET_RUNTIME_MISSING_LABEL = "TARGET_RUNTIME_MISSING"

    private const val STATUS_OK_LABEL = "STATUS_OK"
    private const val STATUS_NO_DETACHED_DISPLAY_LABEL = "STATUS_NO_DETACHED_DISPLAY"
    private const val STATUS_NO_TARGET_TASK_LABEL = "STATUS_NO_TARGET_TASK"
    private const val STATUS_LAUNCH_FAILED_LABEL = "STATUS_LAUNCH_FAILED"
    private const val STATUS_INTERNAL_ERROR_LABEL = "STATUS_INTERNAL_ERROR"
    private const val STATUS_CAPTURE_FAILED_LABEL = "STATUS_CAPTURE_FAILED"

    data class DetachedTargetState(
        val value: Int?,
        val label: String,
    ) {
        fun isMissing(): Boolean = label == TARGET_RUNTIME_MISSING_LABEL
    }

    data class DetachedTargetControlResult(
        val status: Int?,
        val statusLabel: String,
        val targetRuntime: DetachedTargetState,
        val detachedDisplayId: Int?,
        val message: String?,
    ) {
        fun isOk(): Boolean = statusLabel == STATUS_OK_LABEL

        fun needsRecovery(): Boolean {
            return statusLabel == STATUS_NO_DETACHED_DISPLAY_LABEL ||
                statusLabel == STATUS_NO_TARGET_TASK_LABEL ||
                targetRuntime.isMissing()
        }

        fun summary(action: String): String {
            return buildString {
                append("Detached target ")
                append(action)
                append(" -> ")
                append(statusLabel)
                append(" (runtime=")
                append(targetRuntime.label)
                detachedDisplayId?.let { displayId ->
                    append(", display=")
                    append(displayId)
                }
                append(")")
                message?.takeIf(String::isNotBlank)?.let { detail ->
                    append(": ")
                    append(detail)
                }
            }
        }
    }

    data class DetachedTargetCaptureResult(
        val status: Int?,
        val statusLabel: String,
        val targetRuntime: DetachedTargetState,
        val detachedDisplayId: Int?,
        val message: String?,
        val captureResult: ScreenCapture.ScreenCaptureResult?,
    ) {
        fun isOk(): Boolean = statusLabel == STATUS_OK_LABEL && captureResult != null

        fun needsRecovery(): Boolean {
            return statusLabel == STATUS_NO_DETACHED_DISPLAY_LABEL ||
                statusLabel == STATUS_NO_TARGET_TASK_LABEL ||
                targetRuntime.isMissing()
        }

        fun summary(): String {
            return buildString {
                append("Detached target capture -> ")
                append(statusLabel)
                append(" (runtime=")
                append(targetRuntime.label)
                detachedDisplayId?.let { displayId ->
                    append(", display=")
                    append(displayId)
                }
                append(")")
                message?.takeIf(String::isNotBlank)?.let { detail ->
                    append(": ")
                    append(detail)
                }
            }
        }
    }

    private val targetRuntimeLabels: Map<Int, String> by lazy(LazyThreadSafetyMode.SYNCHRONIZED) {
        staticIntFields(AgentSessionInfo::class.java, "TARGET_RUNTIME_")
    }

    private val getTargetRuntimeMethod: Method? by lazy(LazyThreadSafetyMode.SYNCHRONIZED) {
        findOptionalMethod(AgentSessionInfo::class.java, METHOD_GET_TARGET_RUNTIME)
    }

    fun getTargetRuntime(sessionInfo: AgentSessionInfo): DetachedTargetState {
        val runtimeValue = getTargetRuntimeMethod?.let { method ->
            invokeChecked { method.invoke(sessionInfo) as? Int }
        }
        if (runtimeValue != null) {
            return DetachedTargetState(
                value = runtimeValue,
                label = targetRuntimeLabels[runtimeValue] ?: runtimeValue.toString(),
            )
        }
        return when {
            sessionInfo.targetPresentation == AgentSessionInfo.TARGET_PRESENTATION_DETACHED_HIDDEN -> {
                DetachedTargetState(
                    value = null,
                    label = TARGET_RUNTIME_DETACHED_HIDDEN_LABEL,
                )
            }
            sessionInfo.targetPresentation == AgentSessionInfo.TARGET_PRESENTATION_DETACHED_SHOWN -> {
                DetachedTargetState(
                    value = null,
                    label = TARGET_RUNTIME_DETACHED_SHOWN_LABEL,
                )
            }
            sessionInfo.isTargetDetached -> {
                DetachedTargetState(
                    value = null,
                    label = TARGET_RUNTIME_DETACHED_LAUNCHING_LABEL,
                )
            }
            sessionInfo.targetPackage != null -> {
                DetachedTargetState(
                    value = null,
                    label = TARGET_RUNTIME_ATTACHED_LABEL,
                )
            }
            else -> DetachedTargetState(
                value = null,
                label = TARGET_RUNTIME_NONE_LABEL,
            )
        }
    }

    fun ensureDetachedTargetHidden(
        callback: GenieService.Callback,
        sessionId: String,
    ): DetachedTargetControlResult {
        return invokeControl(
            callback = callback,
            sessionId = sessionId,
            methodName = METHOD_ENSURE_DETACHED_TARGET_HIDDEN,
            legacyFallback = {
                callback.requestLaunchDetachedTargetHidden(sessionId)
                DetachedTargetControlResult(
                    status = null,
                    statusLabel = STATUS_OK_LABEL,
                    targetRuntime = DetachedTargetState(
                        value = null,
                        label = TARGET_RUNTIME_DETACHED_HIDDEN_LABEL,
                    ),
                    detachedDisplayId = null,
                    message = "Used legacy detached launch callback.",
                )
            },
        )
    }

    fun showDetachedTarget(
        callback: GenieService.Callback,
        sessionId: String,
    ): DetachedTargetControlResult {
        return invokeControl(
            callback = callback,
            sessionId = sessionId,
            methodName = METHOD_SHOW_DETACHED_TARGET,
            legacyFallback = {
                callback.requestShowDetachedTarget(sessionId)
                DetachedTargetControlResult(
                    status = null,
                    statusLabel = STATUS_OK_LABEL,
                    targetRuntime = DetachedTargetState(
                        value = null,
                        label = TARGET_RUNTIME_DETACHED_SHOWN_LABEL,
                    ),
                    detachedDisplayId = null,
                    message = "Used legacy detached show callback.",
                )
            },
        )
    }

    fun hideDetachedTarget(
        callback: GenieService.Callback,
        sessionId: String,
    ): DetachedTargetControlResult {
        return invokeControl(
            callback = callback,
            sessionId = sessionId,
            methodName = METHOD_HIDE_DETACHED_TARGET,
            legacyFallback = {
                callback.requestHideDetachedTarget(sessionId)
                DetachedTargetControlResult(
                    status = null,
                    statusLabel = STATUS_OK_LABEL,
                    targetRuntime = DetachedTargetState(
                        value = null,
                        label = TARGET_RUNTIME_DETACHED_HIDDEN_LABEL,
                    ),
                    detachedDisplayId = null,
                    message = "Used legacy detached hide callback.",
                )
            },
        )
    }

    fun attachDetachedTarget(
        callback: GenieService.Callback,
        sessionId: String,
    ): DetachedTargetControlResult {
        return invokeControl(
            callback = callback,
            sessionId = sessionId,
            methodName = METHOD_ATTACH_DETACHED_TARGET,
            legacyFallback = {
                callback.requestAttachTarget(sessionId)
                DetachedTargetControlResult(
                    status = null,
                    statusLabel = STATUS_OK_LABEL,
                    targetRuntime = DetachedTargetState(
                        value = null,
                        label = TARGET_RUNTIME_ATTACHED_LABEL,
                    ),
                    detachedDisplayId = null,
                    message = "Used legacy target attach callback.",
                )
            },
        )
    }

    fun closeDetachedTarget(
        callback: GenieService.Callback,
        sessionId: String,
    ): DetachedTargetControlResult {
        return invokeControl(
            callback = callback,
            sessionId = sessionId,
            methodName = METHOD_CLOSE_DETACHED_TARGET,
            legacyFallback = {
                callback.requestCloseDetachedTarget(sessionId)
                DetachedTargetControlResult(
                    status = null,
                    statusLabel = STATUS_OK_LABEL,
                    targetRuntime = DetachedTargetState(
                        value = null,
                        label = TARGET_RUNTIME_NONE_LABEL,
                    ),
                    detachedDisplayId = null,
                    message = "Used legacy detached close callback.",
                )
            },
        )
    }

    fun captureDetachedTargetFrameResult(
        callback: GenieService.Callback,
        sessionId: String,
    ): DetachedTargetCaptureResult {
        val method = findOptionalMethod(
            callback.javaClass,
            METHOD_CAPTURE_DETACHED_TARGET_FRAME_RESULT,
            String::class.java,
        )
        if (method == null) {
            val captureResult = callback.captureDetachedTargetFrame(sessionId)
            return DetachedTargetCaptureResult(
                status = null,
                statusLabel = if (captureResult != null) STATUS_OK_LABEL else STATUS_CAPTURE_FAILED_LABEL,
                targetRuntime = DetachedTargetState(
                    value = null,
                    label = if (captureResult != null) {
                        TARGET_RUNTIME_DETACHED_HIDDEN_LABEL
                    } else {
                        TARGET_RUNTIME_NONE_LABEL
                    },
                ),
                detachedDisplayId = null,
                message = if (captureResult != null) {
                    "Used legacy detached-frame capture callback."
                } else {
                    "Legacy detached-frame capture returned null."
                },
                captureResult = captureResult,
            )
        }
        val resultObject = invokeChecked {
            method.invoke(callback, sessionId)
        } ?: return DetachedTargetCaptureResult(
            status = null,
            statusLabel = STATUS_CAPTURE_FAILED_LABEL,
            targetRuntime = DetachedTargetState(
                value = null,
                label = TARGET_RUNTIME_NONE_LABEL,
            ),
            detachedDisplayId = null,
            message = "Detached target capture returned null result object.",
            captureResult = null,
        )
        return parseCaptureResult(resultObject)
    }

    private fun invokeControl(
        callback: GenieService.Callback,
        sessionId: String,
        methodName: String,
        legacyFallback: () -> DetachedTargetControlResult,
    ): DetachedTargetControlResult {
        val method = findOptionalMethod(callback.javaClass, methodName, String::class.java)
        if (method == null) {
            return legacyFallback()
        }
        val resultObject = invokeChecked {
            method.invoke(callback, sessionId)
        } ?: return DetachedTargetControlResult(
            status = null,
            statusLabel = STATUS_INTERNAL_ERROR_LABEL,
            targetRuntime = DetachedTargetState(
                value = null,
                label = TARGET_RUNTIME_NONE_LABEL,
            ),
            detachedDisplayId = null,
            message = "$methodName returned null result object.",
        )
        return parseControlResult(resultObject)
    }

    private fun parseControlResult(resultObject: Any): DetachedTargetControlResult {
        val resultClass = resultObject.javaClass
        val status = invokeChecked {
            findRequiredMethod(resultClass, METHOD_GET_STATUS).invoke(resultObject) as? Int
        }
        return DetachedTargetControlResult(
            status = status,
            statusLabel = statusLabel(resultClass, status),
            targetRuntime = parseTargetRuntime(resultObject),
            detachedDisplayId = optionalInt(resultObject, METHOD_GET_DETACHED_DISPLAY_ID),
            message = optionalString(resultObject, METHOD_GET_MESSAGE),
        )
    }

    private fun parseCaptureResult(resultObject: Any): DetachedTargetCaptureResult {
        val resultClass = resultObject.javaClass
        val status = invokeChecked {
            findRequiredMethod(resultClass, METHOD_GET_STATUS).invoke(resultObject) as? Int
        }
        val captureGetter = findOptionalMethod(resultClass, "getCaptureResult")
            ?: findOptionalMethod(resultClass, "getScreenCaptureResult")
        val captureResult = captureGetter?.let { method ->
            invokeChecked { method.invoke(resultObject) as? ScreenCapture.ScreenCaptureResult }
        }
        return DetachedTargetCaptureResult(
            status = status,
            statusLabel = statusLabel(resultClass, status),
            targetRuntime = parseTargetRuntime(resultObject),
            detachedDisplayId = optionalInt(resultObject, METHOD_GET_DETACHED_DISPLAY_ID),
            message = optionalString(resultObject, METHOD_GET_MESSAGE),
            captureResult = captureResult,
        )
    }

    private fun parseTargetRuntime(resultObject: Any): DetachedTargetState {
        val runtime = optionalInt(resultObject, METHOD_GET_TARGET_RUNTIME)
        return if (runtime != null) {
            DetachedTargetState(
                value = runtime,
                label = targetRuntimeLabels[runtime] ?: runtime.toString(),
            )
        } else {
            DetachedTargetState(
                value = null,
                label = TARGET_RUNTIME_NONE_LABEL,
            )
        }
    }

    private fun statusLabel(
        resultClass: Class<*>,
        status: Int?,
    ): String {
        if (status == null) {
            return STATUS_INTERNAL_ERROR_LABEL
        }
        return staticIntFields(resultClass, "STATUS_")[status] ?: status.toString()
    }

    private fun optionalInt(
        target: Any,
        methodName: String,
    ): Int? {
        val method = findOptionalMethod(target.javaClass, methodName) ?: return null
        return invokeChecked { method.invoke(target) as? Int }
    }

    private fun optionalString(
        target: Any,
        methodName: String,
    ): String? {
        val method = findOptionalMethod(target.javaClass, methodName) ?: return null
        return invokeChecked { method.invoke(target) as? String }?.ifBlank { null }
    }

    private fun staticIntFields(
        clazz: Class<*>,
        prefix: String,
    ): Map<Int, String> {
        return clazz.fields
            .filter(::isStaticIntField)
            .filter { field -> field.name.startsWith(prefix) }
            .associate { field ->
                field.getInt(null) to field.name
            }
    }

    private fun isStaticIntField(field: Field): Boolean {
        return Modifier.isStatic(field.modifiers) && field.type == Int::class.javaPrimitiveType
    }

    private fun findRequiredMethod(
        clazz: Class<*>,
        name: String,
        vararg parameterTypes: Class<*>,
    ): Method {
        return clazz.getMethod(name, *parameterTypes)
    }

    private fun findOptionalMethod(
        clazz: Class<*>,
        name: String,
        vararg parameterTypes: Class<*>,
    ): Method? {
        return runCatching {
            clazz.getMethod(name, *parameterTypes)
        }.getOrNull()
    }

    private fun <T> invokeChecked(block: () -> T): T {
        try {
            return block()
        } catch (err: InvocationTargetException) {
            throw err.targetException ?: err
        }
    }
}
