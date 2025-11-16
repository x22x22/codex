# Codex App-Server 教学级解读

## 概述

`codex app-server` 是 Codex 用于支持富交互界面（如 [Codex VS Code 扩展](https://marketplace.visualstudio.com/items?itemName=openai.chatgpt)）的核心通信组件。它提供了一个基于 JSON-RPC 2.0 的双向通信协议，通过 stdio 进行 JSONL 流式通信。

## 核心架构

### 1. 通信协议

app-server 使用 JSON-RPC 2.0 协议（省略 `"jsonrpc":"2.0"` 头），类似于 [MCP (Model Context Protocol)](https://modelcontextprotocol.io/)，支持：

- **请求-响应模式**：客户端发送请求，服务器返回响应
- **通知模式**：服务器主动推送事件通知给客户端
- **双向通信**：通过 stdin/stdout 进行流式 JSONL 通信

### 2. 主要组件

#### 2.1 MessageProcessor
负责处理所有传入的 JSON-RPC 消息：
- 请求（Request）
- 响应（Response）
- 通知（Notification）
- 错误（Error）

```rust
// 核心处理流程
pub async fn process_request(&mut self, request: JSONRPCRequest) {
    // 1. 解析请求
    // 2. 验证初始化状态
    // 3. 路由到相应的处理器
    // 4. 返回响应或错误
}
```

#### 2.2 CodexMessageProcessor
处理具体的 Codex 业务逻辑：
- 会话管理（创建、恢复、列表、归档）
- 消息发送（发送用户消息和回合）
- 认证（API Key、ChatGPT 登录）
- 配置管理
- 文件搜索
- 模型选择

#### 2.3 通信通道

app-server 使用 tokio 异步运行时和多个独立任务：

```rust
// 三个主要任务
1. stdin_reader: 从 stdin 读取消息 -> incoming_tx
2. processor: 处理消息 incoming_rx -> outgoing_tx
3. stdout_writer: 写入响应到 stdout <- outgoing_rx
```

## 协议详解

### 3. 客户端请求类型

#### 3.1 初始化流程

```typescript
// 1. Initialize - 必须首先调用
{
  "method": "initialize",
  "id": 1,
  "params": {
    "clientInfo": {
      "name": "vscode-codex",
      "version": "1.0.0",
      "title": "VS Code Codex Extension"
    }
  }
}

// 响应
{
  "id": 1,
  "result": {
    "userAgent": "codex_cli_rs/1.0.0 (macOS 14.0; arm64) vscode-codex; 1.0.0"
  }
}
```

#### 3.2 会话管理

##### 创建新会话
```typescript
// NewConversation
{
  "method": "newConversation",
  "id": 2,
  "params": {
    "model": "o3-mini",              // 可选：指定模型
    "cwd": "/path/to/project",       // 可选：工作目录
    "approval_policy": "untrusted",  // 可选：审批策略
    "sandbox": "workspace-write",    // 可选：沙箱模式
    "config": {                      // 可选：配置覆盖
      "reasoning_effort": "high"
    }
  }
}

// 响应
{
  "id": 2,
  "result": {
    "conversation_id": "uuid-string",
    "model": "o3-mini",
    "reasoning_effort": "high",
    "rollout_path": "/Users/user/.codex/sessions/uuid.jsonl"
  }
}
```

##### 恢复现有会话
```typescript
// ResumeConversation
{
  "method": "resumeConversation",
  "id": 3,
  "params": {
    "conversation_id": "uuid-string"
  }
}

// 响应包含初始消息历史
{
  "id": 3,
  "result": {
    "conversation_id": "uuid-string",
    "model": "o3-mini",
    "initial_messages": [...],
    "rollout_path": "/Users/user/.codex/sessions/uuid.jsonl"
  }
}
```

##### 列出会话
```typescript
// ListConversations - 带分页和过滤
{
  "method": "listConversations",
  "id": 4,
  "params": {
    "page_size": 20,
    "cursor": "optional-pagination-cursor",
    "model_providers": ["openai"]  // 按模型提供商过滤
  }
}
```

#### 3.3 消息交互

##### 发送用户消息（新API）
```typescript
// SendUserMessage - 推荐的新API
{
  "method": "sendUserMessage",
  "id": 5,
  "params": {
    "conversation_id": "uuid-string",
    "items": [
      {
        "type": "text",
        "text": "请帮我修复这个 bug"
      },
      {
        "type": "image",
        "image": "base64-encoded-image-data"
      }
    ]
  }
}
```

##### 发送用户回合（旧API）
```typescript
// SendUserTurn - 向后兼容的API
{
  "method": "sendUserTurn",
  "id": 6,
  "params": {
    "conversation_id": "uuid-string",
    "message": {
      "text": "请帮我修复这个 bug",
      "role": "user"
    }
  }
}
```

#### 3.4 会话监听

订阅会话事件以接收实时更新：

```typescript
// AddConversationListener
{
  "method": "addConversationListener",
  "id": 7,
  "params": {
    "conversation_id": "uuid-string",
    "experimental_raw_events": false  // 是否接收原始事件
  }
}

// 响应
{
  "id": 7,
  "result": {
    "subscription_id": "subscription-uuid"
  }
}

// 之后会收到通知
{
  "method": "conversationEvent",
  "params": {
    "subscription_id": "subscription-uuid",
    "event": {
      "type": "assistant_message",
      "content": "我来帮你分析这个问题..."
    }
  }
}
```

### 4. 服务器通知类型

服务器主动推送的事件通知：

#### 4.1 会话事件通知
```typescript
{
  "method": "conversationEvent",
  "params": {
    "subscription_id": "uuid",
    "event": {
      "type": "agent_message",
      "text": "正在分析代码...",
      // ... 其他事件数据
    }
  }
}
```

#### 4.2 认证状态变化
```typescript
{
  "method": "authStatusChange",
  "params": {
    "auth_mode": "chatgpt",
    "logged_in": true,
    "email": "user@example.com"
  }
}
```

#### 4.3 会话配置通知
```typescript
{
  "method": "sessionConfigured",
  "params": {
    "conversation_id": "uuid",
    "model": "o3-mini",
    "config": { /* 配置详情 */ }
  }
}
```

## 使用场景与示例

### 场景 1：构建自定义 IDE 扩展

**需求**：在自定义的代码编辑器中集成 Codex 功能

**实现步骤**：

```typescript
// 1. 启动 app-server 进程
import { spawn } from 'child_process';

const appServer = spawn('codex-app-server', [], {
  stdio: ['pipe', 'pipe', 'inherit']
});

// 2. 设置消息处理
let requestId = 0;

function sendRequest(method: string, params: any): Promise<any> {
  return new Promise((resolve, reject) => {
    const id = ++requestId;
    const request = { method, id, params };
    
    appServer.stdin.write(JSON.stringify(request) + '\n');
    
    // 监听响应
    const onData = (data: Buffer) => {
      const lines = data.toString().split('\n');
      for (const line of lines) {
        if (line.trim()) {
          const response = JSON.parse(line);
          if (response.id === id) {
            appServer.stdout.off('data', onData);
            resolve(response.result);
          }
        }
      }
    };
    
    appServer.stdout.on('data', onData);
  });
}

// 3. 初始化
await sendRequest('initialize', {
  clientInfo: {
    name: 'my-custom-ide',
    version: '1.0.0'
  }
});

// 4. 创建会话
const session = await sendRequest('newConversation', {
  cwd: '/path/to/project',
  model: 'o3-mini'
});

// 5. 发送消息
await sendRequest('sendUserMessage', {
  conversation_id: session.conversation_id,
  items: [{ type: 'text', text: '重构这个函数' }]
});

// 6. 监听事件
appServer.stdout.on('data', (data: Buffer) => {
  const lines = data.toString().split('\n');
  for (const line of lines) {
    if (line.trim()) {
      const msg = JSON.parse(line);
      if (msg.method === 'conversationEvent') {
        // 处理会话事件
        handleEvent(msg.params.event);
      }
    }
  }
});
```

### 场景 2：构建 Web 应用界面

**需求**：创建一个 Web 界面与 Codex 交互

**实现思路**：

```typescript
// 后端服务（Node.js + Express）
import express from 'express';
import { spawn } from 'child_process';
import { WebSocketServer } from 'ws';

const app = express();
const wss = new WebSocketServer({ port: 8080 });

// 为每个 WebSocket 连接启动独立的 app-server
wss.on('connection', (ws) => {
  const appServer = spawn('codex-app-server');
  
  // 转发客户端消息到 app-server
  ws.on('message', (data) => {
    appServer.stdin.write(data + '\n');
  });
  
  // 转发 app-server 响应到客户端
  appServer.stdout.on('data', (data) => {
    ws.send(data.toString());
  });
  
  // 清理
  ws.on('close', () => {
    appServer.kill();
  });
});
```

### 场景 3：命令行自动化工具

**需求**：创建一个脚本批量处理多个项目

```typescript
import { spawn } from 'child_process';

async function processProject(projectPath: string) {
  const appServer = spawn('codex-app-server', [], {
    stdio: ['pipe', 'pipe', 'inherit']
  });
  
  // 初始化
  await sendMessage(appServer, {
    method: 'initialize',
    id: 1,
    params: {
      clientInfo: { name: 'batch-processor', version: '1.0.0' }
    }
  });
  
  // 创建会话
  const session = await sendMessage(appServer, {
    method: 'newConversation',
    id: 2,
    params: { cwd: projectPath }
  });
  
  // 执行任务
  await sendMessage(appServer, {
    method: 'sendUserMessage',
    id: 3,
    params: {
      conversation_id: session.result.conversation_id,
      items: [{ 
        type: 'text', 
        text: '分析代码质量并生成报告' 
      }]
    }
  });
  
  // 等待完成...
  appServer.kill();
}

// 批量处理
const projects = ['/proj1', '/proj2', '/proj3'];
for (const proj of projects) {
  await processProject(proj);
}
```

### 场景 4：使用 TypeScript SDK（推荐）

**最简单的方式**：使用官方 TypeScript SDK，它封装了 app-server 的复杂性

```typescript
import { Codex } from '@openai/codex-sdk';

// SDK 内部会启动和管理 app-server 进程
const codex = new Codex();
const thread = codex.startThread({
  workingDirectory: '/path/to/project'
});

// 发送消息并等待完成
const turn = await thread.run('修复测试失败');
console.log(turn.finalResponse);
console.log(turn.items);

// 流式处理
const { events } = await thread.runStreamed('实现修复');
for await (const event of events) {
  switch (event.type) {
    case 'item.completed':
      console.log('完成:', event.item);
      break;
    case 'turn.completed':
      console.log('用量:', event.usage);
      break;
  }
}
```

## 高级功能

### 5.1 沙箱控制

控制 Codex 的文件系统访问权限：

```typescript
{
  "method": "newConversation",
  "id": 1,
  "params": {
    "sandbox": "read-only",           // 只读模式
    // "sandbox": "workspace-write",  // 工作区写入
    // "sandbox": "danger-full-access" // 完全访问
    "approval_policy": "untrusted"    // 需要审批
  }
}
```

### 5.2 命令审批

响应命令执行审批请求：

```typescript
// 服务器请求审批
{
  "method": "conversation/approval/request",
  "id": 10,
  "params": {
    "conversation_id": "uuid",
    "command": "rm -rf node_modules",
    "assessment": "potentially_dangerous"
  }
}

// 客户端响应
{
  "id": 10,
  "result": {
    "approved": false,
    "reason": "危险命令，拒绝执行"
  }
}
```

### 5.3 补丁应用审批

```typescript
// 应用代码补丁
{
  "method": "applyPatchApproval",
  "id": 11,
  "params": {
    "conversation_id": "uuid",
    "approval_id": "approval-uuid",
    "decision": "approve"  // 或 "reject"
  }
}
```

### 5.4 模糊文件搜索

在项目中快速查找文件：

```typescript
{
  "method": "fuzzyFileSearch",
  "id": 12,
  "params": {
    "query": "component",
    "root_path": "/path/to/project",
    "max_results": 10
  }
}

// 响应
{
  "id": 12,
  "result": {
    "results": [
      { "path": "src/components/Header.tsx", "score": 0.95 },
      { "path": "src/components/Footer.tsx", "score": 0.92 }
    ]
  }
}
```

### 5.5 认证管理

#### API Key 登录
```typescript
{
  "method": "loginApiKey",
  "id": 13,
  "params": {
    "api_key": "sk-..."
  }
}
```

#### ChatGPT 登录
```typescript
// 启动登录流程
{
  "method": "loginChatGpt",
  "id": 14,
  "params": {}
}

// 响应包含登录 URL
{
  "id": 14,
  "result": {
    "login_url": "https://...",
    "login_id": "uuid"
  }
}

// 登录完成后收到通知
{
  "method": "loginChatGptComplete",
  "params": {
    "login_id": "uuid",
    "success": true
  }
}
```

## 协议生成与类型安全

### 导出 TypeScript 类型

app-server 提供了工具来生成 TypeScript 类型定义：

```bash
# 生成 TypeScript 绑定和 JSON Schema
codex generate-ts --out ./generated
```

这会生成：
- TypeScript 类型定义（.ts 文件）
- JSON Schema 文件（用于验证）

## 最佳实践

### 1. 错误处理

```typescript
try {
  const response = await sendRequest('sendUserMessage', params);
} catch (error) {
  if (error.code === -32600) {
    // 无效请求
  } else if (error.code === -32601) {
    // 方法不存在
  } else if (error.code === -32603) {
    // 内部错误
  }
}
```

### 2. 连接管理

- 始终先调用 `initialize`
- 正确处理进程生命周期
- 实现超时机制
- 优雅关闭连接

```typescript
// 设置超时
const timeout = setTimeout(() => {
  appServer.kill('SIGTERM');
}, 60000);

// 清理
process.on('exit', () => {
  appServer.kill();
});
```

### 3. 事件处理

```typescript
// 分离不同类型的消息
appServer.stdout.on('data', (data) => {
  const lines = data.toString().split('\n');
  for (const line of lines) {
    if (!line.trim()) continue;
    
    const msg = JSON.parse(line);
    
    if (msg.id !== undefined) {
      // 响应消息
      handleResponse(msg);
    } else if (msg.method !== undefined) {
      // 通知消息
      handleNotification(msg);
    } else if (msg.error !== undefined) {
      // 错误消息
      handleError(msg);
    }
  }
});
```

### 4. 性能优化

- 复用 app-server 进程（多个会话）
- 使用流式处理减少延迟
- 正确实现背压控制
- 监控内存使用

## 调试技巧

### 启用详细日志

```bash
# 设置环境变量
RUST_LOG=debug codex-app-server
```

### 消息追踪

在代码中添加日志：

```typescript
// 记录发送的消息
const request = { method, id, params };
console.log('发送:', JSON.stringify(request, null, 2));
appServer.stdin.write(JSON.stringify(request) + '\n');

// 记录接收的消息
appServer.stdout.on('data', (data) => {
  console.log('接收:', data.toString());
});
```

### 使用测试套件

参考 `codex-rs/app-server/tests/suite/` 中的测试用例来理解正确的使用方式。

## 架构图

```
┌─────────────┐
│   Client    │ (VS Code, Web UI, Custom App)
│ (TypeScript)│
└──────┬──────┘
       │ JSON-RPC over stdio
       │ (JSONL stream)
       ↓
┌──────────────────────────────────────┐
│      codex-app-server               │
├──────────────────────────────────────┤
│ ┌────────────┐   ┌────────────────┐ │
│ │  Message   │ → │ Codex Message  │ │
│ │ Processor  │   │   Processor    │ │
│ └────────────┘   └────────────────┘ │
│       ↕                   ↕          │
│ ┌────────────┐   ┌────────────────┐ │
│ │   Auth     │   │ Conversation   │ │
│ │  Manager   │   │   Manager      │ │
│ └────────────┘   └────────────────┘ │
└──────────────┬───────────────────────┘
               │
               ↓
┌─────────────────────────────────────┐
│        codex-core                   │
│  (会话管理、工具调用、LLM 交互)      │
└─────────────────────────────────────┘
```

## 总结

`codex app-server` 提供了一个强大而灵活的接口，使得任何客户端都能够集成 Codex 的能力。通过标准的 JSON-RPC 协议和清晰的消息模式，开发者可以：

1. **构建自定义 UI**：VS Code 扩展、Web 应用、桌面应用
2. **自动化任务**：批处理脚本、CI/CD 集成
3. **嵌入式集成**：将 Codex 集成到现有工具链

对于大多数用途，推荐使用官方的 [TypeScript SDK](../sdk/typescript/README.md)，它提供了更高级的抽象和更好的开发体验。如果需要完全控制或实现其他语言的客户端，则可以直接使用 app-server 协议。

## 参考资源

- [Codex App-Server README](../codex-rs/app-server/README.md)
- [TypeScript SDK 文档](../sdk/typescript/README.md)
- [协议定义](../codex-rs/app-server-protocol/src/protocol.rs)
- [测试示例](../codex-rs/app-server/tests/suite/)
- [VS Code 扩展](https://marketplace.visualstudio.com/items?itemName=openai.chatgpt)
