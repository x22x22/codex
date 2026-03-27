package com.openai.codex.bridge

object DesktopSessionBootstrap {
    private const val IDLE_ATTACH_SENTINEL = "__CODEX_DESKTOP_IDLE_ATTACH__"

    fun idleAttachPrompt(): String = IDLE_ATTACH_SENTINEL

    fun isIdleAttachPrompt(prompt: String?): Boolean = prompt?.trim() == IDLE_ATTACH_SENTINEL
}
