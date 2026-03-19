package com.openai.codex.genie

import android.app.agent.AgentSessionInfo
import android.app.agent.GenieRequest
import android.app.agent.GenieService
import android.content.Context
import android.util.Log
import java.io.BufferedWriter
import java.io.Closeable
import java.io.File
import java.io.IOException
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
    private val targetAppContext: TargetAppContext?,
) : Closeable {
    companion object {
        private const val TAG = "CodexAppServerHost"
        private const val REQUEST_TIMEOUT_MS = 30_000L
        private const val POLL_TIMEOUT_MS = 250L
    }

    private val requestIdSequence = AtomicInteger(1)
    private val pendingResponses = ConcurrentHashMap<String, LinkedBlockingQueue<JSONObject>>()
    private val inboundMessages = LinkedBlockingQueue<JSONObject>()
    private val writerLock = Any()
    private val streamedAgentMessages = mutableMapOf<String, StringBuilder>()

    private lateinit var process: Process
    private lateinit var writer: BufferedWriter
    private var stdoutThread: Thread? = null
    private var stderrThread: Thread? = null
    private var finalAgentMessage: String? = null
    private var resultPublished = false
    private var localProxy: GenieLocalCodexProxy? = null

    fun run() {
        startProcess()
        initialize()
        val threadId = startThread()
        startTurn(threadId)
        callback.publishTrace(request.sessionId, "Hosted codex app-server thread $threadId for ${request.targetPackage}.")
        eventLoop()
    }

    override fun close() {
        stdoutThread?.interrupt()
        stderrThread?.interrupt()
        localProxy?.close()
        synchronized(writerLock) {
            runCatching { writer.close() }
        }
        if (::process.isInitialized) {
            process.destroy()
        }
        control.process = null
    }

    private fun startProcess() {
        val codexHome = File(context.filesDir, "codex-home").apply { mkdirs() }
        localProxy = GenieLocalCodexProxy(
            sessionId = request.sessionId,
            requestForwarder = bridgeClient,
        ).also(GenieLocalCodexProxy::start)
        val proxyBaseUrl = localProxy?.baseUrl
            ?: throw IOException("local Genie proxy did not start")
        val processBuilder = ProcessBuilder(
            listOf(
                CodexBinaryLocator.resolve(context).absolutePath,
                "-c",
                "enable_request_compression=false",
                "-c",
                "openai_base_url=\"$proxyBaseUrl\"",
                "app-server",
                "--listen",
                "stdio://",
            ),
        )
        val env = processBuilder.environment()
        env["CODEX_HOME"] = codexHome.absolutePath
        env["CODEX_USE_AGENT_AUTH_PROXY"] = "1"
        env["RUST_LOG"] = "info"
        process = processBuilder.start()
        control.process = process
        writer = process.outputStream.bufferedWriter()
        startStdoutPump()
        startStderrPump()
    }

    private fun startStdoutPump() {
        stdoutThread = Thread {
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
        }.also {
            it.name = "CodexAppServerStdout-${request.sessionId}"
            it.start()
        }
    }

    private fun startStderrPump() {
        stderrThread = Thread {
            process.errorStream.bufferedReader().useLines { lines ->
                lines.forEach { line ->
                    if (line.isNotBlank()) {
                        Log.i(TAG, line)
                    }
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

    private fun startThread(): String {
        val params = JSONObject()
            .put("approvalPolicy", "never")
            .put("sandbox", "read-only")
            .put("ephemeral", true)
            .put("cwd", context.filesDir.absolutePath)
            .put("serviceName", "android_genie")
            .put("baseInstructions", buildBaseInstructions())
            .put("dynamicTools", buildDynamicToolSpecs())
        runtimeStatus.effectiveModel?.takeIf(String::isNotBlank)?.let { model ->
            params.put("model", model)
        }
        val result = request(
            method = "thread/start",
            params = params,
        )
        return result.getJSONObject("thread").getString("id")
    }

    private fun startTurn(threadId: String) {
        request(
            method = "turn/start",
            params = JSONObject()
                .put("threadId", threadId)
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
        when (method) {
            "item/tool/call" -> handleDynamicToolCall(requestId, params)
            "item/tool/requestUserInput" -> handleRequestUserInput(requestId, params)
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
        val toolExecutor = AndroidGenieToolExecutor(
            context = context,
            callback = callback,
            sessionId = request.sessionId,
            defaultTargetPackage = request.targetPackage,
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
        callback.publishQuestion(request.sessionId, renderedQuestion)
        callback.updateState(request.sessionId, AgentSessionInfo.STATE_WAITING_FOR_USER)
        val answer = control.waitForUserResponse()
        callback.updateState(request.sessionId, AgentSessionInfo.STATE_RUNNING)
        callback.publishTrace(request.sessionId, "Received Agent answer for ${request.targetPackage}.")
        sendResult(
            requestId = requestId,
            result = JSONObject().put("answers", buildQuestionAnswers(questions, answer)),
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
        when (item.optString("type")) {
            "dynamicToolCall" -> {
                val tool = item.optString("tool")
                callback.publishTrace(request.sessionId, "Codex requested dynamic tool $tool.")
            }
            "commandExecution" -> {
                val command = item.optJSONArray("command")?.join(" ") ?: "command"
                callback.publishTrace(request.sessionId, "Codex started command execution: $command")
            }
        }
    }

    private fun captureCompletedItem(item: JSONObject?) {
        if (item == null) {
            return
        }
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
                callback.publishTrace(
                    request.sessionId,
                    "Command execution completed with status=$status exitCode=${exitCode ?: "unknown"}.",
                )
            }
            "dynamicToolCall" -> {
                val tool = item.optString("tool")
                val status = item.optString("status")
                callback.publishTrace(request.sessionId, "Dynamic tool $tool completed with status=$status.")
            }
        }
    }

    private fun finishTurn(params: JSONObject) {
        val turn = params.optJSONObject("turn") ?: JSONObject()
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
            Prefer the Android dynamic tools for observing and driving the target app.
            If you need clarification or a decision from the supervising Agent, call request_user_input with concise free-form question text.
            Do not use hidden control protocols.
            Finish with a normal assistant message describing what you accomplished or what blocked you.
            Detached target mode allowed: ${request.isDetachedModeAllowed}.
            Agent-owned runtime provider: ${runtimeStatus.modelProviderId}.
        """.trimIndent()
    }

    private fun buildDelegatedPrompt(): String {
        val targetSection = targetAppContext?.renderPromptSection()
            ?: "Target app inspection:\n- unavailable"
        return """
            Delegated objective:
            ${request.prompt}

            $targetSection
        """.trimIndent()
    }

    private fun buildDynamicToolSpecs(): JSONArray {
        return JSONArray()
            .put(
                dynamicToolSpec(
                    name = "android.package.inspect",
                    description = "Inspect package metadata for the paired Android target app.",
                    inputSchema = objectSchema(
                        properties = mapOf(
                            "packageName" to stringSchema("Optional package name override."),
                        ),
                    ),
                ),
            )
            .put(
                dynamicToolSpec(
                    name = "android.intent.launch",
                    description = "Launch the target app or an explicit target activity/intent.",
                    inputSchema = objectSchema(
                        properties = mapOf(
                            "packageName" to stringSchema("Optional package name override."),
                            "action" to stringSchema("Optional Android intent action."),
                            "component" to stringSchema("Optional flattened component name."),
                        ),
                    ),
                ),
            )
            .put(dynamicToolSpec("android.target.show", "Show the detached target window.", emptyObjectSchema()))
            .put(dynamicToolSpec("android.target.hide", "Hide the detached target window.", emptyObjectSchema()))
            .put(dynamicToolSpec("android.target.attach", "Reattach the detached target back to the main display.", emptyObjectSchema()))
            .put(dynamicToolSpec("android.target.close", "Close the detached target window.", emptyObjectSchema()))
            .put(dynamicToolSpec("android.target.capture_frame", "Capture the detached target window as an image.", emptyObjectSchema()))
            .put(dynamicToolSpec("android.ui.dump", "Dump the current UI hierarchy via uiautomator.", emptyObjectSchema()))
            .put(
                dynamicToolSpec(
                    name = "android.input.tap",
                    description = "Inject a tap at absolute screen coordinates.",
                    inputSchema = objectSchema(
                        properties = mapOf(
                            "x" to numberSchema("Absolute X coordinate."),
                            "y" to numberSchema("Absolute Y coordinate."),
                        ),
                        required = listOf("x", "y"),
                    ),
                ),
            )
            .put(
                dynamicToolSpec(
                    name = "android.input.text",
                    description = "Inject text into the focused field.",
                    inputSchema = objectSchema(
                        properties = mapOf(
                            "text" to stringSchema("Text to type."),
                        ),
                        required = listOf("text"),
                    ),
                ),
            )
            .put(
                dynamicToolSpec(
                    name = "android.input.key",
                    description = "Inject an Android keyevent by name or keycode token.",
                    inputSchema = objectSchema(
                        properties = mapOf(
                            "key" to stringSchema("Android keyevent token, for example ENTER or BACK."),
                        ),
                        required = listOf("key"),
                    ),
                ),
            )
            .put(
                dynamicToolSpec(
                    name = "android.wait",
                    description = "Pause briefly to let the UI settle.",
                    inputSchema = objectSchema(
                        properties = mapOf(
                            "millis" to numberSchema("Milliseconds to sleep (1-10000)."),
                        ),
                        required = listOf("millis"),
                    ),
                ),
            )
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

    private fun stringSchema(description: String): JSONObject {
        return JSONObject()
            .put("type", "string")
            .put("description", description)
    }

    private fun numberSchema(description: String): JSONObject {
        return JSONObject()
            .put("type", "number")
            .put("description", description)
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
