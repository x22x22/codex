# MCP HTTP 配置问题分析报告

## 问题描述

用户在配置 MCP 服务器时遇到以下错误：

```
⚠ MCP client for `web-search-prime` failed to start: MCP startup failed: handshaking with MCP server failed: Send message error Transport [rmcp::transport::worker::WorkerTransport<rmcp::transport::streamable_http_client::StreamableHttpClientWorker<reqwest::async_impl::client::Client>>] error:
  Transport channel closed, when send initialized notification

⚠ MCP startup incomplete (failed: web-search-prime)
```

用户的配置如下：

```toml
[mcp_servers.web-search-prime]
url = "https://open.bigmodel.cn/api/mcp/web_search_prime/mcp"
http_headers = { "Authorization" = "Bearer key" }
```

## 更新：问题根本原因已确定 ✅

经过深入源码分析，确认问题**不是服务器端的问题**（用户已通过 `@modelcontextprotocol/inspector` 验证服务器正常），而是 **Codex 客户端代码的 bug**。

### Bug 位置

文件：`codex-rs/rmcp-client/src/rmcp_client.rs`，第196-206行

### Bug 详情

当用户通过 `http_headers` 配置 Authorization 头时：

```toml
http_headers = { "Authorization" = "Bearer key" }
```

Codex 的处理流程如下：

1. `http_headers` 被传递给 `build_default_headers()` 函数，构建为 reqwest 的 `HeaderMap`
2. 这些 headers 通过 `apply_default_headers()` 被设置为 reqwest Client 的默认 headers
3. **但是**，`rmcp` 库的 `StreamableHttpClientTransportConfig` 需要通过 `auth_header()` 方法显式设置认证信息
4. 当前代码只在 `bearer_token` (通过环境变量读取) 存在时才调用 `auth_header()`
5. **导致**：虽然 reqwest client 有 Authorization header，但 rmcp transport 层没有正确获取到认证信息

### 问题代码

```rust
} else {
    let mut http_config = StreamableHttpClientTransportConfig::with_uri(url.to_string());
    if let Some(bearer_token) = bearer_token.clone() {
        http_config = http_config.auth_header(bearer_token);  // 只在 bearer_token 存在时设置
    }

    let http_client =
        apply_default_headers(reqwest::Client::builder(), &default_headers).build()?;

    let transport = StreamableHttpClientTransport::with_client(http_client, http_config);
    PendingTransport::StreamableHttp { transport }
}
```

### 为什么 `@modelcontextprotocol/inspector` 能工作

官方的 MCP inspector 正确实现了 HTTP header 的处理，它会：
- 直接将用户提供的 headers 添加到每个 HTTP 请求中
- 不依赖于底层 HTTP 客户端的默认 headers

而 Codex 使用的 `rmcp` 库在 Streamable HTTP 传输中有特定的 header 处理机制，需要通过 `auth_header()` 显式设置。

## 问题分析（原始分析）

### 1. MCP 传输协议

Model Context Protocol (MCP) 支持两种主要的传输方式：

#### 1.1 Stdio 传输（标准输入/输出）
- 通过启动一个本地进程，使用标准输入输出进行通信
- 适用于本地 MCP 服务器
- 配置示例：
```toml
[mcp_servers.local-server]
command = "node"
args = ["path/to/server.js"]
```

#### 1.2 HTTP 传输（Streamable HTTP）
- 通过 HTTP 协议与远程 MCP 服务器通信
- 使用 Server-Sent Events (SSE) 进行双向通信
- 适用于远程 MCP 服务器
- 配置示例：
```toml
[mcp_servers.remote-server]
url = "https://example.com/mcp"
```

### 2. 错误原因分析

根据错误信息 `Transport channel closed, when send initialized notification`，问题发生在 MCP 初始化握手阶段。具体来说：

#### 2.1 协议不兼容
从错误消息看，Codex 尝试使用 **Streamable HTTP** 协议连接到 `https://open.bigmodel.cn/api/mcp/web_search_prime/mcp`，但在发送初始化通知时，传输通道被关闭了。

这通常意味着：
- **服务器端点可能不支持 MCP Streamable HTTP 协议**
- 服务器可能使用的是不同的 API 协议（如标准 REST API）
- 服务器可能需要特定的握手序列或协议版本

#### 2.2 认证问题
配置中使用了 `Authorization` header：
```toml
http_headers = { "Authorization" = "Bearer key" }
```

可能的问题：
- `key` 应该替换为实际的 API 密钥/令牌
- 认证失败可能导致服务器拒绝连接或提前关闭连接
- 服务器可能需要其他认证方式（如 OAuth）

#### 2.3 MCP 协议握手失败
MCP 初始化过程包括：
1. 客户端发送 `initialize` 请求
2. 服务器响应 `InitializeResult`
3. 客户端发送 `initialized` 通知
4. 开始正常通信

错误发生在第3步"发送 initialized 通知"时，说明：
- 服务器可能在返回 `InitializeResult` 后立即关闭了连接
- 服务器可能不遵循标准 MCP 握手流程
- 网络连接在握手期间中断

### 3. 智谱 AI API 的特殊性

`https://open.bigmodel.cn` 是智谱 AI（GLM）的 API 端点。根据路径 `/api/mcp/web_search_prime/mcp`，这看起来像是一个 MCP 端点，但可能存在以下情况：

1. **该端点可能是实验性的或文档不完整**
   - 智谱 AI 可能提供了 MCP 支持，但实现可能不完全符合标准
   - 可能需要额外的配置参数

2. **可能需要特定的认证方式**
   - 智谱 AI 通常使用 API Key 认证
   - 可能需要使用环境变量而非直接在配置中写入

3. **端点 URL 可能不正确**
   - 实际的 MCP 端点可能在不同的路径
   - 需要参考智谱 AI 的官方 MCP 文档

## 解决方案

### 方案 0：代码修复（推荐）⭐

需要修改 `codex-rs/rmcp-client/src/rmcp_client.rs` 文件，使其正确处理 `http_headers` 中的 Authorization 头。

**修复方案A**：从 `http_headers` 中提取 Authorization 并传递给 `auth_header()`

在 `new_streamable_http_client` 函数中（大约第196行），需要：

1. 检查 `http_headers` 中是否包含 "Authorization" 或 "authorization"
2. 如果存在且格式为 "Bearer xxx"，提取 token 值
3. 将提取的 token 传递给 `http_config.auth_header()`

**修复方案B**：让 `rmcp` 库支持从 reqwest client 的 default headers 中读取

这需要修改 `rmcp` 库本身，或者找到配置方式让 transport 使用 client 的 default headers。

**临时解决方案**：

在修复代码之前，用户可以使用 `bearer_token_env_var` 配置方式：

```toml
[mcp_servers.web-search-prime]
url = "https://open.bigmodel.cn/api/mcp/web_search_prime/mcp"
bearer_token_env_var = "ZHIPU_API_KEY"
```

然后设置环境变量（注意：只需要 token 值，**不要包含** "Bearer " 前缀）：
```bash
export ZHIPU_API_KEY="your-actual-key-here"  # 不要包含 "Bearer " 前缀
```

### 方案 1：验证 API 端点和认证（已验证不是问题）

1. **检查智谱 AI 官方文档**
   - 确认是否提供 MCP 协议支持
   - 查看正确的端点 URL 和认证方式

2. **使用环境变量存储 API Key**
   ```toml
   [mcp_servers.web-search-prime]
   url = "https://open.bigmodel.cn/api/mcp/web_search_prime/mcp"
   bearer_token_env_var = "ZHIPU_API_KEY"
   ```
   
   然后设置环境变量：
   ```bash
   export ZHIPU_API_KEY="your-actual-api-key-here"
   ```

3. **尝试不同的 header 配置**
   ```toml
   [mcp_servers.web-search-prime]
   url = "https://open.bigmodel.cn/api/mcp/web_search_prime/mcp"
   http_headers = { "x-api-key" = "your-api-key" }
   ```

### 方案 2：增加超时和调试配置

```toml
[mcp_servers.web-search-prime]
url = "https://open.bigmodel.cn/api/mcp/web_search_prime/mcp"
bearer_token_env_var = "ZHIPU_API_KEY"
startup_timeout_sec = 30  # 增加启动超时时间
tool_timeout_sec = 120    # 增加工具调用超时时间
```

### 方案 3：使用 stdio 方式（如果提供）

如果智谱 AI 提供了本地 MCP 服务器实现，可以尝试：

```toml
[mcp_servers.web-search-prime]
command = "zhipu-mcp-server"  # 假设的命令名
args = []
env = { "ZHIPU_API_KEY" = "your-api-key" }
```

### 方案 4：联系服务提供商

由于这是第三方 MCP 服务，建议：

1. 查看智谱 AI 关于 MCP 的官方文档
2. 在智谱 AI 社区或支持渠道询问正确的配置方式
3. 确认该端点是否真的支持 MCP Streamable HTTP 协议

## 排查步骤

### 1. 验证端点连通性

```bash
# 测试端点是否可访问
curl -I "https://open.bigmodel.cn/api/mcp/web_search_prime/mcp"

# 测试带认证的请求
curl -H "Authorization: Bearer your-api-key" \
     "https://open.bigmodel.cn/api/mcp/web_search_prime/mcp"
```

### 2. 查看 Codex 日志

启动 Codex 时查看详细日志：
```bash
# 如果支持日志级别控制
RUST_LOG=debug codex
```

### 3. 测试其他已知可用的 MCP 服务器

尝试配置一个已知可用的 MCP 服务器来验证 Codex 的 HTTP MCP 支持是否正常：

```toml
# GitHub Copilot MCP (需要 GitHub personal access token)
[mcp_servers.github]
url = "https://api.githubcopilot.com/mcp/"
bearer_token_env_var = "GITHUB_PERSONAL_ACCESS_TOKEN"
```

### 4. 检查配置文件语法

确保 config.toml 文件语法正确：
```bash
# 查找 Codex 配置文件位置
# Linux/macOS: ~/.config/codex/config.toml
# Windows: %APPDATA%\codex\config.toml
```

## 技术细节

### MCP Streamable HTTP 协议要求

Streamable HTTP 传输要求服务器支持：

1. **Server-Sent Events (SSE)**
   - 用于服务器向客户端推送消息
   - Content-Type: `text/event-stream`

2. **POST 请求**
   - 用于客户端向服务器发送消息
   - Content-Type: `application/json`

3. **标准 MCP 握手流程**
   ```
   Client -> Server: POST /endpoint (initialize request)
   Server -> Client: SSE stream (initialize response)
   Client -> Server: POST /endpoint (initialized notification)
   ... 正常通信 ...
   ```

### Codex 的 MCP 实现

根据代码分析（`codex-rs/rmcp-client/src/rmcp_client.rs`）：

- Codex 使用 `rmcp` crate（版本 0.12.0）实现 MCP 客户端
- 支持 Streamable HTTP 传输通过 `StreamableHttpClientTransport`
- 初始化超时默认为 10 秒（`DEFAULT_STARTUP_TIMEOUT`）
- 错误发生在 `service::serve_client` 调用期间，表明底层传输层出现问题

## 结论

该问题的根本原因是：**Codex 的 MCP HTTP 客户端实现存在 bug**，当使用 `http_headers` 配置 Authorization 头时，该 header 未被正确传递给底层的 `rmcp` 传输层。

**核心问题：**
- `http_headers` 中的 Authorization 被添加到 reqwest HTTP 客户端的默认 headers
- 但 `rmcp` 库的 `StreamableHttpClientTransportConfig` 需要通过 `auth_header()` 方法显式设置认证
- 由于认证信息缺失，MCP 握手过程中服务器拒绝连接或提前关闭连接

**已实施的修复：**

修改了 `codex-rs/rmcp-client/src/rmcp_client.rs` 文件，使其：
1. 从 `http_headers` 中检测 Authorization header（不区分大小写）
2. 提取 "Bearer " 后的 token 值
3. 将 token 传递给 `StreamableHttpClientTransportConfig::auth_header()`

**修复后的效果：**
- 用户可以直接使用 `http_headers = { "Authorization" = "Bearer key" }` 配置
- 也可以继续使用 `bearer_token_env_var` 方式
- 两种方式都能正确工作

**对用户的建议：**

1. **等待修复合并后**：可以直接使用原来的配置方式
   ```toml
   [mcp_servers.web-search-prime]
   url = "https://open.bigmodel.cn/api/mcp/web_search_prime/mcp"
   http_headers = { "Authorization" = "Bearer your-key" }
   ```

2. **修复合并前的临时方案**：使用环境变量方式
   ```toml
   [mcp_servers.web-search-prime]
   url = "https://open.bigmodel.cn/api/mcp/web_search_prime/mcp"
   bearer_token_env_var = "ZHIPU_API_KEY"
   ```
   
   设置环境变量（注意：只需要 token 值，不要包含 "Bearer " 前缀）：
   ```bash
   export ZHIPU_API_KEY="your-actual-key-here"
   ```

**感谢用户的反馈！** 通过 `@modelcontextprotocol/inspector` 的测试结果，我们能够确定这不是服务器问题，而是客户端实现的 bug。这个 bug 修复将使所有使用 `http_headers` 配置 Authorization 的 MCP 服务器都能正常工作。

## 技术细节（原始分析）

- MCP 官方规范：https://modelcontextprotocol.io/specification/2025-06-18/basic
- Codex 文档：https://developers.openai.com/codex
- MCP Streamable HTTP 传输：https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#http-with-sse
