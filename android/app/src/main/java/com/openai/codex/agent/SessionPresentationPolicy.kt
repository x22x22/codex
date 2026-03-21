package com.openai.codex.agent

import android.app.agent.AgentSessionInfo
import java.io.IOException

enum class SessionFinalPresentationPolicy(
    val wireValue: String,
    val description: String,
) {
    ATTACHED(
        wireValue = "ATTACHED",
        description = "Finish with the target attached to the main user-facing display/task stack.",
    ),
    DETACHED_HIDDEN(
        wireValue = "DETACHED_HIDDEN",
        description = "Finish with the target still detached and hidden from view.",
    ),
    DETACHED_SHOWN(
        wireValue = "DETACHED_SHOWN",
        description = "Finish with the target detached but visibly shown through the detached host.",
    ),
    AGENT_CHOICE(
        wireValue = "AGENT_CHOICE",
        description = "The Agent does not require a specific final presentation state for this target.",
    ),
    ;

    fun matches(actualPresentation: Int): Boolean {
        return when (this) {
            ATTACHED -> actualPresentation == AgentSessionInfo.TARGET_PRESENTATION_ATTACHED
            DETACHED_HIDDEN -> {
                actualPresentation == AgentSessionInfo.TARGET_PRESENTATION_DETACHED_HIDDEN
            }
            DETACHED_SHOWN -> {
                actualPresentation == AgentSessionInfo.TARGET_PRESENTATION_DETACHED_SHOWN
            }
            AGENT_CHOICE -> true
        }
    }

    fun requiresDetachedMode(): Boolean {
        return when (this) {
            DETACHED_HIDDEN, DETACHED_SHOWN -> true
            ATTACHED, AGENT_CHOICE -> false
        }
    }

    fun promptGuidance(): String {
        return when (this) {
            ATTACHED -> {
                "Before reporting success, ensure the target is ATTACHED to the primary user-facing display. Detached-only visibility is not sufficient."
            }
            DETACHED_HIDDEN -> {
                "Before reporting success, ensure the target remains DETACHED_HIDDEN. Do not attach it or leave it shown."
            }
            DETACHED_SHOWN -> {
                "Before reporting success, ensure the target remains DETACHED_SHOWN. It should stay detached but visibly shown through the detached host."
            }
            AGENT_CHOICE -> {
                "Choose the final target presentation state yourself and describe the final state accurately in your result."
            }
        }
    }

    companion object {
        fun fromWireValue(value: String?): SessionFinalPresentationPolicy? {
            val normalized = value?.trim().orEmpty()
            if (normalized.isEmpty()) {
                return null
            }
            return entries.firstOrNull { it.wireValue.equals(normalized, ignoreCase = true) }
        }

        fun requireFromWireValue(
            value: String?,
            fieldName: String,
        ): SessionFinalPresentationPolicy {
            return fromWireValue(value)
                ?: throw IOException("Unsupported $fieldName: ${value?.trim().orEmpty()}")
        }
    }
}

object AgentTargetPresentationValues {
    const val ATTACHED = AgentSessionInfo.TARGET_PRESENTATION_ATTACHED
    const val DETACHED_HIDDEN = AgentSessionInfo.TARGET_PRESENTATION_DETACHED_HIDDEN
    const val DETACHED_SHOWN = AgentSessionInfo.TARGET_PRESENTATION_DETACHED_SHOWN
}

fun targetPresentationToString(targetPresentation: Int): String {
    return when (targetPresentation) {
        AgentTargetPresentationValues.ATTACHED -> "ATTACHED"
        AgentTargetPresentationValues.DETACHED_HIDDEN -> "DETACHED_HIDDEN"
        AgentTargetPresentationValues.DETACHED_SHOWN -> "DETACHED_SHOWN"
        else -> targetPresentation.toString()
    }
}
