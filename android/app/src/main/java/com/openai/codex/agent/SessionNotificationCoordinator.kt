package com.openai.codex.agent

import android.content.Context

object SessionNotificationCoordinator {
    @Suppress("UNUSED_PARAMETER")
    fun acknowledgeSessionTree(
        context: Context,
        sessionController: AgentSessionController,
        topLevelSessionId: String,
        sessionIds: Collection<String>,
    ) {
        sessionController.acknowledgeSessionUi(topLevelSessionId)
    }
}
