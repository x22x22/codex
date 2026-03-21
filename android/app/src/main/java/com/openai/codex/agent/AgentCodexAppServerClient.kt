package com.openai.codex.agent

import android.content.Context
import android.util.Log
import com.openai.codex.bridge.HostedCodexConfig
import java.io.BufferedWriter
import java.io.File
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArraySet
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlin.concurrent.thread
import org.json.JSONArray
import org.json.JSONObject

object AgentCodexAppServerClient {
    private const val TAG = "AgentCodexClient"
    private const val REQUEST_TIMEOUT_MS = 30_000L
    private const val DEFAULT_AGENT_MODEL = "gpt-5.3-codex"
    private const val AGENT_APP_SERVER_RUST_LOG = "warn"

    data class RuntimeStatus(
        val authenticated: Boolean,
        val accountEmail: String?,
        val clientCount: Int,
        val modelProviderId: String,
        val configuredModel: String?,
        val effectiveModel: String?,
        val upstreamBaseUrl: String,
    )

    data class ChatGptLoginSession(
        val loginId: String,
        val authUrl: String,
    )

    fun interface RuntimeStatusListener {
        fun onRuntimeStatusChanged(status: RuntimeStatus?)
    }

    private val lifecycleLock = Any()
    private val requestIdSequence = AtomicInteger(1)
    private val activeRequests = AtomicInteger(0)
    private val pendingResponses = ConcurrentHashMap<String, LinkedBlockingQueue<JSONObject>>()
    private val notifications = LinkedBlockingQueue<JSONObject>()
    private val runtimeStatusListeners = CopyOnWriteArraySet<RuntimeStatusListener>()

    private var process: Process? = null
    private var writer: BufferedWriter? = null
    private var stdoutThread: Thread? = null
    private var stderrThread: Thread? = null
    private var localProxy: AgentLocalCodexProxy? = null
    private var initialized = false
    @Volatile
    private var cachedRuntimeStatus: RuntimeStatus? = null
    @Volatile
    private var applicationContext: Context? = null
    private val runtimeStatusRefreshInFlight = AtomicBoolean(false)

    fun currentRuntimeStatus(): RuntimeStatus? = cachedRuntimeStatus

    fun registerRuntimeStatusListener(listener: RuntimeStatusListener) {
        runtimeStatusListeners += listener
        listener.onRuntimeStatusChanged(cachedRuntimeStatus)
    }

    fun unregisterRuntimeStatusListener(listener: RuntimeStatusListener) {
        runtimeStatusListeners -= listener
    }

    fun refreshRuntimeStatusAsync(
        context: Context,
        refreshToken: Boolean = false,
    ) {
        if (!runtimeStatusRefreshInFlight.compareAndSet(false, true)) {
            return
        }
        thread(name = "AgentRuntimeStatusRefresh") {
            try {
                runCatching {
                    readRuntimeStatus(context, refreshToken)
                }.onFailure {
                    updateCachedRuntimeStatus(null)
                }
            } finally {
                runtimeStatusRefreshInFlight.set(false)
            }
        }
    }

    fun requestText(
        context: Context,
        instructions: String,
        prompt: String,
        outputSchema: JSONObject? = null,
        dynamicTools: JSONArray? = null,
        toolCallHandler: ((String, JSONObject) -> JSONObject)? = null,
        requestUserInputHandler: ((JSONArray) -> JSONObject)? = null,
    ): String = synchronized(lifecycleLock) {
        ensureStarted(context.applicationContext)
        activeRequests.incrementAndGet()
        updateClientCount()
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
            startTurn(
                threadId = threadId,
                prompt = prompt,
                outputSchema = outputSchema,
            )
            waitForTurnCompletion(toolCallHandler, requestUserInputHandler).also { response ->
                Log.i(TAG, "requestText completed response=${response.take(160)}")
            }
        } finally {
            activeRequests.decrementAndGet()
            updateClientCount()
        }
    }

    fun readRuntimeStatus(
        context: Context,
        refreshToken: Boolean = false,
    ): RuntimeStatus = synchronized(lifecycleLock) {
        ensureStarted(context.applicationContext)
        activeRequests.incrementAndGet()
        updateClientCount()
        try {
            val accountResponse = request(
                method = "account/read",
                params = JSONObject().put("refreshToken", refreshToken),
            )
            val configResponse = request(
                method = "config/read",
                params = JSONObject().put("includeLayers", false),
            )
            parseRuntimeStatus(accountResponse, configResponse).also(::updateCachedRuntimeStatus)
        } finally {
            activeRequests.decrementAndGet()
            updateClientCount()
        }
    }

    fun startChatGptLogin(context: Context): ChatGptLoginSession = synchronized(lifecycleLock) {
        ensureStarted(context.applicationContext)
        val response = request(
            method = "account/login/start",
            params = JSONObject().put("type", "chatgpt"),
        )
        if (response.optString("type") != "chatgpt") {
            throw IOException("Unexpected login response type: ${response.optString("type")}")
        }
        return ChatGptLoginSession(
            loginId = response.optString("loginId"),
            authUrl = response.optString("authUrl"),
        )
    }

    fun logoutAccount(context: Context) = synchronized(lifecycleLock) {
        ensureStarted(context.applicationContext)
        request(
            method = "account/logout",
            params = null,
        )
        refreshRuntimeStatusAsync(context.applicationContext)
    }

    private fun ensureStarted(context: Context) {
        if (process?.isAlive == true && writer != null && initialized) {
            return
        }
        closeProcess()
        applicationContext = context
        notifications.clear()
        pendingResponses.clear()
        val codexHome = File(context.filesDir, "codex-home").apply(File::mkdirs)
        localProxy = AgentLocalCodexProxy { requestBody ->
            AgentResponsesProxy.sendResponsesRequest(context, requestBody)
        }.also(AgentLocalCodexProxy::start)
        val proxyBaseUrl = localProxy?.baseUrl
            ?: throw IOException("local Agent proxy did not start")
        HostedCodexConfig.write(context, codexHome, proxyBaseUrl)
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
            environment()["RUST_LOG"] = AGENT_APP_SERVER_RUST_LOG
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
        updateCachedRuntimeStatus(null)
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
        outputSchema: JSONObject?,
    ) {
        val turnParams = JSONObject()
            .put("threadId", threadId)
            .put(
                "input",
                JSONArray().put(
                    JSONObject()
                        .put("type", "text")
                        .put("text", prompt),
                ),
            )
        if (outputSchema != null) {
            turnParams.put("outputSchema", outputSchema)
        }
        request(
            method = "turn/start",
            params = turnParams,
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
                "item/commandExecution/outputDelta" -> {
                    val itemId = params.optString("itemId")
                    val delta = params.optString("delta")
                    if (delta.isNotBlank()) {
                        Log.i(
                            TAG,
                            "commandExecution/outputDelta itemId=$itemId delta=${delta.take(400)}",
                        )
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
                    if (item.optString("type") == "commandExecution") {
                        Log.i(TAG, "commandExecution/completed item=$item")
                    }
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
        params: JSONObject?,
    ): JSONObject {
        val requestId = requestIdSequence.getAndIncrement().toString()
        val responseQueue = LinkedBlockingQueue<JSONObject>(1)
        pendingResponses[requestId] = responseQueue
        try {
            val message = JSONObject()
                .put("id", requestId)
                .put("method", method)
            if (params != null) {
                message.put("params", params)
            }
            sendMessage(message)
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
                    logAgentStderrLine(line)
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
        handleInboundSideEffects(message)
        notifications.offer(message)
    }

    private fun handleInboundSideEffects(message: JSONObject) {
        when (message.optString("method")) {
            "account/updated" -> {
                applicationContext?.let { context ->
                    refreshRuntimeStatusAsync(context)
                }
            }
            "account/login/completed" -> {
                applicationContext?.let { context ->
                    refreshRuntimeStatusAsync(context, refreshToken = true)
                }
            }
        }
    }

    private fun checkProcessAlive() {
        val activeProcess = process ?: throw IOException("Agent app-server unavailable")
        if (!activeProcess.isAlive) {
            initialized = false
            updateCachedRuntimeStatus(null)
            throw IOException("Agent app-server exited with code ${activeProcess.exitValue()}")
        }
    }

    private fun logAgentStderrLine(line: String) {
        if (line.isBlank()) {
            return
        }
        when {
            line.contains(" ERROR ") || line.startsWith("ERROR") -> Log.e(TAG, line)
            line.contains(" WARN ") || line.startsWith("WARN") -> Log.w(TAG, line)
        }
    }

    private fun updateClientCount() {
        val currentStatus = cachedRuntimeStatus ?: return
        val updatedStatus = currentStatus.copy(clientCount = activeRequests.get())
        updateCachedRuntimeStatus(updatedStatus)
    }

    private fun updateCachedRuntimeStatus(status: RuntimeStatus?) {
        if (cachedRuntimeStatus == status) {
            return
        }
        cachedRuntimeStatus = status
        runtimeStatusListeners.forEach { listener ->
            runCatching {
                listener.onRuntimeStatusChanged(status)
            }.onFailure { err ->
                Log.w(TAG, "Runtime status listener failed", err)
            }
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
