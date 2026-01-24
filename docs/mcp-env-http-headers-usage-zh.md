# Streamable HTTP 服务器中 `env_http_headers` 用法详细分析

## 概述

`env_http_headers` 是 Codex 为 Streamable HTTP MCP 服务器提供的一个配置选项，用于从环境变量动态读取 HTTP 请求头的值。

## 配置定义

### 数据结构

```rust
// codex-rs/core/src/config/types.rs
pub enum McpServerTransportConfig {
    StreamableHttp {
        url: String,
        bearer_token_env_var: Option<String>,
        http_headers: Option<HashMap<String, String>>,
        env_http_headers: Option<HashMap<String, String>>,  // 关键配置
    },
}
```

**类型**：`Option<HashMap<String, String>>`

**语义**：
- **键（Key）**：HTTP 请求头的名称（例如 `"X-Custom-Header"`）
- **值（Value）**：环境变量的名称（例如 `"MY_CUSTOM_VAR"`）

配置后，Codex 会在运行时从环境变量中读取值，并将其设置为对应的 HTTP 请求头。

---

## 实现机制

### 1. 配置读取

在 TOML 配置文件中定义：

```toml
[mcp_servers.my_server]
url = "https://api.example.com/mcp"

[mcp_servers.my_server.env_http_headers]
"X-API-Key" = "API_KEY_ENV_VAR"
"X-User-ID" = "USER_ID_ENV_VAR"
"X-Session-Token" = "SESSION_TOKEN"
```

这个配置表示：
- 从环境变量 `API_KEY_ENV_VAR` 读取值，设置为请求头 `X-API-Key`
- 从环境变量 `USER_ID_ENV_VAR` 读取值，设置为请求头 `X-User-ID`
- 从环境变量 `SESSION_TOKEN` 读取值，设置为请求头 `X-Session-Token`

### 2. 运行时处理

#### 代码实现（`codex-rs/rmcp-client/src/utils.rs`）

```rust
pub(crate) fn build_default_headers(
    http_headers: Option<HashMap<String, String>>,
    env_http_headers: Option<HashMap<String, String>>,
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();

    // 第一步：处理静态请求头（http_headers）
    if let Some(static_headers) = http_headers {
        for (name, value) in static_headers {
            let header_name = match HeaderName::from_bytes(name.as_bytes()) {
                Ok(name) => name,
                Err(err) => {
                    tracing::warn!("invalid HTTP header name `{name}`: {err}");
                    continue;  // 跳过无效的请求头名称
                }
            };
            let header_value = match HeaderValue::from_str(value.as_str()) {
                Ok(value) => value,
                Err(err) => {
                    tracing::warn!("invalid HTTP header value for `{name}`: {err}");
                    continue;  // 跳过无效的请求头值
                }
            };
            headers.insert(header_name, header_value);
        }
    }

    // 第二步：处理动态请求头（env_http_headers）
    if let Some(env_headers) = env_http_headers {
        for (name, env_var) in env_headers {
            // 从环境变量读取值
            if let Ok(value) = env::var(&env_var) {
                // 跳过空值
                if value.trim().is_empty() {
                    continue;
                }

                // 验证请求头名称
                let header_name = match HeaderName::from_bytes(name.as_bytes()) {
                    Ok(name) => name,
                    Err(err) => {
                        tracing::warn!("invalid HTTP header name `{name}`: {err}");
                        continue;
                    }
                };

                // 验证请求头值
                let header_value = match HeaderValue::from_str(value.as_str()) {
                    Ok(value) => value,
                    Err(err) => {
                        tracing::warn!(
                            "invalid HTTP header value read from {env_var} for `{name}`: {err}"
                        );
                        continue;
                    }
                };
                
                // 插入请求头
                headers.insert(header_name, header_value);
            }
            // 注意：如果环境变量不存在，不会报错，直接跳过
        }
    }

    Ok(headers)
}
```

#### 关键行为

1. **环境变量不存在**：
   - 不会报错
   - 该请求头不会被添加
   - 继续处理其他请求头

2. **环境变量值为空**：
   - 会被跳过（`if value.trim().is_empty()`）
   - 该请求头不会被添加

3. **请求头名称或值无效**：
   - 记录警告日志
   - 跳过该请求头
   - 继续处理其他请求头

4. **静态和动态请求头冲突**：
   - 如果同名，后处理的会覆盖先处理的
   - `env_http_headers` 在 `http_headers` 之后处理
   - 因此 `env_http_headers` 的优先级更高

### 3. 应用到 HTTP 客户端

```rust
// codex-rs/rmcp-client/src/rmcp_client.rs
pub async fn new_streamable_http_client(
    server_name: &str,
    url: &str,
    bearer_token: Option<String>,
    http_headers: Option<HashMap<String, String>>,
    env_http_headers: Option<HashMap<String, String>>,
    store_mode: OAuthCredentialsStoreMode,
) -> Result<Self> {
    // 构造请求头
    let default_headers = build_default_headers(http_headers, env_http_headers)?;

    // ... OAuth 处理 ...

    // 将请求头应用到 HTTP 客户端
    let http_client = apply_default_headers(
        reqwest::Client::builder(),
        &default_headers
    ).build()?;

    // 创建 MCP 传输层
    let transport = StreamableHttpClientTransport::with_client(http_client, http_config);
    
    // ...
}
```

---

## 使用场景

### 场景 1：传递认证令牌

```toml
[mcp_servers.secure_server]
url = "https://secure-api.example.com/mcp"

[mcp_servers.secure_server.env_http_headers]
"Authorization" = "AUTH_TOKEN"
"X-API-Key" = "API_KEY"
```

启动前设置环境变量：

```bash
export AUTH_TOKEN="Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
export API_KEY="sk-1234567890abcdef"
codex run
```

**结果**：所有发送到 `secure-api.example.com` 的 MCP 请求都会包含这些请求头。

### 场景 2：传递用户上下文

```toml
[mcp_servers.analytics_server]
url = "https://analytics.example.com/mcp"

[mcp_servers.analytics_server.env_http_headers]
"X-User-ID" = "CURRENT_USER_ID"
"X-Session-ID" = "CURRENT_SESSION_ID"
"X-Tenant-ID" = "TENANT_ID"
```

启动前设置：

```bash
export CURRENT_USER_ID="user-12345"
export CURRENT_SESSION_ID="session-$(uuidgen)"
export TENANT_ID="tenant-abc"
codex run
```

### 场景 3：动态配置（开发/测试/生产环境）

```toml
[mcp_servers.api_server]
url = "https://api.example.com/mcp"

[mcp_servers.api_server.env_http_headers]
"X-Environment" = "DEPLOYMENT_ENV"
"X-Debug-Mode" = "DEBUG_ENABLED"
```

不同环境使用不同的环境变量：

```bash
# 开发环境
export DEPLOYMENT_ENV="development"
export DEBUG_ENABLED="true"

# 生产环境
export DEPLOYMENT_ENV="production"
export DEBUG_ENABLED="false"
```

### 场景 4：传递 Codex 内部会话 ID

这是用户在评论中询问的核心场景：

```toml
[mcp_servers.my_server]
url = "https://mcp.example.com/v1"

[mcp_servers.my_server.env_http_headers]
"X-Codex-Conversation-ID" = "CODEX_CONVERSATION_ID"
"X-Codex-Turn-ID" = "CODEX_TURN_ID"
```

在 Codex 内部，当 Session 创建后，可以设置环境变量：

```rust
// 在 Session 初始化时
std::env::set_var("CODEX_CONVERSATION_ID", conversation_id.to_string());
std::env::set_var("CODEX_TURN_ID", turn_id.to_string());
```

然后所有后续的 MCP HTTP 请求都会自动包含这些请求头。

---

## 与 `http_headers` 的区别

### `http_headers`：静态请求头

```toml
[mcp_servers.my_server.http_headers]
"X-Client-Version" = "1.0.0"
"X-Client-Name" = "Codex"
```

- **值来源**：直接在配置文件中定义
- **特点**：固定不变，每次请求都是相同的值
- **优点**：简单直接，无需额外操作
- **缺点**：不能动态变化，敏感信息暴露在配置文件中

### `env_http_headers`：动态请求头

```toml
[mcp_servers.my_server.env_http_headers]
"X-Session-ID" = "SESSION_ID_VAR"
```

- **值来源**：从环境变量读取
- **特点**：动态灵活，可以在运行时改变
- **优点**：
  - 敏感信息不暴露在配置文件中
  - 可以根据环境（开发/测试/生产）使用不同的值
  - 支持动态生成的值（如会话 ID）
- **缺点**：需要在启动前设置环境变量

### 组合使用

可以同时使用两者：

```toml
[mcp_servers.my_server]
url = "https://api.example.com/mcp"

# 静态请求头（固定不变）
[mcp_servers.my_server.http_headers]
"X-Client-Name" = "Codex"
"X-Client-Version" = "1.0.0"

# 动态请求头（从环境变量读取）
[mcp_servers.my_server.env_http_headers]
"X-API-Key" = "API_KEY"
"X-Session-ID" = "SESSION_ID"
```

**注意**：如果同名，`env_http_headers` 优先级更高（会覆盖 `http_headers`）。

---

## 安全性考虑

### 1. 敏感信息保护

❌ **错误做法**（暴露在配置文件中）：

```toml
[mcp_servers.my_server.http_headers]
"X-API-Key" = "sk-1234567890abcdef"  # 敏感信息暴露！
```

✅ **正确做法**（使用环境变量）：

```toml
[mcp_servers.my_server.env_http_headers]
"X-API-Key" = "API_KEY_ENV_VAR"
```

```bash
export API_KEY_ENV_VAR="sk-1234567890abcdef"
codex run
```

### 2. 环境变量命名建议

- 使用清晰的前缀，如 `CODEX_`, `MCP_`, `APP_`
- 避免使用常见系统环境变量名（如 `PATH`, `HOME`）
- 使用大写字母和下划线分隔

```bash
# 推荐
export CODEX_API_KEY="..."
export MCP_AUTH_TOKEN="..."
export APP_SESSION_ID="..."

# 不推荐
export api_key="..."
export token="..."
export id="..."
```

### 3. 生命周期管理

环境变量在进程启动时确定，MCP 客户端初始化时读取。如果需要在运行时更新：

- **方案 A**：重启 Codex（简单但不优雅）
- **方案 B**：在 Codex 内部动态设置环境变量（需要修改源码）
- **方案 C**：使用配置文件热加载（需要额外实现）

---

## 限制和注意事项

### 1. 环境变量不存在时的行为

**不会报错**，只是不添加该请求头。

示例：

```toml
[mcp_servers.my_server.env_http_headers]
"X-Optional-Header" = "OPTIONAL_VAR"
"X-Required-Header" = "REQUIRED_VAR"
```

如果 `OPTIONAL_VAR` 不存在，`X-Optional-Header` 不会被添加，但不会影响 `X-Required-Header`。

### 2. 空值处理

空字符串会被跳过：

```bash
export EMPTY_VAR=""
```

配置：

```toml
[mcp_servers.my_server.env_http_headers]
"X-Header" = "EMPTY_VAR"
```

结果：`X-Header` 不会被添加。

### 3. HTTP 请求头验证

请求头名称和值必须符合 HTTP 规范：

- **名称**：只能包含 ASCII 字母、数字、连字符（`-`）
- **值**：不能包含控制字符、换行符等

如果不符合规范，会记录警告日志并跳过。

### 4. 仅适用于 HTTP 传输

`env_http_headers` **仅适用于 `StreamableHttp` 传输方式**，不适用于 `Stdio` 传输。

对于 Stdio 传输，需要使用 `env` 或 `env_vars` 配置：

```toml
[mcp_servers.local_server]
command = "node"
args = ["server.js"]

# 通过 env 传递环境变量给子进程
[mcp_servers.local_server.env]
SESSION_ID = "my-session-123"

# 或通过 env_vars 从父进程继承
env_vars = ["SESSION_ID", "USER_ID"]
```

---

## 实际示例

### 示例 1：多环境配置

**配置文件**（`~/.codex/config.toml`）：

```toml
[mcp_servers.backend_api]
url = "https://api.example.com/mcp"

[mcp_servers.backend_api.http_headers]
"X-Client" = "Codex"

[mcp_servers.backend_api.env_http_headers]
"X-Environment" = "DEPLOYMENT_ENV"
"X-API-Key" = "API_KEY"
"X-Debug" = "DEBUG_MODE"
```

**开发环境启动脚本**（`dev.sh`）：

```bash
#!/bin/bash
export DEPLOYMENT_ENV="development"
export API_KEY="dev-key-12345"
export DEBUG_MODE="true"
codex run
```

**生产环境启动脚本**（`prod.sh`）：

```bash
#!/bin/bash
export DEPLOYMENT_ENV="production"
export API_KEY="$(vault read -field=value secret/api-key)"
export DEBUG_MODE="false"
codex run
```

### 示例 2：传递 Codex 内部会话 ID（需要源码修改）

**配置文件**：

```toml
[mcp_servers.analytics]
url = "https://analytics.example.com/mcp"

[mcp_servers.analytics.env_http_headers]
"X-Codex-Conversation-ID" = "CODEX_INTERNAL_CONVERSATION_ID"
"X-Codex-User" = "CODEX_USER"
```

**源码修改**（在 `codex-rs/core/src/codex.rs`）：

```rust
// 在 Session 创建后
impl Session {
    pub(crate) fn new(conversation_id: ThreadId, /* ... */) -> Self {
        // 导出内部会话 ID 到环境变量
        std::env::set_var(
            "CODEX_INTERNAL_CONVERSATION_ID",
            conversation_id.to_string()
        );
        
        // 导出用户信息（如果有）
        if let Ok(user) = std::env::var("USER") {
            std::env::set_var("CODEX_USER", user);
        }
        
        // ... 创建 Session
    }
}
```

**MCP 服务器端**（Node.js）：

```javascript
app.post('/mcp', (req, res) => {
  // 读取 Codex 传递的会话 ID
  const conversationId = req.headers['x-codex-conversation-id'];
  const user = req.headers['x-codex-user'];
  
  console.log(`Request from Codex - Conversation: ${conversationId}, User: ${user}`);
  
  // 记录到数据库
  logToDatabase({
    conversationId,
    user,
    timestamp: new Date(),
    request: req.body,
  });
  
  // 处理 MCP 请求
  // ...
});
```

---

## 调试技巧

### 1. 查看实际发送的请求头

在 MCP 服务器端记录请求头：

```javascript
app.use((req, res, next) => {
  console.log('Received headers:', req.headers);
  next();
});
```

### 2. 检查环境变量

在启动 Codex 前：

```bash
# 列出所有环境变量
env | grep CODEX

# 检查特定环境变量
echo $CODEX_SESSION_ID
```

### 3. 启用 Codex 日志

```bash
RUST_LOG=rmcp_client=debug codex run
```

会输出请求头验证的警告信息。

### 4. 使用网络抓包工具

- **Wireshark**：抓取 HTTP 请求包
- **Charles Proxy**：代理 HTTP 请求并查看请求头
- **mitmproxy**：命令行 HTTP 代理工具

---

## 总结

### `env_http_headers` 的核心特性

1. **动态性**：值从环境变量读取，可以在运行时确定
2. **安全性**：敏感信息不暴露在配置文件中
3. **灵活性**：支持多环境配置，无需修改配置文件
4. **容错性**：环境变量不存在时不报错，只是跳过
5. **优先级**：高于 `http_headers`（如果同名会覆盖）

### 使用建议

- ✅ **推荐用于**：
  - API 密钥、访问令牌等敏感信息
  - 会话 ID、用户 ID 等动态上下文
  - 多环境配置（开发/测试/生产）

- ❌ **不推荐用于**：
  - 固定不变的元数据（如客户端版本号）
  - Stdio 传输的 MCP 服务器（使用 `env` 或 `env_vars`）

### 与其他配置的关系

| 配置项 | 用途 | 值来源 | 适用传输 |
|-------|------|--------|---------|
| `http_headers` | 静态请求头 | 配置文件 | HTTP |
| `env_http_headers` | 动态请求头 | 环境变量 | HTTP |
| `bearer_token_env_var` | 认证令牌 | 环境变量 | HTTP |
| `env` | 环境变量传递 | 配置文件 | Stdio |
| `env_vars` | 环境变量继承 | 父进程 | Stdio |

---

**文档版本**：v1.0  
**最后更新**：2026-01-24  
**相关文档**：
- `docs/mcp-session-id-analysis-zh.md`
- `docs/mcp-session-id-extension-analysis-zh.md`
- `docs/mcp-internal-session-id-analysis-zh.md`
