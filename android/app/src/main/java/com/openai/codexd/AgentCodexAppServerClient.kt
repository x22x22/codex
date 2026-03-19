package com.openai.codexd

import android.content.Context
import android.util.Log
import java.io.BufferedWriter
import java.io.File
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.json.JSONArray
import org.json.JSONObject

object AgentCodexAppServerClient {
    private const val TAG = "AgentCodexClient"
    private const val REQUEST_TIMEOUT_MS = 30_000L

    data class RuntimeStatus(
        val authenticated: Boolean,
        val accountEmail: String?,
        val clientCount: Int,
        val modelProviderId: String,
        val configuredModel: String?,
        val effectiveModel: String?,
        val upstreamBaseUrl: String,
    )

    private val lifecycleLock = Any()
    private val requestIdSequence = AtomicInteger(1)
    private val activeRequests = AtomicInteger(0)
    private val pendingResponses = ConcurrentHashMap<String, LinkedBlockingQueue<JSONObject>>()
    private val notifications = LinkedBlockingQueue<JSONObject>()

    private var process: Process? = null
    private var writer: BufferedWriter? = null
    private var stdoutThread: Thread? = null
    private var stderrThread: Thread? = null
    private var initialized = false

    fun requestText(
        context: Context,
        instructions: String,
        prompt: String,
    ): String = synchronized(lifecycleLock) {
        ensureStarted(context.applicationContext)
        activeRequests.incrementAndGet()
        try {
            notifications.clear()
            val threadId = startThread(context.applicationContext, instructions)
            startTurn(threadId, prompt)
            waitForTurnCompletion()
        } finally {
            activeRequests.decrementAndGet()
        }
    }

    fun readRuntimeStatus(context: Context): RuntimeStatus = synchronized(lifecycleLock) {
        ensureStarted(context.applicationContext)
        activeRequests.incrementAndGet()
        try {
            val accountResponse = request(
                method = "account/read",
                params = JSONObject().put("refreshToken", false),
            )
            val configResponse = request(
                method = "config/read",
                params = JSONObject().put("includeLayers", false),
            )
            parseRuntimeStatus(accountResponse, configResponse)
        } finally {
            activeRequests.decrementAndGet()
        }
    }

    private fun ensureStarted(context: Context) {
        if (process?.isAlive == true && writer != null && initialized) {
            return
        }
        closeProcess()
        notifications.clear()
        pendingResponses.clear()
        val codexHome = File(context.filesDir, "codex-home").apply(File::mkdirs)
        val startedProcess = ProcessBuilder(
            listOf(
                CodexCliBinaryLocator.resolve(context).absolutePath,
                "-c",
                "enable_request_compression=false",
                "app-server",
                "--listen",
                "stdio://",
            ),
        ).apply {
            environment()["CODEX_HOME"] = codexHome.absolutePath
            environment()["RUST_LOG"] = "info"
        }.start()
        process = startedProcess
        writer = startedProcess.outputStream.bufferedWriter()
        startStdoutPump(startedProcess)
        startStderrPump(startedProcess)
        initialize()
        initialized = true
    }

    private fun closeProcess() {
        stdoutThread?.interrupt()
        stderrThread?.interrupt()
        runCatching { writer?.close() }
        writer = null
        process?.destroy()
        process = null
        initialized = false
    }

    private fun initialize() {
        request(
            method = "initialize",
            params = JSONObject()
                .put(
                    "clientInfo",
                    JSONObject()
                        .put("name", "android_agent")
                        .put("title", "Android Agent")
                        .put("version", "0.1.0"),
                )
                .put("capabilities", JSONObject().put("experimentalApi", true)),
        )
        notify("initialized", JSONObject())
    }

    private fun startThread(
        context: Context,
        instructions: String,
    ): String {
        val result = request(
            method = "thread/start",
            params = JSONObject()
                .put("approvalPolicy", "never")
                .put("sandbox", "read-only")
                .put("ephemeral", true)
                .put("cwd", context.filesDir.absolutePath)
                .put("serviceName", "android_agent")
                .put("baseInstructions", instructions),
        )
        return result.getJSONObject("thread").getString("id")
    }

    private fun startTurn(
        threadId: String,
        prompt: String,
    ) {
        request(
            method = "turn/start",
            params = JSONObject()
                .put("threadId", threadId)
                .put(
                    "input",
                    JSONArray().put(
                        JSONObject()
                            .put("type", "text")
                            .put("text", prompt),
                    ),
                ),
        )
    }

    private fun waitForTurnCompletion(): String {
        val streamedAgentMessages = mutableMapOf<String, StringBuilder>()
        var finalAgentMessage: String? = null
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(REQUEST_TIMEOUT_MS)
        while (true) {
            val remainingNanos = deadline - System.nanoTime()
            if (remainingNanos <= 0L) {
                throw IOException("Timed out waiting for Agent turn completion")
            }
            val notification = notifications.poll(remainingNanos, TimeUnit.NANOSECONDS)
            if (notification == null) {
                checkProcessAlive()
                continue
            }
            if (notification.has("id") && notification.has("method")) {
                rejectUnsupportedServerRequest(notification)
                continue
            }
            val params = notification.optJSONObject("params") ?: JSONObject()
            when (notification.optString("method")) {
                "item/agentMessage/delta" -> {
                    val itemId = params.optString("itemId")
                    if (itemId.isNotBlank()) {
                        streamedAgentMessages.getOrPut(itemId, ::StringBuilder)
                            .append(params.optString("delta"))
                    }
                }
                "item/completed" -> {
                    val item = params.optJSONObject("item") ?: continue
                    if (item.optString("type") == "agentMessage") {
                        val itemId = item.optString("id")
                        val text = item.optString("text").ifBlank {
                            streamedAgentMessages[itemId]?.toString().orEmpty()
                        }
                        if (text.isNotBlank()) {
                            finalAgentMessage = text
                        }
                    }
                }
                "turn/completed" -> {
                    val turn = params.optJSONObject("turn") ?: JSONObject()
                    return when (turn.optString("status")) {
                        "completed" -> finalAgentMessage?.takeIf(String::isNotBlank)
                            ?: throw IOException("Agent turn completed without an assistant message")
                        "interrupted" -> throw IOException("Agent turn interrupted")
                        else -> throw IOException(
                            turn.opt("error")?.toString()
                                ?: "Agent turn failed with status ${turn.optString("status", "unknown")}",
                        )
                    }
                }
            }
        }
    }

    private fun rejectUnsupportedServerRequest(message: JSONObject) {
        val requestId = message.opt("id") ?: return
        val method = message.optString("method", "unknown")
        sendMessage(
            JSONObject()
                .put("id", requestId)
                .put(
                    "error",
                    JSONObject()
                        .put("code", -32601)
                        .put("message", "Unsupported Agent app-server request: $method"),
                ),
        )
    }

    private fun request(
        method: String,
        params: JSONObject,
    ): JSONObject {
        val requestId = requestIdSequence.getAndIncrement().toString()
        val responseQueue = LinkedBlockingQueue<JSONObject>(1)
        pendingResponses[requestId] = responseQueue
        try {
            sendMessage(
                JSONObject()
                    .put("id", requestId)
                    .put("method", method)
                    .put("params", params),
            )
            val response = responseQueue.poll(REQUEST_TIMEOUT_MS, TimeUnit.MILLISECONDS)
                ?: throw IOException("Timed out waiting for $method response")
            val error = response.optJSONObject("error")
            if (error != null) {
                throw IOException("$method failed: ${error.optString("message", error.toString())}")
            }
            return response.optJSONObject("result") ?: JSONObject()
        } finally {
            pendingResponses.remove(requestId)
        }
    }

    private fun notify(
        method: String,
        params: JSONObject,
    ) {
        sendMessage(
            JSONObject()
                .put("method", method)
                .put("params", params),
        )
    }

    private fun sendMessage(message: JSONObject) {
        val activeWriter = writer ?: throw IOException("Agent app-server writer unavailable")
        activeWriter.write(message.toString())
        activeWriter.newLine()
        activeWriter.flush()
    }

    private fun startStdoutPump(process: Process) {
        stdoutThread = Thread {
            process.inputStream.bufferedReader().useLines { lines ->
                lines.forEach { line ->
                    if (line.isBlank()) {
                        return@forEach
                    }
                    val message = runCatching { JSONObject(line) }
                        .getOrElse { err ->
                            Log.w(TAG, "Failed to parse Agent app-server stdout line", err)
                            return@forEach
                        }
                    routeInbound(message)
                }
            }
        }.also {
            it.name = "AgentCodexStdout"
            it.start()
        }
    }

    private fun startStderrPump(process: Process) {
        stderrThread = Thread {
            process.errorStream.bufferedReader().useLines { lines ->
                lines.forEach { line ->
                    if (line.isNotBlank()) {
                        Log.i(TAG, line)
                    }
                }
            }
        }.also {
            it.name = "AgentCodexStderr"
            it.start()
        }
    }

    private fun routeInbound(message: JSONObject) {
        if (message.has("id") && !message.has("method")) {
            pendingResponses[message.get("id").toString()]?.offer(message)
            return
        }
        notifications.offer(message)
    }

    private fun checkProcessAlive() {
        val activeProcess = process ?: throw IOException("Agent app-server unavailable")
        if (!activeProcess.isAlive) {
            initialized = false
            throw IOException("Agent app-server exited with code ${activeProcess.exitValue()}")
        }
    }

    private fun parseRuntimeStatus(
        accountResponse: JSONObject,
        configResponse: JSONObject,
    ): RuntimeStatus {
        val account = accountResponse.optJSONObject("account")
        val config = configResponse.optJSONObject("config") ?: JSONObject()
        val configuredModel = config.optString("model").ifBlank { null }
        val configuredProvider = config.optString("model_provider").ifBlank { null }
        val accountType = account?.optString("type").orEmpty()
        return RuntimeStatus(
            authenticated = account != null,
            accountEmail = account?.optString("email")?.ifBlank { null },
            clientCount = activeRequests.get(),
            modelProviderId = configuredProvider ?: inferModelProviderId(accountType),
            configuredModel = configuredModel,
            effectiveModel = configuredModel,
            upstreamBaseUrl = config.optString("chatgpt_base_url").ifBlank { "provider-default" },
        )
    }

    private fun inferModelProviderId(accountType: String): String {
        return when (accountType) {
            "chatgpt" -> "chatgpt"
            "apiKey" -> "openai"
            else -> "unknown"
        }
    }
}
