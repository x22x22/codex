# Codex MCP 会话 ID 传递分析报告

## 执行摘要

本报告分析了 Codex 项目在调用 MCP（Model Context Protocol）服务器时是否传递会话标识符（Session ID）。

**核心结论：Codex 在调用 MCP 时不传递会话 ID，无论是在请求头还是请求体中。**

## 分析范围

- MCP 客户端实现（`codex-rs/rmcp-client`）
- MCP 连接管理器（`codex-rs/core/src/mcp_connection_manager.rs`）
- MCP 工具调用处理（`codex-rs/core/src/mcp_tool_call.rs`）
- MCP 请求类型定义（`codex-rs/mcp-types`）
- HTTP 请求头构造逻辑
- 代码库搜索（会话 ID 相关关键词）

## 技术架构

### MCP 请求流程

```
用户请求
  ↓
Session::call_tool()
  ↓
McpConnectionManager::call_tool()
  ↓
RmcpClient::call_tool()
  ↓
rmcp SDK（官方 Rust SDK）
  ↓
MCP 服务器
```

### 核心组件

1. **RmcpClient** (`codex-rs/rmcp-client/src/rmcp_client.rs`)
   - 基于官方 `rmcp` SDK 实现的 MCP 客户端
   - 支持 stdio 和 HTTP 传输协议
   - 处理 OAuth 认证和令牌管理

2. **McpConnectionManager** (`codex-rs/core/src/mcp_connection_manager.rs`)
   - 管理多个 MCP 服务器连接
   - 处理服务器初始化和生命周期
   - 聚合工具和资源

3. **请求工具类** (`codex-rs/rmcp-client/src/utils.rs`)
   - 构造 HTTP 请求头
   - 创建 MCP 服务器环境变量
   - 类型转换（mcp-types ↔ rmcp SDK 类型）

## 详细分析结果

### 1. 初始化请求参数

**结构定义：** `InitializeRequestParams`

```rust
pub struct InitializeRequestParams {
    pub capabilities: ClientCapabilities,
    pub client_info: Implementation,
    pub protocol_version: String,
}
```

**实际传递的参数：**
```rust
let params = mcp_types::InitializeRequestParams {
    capabilities: ClientCapabilities {
        experimental: None,
        roots: None,
        sampling: None,
        elicitation: Some(json!({})),
    },
    client_info: Implementation {
        name: "codex-mcp-client".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        title: Some("Codex".into()),
        user_agent: None,
    },
    protocol_version: mcp_types::MCP_SCHEMA_VERSION.to_owned(),
};
```

**结论：** ❌ 无会话 ID 字段

### 2. 工具调用请求参数

**结构定义：** `CallToolRequestParams`

```rust
pub struct CallToolRequestParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
    pub name: String,
}
```

**调用代码：**
```rust
pub async fn call_tool(
    &self,
    name: String,
    arguments: Option<serde_json::Value>,
    timeout: Option<Duration>,
) -> Result<CallToolResult> {
    // ...
    let params = CallToolRequestParams { arguments, name };
    let rmcp_params: CallToolRequestParam = convert_to_rmcp(params)?;
    let fut = service.call_tool(rmcp_params);
    // ...
}
```

**结论：** ❌ 无会话 ID 字段

### 3. HTTP 请求头分析

**请求头构造函数：** `build_default_headers()`

```rust
pub(crate) fn build_default_headers(
    http_headers: Option<HashMap<String, String>>,
    env_http_headers: Option<HashMap<String, String>>,
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    
    // 添加静态配置的请求头
    if let Some(static_headers) = http_headers {
        for (name, value) in static_headers {
            // ... 添加到 headers
        }
    }
    
    // 添加从环境变量读取的请求头
    if let Some(env_headers) = env_http_headers {
        for (name, env_var) in env_headers {
            if let Ok(value) = env::var(&env_var) {
                // ... 添加到 headers
            }
        }
    }
    
    Ok(headers)
}
```

**支持的功能：**
- ✅ 静态 HTTP 请求头配置（`http_headers`）
- ✅ 从环境变量读取的动态请求头（`env_http_headers`）
- ✅ OAuth 认证请求头（Bearer Token）

**结论：** ❌ 未自动添加任何会话相关的请求头

### 4. 代码库搜索结果

**搜索关键词及结果：**

| 关键词 | 搜索范围 | 结果 |
|-------|---------|-----|
| `session_id` | MCP 相关代码 | 无匹配 |
| `sessionId` | MCP 相关代码 | 无匹配 |
| `session-id` | MCP 相关代码 | 无匹配 |
| `X-Session` | 请求头代码 | 无匹配 |
| `x-session` | 请求头代码 | 无匹配 |
| `conversation` | MCP 类型定义 | 无匹配 |
| `turn` | MCP 类型定义 | 无匹配（仅用于内部会话管理）|
| `context.*id` | MCP 参数 | 无匹配 |

**结论：** ❌ 代码库中无会话 ID 相关实现

### 5. MCP 协议规范符合性

**协议版本：** `2025-06-18`

**标准请求类型：**
- `initialize` - 初始化握手
- `tools/call` - 调用工具
- `tools/list` - 列出工具
- `resources/read` - 读取资源
- `resources/list` - 列出资源

**协议特点：**
- MCP 协议是无状态的请求-响应模型
- 标准规范不包含会话或对话上下文标识符
- 每个请求独立处理，不依赖会话状态

**结论：** ✅ Codex 实现符合 MCP 标准协议规范

## 传输方式对比

### Stdio 传输（子进程）

```rust
pub async fn new_stdio_client(
    program: OsString,
    args: Vec<OsString>,
    env: Option<HashMap<String, String>>,
    env_vars: &[String],
    cwd: Option<PathBuf>,
) -> io::Result<Self>
```

**特点：**
- 通过标准输入/输出与子进程通信
- 使用 JSON-RPC 2.0 协议
- 不涉及 HTTP 请求头
- **无会话 ID 传递机制**

### HTTP 传输（远程服务器）

```rust
pub async fn new_streamable_http_client(
    server_name: &str,
    url: &str,
    bearer_token: Option<String>,
    http_headers: Option<HashMap<String, String>>,
    env_http_headers: Option<HashMap<String, String>>,
    store_mode: OAuthCredentialsStoreMode,
) -> Result<Self>
```

**特点：**
- HTTP POST 请求到远程 MCP 服务器
- 支持自定义请求头配置
- 支持 OAuth 认证
- **可通过配置添加会话 ID 请求头（但默认不添加）**

## 配置示例

### 当前 MCP 服务器配置结构

```rust
pub struct McpServerConfig {
    pub transport: McpServerTransportConfig,  // stdio 或 http
    pub enabled: bool,
    pub startup_timeout_sec: Option<Duration>,
    pub tool_timeout_sec: Option<Duration>,
    pub enabled_tools: Option<Vec<String>>,   // 工具白名单
    pub disabled_tools: Option<Vec<String>>,  // 工具黑名单
}
```

### HTTP 传输配置

```rust
pub enum McpServerTransportConfig {
    Stdio {
        command: String,
        args: Option<Vec<String>>,
        env: Option<HashMap<String, String>>,
        cwd: Option<PathBuf>,
    },
    Http {
        url: String,
        bearer_token: Option<String>,
        http_headers: Option<HashMap<String, String>>,      // 静态请求头
        env_http_headers: Option<HashMap<String, String>>,  // 动态请求头
    },
}
```

## 如需传递会话 ID 的实现方案

### 方案一：自定义 HTTP 请求头（推荐用于 HTTP 传输）

**配置示例：**
```toml
[mcp_servers.my_server.transport]
type = "http"
url = "https://api.example.com/mcp"

# 静态会话 ID（不推荐）
[mcp_servers.my_server.transport.http_headers]
"X-Session-ID" = "fixed-session-123"

# 从环境变量读取会话 ID（推荐）
[mcp_servers.my_server.transport.env_http_headers]
"X-Session-ID" = "CODEX_SESSION_ID"
```

**优点：**
- ✅ 不需要修改 Codex 代码
- ✅ 灵活配置
- ✅ 支持动态会话 ID（通过环境变量）

**缺点：**
- ❌ 仅适用于 HTTP 传输
- ❌ 需要用户手动配置
- ❌ 不符合 MCP 标准（自定义扩展）

### 方案二：在工具参数中嵌入会话 ID

**实现方式：**
修改工具调用逻辑，在 `arguments` 中自动注入会话 ID：

```rust
// 在 handle_mcp_tool_call 中修改
pub(crate) async fn handle_mcp_tool_call(
    sess: &Session,
    turn_context: &TurnContext,
    call_id: String,
    server: String,
    tool_name: String,
    arguments: String,
) -> ResponseInputItem {
    // 解析原始参数
    let mut arguments_value = // ... 解析 arguments
    
    // 注入会话 ID
    if let Some(obj) = arguments_value.as_object_mut() {
        obj.insert("_session_id".to_string(), json!(sess.session_id()));
    }
    
    // 继续调用...
}
```

**优点：**
- ✅ 同时适用于 stdio 和 HTTP 传输
- ✅ 透明传递，用户无感知

**缺点：**
- ❌ 需要修改 Codex 核心代码
- ❌ 可能与工具原有参数冲突
- ❌ 不符合 MCP 标准（污染工具参数）

### 方案三：扩展 MCP 协议能力（最标准）

**实现方式：**
使用 MCP 协议的实验性能力（experimental capabilities）：

```rust
// 在初始化时声明会话能力
let params = mcp_types::InitializeRequestParams {
    capabilities: ClientCapabilities {
        experimental: Some(json!({
            "sessionContext": {
                "sessionId": sess.session_id(),
                "conversationId": turn_context.conversation_id(),
            }
        })),
        // ...
    },
    // ...
};
```

**优点：**
- ✅ 符合 MCP 扩展机制
- ✅ 语义清晰
- ✅ 可被 MCP 服务器识别和处理

**缺点：**
- ❌ 需要修改初始化逻辑
- ❌ 会话 ID 仅在初始化时传递一次
- ❌ 需要 MCP 服务器端支持此扩展

### 方案四：使用自定义 MCP 通知

**实现方式：**
在每次工具调用前发送自定义通知：

```rust
// 在工具调用前发送会话上下文
client.send_custom_notification(
    "codex/sessionContext",
    Some(json!({
        "sessionId": sess.session_id(),
        "turnId": turn_context.turn_id(),
        "timestamp": Utc::now().to_rfc3339(),
    }))
).await?;

// 然后执行工具调用
client.call_tool(tool_name, arguments, timeout).await?;
```

**优点：**
- ✅ 符合 MCP 自定义通知机制
- ✅ 不污染标准请求参数
- ✅ 支持动态更新会话上下文

**缺点：**
- ❌ 需要修改工具调用流程
- ❌ 增加网络往返次数
- ❌ 需要 MCP 服务器端支持

## 方案对比总结

| 方案 | 适用场景 | 开发成本 | 标准符合性 | 推荐度 |
|-----|---------|---------|-----------|--------|
| 自定义 HTTP 请求头 | HTTP 传输 | 低（仅配置）| 低（非标准）| ⭐⭐⭐⭐ |
| 工具参数嵌入 | 所有传输 | 中（代码修改）| 低（污染参数）| ⭐⭐ |
| 扩展协议能力 | 所有传输 | 中（代码修改）| 高（标准扩展）| ⭐⭐⭐⭐⭐ |
| 自定义通知 | 所有传输 | 高（流程改造）| 高（标准机制）| ⭐⭐⭐ |

## 相关代码文件清单

### 核心实现文件

| 文件路径 | 功能描述 |
|---------|---------|
| `codex-rs/rmcp-client/src/rmcp_client.rs` | MCP 客户端主实现 |
| `codex-rs/rmcp-client/src/utils.rs` | 请求头构造和工具函数 |
| `codex-rs/core/src/mcp_connection_manager.rs` | MCP 连接管理器 |
| `codex-rs/core/src/mcp_tool_call.rs` | MCP 工具调用处理 |
| `codex-rs/core/src/tools/handlers/mcp.rs` | MCP 工具处理器 |
| `codex-rs/mcp-types/src/lib.rs` | MCP 类型定义（自动生成）|
| `codex-rs/core/src/config/types.rs` | MCP 服务器配置类型 |

### 测试和示例文件

| 文件路径 | 功能描述 |
|---------|---------|
| `codex-rs/rmcp-client/tests/resources.rs` | MCP 客户端测试 |
| `codex-rs/core/tests/suite/rmcp_client.rs` | 集成测试 |
| `codex-rs/cli/tests/mcp_add_remove.rs` | MCP CLI 命令测试 |

## 技术细节补充

### JSON-RPC 2.0 请求格式

MCP 使用 JSON-RPC 2.0 协议格式：

```json
{
  "jsonrpc": "2.0",
  "id": "request-id-123",
  "method": "tools/call",
  "params": {
    "name": "tool_name",
    "arguments": {
      "param1": "value1"
    }
  }
}
```

**观察：** 标准 JSON-RPC 2.0 格式不包含会话上下文字段。

### HTTP 请求示例

```http
POST /mcp HTTP/1.1
Host: api.example.com
Content-Type: application/json
Authorization: Bearer <token>

{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "example_tool",
    "arguments": {}
  }
}
```

**观察：** 当前实现不添加会话相关的自定义请求头。

## 结论与建议

### 当前状态总结

1. **协议符合性**：✅ Codex 完全遵循 MCP 标准协议规范
2. **会话 ID 传递**：❌ 不传递任何会话标识符
3. **扩展机制**：✅ 支持通过配置添加自定义请求头（HTTP 传输）

### 如果需要会话跟踪功能

**短期方案（立即可用）：**
- 对于 HTTP 传输的 MCP 服务器，通过 `env_http_headers` 配置传递会话 ID
- 在启动 Codex 前设置环境变量，例如：
  ```bash
  export CODEX_SESSION_ID=$(uuidgen)
  codex run
  ```

**长期方案（需要开发）：**
- 实现方案三（扩展协议能力），在 `experimental` 字段中传递会话上下文
- 这是最符合 MCP 标准的扩展方式
- 需要与 MCP 服务器端配合实现

### 风险评估

如果实现会话 ID 传递：
- ⚠️ **兼容性风险**：现有 MCP 服务器可能不支持自定义扩展
- ⚠️ **安全风险**：会话 ID 可能包含敏感信息，需要注意传输安全
- ⚠️ **性能影响**：额外的元数据会增加请求大小（影响极小）

## 附录

### A. 环境变量配置示例

```bash
# 设置会话 ID
export CODEX_SESSION_ID="session-$(date +%s)"

# 设置其他上下文信息
export CODEX_USER_ID="user-123"
export CODEX_CONVERSATION_ID="conv-456"
```

### B. 配置文件示例

```toml
# config.toml
[mcp_servers.my_server]
enabled = true

[mcp_servers.my_server.transport]
type = "http"
url = "https://mcp.example.com/v1"

# 从环境变量读取会话信息
[mcp_servers.my_server.transport.env_http_headers]
"X-Codex-Session-ID" = "CODEX_SESSION_ID"
"X-Codex-User-ID" = "CODEX_USER_ID"
"X-Codex-Conversation-ID" = "CODEX_CONVERSATION_ID"
```

### C. MCP 协议版本信息

- **当前版本**：2025-06-18
- **协议仓库**：https://github.com/modelcontextprotocol/specification
- **Rust SDK**：https://github.com/modelcontextprotocol/rust-sdk

### D. 相关 MCP 规范章节

- [初始化生命周期](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle#initialization)
- [工具调用](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)
- [自定义扩展](https://modelcontextprotocol.io/specification/2025-06-18/basic/capabilities#experimental-capabilities)

---

**报告生成日期**：2026-01-24  
**分析版本**：基于 commit `25f3ec9`  
**分析人员**：GitHub Copilot
