package com.openai.codex.bridge

import org.json.JSONObject

object DesktopSessionBootstrap {
    private const val IDLE_ATTACH_SENTINEL = "__CODEX_DESKTOP_IDLE_ATTACH__"
    private const val PAYLOAD_KIND_KEY = "kind"
    private const val PAYLOAD_INITIAL_PROMPT_KEY = "initialPrompt"

    fun idleAttachPrompt(initialPrompt: String? = null): String {
        val trimmedInitialPrompt = initialPrompt?.trim().orEmpty()
        if (trimmedInitialPrompt.isEmpty()) {
            return IDLE_ATTACH_SENTINEL
        }
        return JSONObject()
            .put(PAYLOAD_KIND_KEY, IDLE_ATTACH_SENTINEL)
            .put(PAYLOAD_INITIAL_PROMPT_KEY, trimmedInitialPrompt)
            .toString()
    }

    fun isIdleAttachPrompt(prompt: String?): Boolean {
        val trimmedPrompt = prompt?.trim().orEmpty()
        return trimmedPrompt == IDLE_ATTACH_SENTINEL || parseIdleAttachPayload(trimmedPrompt) != null
    }

    fun stagedInitialPrompt(prompt: String?): String? {
        val trimmedPrompt = prompt?.trim().orEmpty()
        return parseIdleAttachPayload(trimmedPrompt)
            ?.optString(PAYLOAD_INITIAL_PROMPT_KEY)
            ?.trim()
            ?.ifEmpty { null }
    }

    private fun parseIdleAttachPayload(prompt: String): JSONObject? {
        if (prompt.isEmpty() || prompt == IDLE_ATTACH_SENTINEL) {
            return null
        }
        return runCatching { JSONObject(prompt) }
            .getOrNull()
            ?.takeIf { payload ->
                payload.optString(PAYLOAD_KIND_KEY) == IDLE_ATTACH_SENTINEL
            }
    }
}
