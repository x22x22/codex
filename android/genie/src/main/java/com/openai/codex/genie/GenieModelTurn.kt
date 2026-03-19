package com.openai.codex.genie

import java.io.IOException
import org.json.JSONObject

sealed interface GenieModelTurn {
    data class Question(val text: String) : GenieModelTurn

    data class Result(val text: String) : GenieModelTurn

    data class ToolCall(
        val name: String,
        val arguments: JSONObject,
    ) : GenieModelTurn
}

object GenieModelTurnParser {
    fun parse(message: String): GenieModelTurn {
        val trimmed = message.trim()
        stripTurnPrefix(trimmed, "TOOL:")?.let(::parseToolCall)?.let { return it }
        stripTurnPrefix(trimmed, "QUESTION:")?.let { return GenieModelTurn.Question(it) }
        stripTurnPrefix(trimmed, "RESULT:")?.let { return GenieModelTurn.Result(it) }
        return if (trimmed.endsWith("?")) {
            GenieModelTurn.Question(trimmed)
        } else {
            GenieModelTurn.Result(trimmed)
        }
    }

    private fun parseToolCall(payload: String): GenieModelTurn.ToolCall {
        val call = try {
            JSONObject(payload)
        } catch (err: Exception) {
            throw IOException("Invalid TOOL payload: ${err.message}", err)
        }
        val name = call.optString("name").trim()
        if (name.isEmpty()) {
            throw IOException("TOOL payload missing name")
        }
        return GenieModelTurn.ToolCall(
            name = name,
            arguments = call.optJSONObject("arguments") ?: JSONObject(),
        )
    }

    private fun stripTurnPrefix(message: String, prefix: String): String? {
        if (!message.startsWith(prefix, ignoreCase = true)) {
            return null
        }
        return message.substring(prefix.length).trim().ifEmpty { "continue" }
    }
}
