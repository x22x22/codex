package com.openai.codex.agent

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Build

class AgentRuntimeForegroundService : Service() {
    companion object {
        private const val CHANNEL_ID = "codex_agent_runtime"
        private const val CHANNEL_NAME = "Codex Agent Runtime"
        private const val NOTIFICATION_ID = 0xC0D3002
        private const val ACTION_START = "com.openai.codex.agent.action.START_RUNTIME_FOREGROUND"
        private const val ACTION_STOP = "com.openai.codex.agent.action.STOP_RUNTIME_FOREGROUND"

        fun start(context: Context) {
            val intent = Intent(context, AgentRuntimeForegroundService::class.java).apply {
                action = ACTION_START
            }
            context.startForegroundService(intent)
        }

        fun stop(context: Context) {
            val intent = Intent(context, AgentRuntimeForegroundService::class.java).apply {
                action = ACTION_STOP
            }
            context.startService(intent)
        }
    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelfResult(startId)
            }

            else -> {
                val manager = getSystemService(NotificationManager::class.java)
                ensureChannel(manager)
                startForeground(NOTIFICATION_ID, buildNotification())
            }
        }
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?) = null

    private fun buildNotification(): Notification {
        val openAgentIntent = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            },
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_codex)
            .setContentTitle("Codex Agent is working")
            .setContentText("Planning or supervising an active Agent session.")
            .setContentIntent(openAgentIntent)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .build()
    }

    private fun ensureChannel(manager: NotificationManager) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O || manager.getNotificationChannel(CHANNEL_ID) != null) {
            return
        }
        val channel = NotificationChannel(
            CHANNEL_ID,
            CHANNEL_NAME,
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = "Shows when Codex Agent is actively planning or supervising a session."
            setSound(null, null)
            enableVibration(false)
        }
        manager.createNotificationChannel(channel)
    }
}
