# Codex Agent 可用工具分析报告

本报告详细分析了 Codex Agent 在运行时可以使用的各种工具。这些工具使得 Agent 能够执行代码编辑、命令运行、文件操作等任务。

> **源代码位置**: 工具定义主要位于 `codex-rs/core/src/tools/` 目录下

---

## 目录

1. [工具概述](#工具概述)
2. [核心工具](#核心工具)
   - [Shell 命令执行工具](#shell-命令执行工具)
   - [文件编辑工具](#文件编辑工具)
   - [计划管理工具](#计划管理工具)
   - [文件读取工具](#文件读取工具)
   - [目录列表工具](#目录列表工具)
   - [文件搜索工具](#文件搜索工具)
   - [图像查看工具](#图像查看工具)
3. [MCP 工具](#mcp-工具)
   - [MCP 资源列表工具](#mcp-资源列表工具)
   - [MCP 资源模板列表工具](#mcp-资源模板列表工具)
   - [MCP 资源读取工具](#mcp-资源读取工具)
   - [自定义 MCP 工具](#自定义-mcp-工具)
4. [网页搜索工具](#网页搜索工具)
5. [工具配置与启用条件](#工具配置与启用条件)
6. [总结](#总结)

---

## 工具概述

Codex Agent 的工具系统采用模块化设计，包含两种主要类型：

1. **函数工具 (Function Tools)**: 内置的核心工具，用于执行常见操作
2. **MCP 工具 (Model Context Protocol Tools)**: 通过 MCP 服务器扩展的外部工具

工具的配置和启用取决于多个因素：
- 所使用的模型类型
- 功能特性标志 (Feature Flags)
- 用户配置

---

## 核心工具

### Shell 命令执行工具

Agent 有多种执行 Shell 命令的方式：

#### 1. `shell` 工具

> **源码位置**: [`codex-rs/core/src/tools/spec.rs:259-321`](../codex-rs/core/src/tools/spec.rs#L259-L321)
>
> **处理程序**: [`codex-rs/core/src/tools/handlers/shell.rs`](../codex-rs/core/src/tools/handlers/shell.rs)
>
> **TUI 显示**: 在 TUI 中显示为带语法高亮的命令行

**描述**: 运行 Shell 命令并返回输出

**工具定义代码片段**:
```rust
ToolSpec::Function(ResponsesApiTool {
    name: "shell".to_string(),
    description: r#"Runs a shell command and returns its output.
- The arguments to `shell` will be passed to execvp(). Most terminal commands should be prefixed with ["bash", "-lc"].
- Always set the `workdir` param when using the shell function. Do not use `cd` unless absolutely necessary."#.to_string(),
    strict: false,
    parameters: JsonSchema::Object { ... },
})
```

**参数**:
| 参数名 | 类型 | 必填 | 描述 |
|--------|------|------|------|
| `command` | 字符串数组 | 是 | 要执行的命令及参数 |
| `workdir` | 字符串 | 否 | 命令执行的工作目录 |
| `timeout_ms` | 数字 | 否 | 命令超时时间（毫秒） |
| `sandbox_permissions` | 字符串 | 否 | 沙箱权限设置 |
| `justification` | 字符串 | 否 | 请求提升权限时的说明 |

**使用示例**:
- Linux/macOS: `["bash", "-lc", "ls -la"]`
- Windows: `["powershell.exe", "-Command", "Get-ChildItem -Force"]`

---

#### 2. `shell_command` 工具

> **源码位置**: [`codex-rs/core/src/tools/spec.rs:323-393`](../codex-rs/core/src/tools/spec.rs#L323-L393)
>
> **处理程序**: [`codex-rs/core/src/tools/handlers/shell.rs`](../codex-rs/core/src/tools/handlers/shell.rs)
>
> **TUI 显示**: 在 TUI 中显示为带语法高亮的命令行

**描述**: 在用户默认 Shell 中执行脚本命令

**工具定义代码片段**:
```rust
ToolSpec::Function(ResponsesApiTool {
    name: "shell_command".to_string(),
    description: r#"Runs a shell command and returns its output.
- Always set the `workdir` param when using the shell_command function. Do not use `cd` unless absolutely necessary."#.to_string(),
    strict: false,
    parameters: JsonSchema::Object { ... },
})
```

**参数**:
| 参数名 | 类型 | 必填 | 描述 |
|--------|------|------|------|
| `command` | 字符串 | 是 | 要执行的 Shell 脚本 |
| `workdir` | 字符串 | 否 | 命令执行的工作目录 |
| `login` | 布尔值 | 否 | 是否使用登录 Shell 语义（默认 true） |
| `timeout_ms` | 数字 | 否 | 命令超时时间（毫秒） |
| `sandbox_permissions` | 字符串 | 否 | 沙箱权限设置 |
| `justification` | 字符串 | 否 | 请求提升权限时的说明 |

---

#### 3. `exec_command` 工具（统一执行）

> **源码位置**: [`codex-rs/core/src/tools/spec.rs:134-212`](../codex-rs/core/src/tools/spec.rs#L134-L212)
>
> **处理程序**: [`codex-rs/core/src/tools/handlers/unified_exec.rs`](../codex-rs/core/src/tools/handlers/unified_exec.rs)
>
> **TUI 显示**: 在 TUI 中显示为 `Interacted with` 或 `Waited for` 前缀

**描述**: 在 PTY 中运行命令，返回输出或会话 ID 用于持续交互

**工具定义代码片段**:
```rust
ToolSpec::Function(ResponsesApiTool {
    name: "exec_command".to_string(),
    description: "Runs a command in a PTY, returning output or a session ID for ongoing interaction.".to_string(),
    strict: false,
    parameters: JsonSchema::Object { ... },
})
```

**参数**:
| 参数名 | 类型 | 必填 | 描述 |
|--------|------|------|------|
| `cmd` | 字符串 | 是 | 要执行的 Shell 命令 |
| `workdir` | 字符串 | 否 | 工作目录 |
| `shell` | 字符串 | 否 | Shell 二进制文件路径（默认 /bin/bash） |
| `login` | 布尔值 | 否 | 是否使用 -l/-i 语义（默认 true） |
| `yield_time_ms` | 数字 | 否 | 等待输出的时间（毫秒） |
| `max_output_tokens` | 数字 | 否 | 返回的最大 token 数量 |
| `sandbox_permissions` | 字符串 | 否 | 沙箱权限设置 |
| `justification` | 字符串 | 否 | 请求提升权限时的说明 |

---

#### 4. `write_stdin` 工具

> **源码位置**: [`codex-rs/core/src/tools/spec.rs:214-257`](../codex-rs/core/src/tools/spec.rs#L214-L257)
>
> **处理程序**: [`codex-rs/core/src/tools/handlers/unified_exec.rs`](../codex-rs/core/src/tools/handlers/unified_exec.rs)
>
> **TUI 显示**: 在 TUI 中显示为 `Interacted with ... sent ...`

**描述**: 向现有的统一执行会话写入字符并返回最近的输出

**工具定义代码片段**:
```rust
ToolSpec::Function(ResponsesApiTool {
    name: "write_stdin".to_string(),
    description: "Writes characters to an existing unified exec session and returns recent output.".to_string(),
    strict: false,
    parameters: JsonSchema::Object { ... },
})
```

**参数**:
| 参数名 | 类型 | 必填 | 描述 |
|--------|------|------|------|
| `session_id` | 数字 | 是 | 运行中的统一执行会话标识符 |
| `chars` | 字符串 | 否 | 要写入 stdin 的字符（可为空以轮询） |
| `yield_time_ms` | 数字 | 否 | 等待输出的时间（毫秒） |
| `max_output_tokens` | 数字 | 否 | 返回的最大 token 数量 |

---

### 文件编辑工具

#### `apply_patch` 工具

> **源码位置**: [`codex-rs/core/src/tools/handlers/apply_patch.rs:268-370`](../codex-rs/core/src/tools/handlers/apply_patch.rs#L268-L370)
>
> **语法定义**: [`codex-rs/core/src/tools/handlers/tool_apply_patch.lark`](../codex-rs/core/src/tools/handlers/tool_apply_patch.lark)
>
> **TUI 显示**: 显示为带颜色的 diff 摘要，绿色为添加，红色为删除

**描述**: 使用补丁格式编辑文件。这是一个自由格式工具，不需要将补丁包装在 JSON 中。

**工具定义代码片段 (Freeform 版本)**:
```rust
ToolSpec::Freeform(FreeformTool {
    name: "apply_patch".to_string(),
    description: "Use the `apply_patch` tool to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON.".to_string(),
    format: FreeformToolFormat {
        r#type: "grammar".to_string(),
        syntax: "lark".to_string(),
        definition: APPLY_PATCH_LARK_GRAMMAR.to_string(),
    },
})
```

**工具定义代码片段 (JSON 版本)**:
```rust
ToolSpec::Function(ResponsesApiTool {
    name: "apply_patch".to_string(),
    description: r#"Use the `apply_patch` tool to edit files.
Your patch language is a stripped‑down, file‑oriented diff format designed to be easy to parse and safe to apply..."#.to_string(),
    strict: false,
    parameters: JsonSchema::Object { ... },
})
```

**Lark 语法定义** (`tool_apply_patch.lark`):
```lark
start: begin_patch hunk+ end_patch
begin_patch: "*** Begin Patch" LF
end_patch: "*** End Patch" LF?

hunk: add_hunk | delete_hunk | update_hunk
add_hunk: "*** Add File: " filename LF add_line+
delete_hunk: "*** Delete File: " filename LF
update_hunk: "*** Update File: " filename LF change_move? change?

filename: /(.+)/
add_line: "+" /(.*)/ LF -> line

change_move: "*** Move to: " filename LF
change: (change_context | change_line)+ eof_line?
change_context: ("@@" | "@@ " /(.+)/) LF
change_line: ("+" | "-" | " ") /(.*)/ LF
eof_line: "*** End of File" LF

%import common.LF
```

**补丁语法结构**:
```
*** Begin Patch
[ 一个或多个文件操作 ]
*** End Patch
```

**支持的操作**:

1. **添加文件**: `*** Add File: <路径>`
2. **删除文件**: `*** Delete File: <路径>`
3. **更新文件**: `*** Update File: <路径>`
   - 可选移动: `*** Move to: <新路径>`
   - 修改块: `@@ [可选的上下文标识符]`

**行前缀**:
- ` ` (空格): 上下文行
- `-`: 删除行
- `+`: 添加行

**完整示例**:
```
*** Begin Patch
*** Add File: hello.txt
+Hello world
*** Update File: src/app.py
*** Move to: src/main.py
@@ def greet():
-print("Hi")
+print("Hello, world!")
*** Delete File: obsolete.txt
*** End Patch
```

---

### 计划管理工具

#### `update_plan` 工具

> **源码位置**: [`codex-rs/core/src/tools/handlers/plan.rs:20-60`](../codex-rs/core/src/tools/handlers/plan.rs#L20-L60)
>
> **TUI 显示**: 显示为计划列表，带有 ✓（已完成）、○（进行中）、⊘（待定）状态图标

**描述**: 更新任务计划。提供可选说明和计划项列表，每个项目包含步骤和状态。

**工具定义代码片段**:
```rust
pub static PLAN_TOOL: LazyLock<ToolSpec> = LazyLock::new(|| {
    ToolSpec::Function(ResponsesApiTool {
        name: "update_plan".to_string(),
        description: r#"Updates the task plan.
Provide an optional explanation and a list of plan items, each with a step and status.
At most one step can be in_progress at a time.
"#.to_string(),
        strict: false,
        parameters: JsonSchema::Object { ... },
    })
});
```

**系统提示词** (来自 `codex-rs/core/prompt.md:302-310`):
```markdown
## `update_plan`

A tool named `update_plan` is available to you. You can use it to keep an up‑to‑date, step‑by‑step plan for the task.

To create a new plan, call `update_plan` with a short list of 1‑sentence steps (no more than 5-7 words each) with a `status` for each step (`pending`, `in_progress`, or `completed`).

When steps have been completed, use `update_plan` to mark each finished step as `completed` and the next step you are working on as `in_progress`. There should always be exactly one `in_progress` step until everything is done. You can mark multiple items as complete in a single `update_plan` call.

If all steps are complete, ensure you call `update_plan` to mark all steps as `completed`.
```

**参数**:
| 参数名 | 类型 | 必填 | 描述 |
|--------|------|------|------|
| `explanation` | 字符串 | 否 | 对计划变更的说明 |
| `plan` | 数组 | 是 | 步骤列表 |

**计划项结构**:
| 字段 | 类型 | 描述 |
|------|------|------|
| `step` | 字符串 | 步骤描述 |
| `status` | 字符串 | 状态：`pending`、`in_progress` 或 `completed` |

**使用规则**:
- 同一时间最多只有一个步骤处于 `in_progress` 状态
- 用于跟踪复杂、多阶段任务的进度
- 不应用于简单的单步查询

---

### 文件读取工具

#### `read_file` 工具（实验性）

> **源码位置**: [`codex-rs/core/src/tools/spec.rs:531-627`](../codex-rs/core/src/tools/spec.rs#L531-L627)
>
> **处理程序**: [`codex-rs/core/src/tools/handlers/read_file.rs`](../codex-rs/core/src/tools/handlers/read_file.rs)
>
> **TUI 显示**: 不在 TUI 中单独显示，结果直接返回给模型

**描述**: 读取本地文件，支持 1 索引的行号，支持切片和缩进感知块模式

**工具定义代码片段**:
```rust
ToolSpec::Function(ResponsesApiTool {
    name: "read_file".to_string(),
    description: "Reads a local file with 1-indexed line numbers, supporting slice and indentation-aware block modes.".to_string(),
    strict: false,
    parameters: JsonSchema::Object { ... },
})
```

**参数**:
| 参数名 | 类型 | 必填 | 描述 |
|--------|------|------|------|
| `file_path` | 字符串 | 是 | 文件的绝对路径 |
| `offset` | 数字 | 否 | 开始读取的行号（必须 ≥ 1） |
| `limit` | 数字 | 否 | 返回的最大行数 |
| `mode` | 字符串 | 否 | 模式：`slice`（默认）或 `indentation` |
| `indentation` | 对象 | 否 | 缩进模式的配置 |

**缩进配置参数**:
| 参数名 | 类型 | 描述 |
|--------|------|------|
| `anchor_line` | 数字 | 锚点行（默认为 offset） |
| `max_levels` | 数字 | 要包含的父缩进级别数 |
| `include_siblings` | 布尔值 | 是否包含同级块 |
| `include_header` | 布尔值 | 是否包含文档注释或属性 |
| `max_lines` | 数字 | 返回行数的硬限制 |

---

### 目录列表工具

#### `list_dir` 工具（实验性）

> **源码位置**: [`codex-rs/core/src/tools/spec.rs:629-672`](../codex-rs/core/src/tools/spec.rs#L629-L672)
>
> **处理程序**: [`codex-rs/core/src/tools/handlers/list_dir.rs`](../codex-rs/core/src/tools/handlers/list_dir.rs)
>
> **TUI 显示**: 不在 TUI 中单独显示，结果直接返回给模型

**描述**: 列出本地目录中的条目，带有 1 索引的条目号和简单类型标签

**工具定义代码片段**:
```rust
ToolSpec::Function(ResponsesApiTool {
    name: "list_dir".to_string(),
    description: "Lists entries in a local directory with 1-indexed entry numbers and simple type labels.".to_string(),
    strict: false,
    parameters: JsonSchema::Object { ... },
})
```

**参数**:
| 参数名 | 类型 | 必填 | 描述 |
|--------|------|------|------|
| `dir_path` | 字符串 | 是 | 目录的绝对路径 |
| `offset` | 数字 | 否 | 开始列出的条目号（必须 ≥ 1，默认 1） |
| `limit` | 数字 | 否 | 返回的最大条目数（默认 25） |
| `depth` | 数字 | 否 | 最大遍历深度（必须 ≥ 1，默认 2） |

**输出格式**:
- 目录以 `/` 结尾
- 符号链接以 `@` 结尾
- 其他类型以 `?` 结尾
- 常规文件无后缀

---

### 文件搜索工具

#### `grep_files` 工具（实验性）

> **源码位置**: [`codex-rs/core/src/tools/spec.rs:481-529`](../codex-rs/core/src/tools/spec.rs#L481-L529)
>
> **处理程序**: [`codex-rs/core/src/tools/handlers/grep_files.rs`](../codex-rs/core/src/tools/handlers/grep_files.rs)
>
> **TUI 显示**: 不在 TUI 中单独显示，结果直接返回给模型

**描述**: 查找内容匹配模式的文件，并按修改时间列出

**工具定义代码片段**:
```rust
ToolSpec::Function(ResponsesApiTool {
    name: "grep_files".to_string(),
    description: "Finds files whose contents match the pattern and lists them by modification time.".to_string(),
    strict: false,
    parameters: JsonSchema::Object { ... },
})
```

**参数**:
| 参数名 | 类型 | 必填 | 描述 |
|--------|------|------|------|
| `pattern` | 字符串 | 是 | 要搜索的正则表达式模式 |
| `include` | 字符串 | 否 | 限制搜索文件的 glob 模式（如 `*.rs`） |
| `path` | 字符串 | 否 | 搜索的目录或文件路径 |
| `limit` | 数字 | 否 | 返回的最大文件路径数（默认 100） |

**实现说明**: 内部使用 `ripgrep (rg)` 进行搜索

---

### 图像查看工具

#### `view_image` 工具

> **源码位置**: [`codex-rs/core/src/tools/spec.rs:395-417`](../codex-rs/core/src/tools/spec.rs#L395-L417)
>
> **处理程序**: [`codex-rs/core/src/tools/handlers/view_image.rs`](../codex-rs/core/src/tools/handlers/view_image.rs)
>
> **TUI 显示**: 显示为 `• Viewed Image` 加上图像路径 (参见 [`codex-rs/tui/src/history_cell.rs:1652-1661`](../codex-rs/tui/src/history_cell.rs#L1652-L1661))

**描述**: 将本地图像（通过文件系统路径）附加到本轮的线程上下文中

**工具定义代码片段**:
```rust
ToolSpec::Function(ResponsesApiTool {
    name: "view_image".to_string(),
    description: "Attach a local image (by filesystem path) to the thread context for this turn.".to_string(),
    strict: false,
    parameters: JsonSchema::Object { ... },
})
```

**TUI 显示代码** (`history_cell.rs:1652-1661`):
```rust
pub(crate) fn new_view_image_tool_call(path: PathBuf, cwd: &Path) -> PlainHistoryCell {
    let display_path = display_path_for(&path, cwd);

    let lines: Vec<Line<'static>> = vec![
        vec!["• ".dim(), "Viewed Image".bold()].into(),
        vec!["  └ ".dim(), display_path.dim()].into(),
    ];

    PlainHistoryCell { lines }
}
```

**参数**:
| 参数名 | 类型 | 必填 | 描述 |
|--------|------|------|------|
| `path` | 字符串 | 是 | 图像文件的本地文件系统路径 |

---

## MCP 工具

MCP（Model Context Protocol）是一种协议，允许 Codex 连接到外部 MCP 服务器以扩展其功能。

> **连接管理**: [`codex-rs/core/src/mcp_connection_manager.rs`](../codex-rs/core/src/mcp_connection_manager.rs)
>
> **工具调用处理**: [`codex-rs/core/src/mcp_tool_call.rs`](../codex-rs/core/src/mcp_tool_call.rs)

### MCP 资源列表工具

#### `list_mcp_resources` 工具

> **源码位置**: [`codex-rs/core/src/tools/spec.rs:674-705`](../codex-rs/core/src/tools/spec.rs#L674-L705)
>
> **处理程序**: [`codex-rs/core/src/tools/handlers/mcp_resource.rs`](../codex-rs/core/src/tools/handlers/mcp_resource.rs)
>
> **TUI 显示**: 显示为 `/mcp` 命令输出格式

**描述**: 列出 MCP 服务器提供的资源。资源允许服务器共享为语言模型提供上下文的数据，如文件、数据库架构或应用程序特定信息。

**工具定义代码片段**:
```rust
ToolSpec::Function(ResponsesApiTool {
    name: "list_mcp_resources".to_string(),
    description: "Lists resources provided by MCP servers. Resources allow servers to share data that provides context to language models, such as files, database schemas, or application-specific information. Prefer resources over web search when possible.".to_string(),
    strict: false,
    parameters: JsonSchema::Object { ... },
})
```

**参数**:
| 参数名 | 类型 | 必填 | 描述 |
|--------|------|------|------|
| `server` | 字符串 | 否 | MCP 服务器名称。省略时列出所有配置服务器的资源 |
| `cursor` | 字符串 | 否 | 上一次调用返回的不透明游标，用于分页 |

---

### MCP 资源模板列表工具

#### `list_mcp_resource_templates` 工具

> **源码位置**: [`codex-rs/core/src/tools/spec.rs:707-738`](../codex-rs/core/src/tools/spec.rs#L707-L738)
>
> **处理程序**: [`codex-rs/core/src/tools/handlers/mcp_resource.rs`](../codex-rs/core/src/tools/handlers/mcp_resource.rs)
>
> **TUI 显示**: 显示为 `/mcp` 命令输出格式

**描述**: 列出 MCP 服务器提供的资源模板。参数化资源模板允许服务器共享接受参数的数据。

**工具定义代码片段**:
```rust
ToolSpec::Function(ResponsesApiTool {
    name: "list_mcp_resource_templates".to_string(),
    description: "Lists resource templates provided by MCP servers. Parameterized resource templates allow servers to share data that takes parameters and provides context to language models, such as files, database schemas, or application-specific information. Prefer resource templates over web search when possible.".to_string(),
    strict: false,
    parameters: JsonSchema::Object { ... },
})
```

**参数**:
| 参数名 | 类型 | 必填 | 描述 |
|--------|------|------|------|
| `server` | 字符串 | 否 | MCP 服务器名称。省略时列出所有配置服务器的资源模板 |
| `cursor` | 字符串 | 否 | 上一次调用返回的不透明游标，用于分页 |

---

### MCP 资源读取工具

#### `read_mcp_resource` 工具

> **源码位置**: [`codex-rs/core/src/tools/spec.rs:740-773`](../codex-rs/core/src/tools/spec.rs#L740-L773)
>
> **处理程序**: [`codex-rs/core/src/tools/handlers/mcp_resource.rs`](../codex-rs/core/src/tools/handlers/mcp_resource.rs)
>
> **TUI 显示**: 显示为 `/mcp` 命令输出格式

**描述**: 根据服务器名称和资源 URI 从 MCP 服务器读取特定资源

**工具定义代码片段**:
```rust
ToolSpec::Function(ResponsesApiTool {
    name: "read_mcp_resource".to_string(),
    description: "Read a specific resource from an MCP server given the server name and resource URI.".to_string(),
    strict: false,
    parameters: JsonSchema::Object { ... },
})
```

**参数**:
| 参数名 | 类型 | 必填 | 描述 |
|--------|------|------|------|
| `server` | 字符串 | 是 | MCP 服务器名称，必须与配置中的完全匹配 |
| `uri` | 字符串 | 是 | 要读取的资源 URI |

---

### 自定义 MCP 工具

> **MCP 工具处理程序**: [`codex-rs/core/src/tools/handlers/mcp.rs`](../codex-rs/core/src/tools/handlers/mcp.rs)
>
> **TUI 显示**: 显示为 `server__tool(args)` 格式，带有服务器名称和工具名称 (参见 [`codex-rs/tui/src/history_cell.rs:1719-1732`](../codex-rs/tui/src/history_cell.rs#L1719-L1732))

MCP 服务器可以提供自定义工具，这些工具在 Codex 启动时动态注册。工具名称使用完全限定格式：`<服务器名称>__<工具名称>`

**MCP 调用显示代码** (`history_cell.rs:1719-1732`):
```rust
fn format_mcp_invocation<'a>(invocation: McpInvocation) -> Line<'a> {
    let args_str = invocation
        .arguments
        .as_ref()
        .map(|v: &serde_json::Value| {
            serde_json::to_string(v).unwrap_or_else(|_| v.to_string())
        })
        .unwrap_or_default();

    let invocation_spans = vec![
        invocation.server.clone().cyan(),
        // ...
        invocation.tool.cyan(),
        // ...
    ];
    // ...
}
```

**配置位置**: `~/.codex/config.toml`

**配置示例**:
```toml
[mcp_servers.my_server]
command = "my-mcp-server"
args = ["--port", "3000"]
```

---

## 网页搜索工具

#### `web_search` 工具

> **源码位置**: 内置于 OpenAI Responses API
>
> **TUI 显示**: 作为内置工具结果显示

**描述**: 执行网页搜索以获取最新信息

**类型**: 内置工具（非函数工具）

**启用条件**:
- 需要启用 `WebSearchRequest` 或 `WebSearchCached` 功能特性
- `WebSearchCached` 模式下 `external_web_access` 设为 `false`
- `WebSearchRequest` 模式下 `external_web_access` 设为 `true`

---

## 工具配置与启用条件

> **配置源码**: [`codex-rs/core/src/tools/spec.rs:20-76`](../codex-rs/core/src/tools/spec.rs#L20-L76)

### 工具启用矩阵

| 工具名称 | 启用条件 |
|----------|----------|
| `shell` | `ShellTool` 功能启用且 `shell_type = Default` |
| `shell_command` | `ShellTool` 功能启用且 `shell_type = ShellCommand` |
| `exec_command` / `write_stdin` | `UnifiedExec` 功能启用 |
| `apply_patch` | `ApplyPatchFreeform` 功能启用或模型支持 |
| `update_plan` | 始终启用 |
| `view_image` | 始终启用 |
| `list_mcp_resources` | 始终启用 |
| `list_mcp_resource_templates` | 始终启用 |
| `read_mcp_resource` | 始终启用 |
| `grep_files` | 在模型的 `experimental_supported_tools` 中 |
| `read_file` | 在模型的 `experimental_supported_tools` 中 |
| `list_dir` | 在模型的 `experimental_supported_tools` 中 |
| `web_search` | `WebSearchRequest` 或 `WebSearchCached` 功能启用 |
| MCP 自定义工具 | 配置了相应的 MCP 服务器 |

### Shell 工具类型

根据配置和模型，Shell 工具有不同的变体：

| 类型 | 描述 |
|------|------|
| `Default` | 使用 `shell` 工具（命令作为数组） |
| `Local` | 使用 `local_shell` 工具 |
| `UnifiedExec` | 使用 `exec_command` 和 `write_stdin` 工具 |
| `ShellCommand` | 使用 `shell_command` 工具（命令作为字符串） |
| `Disabled` | 不启用 Shell 工具 |

---

## 总结

Codex Agent 提供了一套丰富的工具集，使其能够：

1. **执行命令**: 通过多种 Shell 工具变体运行系统命令
2. **编辑文件**: 使用 `apply_patch` 工具进行精确的文件修改
3. **读取文件**: 支持切片和缩进感知的文件读取（实验性）
4. **浏览目录**: 递归列出目录内容（实验性）
5. **搜索文件**: 使用正则表达式搜索文件内容（实验性）
6. **管理计划**: 跟踪和更新任务进度
7. **查看图像**: 将图像附加到对话上下文
8. **访问 MCP 资源**: 连接外部 MCP 服务器获取额外功能
9. **网页搜索**: 获取最新的网络信息（需启用）

这些工具的组合使得 Codex Agent 成为一个功能强大的编码助手，能够处理从简单的文件编辑到复杂的多步骤开发任务。

---

*报告生成时间: 2024*
*数据来源: `codex-rs/core/src/tools/` 目录下的源代码分析*

