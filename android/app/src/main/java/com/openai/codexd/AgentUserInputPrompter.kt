package com.openai.codexd

import android.app.Activity
import android.app.AlertDialog
import android.widget.EditText
import java.io.IOException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicReference
import org.json.JSONArray
import org.json.JSONObject

object AgentUserInputPrompter {
    fun promptForAnswers(
        activity: Activity,
        questions: JSONArray,
    ): JSONObject {
        val latch = CountDownLatch(1)
        val answerText = AtomicReference("")
        val error = AtomicReference<IOException?>(null)
        activity.runOnUiThread {
            val input = EditText(activity).apply {
                minLines = 4
                maxLines = 8
                setSingleLine(false)
                setText("")
                hint = "Type your answer here"
            }
            AlertDialog.Builder(activity)
                .setTitle("Codex needs input")
                .setMessage(renderQuestions(questions))
                .setView(input)
                .setCancelable(false)
                .setPositiveButton("Submit") { dialog, _ ->
                    answerText.set(input.text?.toString().orEmpty())
                    dialog.dismiss()
                    latch.countDown()
                }
                .setNegativeButton("Cancel") { dialog, _ ->
                    error.set(IOException("User cancelled Agent input"))
                    dialog.dismiss()
                    latch.countDown()
                }
                .show()
        }
        latch.await()
        error.get()?.let { throw it }
        return JSONObject().put("answers", buildQuestionAnswers(questions, answerText.get()))
    }

    internal fun renderQuestions(questions: JSONArray): String {
        if (questions.length() == 0) {
            return "Codex requested input but did not provide a question."
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

    internal fun buildQuestionAnswers(
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
