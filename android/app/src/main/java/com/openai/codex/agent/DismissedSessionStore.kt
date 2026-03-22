package com.openai.codex.agent

import android.content.Context

class DismissedSessionStore(context: Context) {
    companion object {
        private const val PREFS_NAME = "dismissed_sessions"
    }

    private val prefs = context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    fun dismiss(sessionId: String) {
        prefs.edit().putBoolean(sessionId, true).apply()
    }

    fun isDismissed(sessionId: String): Boolean {
        return prefs.getBoolean(sessionId, false)
    }

    fun clearDismissed(sessionId: String) {
        prefs.edit().remove(sessionId).apply()
    }

    fun prune(activeSessionIds: Set<String>) {
        val keysToRemove = prefs.all.keys.filter { it !in activeSessionIds }
        if (keysToRemove.isEmpty()) {
            return
        }
        prefs.edit().apply {
            keysToRemove.forEach(::remove)
        }.apply()
    }
}
