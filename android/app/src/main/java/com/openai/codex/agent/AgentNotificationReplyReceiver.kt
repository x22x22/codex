package com.openai.codex.agent

import android.app.RemoteInput
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import kotlin.concurrent.thread

class AgentNotificationReplyReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != AgentQuestionNotifier.ACTION_REPLY_FROM_NOTIFICATION) {
            return
        }
        val sessionId = intent.getStringExtra(AgentQuestionNotifier.EXTRA_SESSION_ID)?.trim().orEmpty()
        val notificationToken = intent.getStringExtra(
            AgentQuestionNotifier.EXTRA_NOTIFICATION_TOKEN,
        )?.trim().orEmpty()
        val answer = RemoteInput.getResultsFromIntent(intent)
            ?.getCharSequence(AgentQuestionNotifier.REMOTE_INPUT_KEY)
            ?.toString()
            ?.trim()
            .orEmpty()
        if (sessionId.isEmpty() || answer.isEmpty()) {
            return
        }
        val pendingResult = goAsync()
        thread(name = "CodexAgentNotificationReply-$sessionId") {
            try {
                runCatching {
                    AgentSessionController(context).answerQuestionFromNotification(
                        sessionId = sessionId,
                        notificationToken = notificationToken,
                        answer = answer,
                        parentSessionId = null,
                    )
                    AgentQuestionNotifier.cancel(context, sessionId)
                }.onFailure { err ->
                    Log.w(TAG, "Failed to answer notification question for $sessionId", err)
                }
            } finally {
                pendingResult.finish()
            }
        }
    }

    private companion object {
        private const val TAG = "CodexAgentReply"
    }
}
