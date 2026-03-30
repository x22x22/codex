package com.openai.codex.agent

import android.content.Context
import android.content.Intent
import android.util.Log
import java.util.concurrent.ConcurrentHashMap

object DesktopAttachKeepAliveManager {
    private const val TAG = "DesktopAttachKeepAlive"
    private val activeConnections = ConcurrentHashMap.newKeySet<String>()

    fun acquire(
        connectionId: String,
    ) {
        if (!activeConnections.add(connectionId)) {
            return
        }
        Log.i(TAG, "Acquired desktop attach keepalive id=$connectionId count=${activeConnections.size}")
    }

    fun release(
        context: Context,
        connectionId: String,
    ) {
        if (!activeConnections.remove(connectionId)) {
            return
        }
        Log.i(TAG, "Released desktop attach keepalive id=$connectionId count=${activeConnections.size}")
        if (activeConnections.isEmpty()) {
            context.startService(
                Intent(context, DesktopAttachKeepAliveService::class.java)
                    .setAction(DesktopAttachKeepAliveService.ACTION_STOP),
            )
        }
    }
}
