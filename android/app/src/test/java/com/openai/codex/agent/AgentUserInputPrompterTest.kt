package com.openai.codex.agent

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AgentUserInputPrompterTest {
    @Test
    fun buildQuestionAnswersMapsSplitAnswersByQuestionId() {
        val questions = JSONArray()
            .put(
                JSONObject()
                    .put("id", "duration")
                    .put("question", "How long should the timer last?"),
            )
            .put(
                JSONObject()
                    .put("id", "confirm")
                    .put("question", "Should I start it now?"),
            )

        val answers = AgentUserInputPrompter.buildQuestionAnswers(
            questions = questions,
            answer = "5 minutes\n\nYes",
        )

        assertEquals("5 minutes", answers.getJSONObject("duration").getJSONArray("answers").getString(0))
        assertEquals("Yes", answers.getJSONObject("confirm").getJSONArray("answers").getString(0))
    }

    @Test
    fun renderQuestionsMentionsBlankLineSeparatorForMultipleQuestions() {
        val questions = JSONArray()
            .put(
                JSONObject()
                    .put("id", "duration")
                    .put("question", "How long should the timer last?"),
            )
            .put(
                JSONObject()
                    .put("id", "confirm")
                    .put("question", "Should I start it now?"),
            )

        val rendered = AgentUserInputPrompter.renderQuestions(questions)

        assertTrue(rendered.contains("How long should the timer last?"))
        assertTrue(rendered.contains("Should I start it now?"))
        assertTrue(rendered.contains("Reply with one answer per question"))
    }
}
