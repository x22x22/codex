# Codex App-Server 快速参考

本文档提供 `codex app-server` 协议的快速查询参考。

## 基础流程

```
1. 启动 app-server 进程
2. 发送 initialize 请求
3. 创建或恢复会话
4. 添加会话监听器（可选）
5. 发送消息 / 执行操作
6. 接收事件通知
7. 清理资源
```

## 常用请求速查

### 初始化

```json
{"method": "initialize", "id": 1, "params": {"clientInfo": {"name": "my-app", "version": "1.0.0"}}}
```

### 会话操作

```json
// 创建新会话
{"method": "newConversation", "id": 2, "params": {"cwd": "/path", "model": "o3-mini"}}

// 恢复会话
{"method": "resumeConversation", "id": 3, "params": {"conversation_id": "uuid"}}

// 列出会话
{"method": "listConversations", "id": 4, "params": {"page_size": 20}}

// 归档会话
{"method": "archiveConversation", "id": 5, "params": {"conversation_id": "uuid"}}
```

### 消息发送

```json
// 发送文本消息
{"method": "sendUserMessage", "id": 6, "params": {
  "conversation_id": "uuid",
  "items": [{"type": "text", "text": "你好"}]
}}

// 发送带图片的消息
{"method": "sendUserMessage", "id": 7, "params": {
  "conversation_id": "uuid",
  "items": [
    {"type": "text", "text": "分析这张图"},
    {"type": "image", "image": "base64..."}
  ]
}}
```

### 会话监听

```json
// 添加监听器
{"method": "addConversationListener", "id": 8, "params": {
  "conversation_id": "uuid",
  "experimental_raw_events": false
}}

// 移除监听器
{"method": "removeConversationListener", "id": 9, "params": {
  "subscription_id": "uuid"
}}
```

### 认证

```json
// API Key 登录
{"method": "loginApiKey", "id": 10, "params": {"api_key": "sk-..."}}

// ChatGPT 登录
{"method": "loginChatGpt", "id": 11, "params": {}}

// 获取认证状态
{"method": "getAuthStatus", "id": 12, "params": {}}

// 登出
{"method": "logoutChatGpt", "id": 13, "params": {}}
```

### 其他操作

```json
// 列出模型
{"method": "model/list", "id": 14, "params": {}}

// 设置默认模型
{"method": "setDefaultModel", "id": 15, "params": {"model": "o3-mini"}}

// 中断会话
{"method": "interruptConversation", "id": 16, "params": {"conversation_id": "uuid"}}

// 模糊文件搜索
{"method": "fuzzyFileSearch", "id": 17, "params": {
  "query": "component",
  "root_path": "/path",
  "max_results": 10
}}

// 执行一次性命令
{"method": "execOneOffCommand", "id": 18, "params": {
  "argv": ["ls", "-la"],
  "cwd": "/path"
}}
```

## 常见事件通知

### 会话事件

```json
// 助手消息
{"method": "conversationEvent", "params": {
  "subscription_id": "uuid",
  "event": {"type": "agent_message", "text": "..."}
}}

// 推理过程
{"method": "conversationEvent", "params": {
  "subscription_id": "uuid",
  "event": {"type": "reasoning", "text": "..."}
}}

// 命令执行
{"method": "conversationEvent", "params": {
  "subscription_id": "uuid",
  "event": {
    "type": "command_execution",
    "command": "ls -la",
    "status": "completed",
    "exit_code": 0,
    "output": "..."
  }
}}

// 文件变更
{"method": "conversationEvent", "params": {
  "subscription_id": "uuid",
  "event": {
    "type": "file_change",
    "path": "/path/to/file.ts",
    "kind": "modified"
  }
}}

// 回合完成
{"method": "conversationEvent", "params": {
  "subscription_id": "uuid",
  "event": {
    "type": "turn_completed",
    "usage": {
      "input_tokens": 100,
      "output_tokens": 50
    }
  }
}}

// 回合失败
{"method": "conversationEvent", "params": {
  "subscription_id": "uuid",
  "event": {
    "type": "turn_failed",
    "error": "错误信息"
  }
}}
```

### 认证事件

```json
// 认证状态变化
{"method": "authStatusChange", "params": {
  "auth_mode": "chatgpt",
  "logged_in": true,
  "email": "user@example.com"
}}

// ChatGPT 登录完成
{"method": "loginChatGptComplete", "params": {
  "login_id": "uuid",
  "success": true
}}
```

### 会话配置

```json
// 会话已配置
{"method": "sessionConfigured", "params": {
  "conversation_id": "uuid",
  "model": "o3-mini",
  "config": {...}
}}
```

## 错误代码

| 代码 | 含义 | 说明 |
|------|------|------|
| -32600 | Invalid Request | 无效的请求格式 |
| -32601 | Method not found | 请求的方法不存在 |
| -32602 | Invalid params | 参数无效 |
| -32603 | Internal error | 服务器内部错误 |
| -32700 | Parse error | JSON 解析错误 |

## 配置参数

### NewConversation 参数

| 参数 | 类型 | 说明 |
|------|------|------|
| model | string | 模型名称（如 "o3-mini"） |
| model_provider | string | 模型提供商 |
| profile | string | 配置文件名 |
| cwd | string | 工作目录 |
| approval_policy | string | 审批策略：untrusted, on-failure, on-request, never |
| sandbox | string | 沙箱模式：read-only, workspace-write, danger-full-access |
| config | object | 配置覆盖 |
| base_instructions | string | 自定义指令 |
| include_apply_patch_tool | boolean | 是否包含补丁应用工具 |

### 沙箱模式

- **read-only**: 只读模式，不能修改文件
- **workspace-write**: 可以在工作区内写入文件
- **danger-full-access**: 完全访问权限（危险）

### 审批策略

- **untrusted**: 不信任的命令需要审批
- **on-failure**: 失败时需要审批
- **on-request**: 总是请求审批
- **never**: 永不审批（自动执行）

## 环境变量

```bash
# 启用详细日志
RUST_LOG=debug codex-app-server

# 指定 Codex 主目录
CODEX_HOME=/custom/path codex-app-server
```

## 典型消息流

### 创建会话并发送消息

```
Client -> Server: initialize
Server -> Client: initialize response

Client -> Server: newConversation
Server -> Client: newConversation response

Client -> Server: addConversationListener
Server -> Client: addConversationListener response

Client -> Server: sendUserMessage
Server -> Client: sendUserMessage response (immediate)
Server -> Client: conversationEvent (agent_message)
Server -> Client: conversationEvent (command_execution)
Server -> Client: conversationEvent (turn_completed)
```

### 中断执行

```
Client -> Server: sendUserMessage
Server -> Client: sendUserMessage response
Server -> Client: conversationEvent (agent_message)

Client -> Server: interruptConversation
Server -> Client: conversationEvent (turn_aborted)
Server -> Client: interruptConversation response
```

## TypeScript 类型定义

生成类型定义：

```bash
codex generate-ts --out ./types
```

这会生成：
- TypeScript 接口定义
- JSON Schema 文件
- 与当前 Codex 版本兼容的类型

## 最小化示例

### Python

```python
import json
import subprocess

# 启动进程
proc = subprocess.Popen(
    ['codex-app-server'],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    text=True
)

def send(method, params, id=1):
    msg = json.dumps({"method": method, "id": id, "params": params})
    proc.stdin.write(msg + '\n')
    proc.stdin.flush()
    return json.loads(proc.stdout.readline())

# 使用
send('initialize', {'clientInfo': {'name': 'py-client', 'version': '1.0'}})
session = send('newConversation', {'cwd': '.'})
print(session)
```

### JavaScript/Node.js

```javascript
const { spawn } = require('child_process');

const proc = spawn('codex-app-server');
let id = 0;

function send(method, params) {
  const msg = JSON.stringify({ method, id: ++id, params });
  proc.stdin.write(msg + '\n');
}

proc.stdout.on('data', (data) => {
  const lines = data.toString().split('\n');
  lines.forEach(line => {
    if (line.trim()) {
      console.log(JSON.parse(line));
    }
  });
});

// 使用
send('initialize', { clientInfo: { name: 'js-client', version: '1.0' } });
send('newConversation', { cwd: '.' });
```

### Shell/Bash

```bash
#!/bin/bash

# 启动 app-server
codex-app-server &
APP_PID=$!

# 发送请求
send_request() {
  local method=$1
  local params=$2
  local id=$3
  echo "{\"method\":\"$method\",\"id\":$id,\"params\":$params}"
}

# 初始化
send_request "initialize" '{"clientInfo":{"name":"bash-client","version":"1.0"}}' 1

# 创建会话
send_request "newConversation" '{"cwd":"."}' 2

# 清理
kill $APP_PID
```

## 调试技巧

### 1. 记录所有消息

```typescript
// 拦截所有 stdin/stdout
const originalWrite = process.stdin.write.bind(process.stdin);
process.stdin.write = (data: any) => {
  console.error('→', data.toString());
  return originalWrite(data);
};

const originalOn = process.stdout.on.bind(process.stdout);
process.stdout.on = (event: string, handler: any) => {
  if (event === 'data') {
    return originalOn(event, (data: any) => {
      console.error('←', data.toString());
      handler(data);
    });
  }
  return originalOn(event, handler);
};
```

### 2. 验证 JSON 格式

```javascript
function validateMessage(msg) {
  try {
    JSON.parse(JSON.stringify(msg));
    return true;
  } catch (e) {
    console.error('Invalid JSON:', e);
    return false;
  }
}
```

### 3. 超时处理

```typescript
const timeout = (ms: number) => new Promise((_, reject) =>
  setTimeout(() => reject(new Error('Timeout')), ms)
);

const response = await Promise.race([
  sendRequest(method, params),
  timeout(30000)
]);
```

## 性能建议

1. **复用连接**: 一个 app-server 进程可以管理多个会话
2. **使用流式 API**: 对于长时间操作，使用事件监听而不是轮询
3. **批量操作**: 尽可能合并多个操作到一个回合
4. **资源清理**: 及时移除不需要的监听器
5. **背压控制**: 监控 stdin/stdout 缓冲区大小

## 常见问题

### Q: 如何知道回合何时完成？

A: 监听 `turn_completed` 或 `turn_failed` 事件。

### Q: 可以并发发送多个请求吗？

A: 可以，使用不同的 ID。服务器会并行处理。

### Q: 如何处理长时间运行的操作？

A: 使用会话监听器接收增量更新，不要阻塞等待最终响应。

### Q: 消息顺序有保证吗？

A: 对于同一个会话，事件按顺序到达。不同会话的事件可能交错。

## 相关资源

- [完整文档](./app-server.md)
- [实战示例](./app-server-examples.md)
- [TypeScript SDK](../sdk/typescript/README.md)
- [源码](../codex-rs/app-server/)
- [协议定义](../codex-rs/app-server-protocol/src/protocol.rs)
