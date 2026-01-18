# TUI "Read" 和 "Search" 显示格式分析报告

## 概述

本报告分析了 Codex TUI（终端用户界面）中 "Read" 和 "Search" 字样的显示触发机制，包括相关的事件消息格式和数据流程。

---

## 解析技术说明

### 使用的解析技术

Codex 使用**混合解析策略**，结合了以下两种技术：

1. **AST 解析（抽象语法树）**：使用 `tree-sitter-bash` 库解析 bash 脚本
2. **模式匹配 + 手工解析**：使用 Rust 的 `match` 语句和 `shlex` 库进行命令词元分析

```
┌─────────────────────────────────────────────────────────────┐
│                     解析流程                                  │
│                                                              │
│  输入: ["bash", "-lc", "cat src/main.rs"]                   │
│                    ↓                                         │
│  ┌──────────────────────────────────────────────┐           │
│  │ Step 1: 检测是否为 bash/zsh 包装             │           │
│  │ extract_bash_command() → ("bash", "cat ...")  │           │
│  └──────────────────────────────────────────────┘           │
│                    ↓                                         │
│  ┌──────────────────────────────────────────────┐           │
│  │ Step 2: tree-sitter-bash 解析脚本            │           │
│  │ try_parse_shell() → AST 语法树               │           │
│  └──────────────────────────────────────────────┘           │
│                    ↓                                         │
│  ┌──────────────────────────────────────────────┐           │
│  │ Step 3: 提取命令词元                         │           │
│  │ try_parse_word_only_commands_sequence()      │           │
│  │ → [["cat", "src/main.rs"]]                   │           │
│  └──────────────────────────────────────────────┘           │
│                    ↓                                         │
│  ┌──────────────────────────────────────────────┐           │
│  │ Step 4: 命令类型识别（模式匹配）             │           │
│  │ summarize_main_tokens()                      │           │
│  │ 匹配 "cat" → ParsedCommand::Read             │           │
│  └──────────────────────────────────────────────┘           │
│                    ↓                                         │
│  输出: ParsedCommand::Read {                                │
│      cmd: "cat src/main.rs",                                │
│      name: "main.rs",                                       │
│      path: "src/main.rs"                                    │
│  }                                                          │
└─────────────────────────────────────────────────────────────┘
```

### 为什么使用混合策略？

| 技术 | 用途 | 优势 |
|------|------|------|
| tree-sitter AST | 解析复杂 bash 语法（管道、逻辑运算符等） | 准确处理嵌套和转义 |
| shlex 词元化 | 分割命令行参数 | 正确处理引号和空格 |
| 模式匹配 | 识别特定命令类型 | 灵活扩展、易于维护 |

---

## 教学级示例

### 简单示例 1：读取单个文件

**输入命令**：
```bash
cat README.md
```

**解析步骤详解**：

```
步骤 1: 词元化
┌────────────────────────────────────────┐
│ 输入: "cat README.md"                  │
│ shlex 分割 → ["cat", "README.md"]      │
└────────────────────────────────────────┘
            ↓
步骤 2: 命令识别（模式匹配）
┌────────────────────────────────────────┐
│ match main_cmd.split_first() {         │
│   Some(("cat", tail)) => {             │  ← 匹配 "cat" 命令
│     // tail = ["README.md"]            │
│   }                                    │
│ }                                      │
└────────────────────────────────────────┘
            ↓
步骤 3: 提取文件路径
┌────────────────────────────────────────┐
│ single_non_flag_operand(tail, &[])     │
│ → Some("README.md")                    │
│                                        │
│ short_display_path("README.md")        │
│ → "README.md"  (提取文件名)            │
└────────────────────────────────────────┘
            ↓
步骤 4: 构造结果
┌────────────────────────────────────────┐
│ ParsedCommand::Read {                  │
│   cmd: "cat README.md",                │
│   name: "README.md",    ← TUI 显示用   │
│   path: "README.md"                    │
│ }                                      │
└────────────────────────────────────────┘
```

**TUI 渲染效果**：
```
• Explored
  └ Read README.md
```

---

### 简单示例 2：搜索关键词

**输入命令**：
```bash
rg "TODO" src
```

**解析步骤详解**：

```
步骤 1: 词元化
┌────────────────────────────────────────┐
│ 输入: "rg \"TODO\" src"                │
│ shlex 分割 → ["rg", "TODO", "src"]     │
└────────────────────────────────────────┘
            ↓
步骤 2: 命令识别（模式匹配）
┌────────────────────────────────────────┐
│ match main_cmd.split_first() {         │
│   Some(("rg", tail)) => {              │  ← 匹配 "rg" 命令
│     // tail = ["TODO", "src"]          │
│   }                                    │
│ }                                      │
└────────────────────────────────────────┘
            ↓
步骤 3: 检查是否为 --files 模式
┌────────────────────────────────────────┐
│ has_files_flag = false                 │
│ （不是列出文件，是搜索模式）           │
└────────────────────────────────────────┘
            ↓
步骤 4: 提取搜索参数
┌────────────────────────────────────────┐
│ non_flags = ["TODO", "src"]            │
│ query = non_flags[0] = "TODO"          │
│ path = short_display_path("src")       │
│      = "src"                           │
└────────────────────────────────────────┘
            ↓
步骤 5: 构造结果
┌────────────────────────────────────────┐
│ ParsedCommand::Search {                │
│   cmd: "rg TODO src",                  │
│   query: Some("TODO"),  ← TUI 显示用   │
│   path: Some("src")                    │
│ }                                      │
└────────────────────────────────────────┘
```

**TUI 渲染效果**：
```
• Explored
  └ Search TODO in src
```

---

### 复杂示例 1：bash 包装 + 管道过滤

**输入命令**：
```bash
bash -lc "cat tui/Cargo.toml | sed -n '1,200p'"
```

**解析步骤详解**：

```
步骤 1: 检测 bash 包装
┌────────────────────────────────────────────────────┐
│ extract_bash_command()                             │
│ 输入: ["bash", "-lc", "cat ... | sed ..."]         │
│ 检测: shell="bash", flag="-lc" ✓                   │
│ 输出: script = "cat tui/Cargo.toml | sed -n '1,200p'" │
└────────────────────────────────────────────────────┘
            ↓
步骤 2: tree-sitter AST 解析
┌────────────────────────────────────────────────────┐
│ try_parse_shell(script)                            │
│                                                    │
│ AST 结构:                                          │
│ program                                            │
│ └─ pipeline                                        │
│    ├─ command: "cat tui/Cargo.toml"               │
│    │  ├─ command_name: "cat"                      │
│    │  └─ word: "tui/Cargo.toml"                   │
│    └─ command: "sed -n '1,200p'"                  │
│       ├─ command_name: "sed"                      │
│       ├─ word: "-n"                               │
│       └─ raw_string: "'1,200p'"                   │
└────────────────────────────────────────────────────┘
            ↓
步骤 3: 提取命令序列
┌────────────────────────────────────────────────────┐
│ try_parse_word_only_commands_sequence()            │
│ → [["cat", "tui/Cargo.toml"],                     │
│    ["sed", "-n", "1,200p"]]                       │
└────────────────────────────────────────────────────┘
            ↓
步骤 4: 过滤辅助命令
┌────────────────────────────────────────────────────┐
│ is_small_formatting_command(["sed", "-n", ...])    │
│ → true (sed -n 无文件参数时是管道格式化命令)        │
│                                                    │
│ 但 sed -n 有范围参数，保留为 Read 命令             │
└────────────────────────────────────────────────────┘
            ↓
步骤 5: 合并为 Read 命令
┌────────────────────────────────────────────────────┐
│ ParsedCommand::Read {                              │
│   cmd: "cat tui/Cargo.toml | sed -n '1,200p'",    │
│   name: "Cargo.toml",                             │
│   path: "tui/Cargo.toml"                          │
│ }                                                 │
└────────────────────────────────────────────────────┘
```

**TUI 渲染效果**：
```
• Explored
  └ Read Cargo.toml
```

---

### 复杂示例 2：cd + 搜索 + 管道

**输入命令**：
```bash
bash -lc "cd codex-rs && rg -n 'codex_api' src -S | head -n 50"
```

**解析步骤详解**：

```
步骤 1: 检测 bash 包装
┌────────────────────────────────────────────────────┐
│ extract_bash_command()                             │
│ → script = "cd codex-rs && rg ... | head ..."     │
└────────────────────────────────────────────────────┘
            ↓
步骤 2: tree-sitter AST 解析
┌────────────────────────────────────────────────────┐
│ AST 结构:                                          │
│ program                                            │
│ └─ list                                            │
│    ├─ command: "cd codex-rs"        (&&)          │
│    └─ pipeline                                     │
│       ├─ command: "rg -n 'codex_api' src -S"      │
│       └─ command: "head -n 50"                    │
└────────────────────────────────────────────────────┘
            ↓
步骤 3: 提取命令序列
┌────────────────────────────────────────────────────┐
│ → [["cd", "codex-rs"],                            │
│    ["rg", "-n", "codex_api", "src", "-S"],        │
│    ["head", "-n", "50"]]                          │
└────────────────────────────────────────────────────┘
            ↓
步骤 4: 处理 cd 命令（目录跟踪）
┌────────────────────────────────────────────────────┐
│ cd_target(["codex-rs"]) → Some("codex-rs")        │
│ cwd = "codex-rs"                                  │
│ （cd 命令被过滤，但记住了目录）                    │
└────────────────────────────────────────────────────┘
            ↓
步骤 5: 处理 rg 命令
┌────────────────────────────────────────────────────┐
│ summarize_main_tokens(["rg", "-n", ...])          │
│                                                    │
│ skip_flag_values() 跳过 -n, -S 等参数             │
│ non_flags = ["codex_api", "src"]                  │
│ query = "codex_api"                               │
│ path = "src"                                      │
└────────────────────────────────────────────────────┘
            ↓
步骤 6: 过滤 head 命令
┌────────────────────────────────────────────────────┐
│ is_small_formatting_command(["head", "-n", "50"]) │
│ → true (管道末端的格式化命令)                      │
│ 被过滤掉                                          │
└────────────────────────────────────────────────────┘
            ↓
步骤 7: 构造结果
┌────────────────────────────────────────────────────┐
│ ParsedCommand::Search {                            │
│   cmd: "rg -n codex_api src -S",                  │
│   query: Some("codex_api"),                       │
│   path: Some("src")                               │
│ }                                                 │
└────────────────────────────────────────────────────┘
```

**TUI 渲染效果**：
```
• Explored
  └ Search codex_api in src
```

---

### 复杂示例 3：多命令组合（搜索 + 读取）

**输入命令**：
```bash
bash -lc "rg --files src | head -n 40 && cat src/main.rs"
```

**解析步骤详解**：

```
步骤 1: tree-sitter AST 解析
┌────────────────────────────────────────────────────┐
│ AST 结构:                                          │
│ program                                            │
│ └─ list                                            │
│    ├─ pipeline                          (&&)      │
│    │  ├─ command: "rg --files src"                │
│    │  └─ command: "head -n 40"                    │
│    └─ command: "cat src/main.rs"                  │
└────────────────────────────────────────────────────┘
            ↓
步骤 2: 提取并分类命令
┌────────────────────────────────────────────────────┐
│ 命令 1: ["rg", "--files", "src"]                  │
│   → has_files_flag = true                         │
│   → ParsedCommand::ListFiles { path: "src" }      │
│                                                    │
│ 命令 2: ["head", "-n", "40"]                      │
│   → is_small_formatting_command() = true          │
│   → 被过滤                                        │
│                                                    │
│ 命令 3: ["cat", "src/main.rs"]                    │
│   → ParsedCommand::Read {                         │
│       name: "main.rs",                            │
│       path: "src/main.rs"                         │
│     }                                             │
└────────────────────────────────────────────────────┘
            ↓
步骤 3: 判断是否为探索模式
┌────────────────────────────────────────────────────┐
│ is_exploring_call() 检查:                         │
│ - 所有命令都是 Read/Search/ListFiles? ✓           │
│ - 不是用户直接输入的 shell 命令? ✓                │
│ → 进入探索模式                                    │
└────────────────────────────────────────────────────┘
            ↓
步骤 4: 最终结果
┌────────────────────────────────────────────────────┐
│ [                                                 │
│   ParsedCommand::ListFiles { path: "src" },       │
│   ParsedCommand::Read { name: "main.rs", ... }    │
│ ]                                                 │
└────────────────────────────────────────────────────┘
```

**TUI 渲染效果**：
```
• Explored
  └ List src
    Read main.rs
```

---

## 解析函数详解

### 核心解析函数

```rust
// 文件: codex-rs/core/src/parse_command.rs

/// 主入口函数
pub fn parse_command(command: &[String]) -> Vec<ParsedCommand> {
    let parsed = parse_command_impl(command);
    // 去重连续相同的命令
    dedup(parsed)
}

/// 实现细节
fn parse_command_impl(command: &[String]) -> Vec<ParsedCommand> {
    // 1. 尝试解析 bash -lc "..." 格式
    if let Some(commands) = parse_shell_lc_commands(command) {
        return commands;
    }
    
    // 2. 回退到直接词元分析
    let tokens = normalize_tokens(command);
    let parts = split_on_connectors(&tokens);  // 按 && || | ; 分割
    
    // 3. 逐个处理子命令
    for tokens in parts {
        let parsed = summarize_main_tokens(&tokens);
        // ...
    }
}

/// 命令类型识别（模式匹配）
fn summarize_main_tokens(main_cmd: &[String]) -> ParsedCommand {
    match main_cmd.split_first() {
        // cat 命令 → Read
        Some(("cat", tail)) => { /* ... */ }
        
        // rg/grep 命令 → Search
        Some(("rg", tail)) | Some(("grep", tail)) => { /* ... */ }
        
        // ls/tree 命令 → ListFiles
        Some(("ls", tail)) | Some(("tree", tail)) => { /* ... */ }
        
        // 其他 → Unknown
        _ => ParsedCommand::Unknown { cmd: shlex_join(main_cmd) }
    }
}
```

### AST 解析函数

```rust
// 文件: codex-rs/core/src/bash.rs

/// 使用 tree-sitter-bash 解析脚本
pub fn try_parse_shell(shell_lc_arg: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&BASH.into()).expect("load bash grammar");
    parser.parse(shell_lc_arg, None)
}

/// 从 AST 提取命令词元序列
pub fn try_parse_word_only_commands_sequence(
    tree: &Tree, 
    src: &str
) -> Option<Vec<Vec<String>>> {
    // 遍历 AST，只接受简单的 word-only 命令
    // 拒绝包含重定向、替换等复杂语法的命令
}
```

---

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
