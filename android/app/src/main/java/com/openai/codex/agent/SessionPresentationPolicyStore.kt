package com.openai.codex.agent

import android.content.Context

class SessionPresentationPolicyStore(
    context: Context,
) {
    companion object {
        private const val PREFS_NAME = "codex_session_presentation_policies"
    }

    private val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    fun savePolicy(
        sessionId: String,
        policy: SessionFinalPresentationPolicy,
    ) {
        prefs.edit().putString(sessionId, policy.wireValue).apply()
    }

    fun getPolicy(sessionId: String): SessionFinalPresentationPolicy? {
        return SessionFinalPresentationPolicy.fromWireValue(
            prefs.getString(sessionId, null),
        )
    }

    fun removePolicy(sessionId: String) {
        prefs.edit().remove(sessionId).apply()
    }

    fun prunePolicies(activeSessionIds: Set<String>) {
        val staleSessionIds = prefs.all.keys - activeSessionIds
        if (staleSessionIds.isEmpty()) {
            return
        }
        prefs.edit().apply {
            staleSessionIds.forEach(::remove)
        }.apply()
    }
}
