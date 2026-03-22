package com.openai.codex.agent

import android.content.Context
import com.openai.codex.bridge.SessionExecutionSettings
import org.json.JSONObject

class SessionExecutionSettingsStore(context: Context) {
    companion object {
        private const val PREFS_NAME = "session_execution_settings"
        private const val KEY_MODEL = "model"
        private const val KEY_REASONING_EFFORT = "reasoningEffort"
    }

    private val prefs = context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    fun saveSettings(
        sessionId: String,
        settings: SessionExecutionSettings,
    ) {
        prefs.edit()
            .putString(key(sessionId, KEY_MODEL), settings.model)
            .putString(key(sessionId, KEY_REASONING_EFFORT), settings.reasoningEffort)
            .apply()
    }

    fun getSettings(sessionId: String): SessionExecutionSettings {
        return SessionExecutionSettings(
            model = prefs.getString(key(sessionId, KEY_MODEL), null),
            reasoningEffort = prefs.getString(key(sessionId, KEY_REASONING_EFFORT), null),
        )
    }

    fun removeSettings(sessionId: String) {
        prefs.edit()
            .remove(key(sessionId, KEY_MODEL))
            .remove(key(sessionId, KEY_REASONING_EFFORT))
            .apply()
    }

    fun pruneSettings(activeSessionIds: Set<String>) {
        val keysToRemove = prefs.all.keys.filter { key ->
            val sessionId = key.substringBefore(':', missingDelimiterValue = "")
            sessionId.isNotBlank() && sessionId !in activeSessionIds
        }
        if (keysToRemove.isEmpty()) {
            return
        }
        prefs.edit().apply {
            keysToRemove.forEach(::remove)
        }.apply()
    }

    fun toJson(sessionId: String): JSONObject {
        val settings = getSettings(sessionId)
        return JSONObject().apply {
            put("model", settings.model)
            put("reasoningEffort", settings.reasoningEffort)
        }
    }

    private fun key(
        sessionId: String,
        suffix: String,
    ): String {
        return "$sessionId:$suffix"
    }
}
