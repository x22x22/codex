package com.openai.codex.genie

import java.io.IOException
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import org.json.JSONObject

class GenieSessionControl {
    companion object {
        private const val BRIDGE_RESPONSE_PREFIX = "__codex_bridge_result__ "
    }

    @Volatile
    var cancelled = false

    @Volatile
    var process: Process? = null

    val userResponses = LinkedBlockingQueue<String>()
    val bridgeResponses = LinkedBlockingQueue<String>()

    fun cancel() {
        cancelled = true
        process?.destroy()
        process = null
    }

    fun waitForUserResponse(): String {
        while (!cancelled) {
            val response = userResponses.poll(100, TimeUnit.MILLISECONDS)
            if (response != null) {
                return response
            }
        }
        throw IOException("Cancelled while waiting for Agent response")
    }

    fun recordResponse(response: String) {
        if (response.startsWith(BRIDGE_RESPONSE_PREFIX)) {
            bridgeResponses.offer(response)
        } else {
            userResponses.offer(response)
        }
    }

    fun waitForBridgeResponse(requestId: String): String {
        while (!cancelled) {
            val response = bridgeResponses.poll(100, TimeUnit.MILLISECONDS)
            if (response == null) {
                continue
            }
            val payload = response.removePrefix(BRIDGE_RESPONSE_PREFIX)
            val responseId = runCatching {
                JSONObject(payload).optString("requestId")
            }.getOrNull()
            if (responseId == requestId) {
                return response
            }
        }
        throw IOException("Cancelled while waiting for Agent bridge response")
    }
}
