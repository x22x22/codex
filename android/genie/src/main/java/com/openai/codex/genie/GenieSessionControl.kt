package com.openai.codex.genie

import java.io.IOException
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

class GenieSessionControl {
    @Volatile
    var cancelled = false

    @Volatile
    var process: Process? = null

    val userResponses = LinkedBlockingQueue<String>()

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
        userResponses.offer(response)
    }
}
