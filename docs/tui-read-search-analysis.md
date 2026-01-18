# TUI "Read" 和 "Search" 显示格式分析报告

## 概述

本报告分析了 Codex TUI（终端用户界面）中 "Read" 和 "Search" 字样的显示触发机制，包括相关的事件消息格式和数据流程。

## 核心发现

### 1. 显示效果

当 TUI 检测到某些命令被解析为文件读取或搜索操作时，会以简化的形式显示：

```
• Exploring
  └ Search shimmer_spans
    Read shimmer.rs, status_indicator_widget.rs
```

或者在完成后：

```
• Explored
  └ Search shimmer_spans
    Read shimmer.rs, status_indicator_widget.rs
```

### 2. 触发条件

"Read" 和 "Search" 字样的显示由以下条件触发：

1. **事件类型**：`ExecCommandBeginEvent` 或 `ExecCommandEndEvent`
2. **命令解析**：命令被解析为 `ParsedCommand::Read` 或 `ParsedCommand::Search` 类型
3. **探索模式**：所有命令都属于"探索类"命令（Read、Search、ListFiles）

## 事件消息格式

### ExecCommandBeginEvent 结构

```rust
pub struct ExecCommandBeginEvent {
    /// 用于配对 ExecCommandEnd 事件的标识符
    pub call_id: String,
    /// 底层 PTY 进程的标识符（可选）
    pub process_id: Option<String>,
    /// 此命令所属的轮次 ID
    pub turn_id: String,
    /// 要执行的命令
    pub command: Vec<String>,
    /// 命令的工作目录
    pub cwd: PathBuf,
    /// 解析后的命令列表 ← 关键字段
    pub parsed_cmd: Vec<ParsedCommand>,
    /// 命令来源
    pub source: ExecCommandSource,
}
```

### ParsedCommand 枚举

```rust
pub enum ParsedCommand {
    Read {
        cmd: String,      // 原始命令字符串
        name: String,     // 文件名（短名称）
        path: PathBuf,    // 文件完整路径
    },
    ListFiles {
        cmd: String,
        path: Option<String>,
    },
    Search {
        cmd: String,           // 原始命令字符串
        query: Option<String>, // 搜索查询内容
        path: Option<String>,  // 搜索路径
    },
    Unknown {
        cmd: String,
    },
}
```

## 命令解析规则

### Read（读取）命令

以下命令会被解析为 `ParsedCommand::Read`：

| 命令 | 示例 |
|------|------|
| `cat` | `cat webview/README.md` |
| `bat`/`batcat` | `bat --theme TwoDark README.md` |
| `less` | `less -p TODO README.md` |
| `more` | `more README.md` |
| `head` | `head -n 50 Cargo.toml` |
| `tail` | `tail -n +522 README.md` |
| `sed -n` | `sed -n '2000,2200p' tui/src/history_cell.rs` |
| `nl` | `nl -ba core/src/parse_command.rs` |
| `awk` | `awk '{print $1}' Cargo.toml` |

### Search（搜索）命令

以下命令会被解析为 `ParsedCommand::Search`：

| 命令 | 示例 |
|------|------|
| `rg`/`ripgrep` | `rg -n "TODO" src` |
| `grep`/`egrep`/`fgrep` | `grep -R TODO src` |
| `ag` (The Silver Searcher) | `ag TODO src` |
| `ack` | `ack TODO src` |
| `pt` (The Platinum Searcher) | `pt TODO src` |
| `git grep` | `git grep TODO src` |
| `fd` (带查询) | `fd main src` |
| `find` (带名称过滤) | `find . -name '*.rs'` |

## 数据流程

```
┌─────────────────────────────────────────────────────────────────────┐
│                          Agent/Core                                  │
│                                                                      │
│  1. 执行 shell 命令                                                  │
│  2. 调用 parse_command() 解析命令                                    │
│  3. 生成 ExecCommandBeginEvent { parsed_cmd: [...] }                │
└───────────────────────────────┬─────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────────┐
│                          TUI ChatWidget                              │
│                                                                      │
│  4. 接收 ExecCommandBeginEvent                                       │
│  5. 检查 parsed_cmd 是否全为探索类命令                               │
│  6. 创建或更新 ExecCell                                              │
└───────────────────────────────┬─────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────────┐
│                          ExecCell 渲染                               │
│                                                                      │
│  7. is_exploring_cell() 检查是否为探索模式                           │
│  8. 如果是，调用 exploring_display_lines()                           │
│  9. 根据 ParsedCommand 类型显示 "Read" 或 "Search"                  │
└─────────────────────────────────────────────────────────────────────┘
```

## 关键代码位置

### 1. 命令解析逻辑

**文件**: `codex-rs/core/src/parse_command.rs`

```rust
pub fn parse_command(command: &[String]) -> Vec<ParsedCommand> {
    // 解析并去重连续相同的命令
    let parsed = parse_command_impl(command);
    // ...
}
```

### 2. 探索模式判断

**文件**: `codex-rs/tui/src/exec_cell/model.rs`

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

### 3. 显示渲染逻辑

**文件**: `codex-rs/tui/src/exec_cell/render.rs`

```rust
fn exploring_display_lines(&self, width: u16) -> Vec<Line<'static>> {
    // 显示 "Exploring" 或 "Explored" 标题
    // ...
    for (title, line) in call_lines {
        // title 为 "Read"、"Search" 或 "List"
        let initial_indent = Line::from(vec![title.cyan(), " ".into()]);
        // ...
    }
}
```

## 示例：触发 "Read" 显示的完整事件

```json
{
    "type": "ExecCommandBegin",
    "call_id": "call-123",
    "turn_id": "turn-456",
    "command": ["bash", "-lc", "cat src/main.rs"],
    "cwd": "/home/user/project",
    "parsed_cmd": [
        {
            "type": "read",
            "cmd": "cat src/main.rs",
            "name": "main.rs",
            "path": "src/main.rs"
        }
    ],
    "source": "Agent"
}
```

## 示例：触发 "Search" 显示的完整事件

```json
{
    "type": "ExecCommandBegin",
    "call_id": "call-789",
    "turn_id": "turn-456",
    "command": ["bash", "-lc", "rg -n 'TODO' src"],
    "cwd": "/home/user/project",
    "parsed_cmd": [
        {
            "type": "search",
            "cmd": "rg -n TODO src",
            "query": "TODO",
            "path": "src"
        }
    ],
    "source": "Agent"
}
```

## 合并显示优化

当连续执行多个 Read 命令时，TUI 会将它们合并显示：

```
• Explored
  └ Search shimmer_spans
    Read shimmer.rs, status_indicator_widget.rs  ← 多个文件合并
```

这个优化在 `exploring_display_lines()` 方法中实现（简化示意）：

```rust
// 实际代码见: codex-rs/tui/src/exec_cell/render.rs
// 以下为简化的逻辑示意
if call.parsed.iter().all(|parsed| matches!(parsed, ParsedCommand::Read { .. })) {
    // 合并连续的 Read 命令
    while let Some(next) = calls.first() {
        if next.parsed.iter().all(|parsed| matches!(parsed, ParsedCommand::Read { .. })) {
            call.parsed.extend(next.parsed.clone());
            calls.remove(0);
        } else {
            break;
        }
    }
}
```

## 总结

TUI 中 "Read" 和 "Search" 的显示是由以下因素共同决定的：

1. **事件触发**: `ExecCommandBeginEvent` 包含 `parsed_cmd` 字段
2. **命令解析**: `parse_command()` 函数将原始命令解析为结构化的 `ParsedCommand`
3. **模式判断**: `is_exploring_call()` 判断是否为探索模式
4. **渲染逻辑**: `exploring_display_lines()` 将 ParsedCommand 类型映射为显示文本

只有当命令被解析为 `Read`、`Search` 或 `ListFiles` 类型，并且不是用户直接输入的 shell 命令时，才会触发简化的探索模式显示。
