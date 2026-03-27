package com.openai.codex.agent

import android.app.Activity
import android.os.Bundle

class DesktopBridgeBootstrapActivity : Activity() {
    companion object {
        const val EXTRA_AUTH_TOKEN = "com.openai.codex.agent.extra.DESKTOP_BRIDGE_AUTH_TOKEN"
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        intent.getStringExtra(EXTRA_AUTH_TOKEN)
            ?.trim()
            ?.takeIf(String::isNotEmpty)
            ?.let { token ->
                DesktopBridgeServer.ensureStarted(applicationContext, token)
            }
        finish()
    }
}
