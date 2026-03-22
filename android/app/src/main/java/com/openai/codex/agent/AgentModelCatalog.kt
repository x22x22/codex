package com.openai.codex.agent

data class AgentModelOption(
    val id: String,
    val model: String,
    val displayName: String,
    val description: String,
    val supportedReasoningEfforts: List<AgentReasoningEffortOption>,
    val defaultReasoningEffort: String,
    val isDefault: Boolean,
)

data class AgentReasoningEffortOption(
    val reasoningEffort: String,
    val description: String,
)
