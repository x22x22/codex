# Codex 内部会话 ID 自动传递方案分析

## 问题澄清

用户需求：让 Codex 在调用 MCP 服务器时，**自动传递 Codex 自己的内部会话 ID**（`conversation_id`），而不是外部生成的会话 ID。

## Codex 内部会话 ID 分析

### 1. 会话 ID 的来源

根据代码分析，Codex 内部维护了会话标识：

```rust
// codex-rs/core/src/codex.rs
pub(crate) struct Session {
    pub(crate) conversation_id: ThreadId,  // Codex 的内部会话 ID
    // ... 其他字段
}
```

**`ThreadId` 类型**：
- 定义在 `codex-rs/protocol/src/thread_id.rs`
- 本质是一个 UUID（128 位唯一标识符）
- 格式类似：`019bbed6-1e9e-7f31-984c-a05b65045719`
- 每个 Codex 会话（conversation）都有唯一的 `conversation_id`

### 2. 会话 ID 的生命周期

```rust
// 在 Session 创建时生成
let conversation_id = ThreadId::default();  // 生成新的 UUID

// Session 在整个会话生命周期内保持不变
pub(crate) struct Session {
    pub(crate) conversation_id: ThreadId,  // 不可变
    // ...
}
```

### 3. MCP 工具调用流程

当前的调用路径：

```
handle_mcp_tool_call(sess: &Session, ...)
  ↓
sess.call_tool(server, tool_name, arguments)
  ↓
mcp_connection_manager.call_tool(server, tool, arguments)
  ↓
client.call_tool(tool.to_string(), arguments, timeout)
  ↓
rmcp SDK (不知道 conversation_id)
```

**关键问题**：`Session::conversation_id` 在调用链中**没有被传递**到 MCP 客户端层。

---

## 实现方案分析

### 方案一：修改源码自动注入会话 ID（需要代码改动）⭐⭐⭐⭐⭐

这是**最正确的方案**，让 Codex 自动将内部 `conversation_id` 传递给 MCP 服务器。

#### 实现步骤

**步骤 1：扩展 `call_tool` 接口传递会话上下文**

修改 `Session::call_tool` 以传递 `conversation_id`：

```rust
// codex-rs/core/src/codex.rs
pub async fn call_tool(
    &self,
    server: &str,
    tool: &str,
    arguments: Option<serde_json::Value>,
) -> anyhow::Result<CallToolResult> {
    // 传递会话 ID
    self.services
        .mcp_connection_manager
        .read()
        .await
        .call_tool(server, tool, arguments, Some(self.conversation_id))
        .await
}
```

**步骤 2：扩展 `McpConnectionManager::call_tool` 接口**

```rust
// codex-rs/core/src/mcp_connection_manager.rs
pub async fn call_tool(
    &self,
    server: &str,
    tool: &str,
    arguments: Option<serde_json::Value>,
    conversation_id: Option<ThreadId>,  // 新增参数
) -> Result<mcp_types::CallToolResult> {
    let client = self.client_by_name(server).await?;
    
    // 传递给 RmcpClient
    client.client
        .call_tool(
            tool.to_string(),
            arguments,
            client.tool_timeout,
            conversation_id,  // 传递会话 ID
        )
        .await
        .with_context(|| format!("tool call failed for `{server}/{tool}`"))
}
```

**步骤 3：扩展 `RmcpClient` 支持会话上下文**

```rust
// codex-rs/rmcp-client/src/rmcp_client.rs
pub async fn call_tool(
    &self,
    name: String,
    arguments: Option<serde_json::Value>,
    timeout: Option<Duration>,
    conversation_id: Option<ThreadId>,  // 新增参数
) -> Result<CallToolResult> {
    self.refresh_oauth_if_needed().await;
    let service = self.service().await?;
    
    // 将 conversation_id 注入到请求中
    let params = self.build_call_tool_params(name, arguments, conversation_id)?;
    
    let rmcp_params: CallToolRequestParam = convert_to_rmcp(params)?;
    let fut = service.call_tool(rmcp_params);
    let rmcp_result = run_with_timeout(fut, timeout, "tools/call").await?;
    let converted = convert_call_tool_result(rmcp_result)?;
    self.persist_oauth_tokens().await;
    Ok(converted)
}

fn build_call_tool_params(
    &self,
    name: String,
    arguments: Option<serde_json::Value>,
    conversation_id: Option<ThreadId>,
) -> Result<CallToolRequestParams> {
    let mut params = CallToolRequestParams { arguments, name };
    
    // 如果有会话 ID，注入到参数或元数据中
    if let Some(id) = conversation_id {
        // 方案 A: 注入到 arguments（如果是对象类型）
        if let Some(Value::Object(ref mut map)) = params.arguments {
            map.insert(
                "_codex_conversation_id".to_string(),
                Value::String(id.to_string()),
            );
        }
        
        // 方案 B: 使用 MCP 元数据（如果协议支持）
        // params._meta = Some(json!({
        //     "conversation_id": id.to_string()
        // }));
    }
    
    Ok(params)
}
```

**步骤 4：对于 HTTP 传输，也可以注入到请求头**

```rust
// codex-rs/rmcp-client/src/rmcp_client.rs
// 在 StreamableHttp 传输的情况下，还可以设置 HTTP 请求头

impl RmcpClient {
    // 保存会话上下文
    conversation_context: Mutex<Option<ConversationContext>>,
}

struct ConversationContext {
    conversation_id: ThreadId,
}

// 在 call_tool 之前设置上下文
pub async fn set_conversation_context(&self, conversation_id: ThreadId) {
    *self.conversation_context.lock().await = Some(ConversationContext {
        conversation_id,
    });
}

// 在构造 HTTP 请求时注入请求头
// （需要修改 rmcp SDK 或使用拦截器）
```

#### 优点

✅ **完全自动化**：无需用户配置或手动操作  
✅ **语义正确**：传递的是真实的 Codex 会话 ID  
✅ **一致性**：所有 MCP 工具调用都自动包含会话 ID  
✅ **类型安全**：使用 Rust 类型系统确保正确性  

#### 缺点

❌ **需要修改多个模块**：涉及 core、rmcp-client 等  
❌ **接口变更**：需要更新多个函数签名  
❌ **需要处理兼容性**：可能影响现有代码  
❌ **MCP 协议限制**：标准 MCP 协议不包含会话上下文  

#### 实现难度

- **代码改动范围**：中等（3-5 个文件）
- **技术复杂度**：中等
- **测试工作量**：中等（需要测试 Stdio 和 HTTP 两种传输）
- **预计工作量**：2-4 小时

---

### 方案二：通过配置映射内部会话 ID 到环境变量（无需改源码）⭐⭐⭐

这个方案的想法是：让 Codex 启动时将内部 `conversation_id` **导出为环境变量**，然后通过现有的 `env_http_headers` 机制传递。

#### 实现方式

**步骤 1：Codex 启动时导出会话 ID**

```rust
// 在 Codex 初始化 Session 后
let conversation_id = session.conversation_id;
std::env::set_var("CODEX_INTERNAL_CONVERSATION_ID", conversation_id.to_string());
```

**步骤 2：配置 MCP 服务器读取该环境变量**

```toml
# ~/.codex/config.toml
[mcp_servers.my_server]
url = "https://mcp.example.com/v1"

[mcp_servers.my_server.env_http_headers]
"X-Codex-Conversation-ID" = "CODEX_INTERNAL_CONVERSATION_ID"
```

#### 优点

✅ **无需修改 Codex 源码**（只需添加一行设置环境变量的代码）  
✅ **利用现有机制**：复用 `env_http_headers` 功能  
✅ **用户可控**：可以选择是否传递  

#### 缺点

❌ **环境变量污染**：会话 ID 暴露在进程环境中  
⚠️ **时序问题**：环境变量需要在 MCP 客户端初始化前设置  
⚠️ **Stdio 传输的限制**：子进程会继承父进程的环境变量，但会话 ID 在子进程启动前已经固定  
❌ **每次会话需要重启 MCP 服务器**：环境变量在进程启动时确定  

#### 实现难度

- **代码改动范围**：极小（1-2 行代码）
- **技术复杂度**：低
- **测试工作量**：低
- **预计工作量**：15-30 分钟

---

### 方案三：使用 MCP 实验性能力传递会话上下文⭐⭐⭐⭐

利用 MCP 协议的 `experimental` 字段在初始化时传递会话信息。

#### 实现方式

**修改初始化参数**：

```rust
// codex-rs/core/src/mcp_connection_manager.rs
async fn start_server_task(
    server_name: String,
    client: Arc<RmcpClient>,
    conversation_id: Option<ThreadId>,  // 新增参数
    // ...
) -> Result<ManagedClient, StartupOutcomeError> {
    let params = mcp_types::InitializeRequestParams {
        capabilities: ClientCapabilities {
            experimental: Some(json!({
                "codex": {
                    "conversation_id": conversation_id.map(|id| id.to_string()),
                    "client_version": env!("CARGO_PKG_VERSION"),
                }
            })),
            // ...
        },
        // ...
    };
    
    // ...
}
```

**MCP 服务器端读取**：

```javascript
// MCP 服务器初始化处理
function handleInitialize(request) {
  const codexContext = request.params.capabilities?.experimental?.codex;
  if (codexContext) {
    console.log('Codex conversation ID:', codexContext.conversation_id);
    // 保存会话 ID 用于后续请求
    this.conversationId = codexContext.conversation_id;
  }
  // ...
}
```

#### 优点

✅ **符合 MCP 扩展机制**：使用协议的实验性功能  
✅ **初始化时传递**：会话 ID 在连接建立时就确定  
✅ **服务器端可选支持**：不支持的服务器会忽略  

#### 缺点

⚠️ **会话 ID 仅在初始化时传递**：后续工具调用不包含  
⚠️ **MCP 连接复用问题**：多个会话可能共享同一个 MCP 连接  
❌ **需要修改初始化逻辑**  
❌ **需要 MCP 服务器端配合**  

#### 实现难度

- **代码改动范围**：小（1-2 个文件）
- **技术复杂度**：低
- **测试工作量**：中
- **预计工作量**：1-2 小时

---

### 方案四：使用自定义 MCP 通知实时同步会话上下文⭐⭐⭐

在每次工具调用前发送自定义通知更新会话上下文。

#### 实现方式

```rust
// codex-rs/core/src/mcp_tool_call.rs
pub(crate) async fn handle_mcp_tool_call(
    sess: &Session,
    turn_context: &TurnContext,
    call_id: String,
    server: String,
    tool_name: String,
    arguments: String,
) -> ResponseInputItem {
    // 在工具调用前发送会话上下文通知
    let _ = sess
        .send_conversation_context_notification(&server)
        .await;
    
    // 继续正常的工具调用...
    // ...
}

// 在 Session 中添加方法
impl Session {
    async fn send_conversation_context_notification(
        &self,
        server: &str,
    ) -> Result<()> {
        self.services
            .mcp_connection_manager
            .read()
            .await
            .send_custom_notification(
                server,
                "codex/conversationContext",
                Some(json!({
                    "conversation_id": self.conversation_id.to_string(),
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                })),
            )
            .await
    }
}
```

#### 优点

✅ **实时更新**：每次调用前都更新上下文  
✅ **符合 MCP 通知机制**  
✅ **不污染工具参数**  

#### 缺点

❌ **额外的网络开销**：每次工具调用前都发送通知  
❌ **需要服务器端支持**：服务器需要监听和处理通知  
❌ **异步问题**：通知和工具调用可能乱序  

#### 实现难度

- **代码改动范围**：中（2-3 个文件）
- **技术复杂度**：中
- **测试工作量**：中
- **预计工作量**：2-3 小时

---

## 方案对比总结

| 方案 | 修改源码 | 自动化程度 | 准确性 | 性能影响 | 推荐度 |
|-----|---------|-----------|--------|---------|--------|
| 方案一：修改接口自动注入 | ✅ 是（中等） | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | 无 | ⭐⭐⭐⭐⭐ |
| 方案二：环境变量映射 | ⚠️ 极少 | ⭐⭐ | ⭐⭐⭐ | 无 | ⭐⭐⭐ |
| 方案三：实验性能力 | ✅ 是（小） | ⭐⭐⭐⭐ | ⭐⭐⭐⭐ | 无 | ⭐⭐⭐⭐ |
| 方案四：自定义通知 | ✅ 是（中等） | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⚠️ 小 | ⭐⭐⭐ |

---

## 推荐实现路径

### 短期方案（临时解决）

使用**方案二：环境变量映射**

在 `Session` 创建后立即设置环境变量：

```rust
// codex-rs/core/src/codex.rs
// 在 Session 创建后
std::env::set_var(
    "CODEX_INTERNAL_CONVERSATION_ID",
    conversation_id.to_string()
);
```

然后在 `config.toml` 中配置：

```toml
[mcp_servers.my_server]
url = "https://mcp.example.com/v1"

[mcp_servers.my_server.env_http_headers]
"X-Codex-Conversation-ID" = "CODEX_INTERNAL_CONVERSATION_ID"
```

**优点**：改动最小，快速验证需求  
**缺点**：不够优雅，有局限性

---

### 中期方案（标准扩展）

使用**方案三：实验性能力**

在 MCP 初始化时传递会话 ID：

```rust
// codex-rs/core/src/mcp_connection_manager.rs
let params = mcp_types::InitializeRequestParams {
    capabilities: ClientCapabilities {
        experimental: Some(json!({
            "codex": {
                "conversation_id": conversation_id.to_string(),
            }
        })),
        // ...
    },
    // ...
};
```

**优点**：符合 MCP 扩展规范，改动较小  
**缺点**：会话 ID 仅在初始化时传递一次

---

### 长期方案（完整实现）

使用**方案一：修改接口自动注入**

这是最正确的方案，需要：

1. 扩展 `Session::call_tool` 接口传递 `conversation_id`
2. 扩展 `McpConnectionManager::call_tool` 接口
3. 扩展 `RmcpClient::call_tool` 接口
4. 在工具调用参数或元数据中注入会话 ID
5. 对于 HTTP 传输，同时注入到请求头

**优点**：完全自动化，语义正确，一致性好  
**缺点**：需要较多代码改动和测试

---

## 实现建议

### 阶段一：快速验证（1 小时）

1. 使用方案二验证需求的可行性
2. 在 MCP 服务器端实现会话 ID 读取和记录
3. 测试基本功能

### 阶段二：标准化扩展（2-3 小时）

1. 实现方案三，在初始化时传递会话 ID
2. 更新文档说明扩展机制
3. 添加配置选项控制是否传递会话 ID

### 阶段三：完整实现（4-6 小时）

1. 实现方案一，完整的接口改造
2. 同时支持参数注入和请求头注入
3. 添加完整的测试覆盖
4. 更新 MCP 客户端文档

---

## 关键代码位置

需要修改的文件（按推荐的方案一）：

1. **`codex-rs/core/src/codex.rs`**
   - `Session::call_tool` 方法：添加 `conversation_id` 参数传递

2. **`codex-rs/core/src/mcp_connection_manager.rs`**
   - `McpConnectionManager::call_tool` 方法：接收并传递 `conversation_id`

3. **`codex-rs/rmcp-client/src/rmcp_client.rs`**
   - `RmcpClient::call_tool` 方法：接收 `conversation_id` 并注入到请求中

4. **`codex-rs/core/src/mcp_tool_call.rs`**
   - `handle_mcp_tool_call` 函数：已有 `sess` 参数，可以访问 `conversation_id`

5. **`codex-rs/core/tests/suite/rmcp_client.rs`**
   - 添加测试验证会话 ID 传递

---

## 配置选项建议

为了让用户可以控制是否传递会话 ID，建议添加配置选项：

```toml
# ~/.codex/config.toml
[mcp_servers.my_server]
url = "https://mcp.example.com/v1"

# 新增配置：是否自动传递 Codex 会话 ID
pass_conversation_id = true  # 默认 false

# 可选：自定义会话 ID 的字段名或请求头名
conversation_id_field = "_codex_conversation_id"  # 对于参数注入
conversation_id_header = "X-Codex-Conversation-ID"  # 对于 HTTP 请求头
```

---

## 总结

### 核心问题

Codex 内部有 `conversation_id`，但当前架构中这个 ID 不会自动传递给 MCP 服务器。

### 根本原因

MCP 客户端层（`RmcpClient`）与会话层（`Session`）解耦，设计为无状态的工具调用客户端。

### 推荐解决方案

1. **短期**：使用环境变量映射（方案二）快速验证
2. **中期**：在初始化时通过实验性能力传递（方案三）
3. **长期**：完整的接口改造自动注入会话 ID（方案一）

### 下一步行动

1. 确认需求范围：
   - 是否所有 MCP 服务器都需要接收会话 ID？
   - 是否需要可配置（允许用户选择是否传递）？
   - 优先支持 HTTP 还是 Stdio 传输？

2. 选择实现方案：
   - 快速验证：方案二
   - 标准实现：方案三
   - 完整实现：方案一

3. 实施开发：
   - 修改相关代码文件
   - 添加测试用例
   - 更新文档

---

**文档版本**：v2.0  
**最后更新**：2026-01-24  
**基于**：用户反馈 - "这里的会话id不是外部的，是codex自己的"
