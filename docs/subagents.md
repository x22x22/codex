# Subagents（子代理）

本文档介绍 Codex 中的子代理（subagent）架构。

## 什么是子代理？

子代理是在 Codex 内部处理特定任务的专用 AI 代理。它们独立于主对话流程运行，由主 Codex 会话生成来处理专门的工作负载。每个子代理都有自己的配置、提示词和用途。

## 子代理 vs 编排器（Orchestrator）

区分**子代理**和**编排器**非常重要：

- **编排器（Orchestrator）**（`codex-rs/core/src/tools/orchestrator.rs`）：这不是一个子代理。编排器是一个工具执行系统，负责管理工具调用、处理审批、管理沙箱环境以及协调工具执行的重试逻辑。它是核心基础设施的一部分，帮助安全地执行命令和工具。

- **子代理（Subagents）**：这些是专门的 AI 代理实例，为特定目的运行独立的对话。它们在协议中实现为 `SessionSource::SubAgent(SubAgentSource)`。

## 可用的子代理

Codex 目前有**两个内置子代理**：

### 1. 审查子代理（Review Subagent）（`SubAgentSource::Review`）

**用途**：对提议的代码更改执行自动化代码审查。

**实现**：`codex-rs/core/src/tasks/review.rs`

**工作原理**：
- 通过 CLI 中的 `/review` 斜杠命令触发
- 作为独立的 Codex 对话运行，使用专门的审查指令
- 使用 `review_model` 配置（默认：`gpt-5.1-codex`）
- 使用专用的审查提示词（`codex-rs/core/review_prompt.md`），其中包含以下指南：
  - 识别错误和问题
  - 确定严重程度级别（P0-P3 优先级）
  - 提供可操作的反馈
  - 评估整体补丁的正确性
- 以 JSON 格式输出结构化的发现，包括：
  - 每个问题的标题和描述
  - 置信度分数
  - 优先级
  - 具体的代码位置（文件路径和行范围）
  - 整体正确性判断

**配置**：
```toml
# 在 ~/.codex/config.toml 中
review_model = "gpt-5.1-codex"
```

**特殊行为**：
- 不使用用户指令运行（仅使用审查规则）
- 不加载项目文档，专注于更改内容
- 禁用某些功能，如网络搜索和图像查看
- 抑制代理消息增量，转而使用结构化输出

### 2. 压缩子代理（Compact Subagent）（`SubAgentSource::Compact`）

**用途**：总结对话历史以防止达到上下文限制。

**实现**：`codex-rs/core/src/tasks/compact.rs`

**工作原理**：
- 通过 `/compact` 斜杠命令触发，或在接近令牌限制时自动触发
- 可以在两种模式下运行：
  - **远程压缩**：当使用 ChatGPT 身份验证时，使用远程服务
  - **本地压缩**：使用本地 LLM 调用来总结对话
- 使用专门的总结提示词（`codex-rs/core/templates/compact/prompt.md`）
- 生成对话历史的压缩版本
- 发出包含总结内容的 `ContextCompactedEvent`

**配置**：
```toml
# 在 ~/.codex/config.toml 中
# 覆盖自动压缩行为（默认：特定于模型家族）
model_auto_compact_token_limit = 0  # 禁用

# 自定义压缩提示词
compact_prompt = "your custom prompt"

# 或从文件加载
experimental_compact_prompt_file = "path/to/compact_prompt.txt"
```

**关键文件**：
- 总结提示词：`codex-rs/core/templates/compact/prompt.md`
- 总结前缀模板：`codex-rs/core/templates/compact/summary_prefix.md`

## 可扩展性

子代理架构通过 `SubAgentSource::Other(String)` 支持扩展，允许在未来添加自定义子代理类型，而无需修改核心协议定义。

## 技术细节

### 协议定义

子代理在 `codex-rs/protocol/src/protocol.rs` 中定义：

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
    Other(String),  // 用于未来的可扩展性
}
```

### 任务实现

两个子代理都实现了 `codex-rs/core/src/tasks/mod.rs` 中定义的 `SessionTask` trait，它提供：
- `kind()` 方法来识别任务类型
- `run()` 方法来异步执行任务
- 可选的 `abort()` 方法用于取消时的清理

### HTTP 头部

当子代理进行 API 调用时，它们会包含 `x-openai-subagent` 头部来标识自己：
- 审查子代理：`x-openai-subagent: review`
- 压缩子代理：`x-openai-subagent: compact`
- 自定义子代理：`x-openai-subagent: <custom_name>`

此头部有助于遥测和调试。

## 另请参阅

- [斜杠命令](./slash_commands.md) - 包括 `/review` 和 `/compact` 的交互式命令
- [配置](./config.md) - 模型和行为的配置选项
- [入门指南](./getting-started.md) - 基本使用和功能
