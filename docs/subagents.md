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

## 如何创建自定义子代理

如果你想创建自己的子代理来处理专门的任务，可以按照以下步骤操作：

### 1. 实现 SessionTask Trait

首先，创建一个实现 `SessionTask` trait 的新结构体。例如：

```rust
use std::sync::Arc;
use async_trait::async_trait;
use codex_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;

use crate::codex::TurnContext;
use crate::state::TaskKind;
use crate::tasks::{SessionTask, SessionTaskContext};

// 定义你的自定义子代理
pub(crate) struct MyCustomTask {
    // 添加任何必要的配置字段
}

#[async_trait]
impl SessionTask for MyCustomTask {
    fn kind(&self) -> TaskKind {
        // 返回任务类型（可能需要在 TaskKind 枚举中添加新变体）
        TaskKind::Regular
    }

    async fn run(
        self: Arc<Self>,
        session: Arc<SessionTaskContext>,
        ctx: Arc<TurnContext>,
        input: Vec<UserInput>,
        cancellation_token: CancellationToken,
    ) -> Option<String> {
        // 实现你的子代理逻辑
        // 这里可以：
        // 1. 创建一个新的配置
        // 2. 设置自定义的系统提示词
        // 3. 使用 run_codex_conversation_one_shot 启动子代理对话
        
        None
    }
}
```

### 2. 使用子代理委托功能

使用 `run_codex_conversation_one_shot` 函数来启动子代理会话：

```rust
use crate::codex_delegate::run_codex_conversation_one_shot;

async fn start_custom_conversation(
    session: Arc<SessionTaskContext>,
    ctx: Arc<TurnContext>,
    input: Vec<UserInput>,
    cancellation_token: CancellationToken,
) -> Option<async_channel::Receiver<Event>> {
    let config = ctx.client.config();
    let mut sub_agent_config = config.as_ref().clone();
    
    // 自定义配置
    sub_agent_config.user_instructions = None;
    sub_agent_config.base_instructions = Some("你的自定义系统提示词".to_string());
    
    // 启动子代理对话
    (run_codex_conversation_one_shot(
        sub_agent_config,
        session.auth_manager(),
        input,
        session.clone_session(),
        ctx.clone(),
        cancellation_token,
        None,
    )
    .await)
        .ok()
        .map(|io| io.rx_event)
}
```

### 3. 使用 SubAgentSource::Other 标识

当创建子代理会话时，使用 `SubAgentSource::Other("your-agent-name")` 来标识你的自定义子代理：

```rust
use codex_protocol::protocol::{SessionSource, SubAgentSource};

// 在 Codex::spawn 中使用
SessionSource::SubAgent(SubAgentSource::Other("my-custom-agent".to_string()))
```

### 4. 处理子代理事件

订阅并处理子代理发出的事件：

```rust
async fn process_custom_events(
    session: Arc<SessionTaskContext>,
    ctx: Arc<TurnContext>,
    receiver: async_channel::Receiver<Event>,
) {
    while let Ok(event) = receiver.recv().await {
        match event.msg {
            EventMsg::AgentMessage(msg) => {
                // 处理代理消息
                session.clone_session().send_event(ctx.as_ref(), EventMsg::AgentMessage(msg)).await;
            }
            EventMsg::TaskComplete(_) => {
                // 任务完成
                break;
            }
            _ => {
                // 转发其他事件
                session.clone_session().send_event(ctx.as_ref(), event.msg).await;
            }
        }
    }
}
```

### 5. 注册和触发你的子代理

将你的子代理集成到 Codex 中：

```rust
// 在适当的位置调用你的任务
session.spawn_task(
    turn_context,
    input,
    MyCustomTask { /* 配置 */ }
).await;
```

### 实践示例参考

查看现有子代理的实现作为参考：

- **审查子代理**：`codex-rs/core/src/tasks/review.rs`
  - 展示了如何设置自定义系统提示词
  - 如何处理结构化输出
  - 如何过滤和转发事件

- **压缩子代理**：`codex-rs/core/src/tasks/compact.rs`
  - 展示了更简单的子代理实现
  - 如何调用内部函数来完成特定任务

### 关键考虑事项

1. **配置隔离**：每个子代理应该有自己的配置副本，避免影响主会话
2. **事件处理**：决定哪些事件应该转发给父会话，哪些应该在子代理内部处理
3. **取消令牌**：正确处理 `cancellation_token`，确保子代理可以被优雅地取消
4. **错误处理**：妥善处理子代理执行过程中可能出现的错误
5. **遥测标识**：使用有意义的名称作为 `SubAgentSource::Other` 的参数，便于调试和遥测

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
