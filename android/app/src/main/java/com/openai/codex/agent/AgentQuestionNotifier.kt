package com.openai.codex.agent

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build

object AgentQuestionNotifier {
    private const val CHANNEL_ID = "codex_agent_questions"
    private const val CHANNEL_NAME = "Codex Agent Questions"

    fun showQuestion(
        context: Context,
        sessionId: String,
        targetPackage: String?,
        question: String,
    ) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        ensureChannel(manager)
        manager.notify(notificationId(sessionId), buildNotification(context, sessionId, targetPackage, question))
    }

    fun cancel(context: Context, sessionId: String) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        manager.cancel(notificationId(sessionId))
    }

    private fun buildNotification(
        context: Context,
        sessionId: String,
        targetPackage: String?,
        question: String,
    ): Notification {
        val title = targetPackage?.let { "Question for $it" } ?: "Question for Codex Agent"
        val contentIntent = PendingIntent.getActivity(
            context,
            notificationId(sessionId),
            Intent(context, SessionDetailActivity::class.java).apply {
                putExtra(SessionDetailActivity.EXTRA_SESSION_ID, sessionId)
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            },
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(context, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentTitle(title)
            .setContentText(question)
            .setStyle(Notification.BigTextStyle().bigText(question))
            .setContentIntent(contentIntent)
            .setAutoCancel(false)
            .setOngoing(true)
            .build()
    }

    private fun ensureChannel(manager: NotificationManager) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return
        }
        if (manager.getNotificationChannel(CHANNEL_ID) != null) {
            return
        }
        val channel = NotificationChannel(
            CHANNEL_ID,
            CHANNEL_NAME,
            NotificationManager.IMPORTANCE_HIGH,
        ).apply {
            description = "Questions that need user input for Codex Agent sessions"
            setShowBadge(true)
        }
        manager.createNotificationChannel(channel)
    }

    private fun notificationId(sessionId: String): Int {
        return sessionId.hashCode()
    }
}
