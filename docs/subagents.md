# Subagents

This document explains the subagent architecture in Codex.

## What are Subagents?

Subagents are specialized, internal AI agents that handle specific tasks within Codex. They operate independently from the main conversation flow but are spawned by the primary Codex session to handle specialized workloads. Each subagent has its own configuration, prompt, and purpose.

## Subagents vs Orchestrator

It's important to distinguish between **subagents** and the **orchestrator**:

- **Orchestrator** (`codex-rs/core/src/tools/orchestrator.rs`): This is not a subagent. The orchestrator is a tool execution system that manages tool calls, handles approvals, manages sandboxing, and coordinates retry logic for tool execution. It's part of the core infrastructure that helps execute commands and tools safely.

- **Subagents**: These are specialized AI agent instances that run separate conversations for specific purposes. They are implemented as `SessionSource::SubAgent(SubAgentSource)` in the protocol.

## Available Subagents

Codex currently has **two built-in subagents**:

### 1. Review Subagent (`SubAgentSource::Review`)

**Purpose**: Performs automated code reviews on proposed changes.

**Implementation**: `codex-rs/core/src/tasks/review.rs`

**How it works**:
- Triggered by the `/review` slash command in the CLI
- Runs as a separate Codex conversation with specialized review instructions
- Uses the `review_model` configuration (default: `gpt-5.1-codex`)
- Operates with a dedicated review prompt (`codex-rs/core/review_prompt.md`) that contains guidelines for:
  - Identifying bugs and issues
  - Determining severity levels (P0-P3 priorities)
  - Providing actionable feedback
  - Evaluating overall patch correctness
- Outputs structured findings in JSON format with:
  - Title and description of each issue
  - Confidence scores
  - Priority levels
  - Specific code locations (file paths and line ranges)
  - Overall correctness verdict

**Configuration**:
```toml
# In ~/.codex/config.toml
review_model = "gpt-5.1-codex"
```

**Special behavior**:
- Runs without user instructions (uses only the review rubric)
- Does not load project documentation to focus on the changes
- Disables certain features like web search and image viewing
- Suppresses agent message deltas in favor of structured output

### 2. Compact Subagent (`SubAgentSource::Compact`)

**Purpose**: Summarizes conversation history to prevent hitting context limits.

**Implementation**: `codex-rs/core/src/tasks/compact.rs`

**How it works**:
- Triggered by the `/compact` slash command or automatically when approaching token limits
- Can run in two modes:
  - **Remote compaction**: When authenticated with ChatGPT, uses a remote service
  - **Local compaction**: Uses a local LLM call to summarize the conversation
- Uses specialized summarization prompts (`codex-rs/core/templates/compact/prompt.md`)
- Generates a compressed version of the conversation history
- Emits a `ContextCompactedEvent` with the summarized content

**Configuration**:
```toml
# In ~/.codex/config.toml
# Override auto-compact behavior (default: model family specific)
model_auto_compact_token_limit = 0  # disable

# Custom compact prompt
compact_prompt = "your custom prompt"

# Or load from file
experimental_compact_prompt_file = "path/to/compact_prompt.txt"
```

**Key files**:
- Summarization prompt: `codex-rs/core/templates/compact/prompt.md`
- Summary prefix template: `codex-rs/core/templates/compact/summary_prefix.md`

## Extensibility

The subagent architecture supports extension through `SubAgentSource::Other(String)`, which allows for custom subagent types to be added in the future without modifying the core protocol definition.

## Technical Details

### Protocol Definition

Subagents are defined in `codex-rs/protocol/src/protocol.rs`:

```rust
pub enum SessionSource {
    Cli,
    VSCode,
    Exec,
    Mcp,
    SubAgent(SubAgentSource),
    Unknown,
}

pub enum SubAgentSource {
    Review,
    Compact,
    Other(String),  // For future extensibility
}
```

### Task Implementation

Both subagents implement the `SessionTask` trait defined in `codex-rs/core/src/tasks/mod.rs`, which provides:
- A `kind()` method to identify the task type
- A `run()` method to execute the task asynchronously
- An optional `abort()` method for cleanup on cancellation

### HTTP Headers

When subagents make API calls, they include an `x-openai-subagent` header to identify themselves:
- Review subagent: `x-openai-subagent: review`
- Compact subagent: `x-openai-subagent: compact`
- Custom subagents: `x-openai-subagent: <custom_name>`

This header helps with telemetry and debugging.

## See Also

- [Slash Commands](./slash_commands.md) - Interactive commands including `/review` and `/compact`
- [Configuration](./config.md) - Configuration options for models and behavior
- [Getting Started](./getting-started.md) - Basic usage and features
