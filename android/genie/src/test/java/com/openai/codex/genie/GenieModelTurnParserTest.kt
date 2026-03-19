package com.openai.codex.genie

import org.junit.Assert.assertEquals
import org.junit.Test

class GenieModelTurnParserTest {
    @Test
    fun parseToolCallTurn() {
        val turn = GenieModelTurnParser.parse(
            """TOOL: {"name":"android.intent.launch","arguments":{"packageName":"com.android.deskclock"}}""",
        )

        val toolCall = turn as GenieModelTurn.ToolCall
        assertEquals("android.intent.launch", toolCall.name)
        assertEquals("com.android.deskclock", toolCall.arguments.getString("packageName"))
    }

    @Test
    fun parseQuestionTurn() {
        assertEquals(
            GenieModelTurn.Question("Should I continue?"),
            GenieModelTurnParser.parse("QUESTION: Should I continue?"),
        )
    }

    @Test
    fun parseResultTurn() {
        assertEquals(
            GenieModelTurn.Result("Launched the target app."),
            GenieModelTurnParser.parse("RESULT: Launched the target app."),
        )
    }
}
