package com.openai.codex.genie

data class GenieToolObservation(
    val name: String,
    val summary: String,
    val promptDetails: String,
    val imageDataUrls: List<String> = emptyList(),
) {
    fun renderForPrompt(): String {
        return """
            Tool: $name
            Observation:
            $promptDetails
        """.trimIndent()
    }
}
