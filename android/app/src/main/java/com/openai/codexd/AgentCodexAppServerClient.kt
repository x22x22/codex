package com.openai.codexd

import android.content.Context
import android.util.Log
import com.openai.codex.bridge.HostedCodexConfig
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
    private const val DEFAULT_AGENT_MODEL = "gpt-5.3-codex"

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
    private var localProxy: AgentLocalCodexProxy? = null
    private var initialized = false

    fun requestText(
        context: Context,
        instructions: String,
        prompt: String,
        dynamicTools: JSONArray? = null,
        toolCallHandler: ((String, JSONObject) -> JSONObject)? = null,
        requestUserInputHandler: ((JSONArray) -> JSONObject)? = null,
    ): String = synchronized(lifecycleLock) {
        ensureStarted(context.applicationContext)
        activeRequests.incrementAndGet()
        try {
            Log.i(
                TAG,
                "requestText start tools=${dynamicTools?.length() ?: 0} prompt=${prompt.take(160)}",
            )
            notifications.clear()
            val threadId = startThread(
                context = context.applicationContext,
                instructions = instructions,
                dynamicTools = dynamicTools,
            )
            startTurn(threadId, prompt)
            waitForTurnCompletion(toolCallHandler, requestUserInputHandler).also { response ->
                Log.i(TAG, "requestText completed response=${response.take(160)}")
            }
        } finally {
            activeRequests.decrementAndGet()
        }
    }

    fun readRuntimeStatus(
        context: Context,
        refreshToken: Boolean = false,
    ): RuntimeStatus = synchronized(lifecycleLock) {
        ensureStarted(context.applicationContext)
        activeRequests.incrementAndGet()
        try {
            val accountResponse = request(
                method = "account/read",
                params = JSONObject().put("refreshToken", refreshToken),
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
        localProxy = AgentLocalCodexProxy { requestBody ->
            AgentResponsesProxy.sendResponsesRequest(context, requestBody)
        }.also(AgentLocalCodexProxy::start)
        val proxyBaseUrl = localProxy?.baseUrl
            ?: throw IOException("local Agent proxy did not start")
        HostedCodexConfig.write(codexHome, proxyBaseUrl)
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
        localProxy?.close()
        localProxy = null
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
        dynamicTools: JSONArray?,
    ): String {
        val params = JSONObject()
            .put("approvalPolicy", "never")
            .put("sandbox", "read-only")
            .put("ephemeral", true)
            .put("cwd", context.filesDir.absolutePath)
            .put("serviceName", "android_agent")
            .put("baseInstructions", instructions)
        if (dynamicTools != null) {
            params.put("dynamicTools", dynamicTools)
        }
        val result = request(
            method = "thread/start",
            params = params,
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

    private fun waitForTurnCompletion(
        toolCallHandler: ((String, JSONObject) -> JSONObject)?,
        requestUserInputHandler: ((JSONArray) -> JSONObject)?,
    ): String {
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
                handleServerRequest(notification, toolCallHandler, requestUserInputHandler)
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
                "item/started" -> {
                    val item = params.optJSONObject("item")
                    Log.i(
                        TAG,
                        "item/started type=${item?.optString("type")} tool=${item?.optString("tool")}",
                    )
                }
                "item/completed" -> {
                    val item = params.optJSONObject("item") ?: continue
                    Log.i(
                        TAG,
                        "item/completed type=${item.optString("type")} status=${item.optString("status")} tool=${item.optString("tool")}",
                    )
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
                    Log.i(
                        TAG,
                        "turn/completed status=${turn.optString("status")} error=${turn.opt("error")} finalMessage=${finalAgentMessage?.take(160)}",
                    )
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

    private fun handleServerRequest(
        message: JSONObject,
        toolCallHandler: ((String, JSONObject) -> JSONObject)?,
        requestUserInputHandler: ((JSONArray) -> JSONObject)?,
    ) {
        val requestId = message.opt("id") ?: return
        val method = message.optString("method", "unknown")
        val params = message.optJSONObject("params") ?: JSONObject()
        Log.i(TAG, "handleServerRequest method=$method")
        when (method) {
            "item/tool/call" -> {
                if (toolCallHandler == null) {
                    sendError(
                        requestId = requestId,
                        code = -32601,
                        message = "No Agent tool handler registered for $method",
                    )
                    return
                }
                val toolName = params.optString("tool").trim()
                val arguments = params.optJSONObject("arguments") ?: JSONObject()
                Log.i(TAG, "tool/call tool=$toolName arguments=$arguments")
                val result = runCatching { toolCallHandler(toolName, arguments) }
                    .getOrElse { err ->
                        sendError(
                            requestId = requestId,
                            code = -32000,
                            message = err.message ?: "Agent tool call failed",
                        )
                        return
                    }
                Log.i(TAG, "tool/call completed tool=$toolName result=$result")
                sendResult(requestId, result)
            }
            "item/tool/requestUserInput" -> {
                if (requestUserInputHandler == null) {
                    sendError(
                        requestId = requestId,
                        code = -32601,
                        message = "No Agent user-input handler registered for $method",
                    )
                    return
                }
                val questions = params.optJSONArray("questions") ?: JSONArray()
                Log.i(TAG, "requestUserInput questions=$questions")
                val result = runCatching { requestUserInputHandler(questions) }
                    .getOrElse { err ->
                        sendError(
                            requestId = requestId,
                            code = -32000,
                            message = err.message ?: "Agent user input request failed",
                        )
                        return
                    }
                Log.i(TAG, "requestUserInput completed result=$result")
                sendResult(requestId, result)
            }
            else -> {
                sendError(
                    requestId = requestId,
                    code = -32601,
                    message = "Unsupported Agent app-server request: $method",
                )
                return
            }
        }
    }

    private fun sendResult(
        requestId: Any,
        result: JSONObject,
    ) {
        sendMessage(
            JSONObject()
                .put("id", requestId)
                .put("result", result),
        )
    }

    private fun sendError(
        requestId: Any,
        code: Int,
        message: String,
    ) {
        sendMessage(
            JSONObject()
                .put("id", requestId)
                .put(
                    "error",
                    JSONObject()
                        .put("code", code)
                        .put("message", message),
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
        val configuredModel = config.optNullableString("model")
        val effectiveModel = configuredModel ?: DEFAULT_AGENT_MODEL
        val configuredProvider = config.optNullableString("model_provider")
        val accountType = account?.optNullableString("type").orEmpty()
        return RuntimeStatus(
            authenticated = account != null,
            accountEmail = account?.optNullableString("email"),
            clientCount = activeRequests.get(),
            modelProviderId = configuredProvider ?: inferModelProviderId(accountType),
            configuredModel = configuredModel,
            effectiveModel = effectiveModel,
            upstreamBaseUrl = resolveUpstreamBaseUrl(
                config = config,
                accountType = accountType,
                configuredProvider = configuredProvider,
            ),
        )
    }

    private fun inferModelProviderId(accountType: String): String {
        return when (accountType) {
            "chatgpt" -> "chatgpt"
            "apiKey" -> "openai"
            else -> "unknown"
        }
    }

    private fun JSONObject.optNullableString(name: String): String? = when {
        isNull(name) -> null
        else -> optString(name).ifBlank { null }
    }

    private fun resolveUpstreamBaseUrl(
        config: JSONObject,
        accountType: String,
        configuredProvider: String?,
    ): String {
        val modelProviders = config.optJSONObject("model_providers")
        val configuredProviderBaseUrl = configuredProvider?.let { providerId ->
            modelProviders
                ?.optJSONObject(providerId)
                ?.optString("base_url")
                ?.ifBlank { null }
        }
        if (configuredProviderBaseUrl != null) {
            return configuredProviderBaseUrl
        }
        return when (accountType) {
            "chatgpt" -> config.optString("chatgpt_base_url")
                .ifBlank { "https://chatgpt.com/backend-api/codex" }
            "apiKey" -> config.optString("openai_base_url")
                .ifBlank { "https://api.openai.com/v1" }
            else -> config.optString("openai_base_url")
                .ifBlank {
                    config.optString("chatgpt_base_url")
                        .ifBlank { "provider-default" }
                }
        }
    }
}
