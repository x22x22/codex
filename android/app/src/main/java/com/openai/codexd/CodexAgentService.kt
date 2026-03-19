package com.openai.codexd

import android.app.agent.AgentService
import android.app.agent.AgentSessionInfo
import android.util.Log

class CodexAgentService : AgentService() {
    companion object {
        private const val TAG = "CodexAgentService"
    }

    override fun onSessionChanged(session: AgentSessionInfo) {
        Log.i(TAG, "onSessionChanged $session")
    }

    override fun onSessionRemoved(sessionId: String) {
        Log.i(TAG, "onSessionRemoved sessionId=$sessionId")
    }
}
