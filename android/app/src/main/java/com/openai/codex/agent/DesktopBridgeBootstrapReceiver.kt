package com.openai.codex.agent

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

class DesktopBridgeBootstrapReceiver : BroadcastReceiver() {
    companion object {
        const val ACTION_BOOTSTRAP_DESKTOP_BRIDGE =
            "com.openai.codex.agent.action.BOOTSTRAP_DESKTOP_BRIDGE"
        const val EXTRA_AUTH_TOKEN = "com.openai.codex.agent.extra.DESKTOP_BRIDGE_AUTH_TOKEN"
    }

    override fun onReceive(
        context: Context,
        intent: Intent,
    ) {
        if (intent.action != ACTION_BOOTSTRAP_DESKTOP_BRIDGE) {
            return
        }
        intent.getStringExtra(EXTRA_AUTH_TOKEN)
            ?.trim()
            ?.takeIf(String::isNotEmpty)
            ?.let { token ->
                DesktopBridgeServer.ensureStarted(context.applicationContext, token)
            }
    }
}
