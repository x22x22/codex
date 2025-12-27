# Codex Responses API 深度分析

本文档深入分析了 Codex 在使用 `wire_api = "responses"` 模式时的内部机制、UI 行为以及与上游 LLM 服务交互可能出现的问题。

## 目录

1. [内置工具识别机制](#1-内置工具识别机制)
2. [Explore 操作合并机制](#2-explore-操作合并机制)
3. [导致 Explore 不合并的报文问题](#3-导致-explore-不合并的报文问题)
4. [Loading 指示器状态管理](#4-loading-指示器状态管理)
5. [上游 Responses API 故障场景](#5-上游-responses-api-故障场景)

---

## 1. 内置工具识别机制

### 核心发现

Codex 对于内置工具（如 `web_search`/`web.run`）的识别**不是动态发现的，而是硬编码的**。

### 实现细节

- **工具定义位置**：`core/src/client_common.rs:198`
  - `web_search` 被定义为 `ToolSpec::WebSearch {}`
  
- **启用方式**：通过配置文件启用
  ```toml
  [features]
  web_search_request = true
  ```

- **添加流程**：`core/src/tools/spec.rs:1096-1098`
  - 当 `features.web_search_request` 启用时，Codex 将该工具添加到工具数组中
  
- **序列化格式**：发送给 LLM 提供商的格式为
  ```json
  {"type": "web_search"}
  ```

### 提供商兼容性

- **OpenAI Responses API**：原生支持 `web_search`，可以正常处理
- **其他提供商**：可能不支持或直接拒绝该工具

### 为什么不同提供商表现不同

Codex 采用**声明式方法**：
1. 根据用户配置声明要使用哪些工具
2. 将工具列表发送给 LLM 提供商
3. 提供商要么支持并处理，要么返回错误

**没有能力协商或动态发现机制**，这解释了为什么 `web_search` 只在 OpenAI 模型中可用。

---

## 2. Explore 操作合并机制

### 什么时候多个 Explore 会合并显示

在 TUI 中，多个探索性命令可以被智能地合并成一个 "Exploring" 块，例如：

```
• Exploring
  └ List src/
    Read auth.rs, shimmer.rs, config.rs
    Search "web_search" in core/
```

### 合并条件

#### 2.1 Exploring Cell 的判断条件

**代码位置**：`codex-rs/tui/src/exec_cell/model.rs:105-107`

**源码**：
```rust
pub(crate) fn is_exploring_cell(&self) -> bool {
    self.calls.iter().all(Self::is_exploring_call)
}
```

**分析**：一个 cell 被判断为 "exploring cell" 需要满足：
- 所有的调用 (calls) 都必须是 "exploring call"（通过 `all()` 方法检查）

#### 2.2 Exploring Call 的定义

**代码位置**：`codex-rs/tui/src/exec_cell/model.rs:128-139`

**源码**：
```rust
pub(super) fn is_exploring_call(call: &ExecCall) -> bool {
    !matches!(call.source, ExecCommandSource::UserShell)
        && !call.parsed.is_empty()
        && call.parsed.iter().all(|p| {
            matches!(
                p,
                ParsedCommand::Read { .. }
                    | ParsedCommand::ListFiles { .. }
                    | ParsedCommand::Search { .. }
            )
        })
}
```

**分析**：一个调用是 "exploring call" 需要满足三个条件（使用 `&&` 连接）：
1. **不是用户 shell 命令**：`!matches!(call.source, ExecCommandSource::UserShell)`
2. **parsed 不为空**：`!call.parsed.is_empty()`
3. **所有 parsed 命令都是探索性类型**：使用 `all()` 确保每个命令都是：
   - `ParsedCommand::Read { .. }`
   - `ParsedCommand::ListFiles { .. }`
   - `ParsedCommand::Search { .. }`

#### 2.3 多个调用合并的条件

**代码位置**：`codex-rs/tui/src/exec_cell/model.rs:42-68`（`with_added_call` 方法）

**源码**：
```rust
pub(crate) fn with_added_call(
    &self,
    call_id: String,
    command: Vec<String>,
    parsed: Vec<ParsedCommand>,
    source: ExecCommandSource,
    interaction_input: Option<String>,
) -> Option<Self> {
    let call = ExecCall {
        call_id,
        command,
        parsed,
        output: None,
        source,
        start_time: Some(Instant::now()),
        duration: None,
        interaction_input,
    };
    if self.is_exploring_cell() && Self::is_exploring_call(&call) {
        Some(Self {
            calls: [self.calls.clone(), vec![call]].concat(),
            animations_enabled: self.animations_enabled,
        })
    } else {
        None
    }
}
```

**分析**：当满足以下条件时，新的调用会被添加到当前 cell 中：
1. **当前 cell 已经是 exploring cell**：`self.is_exploring_cell()` 返回 true
2. **新添加的 call 也是 exploring call**：`Self::is_exploring_call(&call)` 返回 true
3. **返回新的 ExecCell**：将新 call 添加到 calls 列表中（`[self.calls.clone(), vec![call]].concat()`）
4. **如果条件不满足**：返回 `None`，调用者会创建新的 cell

#### 2.4 显示时的合并逻辑

**代码位置**：`codex-rs/tui/src/exec_cell/render.rs:271-292`

**源码**：
```rust
let mut calls = self.calls.clone();
let mut out_indented = Vec::new();
while !calls.is_empty() {
    let mut call = calls.remove(0);
    if call
        .parsed
        .iter()
        .all(|parsed| matches!(parsed, ParsedCommand::Read { .. }))
    {
        while let Some(next) = calls.first() {
            if next
                .parsed
                .iter()
                .all(|parsed| matches!(parsed, ParsedCommand::Read { .. }))
            {
                call.parsed.extend(next.parsed.clone());
                calls.remove(0);
            } else {
                break;
            }
        }
    }
    // ... 继续处理
}
```

**分析**：在渲染时还会进行进一步的合并：
- **检查是否全是 Read 操作**：使用 `all()` 确保 call 中所有 parsed 都是 `ParsedCommand::Read`
- **合并连续的 Read 调用**：如果下一个 call 也全是 Read，则使用 `extend()` 合并到当前 call
- **去重和显示**：后续代码会去重文件名（使用 `.unique()`）
- **最终格式**：以逗号分隔的方式显示：`Read auth.rs, shimmer.rs`

### 不会合并的情况

以下情况会导致创建新的独立 cell：
- 用户手动运行的 shell 命令（`UserShell` source）
- 非探索性命令（如编译、运行测试等）
- 中间插入了非 exploring 类型的调用

### 设计目的

这种设计让 UI 更简洁，避免显示大量重复的"正在读取文件"信息，提升用户体验。

---

## 3. 导致 Explore 不合并的报文问题

### 事件处理流程

**代码位置**：`codex-rs/tui/src/chatwidget.rs:1302-1361`（`handle_exec_begin_now` 方法）

TUI 接收到 `ExecCommandBeginEvent` 消息后，会尝试将其合并到当前活跃的 exploring cell 中：

```rust
if let Some(cell) = self.active_cell.as_mut()
    .and_then(|c| c.as_any_mut().downcast_mut::<ExecCell>())
    && let Some(new_exec) = cell.with_added_call(...)
{
    *cell = new_exec;  // 成功合并
} else {
    self.flush_active_cell();  // 失败，创建新 cell
}
```

### 导致合并失败的原因

#### 3.1 `parsed_cmd` 为空

**代码位置**：`codex-rs/tui/src/exec_cell/model.rs:130`

**源码**（`is_exploring_call` 函数的第二个条件）：
```rust
pub(super) fn is_exploring_call(call: &ExecCall) -> bool {
    !matches!(call.source, ExecCommandSource::UserShell)
        && !call.parsed.is_empty()  // ← 这里检查 parsed 不为空
        && call.parsed.iter().all(|p| {
            matches!(
                p,
                ParsedCommand::Read { .. }
                    | ParsedCommand::ListFiles { .. }
                    | ParsedCommand::Search { .. }
            )
        })
}
```

**分析**：如果 `ExecCommandBeginEvent.parsed_cmd` 是空数组 `[]`：
- 第二个条件 `!call.parsed.is_empty()` 会返回 `false`
- 整个 `is_exploring_call` 返回 `false`（因为使用 `&&` 连接）
- 无法合并，会创建新的独立 cell

**错误报文示例**：
```json
{
  "type": "exec_command_begin",
  "call_id": "call-3",
  "command": ["bash", "-lc", "cat auth.rs"],
  "parsed_cmd": [],  // ❌ 空的 parsed_cmd
  "source": "agent"
}
```

#### 3.2 `parsed_cmd` 包含非探索性命令

**代码位置**：`codex-rs/tui/src/exec_cell/model.rs:131-138`

**源码**（`is_exploring_call` 函数的第三个条件）：
```rust
pub(super) fn is_exploring_call(call: &ExecCall) -> bool {
    !matches!(call.source, ExecCommandSource::UserShell)
        && !call.parsed.is_empty()
        && call.parsed.iter().all(|p| {  // ← 这里检查所有命令都是探索性类型
            matches!(
                p,
                ParsedCommand::Read { .. }
                    | ParsedCommand::ListFiles { .. }
                    | ParsedCommand::Search { .. }
            )
        })
}
```

**分析**：必须全部是 `Read`、`ListFiles` 或 `Search` 类型：
- 使用 `all()` 确保每个 parsed 命令都匹配这三种类型之一
- 如果包含 `Unknown` 或其他类型，`all()` 返回 `false`
- 不满足 exploring call 的条件，无法合并

**错误报文示例**：
```json
{
  "type": "exec_command_begin",
  "parsed_cmd": [
    {"type": "unknown", "cmd": "cat auth.rs"}  // ❌ Unknown 类型
  ],
  "source": "agent"
}
```

#### 3.3 `source` 字段为 `UserShell`

**代码位置**：`codex-rs/tui/src/exec_cell/model.rs:129`

**源码**（`is_exploring_call` 函数的第一个条件）：
```rust
pub(super) fn is_exploring_call(call: &ExecCall) -> bool {
    !matches!(call.source, ExecCommandSource::UserShell)  // ← 这里检查不是用户命令
        && !call.parsed.is_empty()
        && call.parsed.iter().all(|p| {
            matches!(
                p,
                ParsedCommand::Read { .. }
                    | ParsedCommand::ListFiles { .. }
                    | ParsedCommand::Search { .. }
            )
        })
}
```

**分析**：如果 `ExecCommandBeginEvent.source = "user_shell"`：
- 第一个条件 `!matches!(call.source, ExecCommandSource::UserShell)` 返回 `false`
- 整个函数短路返回 `false`（因为使用 `&&` 连接）
- 即使命令是探索性的，也不会合并
- 用户手动命令总是单独显示

#### 3.4 中间有非 Exploring 事件打断

如果两个 exploring 命令之间收到了其他类型的事件：
- 非 exploring 的 `ExecCommandBegin` 事件
- `AgentMessage` 等其他事件导致 `flush_active_cell()` 被调用
- 会终止当前的 exploring cell，下一个命令会创建新 cell

### 正确的报文结构

**能够成功合并的连续报文**：

```json
// 第一个命令
{
  "type": "exec_command_begin",
  "call_id": "call-1",
  "command": ["bash", "-lc", "ls src/"],
  "parsed_cmd": [
    {"type": "list_files", "cmd": "ls src/", "path": "src/"}
  ],
  "source": "agent"
}

// 第二个命令（会合并）
{
  "type": "exec_command_begin", 
  "call_id": "call-2",
  "command": ["bash", "-lc", "cat auth.rs"],
  "parsed_cmd": [
    {"type": "read", "cmd": "cat auth.rs", "name": "auth.rs", "path": "auth.rs"}
  ],
  "source": "agent"
}
```

### 关键字段要求

必须保证：
1. ✅ `parsed_cmd` 不为空
2. ✅ 所有 `parsed_cmd` 都是 `read`/`list_files`/`search` 类型
3. ✅ `source` 不是 `user_shell`
4. ✅ 连续的 exploring 命令之间没有其他事件打断

---

## 4. Loading 指示器状态管理

### 正常的 Loading 显示流程

**代码位置**：`codex-rs/tui/src/chatwidget.rs:561-582`

TUI 显示 loading/working 指示器由 `TaskStartedEvent` 和 `TaskCompleteEvent` 控制：

#### 开始显示（第 561-570 行）

**源码**：
```rust
fn on_task_started(&mut self) {
    self.bottom_pane.clear_ctrl_c_quit_hint();
    self.bottom_pane.set_task_running(true);  // ← 关键：设置为 true
    self.retry_status_header = None;
    self.bottom_pane.set_interrupt_hint_visible(true);
    self.set_status_header(String::from("Working"));
    self.full_reasoning_buffer.clear();
    self.reasoning_buffer.clear();
    self.request_redraw();
}
```

**分析**：收到 `TaskStartedEvent` 后的处理流程：
1. 调用 `on_task_started()`
2. 执行 `self.bottom_pane.set_task_running(true)` ← **这是关键**
3. 内部创建 `StatusIndicatorWidget`（底部的 spinner/loading indicator）
4. 设置状态头为 "Working"
5. 请求重绘界面

#### 停止显示（第 572-582 行）

**源码**：
```rust
fn on_task_complete(&mut self, last_agent_message: Option<String>) {
    // If a stream is currently active, finalize it.
    self.flush_answer_stream_with_separator();
    self.flush_wait_cell();
    // Mark task stopped and request redraw now that all content is in history.
    self.bottom_pane.set_task_running(false);  // ← 关键：设置为 false
    self.running_commands.clear();
    self.suppressed_exec_calls.clear();
    self.last_unified_wait = None;
    self.request_redraw();
    // ... 更多代码
}
```

**分析**：收到 `TaskCompleteEvent` 后的处理流程：
1. 调用 `on_task_complete()`
2. 刷新活跃的流和 cell
3. 执行 `self.bottom_pane.set_task_running(false)` ← **这是关键**
4. 清理运行状态（running_commands, suppressed_exec_calls 等）
5. 请求重绘界面，隐藏 status indicator

### Loading 不显示但 Agent 仍在工作的情况

#### 4.1 缺少 `TaskStartedEvent`

**原因分析**：如果 agent 开始新一轮请求时，**没有发送 `TaskStartedEvent`**：
- TUI 不会调用 `on_task_started()`
- `set_task_running(true)` 不会被执行
- `StatusIndicatorWidget` 不会被创建/显示
- **结果**：用户看不到 loading 样式，但 agent 可能正在后台与 LLM 通信

#### 4.2 提前发送 `TaskCompleteEvent`

**源码引用**：
```rust
fn on_task_complete(&mut self, last_agent_message: Option<String>) {
    // ...
    self.bottom_pane.set_task_running(false);  // 第 577 行
    // ...
}
```

**原因分析**：如果在 agent 还需要继续工作时，错误地发送了 `TaskCompleteEvent`：
- 第 577 行会执行 `set_task_running(false)`
- Loading 消失，但后续的 LLM 请求没有对应的 `TaskStartedEvent`
- **结果**：造成"无 loading 但在工作"的状态

#### 4.3 意外的状态重置

**代码位置**：`codex-rs/tui/src/chatwidget.rs:370-374`

某些事件处理可能错误地隐藏了 status indicator：
- 第 1087 行：`on_undo_completed` 会隐藏 indicator
- 第 1115 行：commit tick 时如果 cell 存在会隐藏
- 第 365 行：`set_task_running(false)` 被调用

如果这些在 agent 仍工作时被触发，loading 会消失。

#### 4.4 MCP 初始化完成但任务未开始

**代码位置**：`codex-rs/tui/src/chatwidget.rs:798-801`

- `on_mcp_startup_complete` 会调用 `set_task_running(false)`
- 如果 MCP 初始化结束后，agent 立即开始新任务但没有发送新的 `TaskStartedEvent`
- 会出现短暂的"无 loading"状态

#### 4.5 中断/错误处理路径

**代码位置**：`codex-rs/tui/src/chatwidget.rs:707-716`

- `finalize_turn()` 会调用 `set_task_running(false)`（第 711 行）
- 如果在 agent 正常工作时触发了此方法（比如某些错误处理）
- Loading 会消失

### 核心控制机制

**代码位置**：`codex-rs/tui/src/bottom_pane/mod.rs:344-367`

`is_task_running` 布尔标志控制 loading 显示：
- `true` → 显示 `StatusIndicatorWidget`
- `false` → 隐藏 indicator

**如果这个标志与 agent 实际状态不同步，就会出现"无 loading 但在工作"的情况。**

### 导致 Loading 不显示的根本原因

必须保证的事件序列：
```
TaskStartedEvent → (各种工作事件) → TaskCompleteEvent
TaskStartedEvent → (更多工作) → TaskCompleteEvent
...
```

问题根源：
1. ❌ **漏发 `TaskStartedEvent`** - agent 开始新轮次时没有通知 TUI
2. ❌ **重复发 `TaskCompleteEvent`** - 任务还未真正完成就发送了 complete 事件
3. ❌ **意外的状态重置** - 其他事件处理误调用了 `set_task_running(false)`
4. ❌ **事件顺序错误** - Complete 在 Started 之前，或多个 Started 没有对应的 Complete

这种不同步会导致用户体验问题：**看起来 idle 但实际上 agent 在后台默默工作**。

---

## 5. 上游 Responses API 故障场景

### SSE 事件流完整性分析

**代码位置**：
- `codex-rs/codex-api/src/sse/responses.rs`
- `codex-rs/core/src/codex.rs:2580-2628`

Codex 的 `TaskStartedEvent` 和 `TaskCompleteEvent` 是在本地生成的，但依赖 Responses API 的完整 SSE 事件流来驱动状态机。

### 上游 Bug 导致的问题

#### 5.1 缺少 `response.completed` 事件（最关键）

**代码位置**：`codex-rs/codex-api/src/sse/responses.rs:261-275` 和 `codex-rs/core/src/codex.rs:2616-2628`

**SSE 处理源码**：
```rust
"response.completed" => {
    if let Some(resp_val) = event.response {
        match serde_json::from_value::<ResponseCompleted>(resp_val) {
            Ok(r) => {
                response_completed = Some(r);  // ← 设置 completed 标志
            }
            Err(e) => {
                let error = format!("failed to parse ResponseCompleted: {e}");
                debug!(error);
                response_error = Some(ApiError::Stream(error));
                continue;
            }
        };
    };
}
```

**响应循环源码**：
```rust
ResponseEvent::Completed {
    response_id: _,
    token_usage,
} => {
    sess.update_token_usage_info(&turn_context, token_usage.as_ref())
        .await;
    should_emit_turn_diff = true;

    break Ok(TurnRunResult {  // ← 只有收到 Completed 才 break
        needs_follow_up,
        last_agent_message,
    });
}
```

**问题描述**：
- 流必须以 `response.completed` 结束
- 如果 API 只发送了 `output_item.done` 事件但从不发送 `completed`
- Codex 的响应循环永远不会执行 `break`
- `TurnRunResult` 永不返回 → `TaskCompleteEvent` 永不发送

**影响**：
- ⚠️ **Loading spinner 会一直转**，即使服务端已停止发送事件
- 用户看到永久的 "Working" 状态

#### 5.2 流提前终止无优雅关闭

**代码位置**：`codex-rs/codex-api/src/sse/responses.rs:486-489`

**问题描述**：
- 网络断开或服务端崩溃导致 SSE 连接关闭
- 没有发送 `response.completed`
- Codex 检测到流关闭，返回错误：`"stream closed before response.completed"`
- `TaskCompleteEvent` 可能已经发送或未发送，取决于终止时机

**影响**：
- ⚠️ 状态不一致：UI 以为任务完成，但实际连接已断
- ⚠️ Loading 可能持续或突然消失

#### 5.3 `local_shell_call` 数据不完整

**代码位置**：
- `codex-rs/protocol/src/models.rs:133+`（ResponseItem 定义）
- `codex-rs/tui/src/exec_cell/model.rs:130`（parsed 检查）

**ResponseItem 定义片段**（注释说明了预期格式）：
```rust
// Emitted by the Responses API when the agent triggers a web search.
// Example payload (from SSE `response.output_item.done`):
// {
//   "id":"ws_...",
//   "type":"web_search_call",
//   "status":"completed",
//   "action": {"type":"search","query":"weather: San Francisco, CA"}
// }
WebSearchCall {
    #[serde(default, skip_serializing)]
    #[ts(skip)]
    id: Option<String>,
    // ...
}
```

**问题描述**：
API 返回的 `local_shell_call` 项中 `action.command` 为空数组或格式错误：
```json
{
  "type": "response.output_item.done",
  "item": {
    "type": "local_shell_call",
    "call_id": "call-1",
    "status": "completed",
    "action": {
      "type": "exec",
      "command": []  // ❌ 空数组
    }
  }
}
```

**源码引用**（检查 parsed 不为空）：
```rust
pub(super) fn is_exploring_call(call: &ExecCall) -> bool {
    !matches!(call.source, ExecCommandSource::UserShell)
        && !call.parsed.is_empty()  // ← 如果 command 为空，parsed 也为空
        && call.parsed.iter().all(|p| {
            // ...
        })
}
```

**分析**：
- Codex 解析时会得到空的 command
- `parsed_cmd` 为空
- `is_exploring_call` 返回 `false`（因为 `!call.parsed.is_empty()` 为 `false`）
- 不满足 exploring call 的条件

**影响**：
- ⚠️ Explore 操作显示为独立的单个 cell，无法合并

#### 5.4 事件顺序错误

**代码位置**：
- `codex-rs/codex-api/src/sse/responses.rs:192-202`（output_item.done）
- `codex-rs/codex-api/src/sse/responses.rs:276-286`（output_item.added）

**SSE 事件处理源码**：
```rust
"response.output_item.done" => {
    let Some(item_val) = event.item else { continue };
    let Ok(item) = serde_json::from_value::<ResponseItem>(item_val) else {
        debug!("failed to parse ResponseItem from output_item.done");
        continue;
    };

    let event = ResponseEvent::OutputItemDone(item);
    if tx_event.send(Ok(event)).await.is_err() {
        return;
    }
}
// ...
"response.output_item.added" => {
    let Some(item_val) = event.item else { continue };
    let Ok(item) = serde_json::from_value::<ResponseItem>(item_val) else {
        debug!("failed to parse ResponseItem from output_item.done");
        continue;
    };

    let event = ResponseEvent::OutputItemAdded(item);
    if tx_event.send(Ok(event)).await.is_err() {
        return;
    }
}
```

**问题描述**：
API 错误地交错发送事件，例如：
```
response.output_item.done (local_shell_call #1)
response.output_text.delta (assistant message)  ← 错误插入
response.output_item.done (local_shell_call #2)
```

**分析**：
- 两个事件处理器独立工作，按接收顺序处理
- 中间的 `output_text.delta` 会被发送到 TUI
- TUI 收到 `AgentMessage` 相关事件时会调用 `flush_active_cell()`
- 打断了 explore 序列，第二个 call 创建新 cell

**影响**：
- ⚠️ Explore 操作无法合并，尽管它们本应是连续的探索性调用
response.output_text.delta (assistant message)  ← 错误插入
response.output_item.done (local_shell_call #2)
```

中间的 `output_text.delta` 会触发 `flush_active_cell()`，打断 explore 序列。

**影响**：
- ⚠️ Explore 操作无法合并，尽管它们本应是连续的探索性调用

#### 5.5 缺少 `output_item.done` 事件

**正常流程**：`output_item.added` → `output_item.done`

**问题描述**：
如果 API 只发送 `added` 没有发送对应的 `done`：
- Codex 的 `active_item` 永远不会被标记为完成
- 下一个命令开始时会强制调用 `flush_active_cell()`
- 打断当前的 exploring cell

**影响**：
- ⚠️ 破坏 explore 合并序列

#### 5.6 重复或提前的 `response.completed`

**问题描述**：
- API 发送多个 `completed` 事件
- 或者在所有 items 完成前就发送 `completed`

**影响**：
- 第一次 `completed` 会 break 响应循环并发送 `TaskCompleteEvent`
- 后续工作发生时没有对应的 `TaskStartedEvent`
- ⚠️ Loading 指示器消失但 agent 继续工作

#### 5.7 工具调用无限循环无状态反馈

**问题描述**：
- 模型调用 tool → Codex 执行 → POST 结果回去
- API 响应新请求但从不发送任何 response 的 `completed`
- `needs_follow_up = true` 使 Codex 保持在循环中
- 每次迭代可能正确或不正确地发送 `TaskStartedEvent`

**影响**：
- ⚠️ 要么永久 loading，要么"静默工作"没有 UI 反馈

#### 5.8 Rate Limit 事件但无完成事件

**问题描述**：
API 发送 `response.rate_limits` 但从不发送 `response.completed`：
```
response.created
response.rate_limits {...}
[never sends response.completed]
```

**影响**：
- Codex 更新 rate limits 但无限期等待 completion
- ⚠️ 永久 loading 状态，无进展

### 根本原因模式

上游 Responses API 可能存在的问题模式：

1. **不完整的事件序列**
   - 缺少关键事件如 `response.completed`
   - 导致状态机无法正确转换

2. **事件顺序违规**
   - 交错发送打破 Codex 状态机的假设
   - 导致 cell 合并失败或状态混乱

3. **格式错误的载荷**
   - `local_shell_call` 或 `function_call` 项中的空数据或不完整数据
   - 导致命令解析失败

4. **网络可靠性问题**
   - 连接提前断开但没有适当的错误信号
   - 导致状态不一致

5. **协议违规**
   - 多次 completion、缺少 items 或不一致的状态转换
   - 导致 UI 状态与实际工作状态脱节

### 影响总结

这些上游问题会直接导致前面分析的 UI 问题，因为 **Codex 的架构假设 SSE 流是完整且格式正确的**。任何偏差都需要防御性处理，但这可能无法完全保持 UI 状态的一致性。

---

## 总结

本文档分析了 Codex 在使用 Responses API 时的五个关键方面：

1. **内置工具**采用硬编码+声明式方法，没有动态发现
2. **Explore 合并**需要满足严格的条件（探索性命令、正确的 source、无中断）
3. **报文格式**必须包含完整的 `parsed_cmd` 且类型正确
4. **Loading 状态**依赖 TaskStarted/Complete 事件的正确配对
5. **上游 API** 的任何不完整或错误都会导致 UI 状态问题

理解这些机制有助于诊断和解决实际使用中遇到的问题。
