# Codex 扩展机制分析：如何在调用 MCP 时传递会话 ID

## 问题概述

如何在不修改 Codex 源码的前提下，使 Codex 在调用 MCP 服务器时能够传递会话 ID（Session ID）？

## 核心答案

**是的，Codex 提供了无需修改源码的扩展机制！**

Codex 通过配置文件 (`~/.codex/config.toml`) 提供了灵活的 MCP 服务器配置系统，允许用户通过**环境变量动态注入自定义 HTTP 请求头**，从而实现会话 ID 的传递。

---

## 一、Codex 的扩展机制

### 1.1 配置文件系统

Codex 使用 TOML 格式的配置文件作为主要的扩展机制：

- **配置文件位置**：`~/.codex/config.toml`
- **配置 JSON Schema**：`codex-rs/core/config.schema.json`
- **文档参考**：https://developers.openai.com/codex/config-reference

### 1.2 MCP 服务器配置结构

在 `config.toml` 中，可以通过 `[mcp_servers]` 表来配置 MCP 服务器：

```toml
[mcp_servers.<server_name>]
enabled = true
startup_timeout_sec = 30
tool_timeout_sec = 60
enabled_tools = ["tool1", "tool2"]  # 可选：工具白名单
disabled_tools = ["tool3"]          # 可选：工具黑名单
```

### 1.3 两种传输协议

Codex 支持两种 MCP 传输协议：

#### 1.3.1 Stdio 传输（本地子进程）

```toml
[mcp_servers.local_server]
command = "node"
args = ["server.js"]
env = { "DEBUG" = "true" }
env_vars = ["PATH", "HOME"]
cwd = "/path/to/server"
```

**特点**：
- 通过标准输入/输出与子进程通信
- 不涉及 HTTP 请求头
- **无法直接传递 HTTP 请求头**

**解决方案**：可以通过 `env` 字段传递会话 ID 作为环境变量，服务器端从环境变量读取。

#### 1.3.2 StreamableHttp 传输（远程 HTTP 服务器）

```toml
[mcp_servers.remote_server]
url = "https://mcp.example.com/v1"
bearer_token_env_var = "MCP_AUTH_TOKEN"
```

**特点**：
- 通过 HTTP 请求与远程服务器通信
- **支持自定义 HTTP 请求头**
- 这是传递会话 ID 的**最佳方式**

---

## 二、无需修改源码的会话 ID 传递方案

### 2.1 方案一：通过环境变量动态注入 HTTP 请求头（推荐）⭐⭐⭐⭐⭐

这是 Codex 提供的**官方扩展机制**，无需修改任何源码。

#### 配置步骤

**步骤 1：在 `config.toml` 中配置 `env_http_headers`**

```toml
[mcp_servers.my_server]
url = "https://mcp.example.com/v1"
enabled = true

# 关键配置：通过环境变量注入请求头
[mcp_servers.my_server.env_http_headers]
"X-Codex-Session-ID" = "CODEX_SESSION_ID"
"X-Codex-Conversation-ID" = "CODEX_CONVERSATION_ID"
"X-Codex-Turn-ID" = "CODEX_TURN_ID"
```

**配置说明**：
- `env_http_headers` 的键是 HTTP 请求头名称
- 值是环境变量的名称
- Codex 会在运行时从环境变量读取值并设置请求头
- 如果环境变量未设置或为空，该请求头不会被添加

**步骤 2：设置环境变量**

在启动 Codex 之前设置环境变量：

```bash
# 生成唯一的会话 ID
export CODEX_SESSION_ID="session-$(uuidgen)"
export CODEX_CONVERSATION_ID="conv-$(date +%s)"
export CODEX_TURN_ID="turn-1"

# 启动 Codex
codex run
```

或者在启动命令中直接设置：

```bash
CODEX_SESSION_ID="my-session-123" \
CODEX_CONVERSATION_ID="conv-456" \
codex run
```

**步骤 3：在 MCP 服务器端读取请求头**

MCP 服务器端可以从 HTTP 请求头中读取会话 ID：

```javascript
// Node.js MCP 服务器示例
app.post('/mcp', (req, res) => {
  const sessionId = req.headers['x-codex-session-id'];
  const conversationId = req.headers['x-codex-conversation-id'];
  const turnId = req.headers['x-codex-turn-id'];
  
  console.log(`收到请求 - Session: ${sessionId}, Conversation: ${conversationId}, Turn: ${turnId}`);
  
  // 处理 MCP 请求...
});
```

#### 优点

✅ **无需修改 Codex 源码**  
✅ **官方支持的扩展机制**  
✅ **动态灵活**：可以在运行时更改环境变量  
✅ **多会话支持**：每次启动可以使用不同的会话 ID  
✅ **安全性**：敏感信息通过环境变量传递，不暴露在配置文件中  

#### 缺点

⚠️ **仅适用于 HTTP 传输**：Stdio 传输无法使用此方法  
⚠️ **需要外部脚本管理**：需要在 Codex 启动前设置环境变量  
⚠️ **会话 ID 需要外部生成**：Codex 不会自动生成或管理会话 ID  

---

### 2.2 方案二：静态 HTTP 请求头（不推荐）⭐⭐

如果会话 ID 是固定的（例如测试环境），可以使用静态请求头。

#### 配置示例

```toml
[mcp_servers.my_server]
url = "https://mcp.example.com/v1"
enabled = true

# 静态请求头（不推荐用于生产环境）
[mcp_servers.my_server.http_headers]
"X-Codex-Session-ID" = "fixed-session-123"
"X-Codex-Environment" = "testing"
```

#### 优点

✅ **配置简单**：无需设置环境变量  
✅ **无需修改源码**  

#### 缺点

❌ **会话 ID 固定**：无法为每次会话生成唯一 ID  
❌ **不安全**：敏感信息暴露在配置文件中  
❌ **不适合生产环境**：所有会话共享同一个 ID  

---

### 2.3 方案三：通过环境变量传递给 Stdio 服务器 ⭐⭐⭐

对于 Stdio 传输的 MCP 服务器，可以通过 `env` 字段传递会话 ID。

#### 配置示例

```toml
[mcp_servers.local_server]
command = "node"
args = ["mcp-server.js"]

# 通过环境变量传递会话信息
[mcp_servers.local_server.env]
CODEX_SESSION_ID = "session-123"
CODEX_CONVERSATION_ID = "conv-456"
```

或者使用 `env_vars` 从父进程继承环境变量：

```toml
[mcp_servers.local_server]
command = "node"
args = ["mcp-server.js"]
env_vars = ["CODEX_SESSION_ID", "CODEX_CONVERSATION_ID", "PATH", "HOME"]
```

然后在启动 Codex 前设置环境变量：

```bash
export CODEX_SESSION_ID="session-$(uuidgen)"
export CODEX_CONVERSATION_ID="conv-$(date +%s)"
codex run
```

#### MCP 服务器端实现

```javascript
// Node.js MCP 服务器
const sessionId = process.env.CODEX_SESSION_ID;
const conversationId = process.env.CODEX_CONVERSATION_ID;

console.log(`MCP 服务器启动 - Session: ${sessionId}, Conversation: ${conversationId}`);

// 在处理工具调用时使用会话 ID...
```

#### 优点

✅ **无需修改 Codex 源码**  
✅ **适用于 Stdio 传输**  
✅ **配置灵活**  

#### 缺点

⚠️ **会话 ID 在服务器启动时固定**：MCP 服务器的生命周期可能跨越多个 Codex 会话  
⚠️ **需要服务器端支持**：服务器需要从环境变量读取会话信息  

---

## 三、完整实践方案

### 3.1 推荐架构

对于需要会话跟踪的场景，推荐使用以下架构：

```
Codex（客户端）
  ↓
  通过环境变量设置会话 ID
  ↓
HTTP 传输 + env_http_headers
  ↓
MCP 服务器（远程）
  ↓
从 HTTP 请求头读取会话 ID
  ↓
记录日志/数据库
```

### 3.2 完整配置示例

**`~/.codex/config.toml`**

```toml
# MCP 服务器配置
[mcp_servers.analytics_server]
url = "https://mcp-analytics.example.com/v1"
enabled = true
startup_timeout_sec = 30
tool_timeout_sec = 120

# 通过环境变量注入会话上下文
[mcp_servers.analytics_server.env_http_headers]
"X-Codex-Session-ID" = "CODEX_SESSION_ID"
"X-Codex-Conversation-ID" = "CODEX_CONVERSATION_ID"
"X-Codex-User-ID" = "CODEX_USER_ID"
"X-Codex-Timestamp" = "CODEX_TIMESTAMP"

# 认证
[mcp_servers.analytics_server]
bearer_token_env_var = "MCP_AUTH_TOKEN"
```

**启动脚本 `start-codex.sh`**

```bash
#!/bin/bash

# 生成会话 ID
export CODEX_SESSION_ID="session-$(uuidgen)"
export CODEX_CONVERSATION_ID="conv-$(date +%s)"
export CODEX_USER_ID="$(whoami)"
export CODEX_TIMESTAMP="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# 从密钥管理器读取认证令牌
export MCP_AUTH_TOKEN="$(get-secret mcp-auth-token)"

# 启动 Codex
echo "启动 Codex - Session: $CODEX_SESSION_ID"
codex run

# 清理敏感环境变量
unset MCP_AUTH_TOKEN
```

### 3.3 MCP 服务器端实现

**Node.js/Express 示例**

```javascript
const express = require('express');
const app = express();

app.post('/mcp', express.json(), (req, res) => {
  // 从请求头读取会话上下文
  const context = {
    sessionId: req.headers['x-codex-session-id'],
    conversationId: req.headers['x-codex-conversation-id'],
    userId: req.headers['x-codex-user-id'],
    timestamp: req.headers['x-codex-timestamp'],
  };
  
  console.log('MCP 请求上下文:', context);
  
  // 记录到数据库
  logToDatabase(context, req.body);
  
  // 处理 MCP 请求
  const response = handleMcpRequest(req.body, context);
  res.json(response);
});

app.listen(3000, () => {
  console.log('MCP 服务器运行在端口 3000');
});
```

**Python/FastAPI 示例**

```python
from fastapi import FastAPI, Header, Request
from typing import Optional
import logging

app = FastAPI()

@app.post("/mcp")
async def mcp_endpoint(
    request: Request,
    x_codex_session_id: Optional[str] = Header(None),
    x_codex_conversation_id: Optional[str] = Header(None),
    x_codex_user_id: Optional[str] = Header(None),
):
    # 会话上下文
    context = {
        "session_id": x_codex_session_id,
        "conversation_id": x_codex_conversation_id,
        "user_id": x_codex_user_id,
    }
    
    logging.info(f"MCP 请求上下文: {context}")
    
    # 获取请求体
    body = await request.json()
    
    # 记录到数据库
    log_to_database(context, body)
    
    # 处理 MCP 请求
    response = handle_mcp_request(body, context)
    return response
```

---

## 四、高级扩展场景

### 4.1 多服务器会话同步

如果有多个 MCP 服务器需要共享会话 ID：

```toml
# 服务器 A
[mcp_servers.server_a]
url = "https://server-a.example.com/mcp"
[mcp_servers.server_a.env_http_headers]
"X-Session-ID" = "CODEX_SESSION_ID"

# 服务器 B
[mcp_servers.server_b]
url = "https://server-b.example.com/mcp"
[mcp_servers.server_b.env_http_headers]
"X-Session-ID" = "CODEX_SESSION_ID"

# 服务器 C
[mcp_servers.server_c]
url = "https://server-c.example.com/mcp"
[mcp_servers.server_c.env_http_headers]
"X-Session-ID" = "CODEX_SESSION_ID"
```

所有服务器都会收到相同的会话 ID，可以实现分布式会话跟踪。

### 4.2 动态会话 ID 生成脚本

**`codex-wrapper.sh`**

```bash
#!/bin/bash

# 函数：生成会话 ID
generate_session_id() {
  if command -v uuidgen &> /dev/null; then
    echo "session-$(uuidgen)"
  else
    echo "session-$(date +%s)-$$"
  fi
}

# 函数：生成对话 ID
generate_conversation_id() {
  echo "conv-$(date +%s)-$(( RANDOM % 10000 ))"
}

# 设置会话环境变量
export CODEX_SESSION_ID="$(generate_session_id)"
export CODEX_CONVERSATION_ID="$(generate_conversation_id)"
export CODEX_USER_ID="${USER:-unknown}"
export CODEX_TIMESTAMP="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
export CODEX_HOSTNAME="$(hostname)"
export CODEX_VERSION="$(codex --version 2>/dev/null | head -1)"

# 打印会话信息
echo "========================================="
echo "Codex 会话信息"
echo "========================================="
echo "Session ID:       $CODEX_SESSION_ID"
echo "Conversation ID:  $CODEX_CONVERSATION_ID"
echo "User ID:          $CODEX_USER_ID"
echo "Timestamp:        $CODEX_TIMESTAMP"
echo "Hostname:         $CODEX_HOSTNAME"
echo "========================================="

# 启动 Codex
exec codex "$@"
```

使用方式：

```bash
chmod +x codex-wrapper.sh
./codex-wrapper.sh run
```

### 4.3 会话持久化

如果希望在多次 Codex 运行之间保持相同的会话 ID：

```bash
#!/bin/bash

SESSION_FILE="$HOME/.codex/current_session"

# 如果会话文件存在，读取现有会话 ID
if [ -f "$SESSION_FILE" ]; then
  export CODEX_SESSION_ID="$(cat "$SESSION_FILE")"
  echo "恢复现有会话: $CODEX_SESSION_ID"
else
  # 生成新会话 ID 并保存
  export CODEX_SESSION_ID="session-$(uuidgen)"
  echo "$CODEX_SESSION_ID" > "$SESSION_FILE"
  echo "创建新会话: $CODEX_SESSION_ID"
fi

# 每次运行生成新的对话 ID
export CODEX_CONVERSATION_ID="conv-$(date +%s)"

codex run

# 可选：在 Codex 退出后清理会话
# rm -f "$SESSION_FILE"
```

---

## 五、Codex 内部会话 ID 的可用性分析

### 5.1 Codex 内部的会话标识

根据代码分析，Codex 内部确实维护了会话标识：

```rust
// 来自 codex-rs/core/src/codex.rs
pub struct Session {
    pub conversation_id: ThreadId,  // 对话/会话 ID
    // ... 其他字段
}
```

**`ThreadId` 类型**：
- 内部使用 UUID 表示
- 每个会话（conversation）有唯一的 `conversation_id`
- 用于区分不同的对话上下文

### 5.2 为什么不能直接使用内部会话 ID？

虽然 Codex 内部有 `conversation_id`，但目前的架构设计中：

1. **MCP 客户端层不感知会话上下文**
   - `RmcpClient` 是无状态的工具调用客户端
   - 设计为可复用、与会话解耦

2. **MCP 协议标准不包含会话概念**
   - MCP 是无状态的请求-响应协议
   - 每个请求独立处理

3. **扩展点在配置层而非运行时**
   - 当前的扩展机制基于配置文件和环境变量
   - 运行时动态注入需要修改源码

### 5.3 如果要自动传递内部会话 ID（需要修改源码）

如果要让 Codex 自动将内部 `conversation_id` 传递给 MCP 服务器，需要修改源码：

**修改位置**：`codex-rs/core/src/mcp_connection_manager.rs`

```rust
pub async fn call_tool(
    &self,
    server: &str,
    tool: &str,
    arguments: Option<serde_json::Value>,
    // 新增参数：会话上下文
    session_context: Option<&SessionContext>,
) -> Result<mcp_types::CallToolResult> {
    let client = self.client_by_name(server).await?;
    
    // 如果是 HTTP 传输且有会话上下文，注入自定义请求头
    if let Some(context) = session_context {
        // 注入会话 ID 到 HTTP 请求头
        // （需要扩展 RmcpClient 接口）
    }
    
    client.client.call_tool(tool.to_string(), arguments, client.tool_timeout).await
}
```

但这需要：
- 修改多个模块的接口
- 扩展 `RmcpClient` 以支持动态请求头
- 可能影响其他功能的兼容性

**因此，推荐使用现有的环境变量机制，而不是修改源码。**

---

## 六、方案对比总结

| 方案 | 修改源码 | 适用传输 | 动态会话 | 安全性 | 推荐度 |
|-----|---------|---------|---------|--------|--------|
| 环境变量 + HTTP 请求头 | ❌ 否 | HTTP | ✅ 是 | ✅ 高 | ⭐⭐⭐⭐⭐ |
| 静态 HTTP 请求头 | ❌ 否 | HTTP | ❌ 否 | ⚠️ 中 | ⭐⭐ |
| 环境变量 + Stdio | ❌ 否 | Stdio | ⚠️ 部分 | ✅ 高 | ⭐⭐⭐ |
| 修改源码自动注入 | ✅ 是 | 全部 | ✅ 是 | ✅ 高 | ⭐⭐⭐⭐ |

---

## 七、常见问题 (FAQ)

### Q1: 为什么推荐使用环境变量而不是静态配置？

**A**: 环境变量提供了以下优势：
- **动态性**：每次运行可以生成新的会话 ID
- **安全性**：敏感信息不会暴露在配置文件中
- **灵活性**：可以在不修改配置文件的情况下更改会话信息
- **多环境支持**：开发、测试、生产环境可以使用不同的会话管理策略

### Q2: 如何验证会话 ID 是否正确传递？

**A**: 可以在 MCP 服务器端添加日志：

```javascript
app.post('/mcp', (req, res) => {
  console.log('收到的请求头:', req.headers);
  console.log('会话 ID:', req.headers['x-codex-session-id']);
  // ...
});
```

或者使用网络抓包工具（如 Wireshark、Charles）查看 HTTP 请求。

### Q3: 会话 ID 的格式有要求吗？

**A**: 没有强制要求，但推荐使用以下格式：
- **UUID**：`session-550e8400-e29b-41d4-a716-446655440000`
- **时间戳**：`session-1738317600`
- **组合格式**：`session-1738317600-1234`

确保会话 ID 是唯一的且不易猜测。

### Q4: 可以同时使用静态和动态请求头吗？

**A**: 可以！配置示例：

```toml
[mcp_servers.my_server]
url = "https://mcp.example.com/v1"

# 静态请求头
[mcp_servers.my_server.http_headers]
"X-API-Version" = "v1"
"X-Client" = "Codex"

# 动态请求头
[mcp_servers.my_server.env_http_headers]
"X-Session-ID" = "CODEX_SESSION_ID"
"Authorization" = "MCP_AUTH_TOKEN"
```

Codex 会合并这两种请求头。如果有冲突，动态请求头优先。

### Q5: 如果环境变量未设置会怎样？

**A**: 根据代码实现（`codex-rs/rmcp-client/src/utils.rs`）：

```rust
if let Ok(value) = env::var(&env_var) {
    if value.trim().is_empty() {
        continue;  // 跳过空值
    }
    // 添加请求头...
}
```

- 如果环境变量未设置，该请求头不会被添加
- 如果环境变量为空字符串，该请求头也不会被添加
- **不会报错**，Codex 会正常运行

### Q6: 如何在 Windows 上设置环境变量？

**A**: 

**PowerShell**:
```powershell
$env:CODEX_SESSION_ID = "session-$(New-Guid)"
$env:CODEX_CONVERSATION_ID = "conv-$((Get-Date).Ticks)"
codex run
```

**命令提示符 (CMD)**:
```cmd
set CODEX_SESSION_ID=session-%RANDOM%-%TIME:~0,8%
set CODEX_CONVERSATION_ID=conv-%DATE:~-4%%DATE:~-7,2%%DATE:~-10,2%
codex run
```

**批处理脚本 (.bat)**:
```batch
@echo off
set CODEX_SESSION_ID=session-%RANDOM%
set CODEX_CONVERSATION_ID=conv-%TIME:~0,8%
echo 会话 ID: %CODEX_SESSION_ID%
codex run
```

---

## 八、总结

### 核心结论

✅ **Codex 提供了无需修改源码的扩展机制**  
✅ **通过 `env_http_headers` 可以动态注入 HTTP 请求头**  
✅ **推荐使用环境变量 + HTTP 传输方案**  

### 最佳实践

1. **配置文件**：在 `config.toml` 中配置 `env_http_headers`
2. **启动脚本**：创建包装脚本动态生成会话 ID
3. **服务器端**：从 HTTP 请求头读取会话上下文
4. **日志记录**：记录会话 ID 用于问题追踪和分析

### 扩展能力评估

| 扩展需求 | 是否支持 | 实现方式 |
|---------|---------|---------|
| 传递会话 ID | ✅ | 环境变量 + HTTP 请求头 |
| 传递用户信息 | ✅ | 环境变量 + HTTP 请求头 |
| 传递时间戳 | ✅ | 环境变量 + HTTP 请求头 |
| 自定义认证 | ✅ | `bearer_token_env_var` 或自定义请求头 |
| 动态配置 | ✅ | 环境变量 |
| 多服务器共享上下文 | ✅ | 相同的环境变量映射 |
| Stdio 传输传递请求头 | ❌ | 只能通过环境变量传递给子进程 |
| 自动注入内部会话 ID | ❌ | 需要修改源码 |

### 下一步行动

如果您需要实现会话 ID 传递：

1. ✅ **第一步**：参考本文档的"完整实践方案"章节
2. ✅ **第二步**：修改 `~/.codex/config.toml` 添加 `env_http_headers` 配置
3. ✅ **第三步**：创建启动脚本设置环境变量
4. ✅ **第四步**：在 MCP 服务器端实现请求头读取逻辑
5. ✅ **第五步**：测试验证会话 ID 传递是否成功

---

**文档版本**：v1.0  
**最后更新**：2026-01-24  
**相关文档**：`docs/mcp-session-id-analysis-zh.md`  
**配置参考**：https://developers.openai.com/codex/config-reference
