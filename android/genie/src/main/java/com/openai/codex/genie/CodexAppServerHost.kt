package com.openai.codex.genie

import android.app.agent.AgentSessionInfo
import android.app.agent.GenieRequest
import android.app.agent.GenieService
import android.content.Context
import android.util.Log
import com.openai.codex.bridge.HostedCodexConfig
import com.openai.codex.bridge.SessionExecutionSettings
import java.io.BufferedWriter
import java.io.Closeable
import java.io.File
import java.io.IOException
import java.io.InterruptedIOException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.json.JSONArray
import org.json.JSONObject

class CodexAppServerHost(
    private val context: Context,
    private val request: GenieRequest,
    private val callback: GenieService.Callback,
    private val control: GenieSessionControl,
    private val bridgeClient: AgentBridgeClient,
    private val runtimeStatus: CodexAgentBridge.RuntimeStatus,
) : Closeable {
    companion object {
        private const val TAG = "CodexAppServerHost"
        private const val APP_SERVER_BRIDGE_ENV_VAR = "CODEX_OPENAI_APP_SERVER_BRIDGE"
        private const val REQUEST_TIMEOUT_MS = 30_000L
        private const val POLL_TIMEOUT_MS = 250L
        private const val DEFAULT_HOSTED_MODEL = "gpt-5.3-codex"
    }

    private val requestIdSequence = AtomicInteger(1)
    private val pendingResponses = ConcurrentHashMap<String, LinkedBlockingQueue<JSONObject>>()
    private val inboundMessages = LinkedBlockingQueue<JSONObject>()
    private val writerLock = Any()
    private val streamedAgentMessages = mutableMapOf<String, StringBuilder>()

    private lateinit var process: Process
    private lateinit var writer: BufferedWriter
    private lateinit var codexHome: File
    private lateinit var executionSettings: SessionExecutionSettings
    private var stdoutThread: Thread? = null
    private var stderrThread: Thread? = null
    private var finalAgentMessage: String? = null
    private var resultPublished = false
    fun run() {
        startProcess()
        initialize()
        executionSettings = bridgeClient.readSessionExecutionSettings()
        val model = resolveModel()
        val threadId = startThread(model)
        startTurn(threadId, model)
        callback.publishTrace(request.sessionId, "Hosted codex app-server thread $threadId for ${request.targetPackage}.")
        eventLoop()
    }

    override fun close() {
        stdoutThread?.interrupt()
        stderrThread?.interrupt()
        synchronized(writerLock) {
            runCatching { writer.close() }
        }
        if (::codexHome.isInitialized) {
            runCatching { codexHome.deleteRecursively() }
        }
        if (::process.isInitialized) {
            process.destroy()
        }
        control.process = null
    }

    private fun startProcess() {
        codexHome = File(context.cacheDir, "codex-home/${request.sessionId}").apply {
            deleteRecursively()
            mkdirs()
        }
        HostedCodexConfig.installAgentsFile(codexHome, bridgeClient.readInstalledAgentsMarkdown())
        val processBuilder = ProcessBuilder(
            listOf(
                CodexBinaryLocator.resolve(context).absolutePath,
                "-c",
                "enable_request_compression=false",
                "app-server",
                "--listen",
                "stdio://",
            ),
        )
        val env = processBuilder.environment()
        env["CODEX_HOME"] = codexHome.absolutePath
        env[APP_SERVER_BRIDGE_ENV_VAR] = "1"
        env["RUST_LOG"] = "warn"
        process = processBuilder.start()
        control.process = process
        writer = process.outputStream.bufferedWriter()
        startStdoutPump()
        startStderrPump()
    }

    private fun startStdoutPump() {
        stdoutThread = Thread {
            try {
                process.inputStream.bufferedReader().useLines { lines ->
                    lines.forEach { line ->
                        if (line.isBlank()) {
                            return@forEach
                        }
                        val message = runCatching { JSONObject(line) }
                            .getOrElse { err ->
                                Log.w(TAG, "Failed to parse codex app-server stdout line", err)
                                return@forEach
                            }
                        routeInbound(message)
                    }
                }
            } catch (_: InterruptedIOException) {
                // Expected when the hosted app-server exits and the stream closes underneath the reader.
            } catch (err: IOException) {
                if (!control.cancelled && process.isAlive) {
                    Log.w(TAG, "Stdout pump failed for ${request.sessionId}", err)
                }
            }
        }.also {
            it.name = "CodexAppServerStdout-${request.sessionId}"
            it.start()
        }
    }

    private fun startStderrPump() {
        stderrThread = Thread {
            try {
                process.errorStream.bufferedReader().useLines { lines ->
                    lines.forEach { line ->
                        if (line.isBlank()) {
                            return@forEach
                        }
                        when {
                            line.contains(" ERROR ") -> Log.e(TAG, line)
                            line.contains(" WARN ") || line.startsWith("WARNING:") -> Log.w(TAG, line)
                        }
                    }
                }
            } catch (_: InterruptedIOException) {
                // Expected when the hosted app-server exits and the stream closes underneath the reader.
            } catch (err: IOException) {
                if (!control.cancelled && process.isAlive) {
                    Log.w(TAG, "Stderr pump failed for ${request.sessionId}", err)
                }
            }
        }.also {
            it.name = "CodexAppServerStderr-${request.sessionId}"
            it.start()
        }
    }

    private fun routeInbound(message: JSONObject) {
        if (message.has("id") && !message.has("method")) {
            pendingResponses[message.get("id").toString()]?.offer(message)
            return
        }
        inboundMessages.offer(message)
    }

    private fun initialize() {
        request(
            method = "initialize",
            params = JSONObject()
                .put(
                    "clientInfo",
                    JSONObject()
                        .put("name", "android_genie")
                        .put("title", "Android Genie")
                        .put("version", "0.1.0"),
                )
                .put(
                    "capabilities",
                    JSONObject().put("experimentalApi", true),
                ),
        )
        notify("initialized", JSONObject())
    }

    private fun startThread(model: String): String {
        val params = JSONObject()
            .put("approvalPolicy", "never")
            .put("sandbox", "read-only")
            .put("ephemeral", true)
            .put("cwd", context.filesDir.absolutePath)
            .put("serviceName", "android_genie")
            .put("baseInstructions", buildBaseInstructions())
            .put("dynamicTools", buildDynamicToolSpecs())
        params.put("model", model)
        val result = request(
            method = "thread/start",
            params = params,
        )
        return result.getJSONObject("thread").getString("id")
    }

    private fun startTurn(
        threadId: String,
        model: String,
    ) {
        Log.i(TAG, "Starting hosted turn for ${request.sessionId} with model=$model")
        request(
            method = "turn/start",
            params = JSONObject()
                .put("threadId", threadId)
                .put("model", model)
                .apply {
                    executionSettings.reasoningEffort
                        ?.takeIf(String::isNotBlank)
                        ?.let { put("effort", it) }
                }
                .put(
                    "input",
                    JSONArray().put(
                        JSONObject()
                            .put("type", "text")
                            .put("text", buildDelegatedPrompt()),
                    ),
                ),
        )
    }

    private fun resolveModel(): String = executionSettings.model
        ?.takeIf(String::isNotBlank)
        ?: runtimeStatus.configuredModel
            ?.takeIf(String::isNotBlank)
        ?: runtimeStatus.effectiveModel
            ?.takeIf(String::isNotBlank)
        ?: DEFAULT_HOSTED_MODEL

    private fun eventLoop() {
        while (!control.cancelled) {
            val message = inboundMessages.poll(POLL_TIMEOUT_MS, TimeUnit.MILLISECONDS)
            if (message == null) {
                if (!process.isAlive) {
                    throw IOException("codex app-server exited with code ${process.exitValue()}")
                }
                continue
            }
            if (message.has("method") && message.has("id")) {
                handleServerRequest(message)
                continue
            }
            if (message.has("method") && handleNotification(message)) {
                return
            }
        }
        throw IOException("Cancelled")
    }

    private fun handleServerRequest(message: JSONObject) {
        val method = message.getString("method")
        val requestId = message.get("id")
        val params = message.optJSONObject("params") ?: JSONObject()
        Log.i(TAG, "Handling app-server request method=$method session=${request.sessionId}")
        when (method) {
            "item/tool/call" -> handleDynamicToolCall(requestId, params)
            "item/tool/requestUserInput" -> handleRequestUserInput(requestId, params)
            "response/send" -> handleResponsesBridgeRequest(requestId, params)
            else -> {
                callback.publishTrace(request.sessionId, "Unsupported codex app-server request: $method")
                sendError(
                    requestId = requestId,
                    code = -32601,
                    message = "Unsupported app-server request: $method",
                )
            }
        }
    }

    private fun handleDynamicToolCall(
        requestId: Any,
        params: JSONObject,
    ) {
        val toolName = params.optString("tool").trim()
        val arguments = params.optJSONObject("arguments") ?: JSONObject()
        Log.i(TAG, "Executing dynamic tool $toolName arguments=$arguments")
        val toolExecutor = AndroidGenieToolExecutor(
            callback = callback,
            sessionId = request.sessionId,
        )
        val observation = runCatching {
            toolExecutor.execute(toolName, arguments)
        }.getOrElse { err ->
            GenieToolObservation(
                name = toolName.ifBlank { "unknown" },
                summary = "Tool $toolName failed: ${err.message}",
                promptDetails = "Tool $toolName failed.\nError: ${err.message ?: err::class.java.simpleName}",
            )
        }
        callback.publishTrace(request.sessionId, observation.summary)
        sendResult(
            requestId = requestId,
            result = JSONObject()
                .put("success", !observation.summary.contains(" failed:"))
                .put("contentItems", buildDynamicToolContentItems(observation)),
        )
    }

    private fun handleRequestUserInput(
        requestId: Any,
        params: JSONObject,
    ) {
        val questions = params.optJSONArray("questions") ?: JSONArray()
        val renderedQuestion = renderAgentQuestion(questions)
        Log.i(TAG, "Requesting Agent input for ${request.sessionId}: $renderedQuestion")
        callback.publishQuestion(request.sessionId, renderedQuestion)
        callback.updateState(request.sessionId, AgentSessionInfo.STATE_WAITING_FOR_USER)
        val answer = control.waitForUserResponse()
        callback.updateState(request.sessionId, AgentSessionInfo.STATE_RUNNING)
        callback.publishTrace(request.sessionId, "Received Agent answer for ${request.targetPackage}.")
        Log.i(TAG, "Received Agent input for ${request.sessionId}: ${answer.take(160)}")
        sendResult(
            requestId = requestId,
            result = JSONObject().put("answers", buildQuestionAnswers(questions, answer)),
        )
    }

    private fun handleResponsesBridgeRequest(
        requestId: Any,
        params: JSONObject,
    ) {
        val requestBody = params.optString("requestBody")
        val httpResponse = bridgeClient.sendResponsesRequest(requestBody)
        sendResult(
            requestId = requestId,
            result = JSONObject()
                .put("statusCode", httpResponse.statusCode)
                .put("body", httpResponse.body),
        )
    }

    private fun handleNotification(message: JSONObject): Boolean {
        val method = message.getString("method")
        val params = message.optJSONObject("params") ?: JSONObject()
        return when (method) {
            "turn/started" -> {
                callback.publishTrace(request.sessionId, "codex turn started for ${request.targetPackage}.")
                false
            }
            "item/agentMessage/delta" -> {
                val itemId = params.optString("itemId")
                if (itemId.isNotBlank()) {
                    streamedAgentMessages.getOrPut(itemId, ::StringBuilder)
                        .append(params.optString("delta"))
                }
                false
            }
            "item/started" -> {
                publishItemStartedTrace(params.optJSONObject("item"))
                false
            }
            "item/completed" -> {
                captureCompletedItem(params.optJSONObject("item"))
                false
            }
            "turn/completed" -> {
                finishTurn(params)
                true
            }
            else -> false
        }
    }

    private fun publishItemStartedTrace(item: JSONObject?) {
        if (item == null) {
            return
        }
        val command = commandForItem(item)
        Log.i(
            TAG,
            "item/started type=${item.optString("type")} tool=${item.optString("tool")} command=${command ?: ""}",
        )
        when (item.optString("type")) {
            "dynamicToolCall" -> {
                val tool = item.optString("tool")
                callback.publishTrace(request.sessionId, "Codex requested dynamic tool $tool.")
            }
            "commandExecution" -> {
                callback.publishTrace(
                    request.sessionId,
                    "Codex started command execution: ${command ?: "command"}",
                )
            }
        }
    }

    private fun captureCompletedItem(item: JSONObject?) {
        if (item == null) {
            return
        }
        val command = commandForItem(item)
        val errorDetail = item.optString("aggregatedOutput").ifBlank {
            item.optString("stderr").ifBlank {
                item.optString("output").ifBlank {
                    item.optString("error")
                }
            }
        }.trim()
        Log.i(
            TAG,
            "item/completed type=${item.optString("type")} status=${item.optString("status")} tool=${item.optString("tool")} command=${command ?: ""} error=${errorDetail.take(200)}",
        )
        when (item.optString("type")) {
            "agentMessage" -> {
                val itemId = item.optString("id")
                val text = item.optString("text").ifBlank {
                    streamedAgentMessages[itemId]?.toString().orEmpty()
                }
                if (text.isNotBlank()) {
                    finalAgentMessage = text
                }
            }
            "commandExecution" -> {
                val status = item.optString("status")
                val exitCode = if (item.has("exitCode")) item.opt("exitCode") else null
                val resolvedCommand = command ?: "command"
                if (status == "failed") {
                    Log.i(TAG, "Failed command item=${item}")
                    val detailSuffix = errorDetail
                        .takeIf(String::isNotBlank)
                        ?.let { " Details: ${it.take(240)}" }
                        .orEmpty()
                    callback.publishTrace(
                        request.sessionId,
                        "Command failed: $resolvedCommand (status=$status, exitCode=${exitCode ?: "unknown"}).$detailSuffix",
                    )
                    if (errorDetail.contains("package=com.android.shell does not belong to uid=")) {
                        callback.publishTrace(
                            request.sessionId,
                            "This shell command requires com.android.shell privileges. The target is already running hidden; use detached-target dynamic tools to show or inspect it instead of retrying the same shell launch surface.",
                        )
                    }
                } else {
                    callback.publishTrace(
                        request.sessionId,
                        "Command completed: $resolvedCommand (status=$status, exitCode=${exitCode ?: "unknown"}).",
                    )
                }
            }
            "dynamicToolCall" -> {
                val tool = item.optString("tool")
                val status = item.optString("status")
                callback.publishTrace(request.sessionId, "Dynamic tool $tool completed with status=$status.")
            }
        }
    }

    private fun commandForItem(item: JSONObject): String? {
        return item.optString("command")
            .takeIf(String::isNotBlank)
            ?: item.optJSONArray("command")?.join(" ")
    }

    private fun finishTurn(params: JSONObject) {
        val turn = params.optJSONObject("turn") ?: JSONObject()
        Log.i(TAG, "turn/completed status=${turn.optString("status")} error=${turn.opt("error")}")
        when (turn.optString("status")) {
            "completed" -> {
                val resultText = finalAgentMessage?.takeIf(String::isNotBlank)
                    ?: "Genie completed without a final assistant message."
                publishResultOnce(resultText)
                callback.updateState(request.sessionId, AgentSessionInfo.STATE_COMPLETED)
            }
            "interrupted" -> {
                callback.publishError(request.sessionId, "Genie turn interrupted")
                callback.updateState(request.sessionId, AgentSessionInfo.STATE_CANCELLED)
            }
            else -> {
                val errorDetail = turn.opt("error")?.toString()
                    ?: "Genie turn failed with status ${turn.optString("status", "unknown")}" 
                callback.publishError(request.sessionId, errorDetail)
                callback.updateState(request.sessionId, AgentSessionInfo.STATE_FAILED)
            }
        }
    }

    private fun publishResultOnce(text: String) {
        if (resultPublished) {
            return
        }
        resultPublished = true
        callback.publishResult(request.sessionId, text)
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

    private fun sendMessage(message: JSONObject) {
        synchronized(writerLock) {
            writer.write(message.toString())
            writer.newLine()
            writer.flush()
        }
    }

    private fun buildBaseInstructions(): String {
        return """
            You are Codex acting as a child Android Genie bound to ${request.targetPackage}.
            The user interacts only with the supervising Agent.
            Decide your own local plan and choose tools yourself.
            Prefer direct self-targeted Android shell commands and intents first when they can satisfy the objective without UI-driving.
            In this platform build, an active Genie session may use self-targeted shell surfaces such as `am start --user 0`, `cmd activity start-activity --user 0`, `cmd package resolve-activity`, `cmd package query-activities --user 0`, `input`, `uiautomator dump`, `screencap`, and `screenrecord`.
            When using `am start`, `cmd activity start-activity`, or `cmd package query-activities`, pass `--user 0`; omitting it can fail with cross-user permission errors.
            Android shell `date` is not GNU coreutils `date`; do not rely on `date -d "+5 minutes"` or similar relative-date parsing because it fails on this platform.
            When you must convert a relative request like “in 5 minutes” into wall-clock alarm fields, compute it with shell arithmetic from `date +%H` and `date +%M`, for example: `h=$(date +%H); m=$(date +%M); total=$((10#${'$'}h * 60 + 10#${'$'}m + 5)); hour=$(((total / 60) % 24)); minute=$((total % 60))`, then pass `hour` and `minute` as integers to `am start`.
            When the objective is a timer duration rather than a wall-clock alarm, prefer direct duration-based intents like `android.intent.action.SET_TIMER` with a length in seconds instead of computing a future clock time.
            Avoid `dumpsys` and `cmd package dump` for package/activity inspection because they require `android.permission.DUMP` in the paired app UID and will not help you complete the task.
            If a direct command or intent clearly accomplishes the objective, stop and report success instead of continuing exploratory UI actions.
            The Genie may request detached target launch through the framework callback, and after that it may use supported self-targeted shell commands to drive the target app.
            If a direct intent launch does not fully complete the task, use detached-target tools to show or inspect the target, then continue with supported shell input and inspection surfaces.
            Use Android dynamic tools only for framework-only detached target operations that do not have a working shell equivalent in the paired app sandbox.
            The delegated objective may include a required final target presentation such as ATTACHED, DETACHED_HIDDEN, or DETACHED_SHOWN. Treat that as a hard completion requirement and do not report success until the framework session actually matches it.
            If you need clarification or a decision from the supervising Agent, call request_user_input with concise free-form question text.
            Do not use hidden control protocols.
            Finish with a normal assistant message describing what you accomplished or what blocked you.
            Detached target mode allowed: ${request.isDetachedModeAllowed}.
            Agent-owned runtime provider: ${runtimeStatus.modelProviderId}.
        """.trimIndent()
    }

    private fun buildDelegatedPrompt(): String {
        return """
            Target package:
            ${request.targetPackage}

            Delegated objective:
            ${request.prompt}
        """.trimIndent()
    }

    private fun buildDynamicToolSpecs(): JSONArray {
        return JSONArray()
            .put(dynamicToolSpec(AndroidGenieToolExecutor.SHOW_TARGET_TOOL, "Show the detached target window.", emptyObjectSchema()))
            .put(dynamicToolSpec(AndroidGenieToolExecutor.HIDE_TARGET_TOOL, "Hide the detached target window.", emptyObjectSchema()))
            .put(dynamicToolSpec(AndroidGenieToolExecutor.ATTACH_TARGET_TOOL, "Reattach the detached target back to the main display.", emptyObjectSchema()))
            .put(dynamicToolSpec(AndroidGenieToolExecutor.CLOSE_TARGET_TOOL, "Close the detached target window.", emptyObjectSchema()))
            .put(dynamicToolSpec(AndroidGenieToolExecutor.CAPTURE_TARGET_FRAME_TOOL, "Capture the detached target window as an image.", emptyObjectSchema()))
    }

    private fun dynamicToolSpec(
        name: String,
        description: String,
        inputSchema: JSONObject,
    ): JSONObject {
        return JSONObject()
            .put("name", name)
            .put("description", description)
            .put("inputSchema", inputSchema)
    }

    private fun emptyObjectSchema(): JSONObject {
        return objectSchema(emptyMap())
    }

    private fun objectSchema(
        properties: Map<String, JSONObject>,
        required: List<String> = emptyList(),
    ): JSONObject {
        val propertiesJson = JSONObject()
        properties.forEach { (name, schema) -> propertiesJson.put(name, schema) }
        return JSONObject()
            .put("type", "object")
            .put("properties", propertiesJson)
            .put("required", JSONArray(required))
            .put("additionalProperties", false)
    }

    private fun buildDynamicToolContentItems(observation: GenieToolObservation): JSONArray {
        val items = JSONArray().put(
            JSONObject()
                .put("type", "inputText")
                .put("text", observation.promptDetails),
        )
        observation.imageDataUrls.forEach { imageUrl ->
            items.put(
                JSONObject()
                    .put("type", "inputImage")
                    .put("imageUrl", imageUrl),
            )
        }
        return items
    }

    private fun renderAgentQuestion(questions: JSONArray): String {
        if (questions.length() == 0) {
            return "Genie requested input but did not provide a question."
        }
        val rendered = buildString {
            for (index in 0 until questions.length()) {
                val question = questions.optJSONObject(index) ?: continue
                if (length > 0) {
                    append("\n\n")
                }
                val header = question.optString("header").takeIf(String::isNotBlank)
                if (header != null) {
                    append(header)
                    append(":\n")
                }
                append(question.optString("question"))
                val options = question.optJSONArray("options")
                if (options != null && options.length() > 0) {
                    append("\nOptions:")
                    for (optionIndex in 0 until options.length()) {
                        val option = options.optJSONObject(optionIndex) ?: continue
                        append("\n- ")
                        append(option.optString("label"))
                        val description = option.optString("description")
                        if (description.isNotBlank()) {
                            append(": ")
                            append(description)
                        }
                    }
                }
            }
        }
        return if (questions.length() == 1) {
            rendered
        } else {
            "$rendered\n\nReply with one answer per question, separated by a blank line."
        }
    }

    private fun buildQuestionAnswers(
        questions: JSONArray,
        answer: String,
    ): JSONObject {
        val splitAnswers = answer
            .split(Regex("\\n\\s*\\n"))
            .map(String::trim)
            .filter(String::isNotEmpty)
        val answersJson = JSONObject()
        for (index in 0 until questions.length()) {
            val question = questions.optJSONObject(index) ?: continue
            val questionId = question.optString("id")
            if (questionId.isBlank()) {
                continue
            }
            val responseText = splitAnswers.getOrNull(index)
                ?: if (index == 0) answer.trim() else ""
            answersJson.put(
                questionId,
                JSONObject().put(
                    "answers",
                    JSONArray().put(responseText),
                ),
            )
        }
        return answersJson
    }
}
