package com.openai.codex.agent

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Build
import java.util.concurrent.atomic.AtomicInteger

class AgentRuntimeForegroundService : Service() {
    companion object {
        private const val CHANNEL_ID = "codex_agent_runtime"
        private const val CHANNEL_NAME = "Codex Agent Runtime"
        private const val NOTIFICATION_ID = 0xC0D3002
        private const val ACTION_START = "com.openai.codex.agent.action.START_RUNTIME_FOREGROUND"
        private const val ACTION_STOP = "com.openai.codex.agent.action.STOP_RUNTIME_FOREGROUND"
        private val activeLeases = AtomicInteger(0)

        fun acquire(context: Context) {
            val previous = activeLeases.getAndIncrement()
            if (previous > 0) {
                return
            }
            val intent = Intent(context, AgentRuntimeForegroundService::class.java).apply {
                action = ACTION_START
            }
            context.startForegroundService(intent)
        }

        fun release(context: Context) {
            while (true) {
                val current = activeLeases.get()
                if (current <= 0) {
                    return
                }
                if (!activeLeases.compareAndSet(current, current - 1)) {
                    continue
                }
                if (current > 1) {
                    return
                }
                val intent = Intent(context, AgentRuntimeForegroundService::class.java).apply {
                    action = ACTION_STOP
                }
                context.startService(intent)
                return
            }
        }

        fun start(context: Context) {
            acquire(context)
        }

        fun stop(context: Context) {
            release(context)
        }

        fun activeLeaseCount(): Int {
            return activeLeases.get()
        }

        fun resetLeases() {
            activeLeases.set(0)
        }

    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                resetLeases()
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
