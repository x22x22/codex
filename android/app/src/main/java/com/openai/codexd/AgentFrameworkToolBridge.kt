package com.openai.codexd

import android.content.Context
import java.io.IOException
import org.json.JSONArray
import org.json.JSONObject

class AgentFrameworkToolBridge(
    private val context: Context,
    private val sessionController: AgentSessionController,
) {
    companion object {
        const val START_DIRECT_SESSION_TOOL = "android.framework.sessions.start_direct"
        const val LIST_SESSIONS_TOOL = "android.framework.sessions.list"
        const val ANSWER_QUESTION_TOOL = "android.framework.sessions.answer_question"
        const val ATTACH_TARGET_TOOL = "android.framework.sessions.attach_target"
        const val CANCEL_SESSION_TOOL = "android.framework.sessions.cancel"

        internal fun parseStartDirectSessionArguments(
            arguments: JSONObject,
            userObjective: String,
            isLaunchablePackage: (String) -> Boolean,
        ): StartDirectSessionRequest {
            val targetsJson = arguments.optJSONArray("targets")
                ?: throw IOException("Framework session tool arguments missing targets")
            val targets = buildList {
                for (index in 0 until targetsJson.length()) {
                    val target = targetsJson.optJSONObject(index) ?: continue
                    val packageName = target.optString("packageName").trim()
                    if (packageName.isEmpty() || !isLaunchablePackage(packageName)) {
                        continue
                    }
                    val objective = target.optString("objective").trim().ifEmpty { userObjective }
                    add(
                        AgentDelegationTarget(
                            packageName = packageName,
                            objective = objective,
                        ),
                    )
                }
            }.distinctBy(AgentDelegationTarget::packageName)
            if (targets.isEmpty()) {
                throw IOException("Framework session tool did not select a launchable package")
            }
            return StartDirectSessionRequest(
                plan = AgentDelegationPlan(
                    originalObjective = userObjective,
                    targets = targets,
                    rationale = arguments.optString("reason").trim().ifEmpty { null },
                    usedOverride = false,
                ),
                allowDetachedMode = arguments.optBoolean("allowDetachedMode", true),
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
        return when (toolName) {
            START_DIRECT_SESSION_TOOL -> {
                val request = parseStartDirectSessionArguments(
                    arguments = arguments,
                    userObjective = userObjective,
                    isLaunchablePackage = ::isLaunchablePackage,
                )
                val startedSession = sessionController.startDirectSession(
                    plan = request.plan,
                    allowDetachedMode = request.allowDetachedMode,
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
                                                    .put("objective", stringSchema("Delegated free-form objective for the child Genie.")),
                                            )
                                            .put("required", JSONArray().put("packageName"))
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
                    .put("targetDetached", session.targetDetached),
            )
        }
        return JSONObject()
            .put("available", snapshot.available)
            .put("selectedGeniePackage", snapshot.selectedGeniePackage)
            .put("selectedSessionId", snapshot.selectedSession?.sessionId)
            .put("parentSessionId", snapshot.parentSession?.sessionId)
            .put("sessions", sessions)
    }

    private fun isLaunchablePackage(packageName: String): Boolean {
        return context.packageManager.getLaunchIntentForPackage(packageName) != null
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
