package com.openai.codex.bridge

data class SessionExecutionSettings(
    val model: String?,
    val reasoningEffort: String?,
) {
    companion object {
        val default = SessionExecutionSettings(
            model = null,
            reasoningEffort = null,
        )
    }

    fun isDefault(): Boolean {
        return model.isNullOrBlank() && reasoningEffort.isNullOrBlank()
    }
}
