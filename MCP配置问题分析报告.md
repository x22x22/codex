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

## 问题分析

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

### 方案 1：验证 API 端点和认证

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

该问题的根本原因是：**智谱 AI 的 MCP 端点可能不完全兼容 MCP Streamable HTTP 协议标准**，或者需要特定的配置参数。

**建议采取的行动：**

1. **首要任务**：查看智谱 AI 官方文档，确认 MCP 支持的状态和正确配置方式
2. **次要任务**：使用环境变量存储 API Key，并尝试不同的认证 header
3. **验证任务**：先测试一个已知可用的 MCP 服务器，确保 Codex 本身的 HTTP MCP 功能正常
4. **最后手段**：联系智谱 AI 技术支持，询问 MCP 服务的正确使用方式

如果智谱 AI 暂不支持标准 MCP 协议，可能需要等待官方更新，或者使用其他方式集成（如自己编写一个 MCP 服务器包装器来调用智谱 AI 的 REST API）。

## 参考资源

- MCP 官方规范：https://modelcontextprotocol.io/specification/2025-06-18/basic
- Codex 文档：https://developers.openai.com/codex
- MCP Streamable HTTP 传输：https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#http-with-sse
