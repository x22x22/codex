package com.openai.codex.agent

import android.content.Context

object SessionNotificationCoordinator {
    fun acknowledgeSessionTree(
        context: Context,
        sessionController: AgentSessionController,
        topLevelSessionId: String,
        sessionIds: Collection<String>,
    ) {
        sessionIds.forEach { sessionId ->
            AgentQuestionNotifier.cancel(context, sessionId)
        }
        sessionController.acknowledgeSessionUi(topLevelSessionId)
    }
}
