package com.openai.codexd

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.os.IBinder
import android.util.Log
import java.io.File
import java.io.InterruptedIOException
import java.io.IOException

class CodexdForegroundService : Service() {
    companion object {
        const val ACTION_START = "com.openai.codexd.action.START"
        const val ACTION_STOP = "com.openai.codexd.action.STOP"
        const val ACTION_AUTH_STATE_CHANGED = "com.openai.codexd.action.AUTH_STATE_CHANGED"
        const val EXTRA_SOCKET_PATH = "com.openai.codexd.extra.SOCKET_PATH"
        const val EXTRA_CODEX_HOME = "com.openai.codexd.extra.CODEX_HOME"
        const val EXTRA_UPSTREAM_BASE_URL = "com.openai.codexd.extra.UPSTREAM_BASE_URL"
        const val EXTRA_RUST_LOG = "com.openai.codexd.extra.RUST_LOG"

        private const val CHANNEL_ID = "codexd_service"
        private const val NOTIFICATION_ID = 1
        private const val TAG = "CodexdService"
    }

    private val processLock = Any()
    private var codexdProcess: Process? = null
    private var logThread: Thread? = null
    private var exitThread: Thread? = null
    private var statusThread: Thread? = null

    override fun onBind(intent: Intent?): IBinder? {
        return null
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> startCodexd(intent)
            ACTION_STOP -> stopSelf()
        }
        return START_STICKY
    }

    override fun onDestroy() {
        synchronized(processLock) {
            codexdProcess?.destroy()
            codexdProcess = null
        }
        statusThread?.interrupt()
        notifyAuthStateChanged()
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }

    private fun startCodexd(intent: Intent) {
        synchronized(processLock) {
            if (codexdProcess != null) {
                return
            }

            createNotificationChannel()
            startForeground(NOTIFICATION_ID, buildNotification("Starting codexd"))

            val socketPath = intent.getStringExtra(EXTRA_SOCKET_PATH) ?: defaultSocketPath()
            val codexHome = intent.getStringExtra(EXTRA_CODEX_HOME) ?: defaultCodexHome()
            File(codexHome).mkdirs()

            val codexdBinary = resolveCodexdBinary()
            val args = mutableListOf(
                codexdBinary.absolutePath,
                "--socket-path",
                socketPath,
                "--codex-home",
                codexHome,
            )
            val upstream = intent.getStringExtra(EXTRA_UPSTREAM_BASE_URL)
            if (!upstream.isNullOrBlank()) {
                args.add("--upstream-base-url")
                args.add(upstream)
            }

            val builder = ProcessBuilder(args)
            builder.redirectErrorStream(true)
            val env = builder.environment()
            env["RUST_LOG"] = intent.getStringExtra(EXTRA_RUST_LOG) ?: "info"

            codexdProcess = builder.start()
            startLogThread(codexdProcess!!)
            startExitWatcher(codexdProcess!!)
            startStatusWatcher(socketPath)

            updateNotification("codexd running")
        }
    }

    private fun startLogThread(process: Process) {
        logThread = Thread {
            try {
                process.inputStream.bufferedReader().useLines { lines ->
                    lines.forEach { line -> Log.i(TAG, line) }
                }
            } catch (_: InterruptedIOException) {
                // Expected when the process exits and closes its stdout pipe.
            } catch (err: IOException) {
                if (process.isAlive) {
                    Log.w(TAG, "codexd log stream failed", err)
                }
            }
        }.also { it.start() }
    }

    private fun startExitWatcher(process: Process) {
        exitThread = Thread {
            val exitCode = process.waitFor()
            Log.i(TAG, "codexd exited with code ${exitCode}")
            stopSelf()
        }.also { it.start() }
    }

    private fun startStatusWatcher(socketPath: String) {
        statusThread?.interrupt()
        statusThread = Thread {
            var lastAuthenticated: Boolean? = null
            var lastEmail: String? = null
            var lastClientCount: Int? = null
            while (!Thread.currentThread().isInterrupted) {
                val status = CodexdLocalClient.fetchAuthStatus(socketPath)
                if (status != null) {
                    val message = if (status.authenticated) {
                        val emailSuffix = status.accountEmail?.let { " (${it})" } ?: ""
                        "codexd signed in${emailSuffix}"
                    } else {
                        "codexd needs sign-in"
                    }
                    val messageWithClients = "${message} (clients: ${status.clientCount})"
                    if (lastAuthenticated != status.authenticated
                        || lastEmail != status.accountEmail
                        || lastClientCount != status.clientCount
                    ) {
                        updateNotification(messageWithClients)
                        notifyAuthStateChanged()
                        lastAuthenticated = status.authenticated
                        lastEmail = status.accountEmail
                        lastClientCount = status.clientCount
                    }
                }
                try {
                    Thread.sleep(3000)
                } catch (_: InterruptedException) {
                    return@Thread
                }
            }
        }.also { it.start() }
    }

    private fun buildNotification(status: String): Notification {
        val launchIntent = Intent(this, MainActivity::class.java)
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        val pendingIntent = PendingIntent.getActivity(this, 0, launchIntent, flags)

        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_codex)
            .setContentTitle("codexd")
            .setContentText(status)
            .setContentIntent(pendingIntent)
            .setOngoing(true)
            .build()
    }

    private fun updateNotification(status: String) {
        val manager = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
        manager.notify(NOTIFICATION_ID, buildNotification(status))
    }

    private fun notifyAuthStateChanged() {
        sendBroadcast(Intent(ACTION_AUTH_STATE_CHANGED).setPackage(packageName))
    }

    private fun createNotificationChannel() {
        val manager = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
        if (manager.getNotificationChannel(CHANNEL_ID) != null) {
            return
        }
        val channel = NotificationChannel(
            CHANNEL_ID,
            "codexd service",
            NotificationManager.IMPORTANCE_LOW,
        )
        manager.createNotificationChannel(channel)
    }

    private fun resolveCodexdBinary(): File {
        val nativeDir = applicationInfo.nativeLibraryDir
        val outputFile = File(nativeDir, "libcodexd.so")
        if (!outputFile.exists()) {
            throw IOException("codexd binary missing at ${outputFile.absolutePath}")
        }
        return outputFile
    }

    private fun defaultSocketPath(): String {
        return CodexSocketConfig.DEFAULT_SOCKET_PATH
    }

    private fun defaultCodexHome(): String {
        return File(filesDir, "codex-home").absolutePath
    }
}
