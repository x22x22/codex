package com.openai.codex.agent

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.os.Build
import android.os.IBinder
import android.util.Log

class DesktopAttachKeepAliveService : Service() {
    companion object {
        private const val TAG = "DesktopAttachKeepAlive"
        const val ACTION_START = "com.openai.codex.agent.action.START_DESKTOP_ATTACH_KEEPALIVE"
        const val ACTION_STOP = "com.openai.codex.agent.action.STOP_DESKTOP_ATTACH_KEEPALIVE"

        private const val CHANNEL_ID = "codex_desktop_attach"
        private const val CHANNEL_NAME = "Codex Desktop Attach"
        private const val NOTIFICATION_ID = 0x43445841
    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        if (intent?.action == ACTION_STOP) {
            Log.i(TAG, "Stopping desktop attach keepalive service")
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            return START_NOT_STICKY
        }
        val manager = getSystemService(NotificationManager::class.java)
        if (manager != null) {
            ensureChannel(manager)
            startForeground(NOTIFICATION_ID, buildNotification())
            Log.i(TAG, "Started desktop attach keepalive service")
        }
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun buildNotification(): Notification {
        val contentIntent = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            },
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentTitle("Codex desktop attach active")
            .setContentText("Keeping the Agent bridge alive for attached desktop sessions.")
            .setContentIntent(contentIntent)
            .setOngoing(true)
            .setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
            .build()
    }

    private fun ensureChannel(manager: NotificationManager) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return
        }
        if (manager.getNotificationChannel(CHANNEL_ID) != null) {
            return
        }
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_ID,
                CHANNEL_NAME,
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description = "Keeps the Codex Agent desktop bridge alive while a desktop session is attached."
                setShowBadge(false)
            },
        )
    }
}
