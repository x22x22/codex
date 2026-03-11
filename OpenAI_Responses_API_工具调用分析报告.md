# OpenAI Responses API 工具调用分析报告

## 项目概述

Codex 是 OpenAI 开发的一个本地运行的编码代理（Coding Agent），它使用 OpenAI 的 Responses API 与模型进行交互。本报告分析了如何使用 `openai.OpenAI` 客户端的 `responses.create` 方法来模拟项目中的工具调用效果。

## 目标

分析如何构造请求体（body）来模拟以下工具调用：

```xml
<tool_call>shell <arg_key>command</arg_key> <arg_value>find . -name "dp-acct" -type d</arg_value> <arg_key>workdir</arg_key> <arg_value>/Users/kdump/Downloads/model</arg_value> </tool_call>
```

## 项目架构分析

### 1. Responses API 的基本结构

从代码分析可以看出，Codex 项目使用 OpenAI 的 **Responses API**（而非 Chat Completions API）进行交互。关键文件：

- **`codex-rs/codex-api/src/requests/responses.rs`**: 负责构建 Responses API 请求
- **`codex-rs/codex-api/src/sse/responses.rs`**: 负责处理服务器发送的事件流（SSE）
- **`codex-rs/protocol/src/models.rs`**: 定义工具调用的数据结构

### 2. 工具调用的类型

在 Codex 项目中，shell 相关的工具调用有以下几种形式：

#### 2.1 `shell` 工具（数组命令格式）
- **参数结构**：`ShellToolCallParams`
- **特点**：命令以字符串数组形式传递
- **源码位置**：`codex-rs/protocol/src/models.rs`

```rust
pub struct ShellToolCallParams {
    pub command: Vec<String>,           // 命令数组，如 ["find", ".", "-name", "dp-acct"]
    pub workdir: Option<String>,        // 工作目录
    pub timeout_ms: Option<u64>,        // 超时时间（毫秒）
    pub sandbox_permissions: Option<SandboxPermissions>,  // 沙箱权限
    pub justification: Option<String>,  // 理由说明
}
```

#### 2.2 `shell_command` 工具（字符串命令格式）
- **参数结构**：`ShellCommandToolCallParams`
- **特点**：命令以单个字符串形式传递
- **源码位置**：`codex-rs/protocol/src/models.rs`

```rust
pub struct ShellCommandToolCallParams {
    pub command: String,                // 命令字符串，如 "find . -name 'dp-acct' -type d"
    pub workdir: Option<String>,        // 工作目录
    pub login: Option<bool>,            // 是否使用登录 shell
    pub timeout_ms: Option<u64>,        // 超时时间（毫秒）
    pub sandbox_permissions: Option<SandboxPermissions>,
    pub justification: Option<String>,
}
```

#### 2.3 `local_shell_call`
- **用途**：表示本地 shell 执行状态
- **特点**：这是 Responses API 返回的输出项类型，而非输入工具

### 3. Responses API 请求结构

根据 `codex-rs/codex-api/src/requests/responses.rs` 中的代码，Responses API 的请求体结构如下：

```rust
pub struct ResponsesApiRequest {
    model: &str,                    // 模型名称，如 "gpt-4o"
    instructions: &str,             // 系统指令
    input: &[ResponseItem],         // 输入项数组
    tools: &[Value],                // 可用工具定义
    tool_choice: "auto",            // 工具选择策略
    parallel_tool_calls: bool,      // 是否支持并行工具调用
    reasoning: Option<Reasoning>,   // 推理设置
    store: bool,                    // 是否存储对话
    stream: true,                   // 是否流式响应
    include: Vec<String>,           // 包含的额外信息
    prompt_cache_key: Option<String>,
    text: Option<TextControls>,
}
```

### 4. SSE 事件流格式

当模型决定调用工具时，服务器通过 SSE（Server-Sent Events）返回以下格式的事件：

```json
{
  "type": "response.output_item.done",
  "item": {
    "type": "function_call",
    "call_id": "call_xxx",
    "name": "shell" 或 "shell_command",
    "arguments": "{\"command\":[\"find\",\".\",\"-name\",\"dp-acct\",\"-type\",\"d\"],\"workdir\":\"/Users/kdump/Downloads/model\"}"
  }
}
```

## 请求体构造方案

### 方案一：使用 `shell` 工具（推荐）

这是 Codex 项目中最常用的方式，命令以数组形式传递，更加结构化和安全。

```python
from openai import OpenAI

client = OpenAI(api_key="your-api-key")

# 构造请求体
response = client.responses.create(
    model="gpt-4o",
    instructions="你是一个帮助用户执行命令的助手。",
    input=[
        {
            "type": "message",
            "role": "user",
            "content": [
                {
                    "type": "input_text",
                    "text": "请在 /Users/kdump/Downloads/model 目录下查找名为 dp-acct 的目录"
                }
            ]
        }
    ],
    tools=[
        {
            "type": "function",
            "function": {
                "name": "shell",
                "description": "Execute shell commands as an array of arguments",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Command to execute as array of arguments"
                        },
                        "workdir": {
                            "type": "string",
                            "description": "Working directory for command execution"
                        },
                        "timeout_ms": {
                            "type": "integer",
                            "description": "Timeout in milliseconds"
                        }
                    },
                    "required": ["command"]
                }
            }
        }
    ],
    tool_choice="auto",
    stream=True
)

# 处理流式响应
for event in response:
    if event.type == "response.output_item.done":
        item = event.item
        if item.type == "function_call" and item.name == "shell":
            import json
            args = json.loads(item.arguments)
            print(f"工具调用 ID: {item.call_id}")
            print(f"命令: {args['command']}")
            print(f"工作目录: {args.get('workdir', '当前目录')}")
            
            # 执行命令后，需要将结果返回给模型
            # 构造下一轮请求，包含工具调用结果
```

### 方案二：使用 `shell_command` 工具

如果命令是字符串格式，可以使用 `shell_command` 工具：

```python
from openai import OpenAI

client = OpenAI(api_key="your-api-key")

response = client.responses.create(
    model="gpt-4o",
    instructions="你是一个帮助用户执行命令的助手。",
    input=[
        {
            "type": "message",
            "role": "user",
            "content": [
                {
                    "type": "input_text",
                    "text": "请在 /Users/kdump/Downloads/model 目录下查找名为 dp-acct 的目录"
                }
            ]
        }
    ],
    tools=[
        {
            "type": "function",
            "function": {
                "name": "shell_command",
                "description": "Execute a shell command as a single string",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command to execute"
                        },
                        "workdir": {
                            "type": "string",
                            "description": "Working directory for command execution"
                        },
                        "login": {
                            "type": "boolean",
                            "description": "Whether to run with login shell semantics"
                        },
                        "timeout_ms": {
                            "type": "integer",
                            "description": "Timeout in milliseconds"
                        }
                    },
                    "required": ["command"]
                }
            }
        }
    ],
    tool_choice="auto",
    stream=True
)
```

### 方案三：直接模拟工具调用（用于测试）

如果是在测试环境中模拟模型已经决定调用工具的场景，可以直接在 `input` 中包含 `function_call` 项：

```python
from openai import OpenAI
import json

client = OpenAI(api_key="your-api-key")

# 模拟模型已经决定调用 shell 工具的情况
response = client.responses.create(
    model="gpt-4o",
    instructions="你是一个帮助用户执行命令的助手。",
    input=[
        {
            "type": "message",
            "role": "user",
            "content": [
                {
                    "type": "input_text",
                    "text": "请查找 dp-acct 目录"
                }
            ]
        },
        {
            "type": "function_call",
            "call_id": "call_test_123",
            "name": "shell",
            "arguments": json.dumps({
                "command": ["find", ".", "-name", "dp-acct", "-type", "d"],
                "workdir": "/Users/kdump/Downloads/model"
            })
        },
        {
            "type": "function_call_output",
            "call_id": "call_test_123",
            "output": {
                "success": True,
                "content": "./path/to/dp-acct\n./another/path/to/dp-acct"
            }
        }
    ],
    stream=True
)
```

## 完整示例：模拟工具调用的完整流程

以下是一个完整的示例，展示如何使用 OpenAI Responses API 模拟 Codex 项目中的工具调用流程：

```python
from openai import OpenAI
import json
import subprocess

client = OpenAI(api_key="your-api-key")

def execute_shell_command(command_array, workdir=None):
    """执行 shell 命令并返回结果"""
    try:
        result = subprocess.run(
            command_array,
            cwd=workdir,
            capture_output=True,
            text=True,
            timeout=30
        )
        return {
            "success": result.returncode == 0,
            "content": result.stdout if result.returncode == 0 else result.stderr
        }
    except subprocess.TimeoutExpired:
        return {
            "success": False,
            "content": "命令执行超时"
        }
    except Exception as e:
        return {
            "success": False,
            "content": f"执行错误: {str(e)}"
        }

def main():
    # 第一轮：发送用户请求
    print("=== 第一轮：发送用户请求 ===")
    
    conversation_input = [
        {
            "type": "message",
            "role": "user",
            "content": [
                {
                    "type": "input_text",
                    "text": "请在 /Users/kdump/Downloads/model 目录下查找名为 dp-acct 的目录"
                }
            ]
        }
    ]
    
    response = client.responses.create(
        model="gpt-4o",
        instructions="你是一个帮助用户执行命令的助手。当用户需要查找文件或目录时，使用 shell 工具执行相应命令。",
        input=conversation_input,
        tools=[
            {
                "type": "function",
                "function": {
                    "name": "shell",
                    "description": "Execute shell commands. The command should be provided as an array of strings.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "Command and arguments as array"
                            },
                            "workdir": {
                                "type": "string",
                                "description": "Working directory"
                            },
                            "timeout_ms": {
                                "type": "integer",
                                "description": "Timeout in milliseconds"
                            }
                        },
                        "required": ["command"]
                    }
                }
            }
        ],
        tool_choice="auto",
        stream=True
    )
    
    # 处理响应，寻找工具调用
    function_call_item = None
    for event in response:
        print(f"事件类型: {event.type}")
        
        if event.type == "response.output_item.done":
            item = event.item
            if item.type == "function_call":
                function_call_item = item
                print(f"\n检测到工具调用:")
                print(f"  - 工具名称: {item.name}")
                print(f"  - 调用 ID: {item.call_id}")
                print(f"  - 参数: {item.arguments}")
    
    if not function_call_item:
        print("模型没有调用工具")
        return
    
    # 第二轮：执行工具并返回结果
    print("\n=== 第二轮：执行工具并返回结果 ===")
    
    # 解析工具调用参数
    args = json.loads(function_call_item.arguments)
    command = args["command"]
    workdir = args.get("workdir")
    
    print(f"执行命令: {' '.join(command)}")
    print(f"工作目录: {workdir or '当前目录'}")
    
    # 执行命令
    execution_result = execute_shell_command(command, workdir)
    
    print(f"执行结果:")
    print(f"  - 成功: {execution_result['success']}")
    print(f"  - 输出: {execution_result['content'][:200]}...")
    
    # 将工具调用和结果添加到对话历史
    conversation_input.append({
        "type": "function_call",
        "call_id": function_call_item.call_id,
        "name": function_call_item.name,
        "arguments": function_call_item.arguments
    })
    
    conversation_input.append({
        "type": "function_call_output",
        "call_id": function_call_item.call_id,
        "output": execution_result
    })
    
    # 发送下一轮请求，让模型总结结果
    response = client.responses.create(
        model="gpt-4o",
        instructions="你是一个帮助用户执行命令的助手。",
        input=conversation_input,
        tools=[],  # 不再需要工具
        stream=True
    )
    
    # 收集模型的响应
    print("\n=== 第三轮：模型总结结果 ===")
    for event in response:
        if event.type == "response.output_item.done":
            item = event.item
            if item.type == "message":
                for content in item.content:
                    if content.type == "output_text":
                        print(f"模型回复: {content.text}")

if __name__ == "__main__":
    main()
```

## 关键发现和注意事项

### 1. 参数序列化

在 Codex 项目中，工具调用的 `arguments` 字段是一个 **JSON 字符串**，而不是直接的 JSON 对象。这在代码中明确说明：

```rust
// 源自 codex-rs/protocol/src/models.rs 第 100-104 行
// The Responses API returns the function call arguments as a *string* that contains
// JSON, not as an already‑parsed object. We keep it as a raw string here and let
// Session::handle_function_call parse it into a Value.
arguments: String,
```

因此在 Python 中构造时需要：
```python
"arguments": json.dumps({"command": ["find", ".", "-name", "dp-acct"]})
```

### 2. 工具输出格式

根据 `codex-rs/protocol/src/models.rs` 中的定义，工具输出可以是两种格式：

```python
# 格式一：简单字符串
{
    "type": "function_call_output",
    "call_id": "call_xxx",
    "output": "命令执行结果文本"
}

# 格式二：结构化对象（推荐）
{
    "type": "function_call_output",
    "call_id": "call_xxx",
    "output": {
        "success": True,  # 必需字段
        "content": "命令执行结果文本"
    }
}
```

### 3. 测试环境支持

在 Codex 项目的测试代码中（`codex-rs/core/tests/common/responses.rs`），提供了多个辅助函数来构造 SSE 事件，例如：

```rust
// 构造 shell 工具调用事件
pub fn ev_function_call(call_id: &str, name: &str, arguments: &str) -> Value {
    serde_json::json!({
        "type": "response.output_item.done",
        "item": {
            "type": "function_call",
            "call_id": call_id,
            "name": name,
            "arguments": arguments
        }
    })
}

// 构造 shell_command 工具调用事件
pub fn ev_shell_command_call(call_id: &str, command: &str) -> Value {
    let args = serde_json::json!({ "command": command });
    ev_shell_command_call_with_args(call_id, &args)
}
```

这些辅助函数展示了 Codex 项目内部如何构造和测试工具调用。

### 4. 命令格式选择

根据您的需求，对于命令 `find . -name "dp-acct" -type d`，有两种表示方式：

**方式一：使用 `shell` 工具（数组格式）**
```python
{
    "command": ["find", ".", "-name", "dp-acct", "-type", "d"],
    "workdir": "/Users/kdump/Downloads/model"
}
```

**方式二：使用 `shell_command` 工具（字符串格式）**
```python
{
    "command": "find . -name \"dp-acct\" -type d",
    "workdir": "/Users/kdump/Downloads/model"
}
```

**推荐使用方式一**，因为：
1. 更加结构化，避免 shell 注入风险
2. Codex 项目内部主要使用这种方式
3. 参数解析更可靠

### 5. call_id 的重要性

`call_id` 是关联工具调用和其输出的关键标识符。在 Codex 项目中有严格的验证机制（见 `validate_request_body_invariants` 函数）：

- 每个 `function_call_output` 必须有对应的 `function_call`
- `call_id` 必须匹配且不能为空
- 不允许孤立的输出项

### 6. 流式响应处理

Codex 项目默认使用流式响应（`stream=True`），这意味着：

1. 事件通过 SSE 逐个发送
2. 需要监听 `response.output_item.done` 事件来获取完整的工具调用
3. 事件类型包括：
   - `response.created` - 响应开始
   - `response.output_item.added` - 输出项添加
   - `response.output_item.done` - 输出项完成
   - `response.completed` - 响应完成

## 总结

要使用 `openai.OpenAI` 的 `responses.create` 方法模拟 Codex 项目中的 shell 工具调用，关键步骤如下：

1. **定义工具**：在 `tools` 参数中定义 `shell` 或 `shell_command` 函数
2. **发送请求**：在 `input` 中包含用户消息
3. **处理响应**：监听 SSE 事件流，找到 `function_call` 类型的输出项
4. **执行工具**：解析 `arguments` JSON 字符串，执行相应命令
5. **返回结果**：构造新的 `input`，包含原始消息、工具调用和工具输出
6. **获取总结**：发送新请求让模型总结执行结果

这个流程完全符合 Codex 项目的实现方式，确保了与项目内部机制的一致性。

## 参考代码位置

- **请求构建**：`codex-rs/codex-api/src/requests/responses.rs`
- **SSE 处理**：`codex-rs/codex-api/src/sse/responses.rs`
- **数据模型**：`codex-rs/protocol/src/models.rs`
- **测试辅助**：`codex-rs/core/tests/common/responses.rs`
- **SDK 示例**：`sdk/typescript/tests/responsesProxy.ts`
