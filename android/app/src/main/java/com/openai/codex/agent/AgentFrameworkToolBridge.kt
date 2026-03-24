package com.openai.codex.agent

import android.content.Context
import android.util.Log
import java.io.IOException
import org.json.JSONArray
import org.json.JSONObject

class AgentFrameworkToolBridge(
    private val context: Context,
    private val sessionController: AgentSessionController,
) {
    companion object {
        private const val TAG = "AgentFrameworkTool"
        private val DISALLOWED_TARGET_PACKAGES = setOf(
            "com.android.shell",
            "com.android.systemui",
            "com.openai.codex.agent",
            "com.openai.codex.genie",
        )
        const val START_DIRECT_SESSION_TOOL = "android_framework_sessions_start_direct"
        const val LIST_SESSIONS_TOOL = "android_framework_sessions_list"
        const val ANSWER_QUESTION_TOOL = "android_framework_sessions_answer_question"
        const val ATTACH_TARGET_TOOL = "android_framework_sessions_attach_target"
        const val CANCEL_SESSION_TOOL = "android_framework_sessions_cancel"

        internal fun parseStartDirectSessionArguments(
            arguments: JSONObject,
            userObjective: String,
            isEligibleTargetPackage: (String) -> Boolean,
        ): StartDirectSessionRequest {
            val targetsJson = arguments.optJSONArray("targets")
                ?: throw IOException("Framework session tool arguments missing targets")
            val rejectedPackages = mutableListOf<String>()
            val targets = buildList {
                for (index in 0 until targetsJson.length()) {
                    val target = targetsJson.optJSONObject(index) ?: continue
                    val packageName = target.optString("packageName").trim()
                    if (packageName.isEmpty()) {
                        continue
                    }
                    if (!isEligibleTargetPackage(packageName)) {
                        rejectedPackages += packageName
                        continue
                    }
                    val objective = target.optString("objective").trim().ifEmpty { userObjective }
                    val finalPresentationPolicy = target.optString("finalPresentationPolicy").trim()
                    val defaultFinalPresentationPolicy = arguments.optString("finalPresentationPolicy").trim()
                    add(
                        AgentDelegationTarget(
                            packageName = packageName,
                            objective = objective,
                            finalPresentationPolicy =
                                SessionFinalPresentationPolicy.fromWireValue(finalPresentationPolicy)
                                    ?: SessionFinalPresentationPolicy.fromWireValue(defaultFinalPresentationPolicy)
                                    ?: SessionFinalPresentationPolicy.AGENT_CHOICE,
                        ),
                    )
                }
            }.distinctBy(AgentDelegationTarget::packageName)
            if (targets.isEmpty()) {
                if (rejectedPackages.isNotEmpty()) {
                    throw IOException(
                        "Framework session tool selected missing or disallowed package(s): ${rejectedPackages.joinToString(", ")}",
                    )
                }
                throw IOException("Framework session tool did not select an eligible target package")
            }
            val allowDetachedMode = arguments.optBoolean("allowDetachedMode", true)
            val detachedPolicyTargets = targets.filter { it.finalPresentationPolicy.requiresDetachedMode() }
            if (!allowDetachedMode && detachedPolicyTargets.isNotEmpty()) {
                throw IOException(
                    "Framework session tool selected detached final presentation without allowDetachedMode: ${detachedPolicyTargets.joinToString(", ") { it.packageName }}",
                )
            }
            return StartDirectSessionRequest(
                plan = AgentDelegationPlan(
                    originalObjective = userObjective,
                    targets = targets,
                    rationale = arguments.optString("reason").trim().ifEmpty { null },
                    usedOverride = false,
                ),
                allowDetachedMode = allowDetachedMode,
            )
        }
    }

    data class StartDirectSessionRequest(
        val plan: AgentDelegationPlan,
        val allowDetachedMode: Boolean,
    )

    fun buildPlanningToolSpecs(): JSONArray {
        return JSONArray().put(buildStartDirectSessionToolSpec())
    }

    fun buildQuestionResolutionToolSpecs(): JSONArray {
        return JSONArray()
            .put(buildListSessionsToolSpec())
            .put(buildAnswerQuestionToolSpec())
    }

    fun buildSessionManagementToolSpecs(): JSONArray {
        return buildQuestionResolutionToolSpecs()
            .put(buildAttachTargetToolSpec())
            .put(buildCancelSessionToolSpec())
    }

    fun handleToolCall(
        toolName: String,
        arguments: JSONObject,
        userObjective: String,
        onSessionStarted: ((SessionStartResult) -> Unit)? = null,
        focusedSessionId: String? = null,
    ): JSONObject {
        Log.i(TAG, "handleToolCall tool=$toolName arguments=$arguments")
        return when (toolName) {
            START_DIRECT_SESSION_TOOL -> {
                val request = parseStartDirectSessionArguments(
                    arguments = arguments,
                    userObjective = userObjective,
                    isEligibleTargetPackage = ::isEligibleTargetPackage,
                )
                val startedSession = sessionController.startDirectSession(
                    plan = request.plan,
                    allowDetachedMode = request.allowDetachedMode,
                )
                Log.i(
                    TAG,
                    "Started framework sessions parent=${startedSession.parentSessionId} children=${startedSession.childSessionIds}",
                )
                onSessionStarted?.invoke(startedSession)
                successText(
                    JSONObject()
                        .put("parentSessionId", startedSession.parentSessionId)
                        .put("childSessionIds", JSONArray(startedSession.childSessionIds))
                        .put("plannedTargets", JSONArray(startedSession.plannedTargets))
                        .put("geniePackage", startedSession.geniePackage)
                        .toString(),
                )
            }
            LIST_SESSIONS_TOOL -> {
                val snapshot = sessionController.loadSnapshot(focusedSessionId)
                successText(renderSessionSnapshot(snapshot).toString())
            }
            ANSWER_QUESTION_TOOL -> {
                val sessionId = requireString(arguments, "sessionId")
                val answer = requireString(arguments, "answer")
                val parentSessionId = arguments.optString("parentSessionId").trim().ifEmpty { null }
                sessionController.answerQuestion(sessionId, answer, parentSessionId)
                successText("Answered framework session $sessionId.")
            }
            ATTACH_TARGET_TOOL -> {
                val sessionId = requireString(arguments, "sessionId")
                sessionController.attachTarget(sessionId)
                successText("Requested target attach for framework session $sessionId.")
            }
            CANCEL_SESSION_TOOL -> {
                val sessionId = requireString(arguments, "sessionId")
                sessionController.cancelSession(sessionId)
                successText("Cancelled framework session $sessionId.")
            }
            else -> throw IOException("Unsupported framework session tool: $toolName")
        }
    }

    private fun buildStartDirectSessionToolSpec(): JSONObject {
        return JSONObject()
            .put("name", START_DIRECT_SESSION_TOOL)
            .put(
                "description",
                "Start direct parent and child framework sessions for one or more target Android packages.",
            )
            .put(
                "inputSchema",
                JSONObject()
                    .put("type", "object")
                    .put(
                        "properties",
                        JSONObject()
                            .put(
                                "targets",
                                JSONObject()
                                    .put("type", "array")
                                    .put(
                                        "items",
                                        JSONObject()
                                            .put("type", "object")
                                            .put(
                                                "properties",
                                                JSONObject()
                                                    .put("packageName", stringSchema("Installed target Android package name."))
                                                    .put("objective", stringSchema("Delegated free-form objective for the child Genie."))
                                                    .put(
                                                        "finalPresentationPolicy",
                                                        stringSchema(
                                                            "Required final target presentation: ATTACHED, DETACHED_HIDDEN, DETACHED_SHOWN, or AGENT_CHOICE.",
                                                        ),
                                                    ),
                                            )
                                            .put(
                                                "required",
                                                JSONArray()
                                                    .put("packageName")
                                                    .put("finalPresentationPolicy"),
                                            )
                                            .put("additionalProperties", false),
                                    ),
                            )
                            .put("reason", stringSchema("Short explanation for why these target packages were selected."))
                            .put(
                                "allowDetachedMode",
                                JSONObject()
                                    .put("type", "boolean")
                                    .put("description", "Whether Genie child sessions may use detached target mode."),
                            ),
                    )
                    .put("required", JSONArray().put("targets"))
                    .put("additionalProperties", false),
            )
    }

    private fun buildListSessionsToolSpec(): JSONObject {
        return JSONObject()
            .put("name", LIST_SESSIONS_TOOL)
            .put("description", "List the current Android framework sessions visible to the Agent.")
            .put(
                "inputSchema",
                JSONObject()
                    .put("type", "object")
                    .put("properties", JSONObject())
                    .put("additionalProperties", false),
            )
    }

    private fun buildAnswerQuestionToolSpec(): JSONObject {
        return JSONObject()
            .put("name", ANSWER_QUESTION_TOOL)
            .put("description", "Answer a waiting Android framework session question.")
            .put(
                "inputSchema",
                JSONObject()
                    .put("type", "object")
                    .put(
                        "properties",
                        JSONObject()
                            .put("sessionId", stringSchema("Framework session id to answer."))
                            .put("answer", stringSchema("Free-form answer text."))
                            .put("parentSessionId", stringSchema("Optional parent framework session id for trace publication.")),
                    )
                    .put("required", JSONArray().put("sessionId").put("answer"))
                    .put("additionalProperties", false),
            )
    }

    private fun buildAttachTargetToolSpec(): JSONObject {
        return JSONObject()
            .put("name", ATTACH_TARGET_TOOL)
            .put("description", "Request the framework to attach the detached target back to the current display.")
            .put(
                "inputSchema",
                JSONObject()
                    .put("type", "object")
                    .put(
                        "properties",
                        JSONObject().put("sessionId", stringSchema("Framework session id whose target should be attached.")),
                    )
                    .put("required", JSONArray().put("sessionId"))
                    .put("additionalProperties", false),
            )
    }

    private fun buildCancelSessionToolSpec(): JSONObject {
        return JSONObject()
            .put("name", CANCEL_SESSION_TOOL)
            .put("description", "Cancel an Android framework session.")
            .put(
                "inputSchema",
                JSONObject()
                    .put("type", "object")
                    .put(
                        "properties",
                        JSONObject().put("sessionId", stringSchema("Framework session id to cancel.")),
                    )
                    .put("required", JSONArray().put("sessionId"))
                    .put("additionalProperties", false),
            )
    }

    private fun renderSessionSnapshot(snapshot: AgentSnapshot): JSONObject {
        val sessions = JSONArray()
        snapshot.sessions.forEach { session ->
            sessions.put(
                JSONObject()
                    .put("sessionId", session.sessionId)
                    .put("parentSessionId", session.parentSessionId)
                    .put("targetPackage", session.targetPackage)
                    .put("state", session.stateLabel)
                    .put("targetDetached", session.targetDetached)
                    .put("targetPresentation", session.targetPresentationLabel)
                    .put("targetRuntime", session.targetRuntimeLabel)
                    .put(
                        "requiredFinalPresentation",
                        session.requiredFinalPresentationPolicy?.wireValue,
                    ),
            )
        }
        return JSONObject()
            .put("available", snapshot.available)
            .put("selectedGeniePackage", snapshot.selectedGeniePackage)
            .put("selectedSessionId", snapshot.selectedSession?.sessionId)
            .put("parentSessionId", snapshot.parentSession?.sessionId)
            .put("sessions", sessions)
    }

    private fun isEligibleTargetPackage(packageName: String): Boolean {
        if (packageName in DISALLOWED_TARGET_PACKAGES) {
            return false
        }
        return sessionController.canStartSessionForTarget(packageName)
    }

    private fun requireString(arguments: JSONObject, fieldName: String): String {
        return arguments.optString(fieldName).trim().ifEmpty {
            throw IOException("Framework session tool requires non-empty $fieldName")
        }
    }

    private fun successText(text: String): JSONObject {
        return JSONObject()
            .put("success", true)
            .put(
                "contentItems",
                JSONArray().put(
                    JSONObject()
                        .put("type", "inputText")
                        .put("text", text),
                ),
            )
    }

    private fun stringSchema(description: String): JSONObject {
        return JSONObject()
            .put("type", "string")
            .put("description", description)
    }
}
