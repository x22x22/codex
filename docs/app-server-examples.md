# Codex App-Server 实战示例

这个文件包含了完整的、可运行的 `codex app-server` 使用示例。

## 示例 1：基础客户端实现

### Node.js 完整实现

```typescript
// codex-client.ts
import { spawn, ChildProcess } from 'child_process';
import { EventEmitter } from 'events';
import * as readline from 'readline';

interface JSONRPCRequest {
  method: string;
  id: number;
  params: any;
}

interface JSONRPCResponse {
  id: number;
  result?: any;
  error?: {
    code: number;
    message: string;
    data?: any;
  };
}

interface JSONRPCNotification {
  method: string;
  params: any;
}

export class CodexClient extends EventEmitter {
  private process: ChildProcess;
  private nextId: number = 1;
  private pendingRequests: Map<number, {
    resolve: (value: any) => void;
    reject: (error: any) => void;
  }> = new Map();
  private initialized: boolean = false;

  constructor(codexPath: string = 'codex-app-server') {
    super();
    
    // 启动 app-server 进程
    this.process = spawn(codexPath, [], {
      stdio: ['pipe', 'pipe', 'pipe']
    });

    // 设置输出处理
    const rl = readline.createInterface({
      input: this.process.stdout!,
      crlfDelay: Infinity
    });

    rl.on('line', (line) => {
      if (line.trim()) {
        this.handleMessage(JSON.parse(line));
      }
    });

    // 设置错误处理
    this.process.stderr!.on('data', (data) => {
      console.error('stderr:', data.toString());
    });

    this.process.on('exit', (code) => {
      console.log(`app-server 进程退出，代码: ${code}`);
      this.emit('exit', code);
    });
  }

  private handleMessage(msg: any) {
    if (msg.id !== undefined) {
      // 这是对某个请求的响应
      const pending = this.pendingRequests.get(msg.id);
      if (pending) {
        this.pendingRequests.delete(msg.id);
        if (msg.error) {
          pending.reject(msg.error);
        } else {
          pending.resolve(msg.result);
        }
      }
    } else if (msg.method) {
      // 这是一个通知
      this.emit('notification', msg);
      this.emit(`notification:${msg.method}`, msg.params);
    }
  }

  private async sendRequest(method: string, params: any): Promise<any> {
    if (!this.initialized && method !== 'initialize') {
      throw new Error('客户端尚未初始化，请先调用 initialize()');
    }

    const id = this.nextId++;
    const request: JSONRPCRequest = { method, id, params };

    return new Promise((resolve, reject) => {
      this.pendingRequests.set(id, { resolve, reject });
      
      const json = JSON.stringify(request) + '\n';
      this.process.stdin!.write(json, (err) => {
        if (err) {
          this.pendingRequests.delete(id);
          reject(err);
        }
      });

      // 设置超时
      setTimeout(() => {
        if (this.pendingRequests.has(id)) {
          this.pendingRequests.delete(id);
          reject(new Error(`请求超时: ${method}`));
        }
      }, 60000); // 60 秒超时
    });
  }

  async initialize(clientInfo: { name: string; version: string; title?: string }) {
    const result = await this.sendRequest('initialize', { clientInfo });
    this.initialized = true;
    return result;
  }

  async newConversation(params: {
    model?: string;
    cwd?: string;
    approval_policy?: string;
    sandbox?: string;
    config?: Record<string, any>;
  } = {}) {
    return this.sendRequest('newConversation', params);
  }

  async resumeConversation(conversationId: string) {
    return this.sendRequest('resumeConversation', { conversation_id: conversationId });
  }

  async sendUserMessage(conversationId: string, items: Array<{
    type: string;
    text?: string;
    image?: string;
  }>) {
    return this.sendRequest('sendUserMessage', {
      conversation_id: conversationId,
      items
    });
  }

  async addConversationListener(conversationId: string, experimentalRawEvents: boolean = false) {
    return this.sendRequest('addConversationListener', {
      conversation_id: conversationId,
      experimental_raw_events: experimentalRawEvents
    });
  }

  async removeConversationListener(subscriptionId: string) {
    return this.sendRequest('removeConversationListener', {
      subscription_id: subscriptionId
    });
  }

  async listConversations(params: {
    page_size?: number;
    cursor?: string;
    model_providers?: string[];
  } = {}) {
    return this.sendRequest('listConversations', params);
  }

  async interruptConversation(conversationId: string) {
    return this.sendRequest('interruptConversation', {
      conversation_id: conversationId
    });
  }

  async listModels(params: { provider?: string } = {}) {
    return this.sendRequest('model/list', params);
  }

  async loginApiKey(apiKey: string) {
    return this.sendRequest('loginApiKey', { api_key: apiKey });
  }

  async loginChatGpt() {
    return this.sendRequest('loginChatGpt', {});
  }

  async getAuthStatus(params: { auth_mode?: string } = {}) {
    return this.sendRequest('getAuthStatus', params);
  }

  async fuzzyFileSearch(params: {
    query: string;
    root_path: string;
    max_results?: number;
  }) {
    return this.sendRequest('fuzzyFileSearch', params);
  }

  close() {
    this.process.kill();
  }
}

// 使用示例
async function main() {
  const client = new CodexClient();

  try {
    // 1. 初始化
    console.log('初始化客户端...');
    await client.initialize({
      name: 'demo-client',
      version: '1.0.0',
      title: 'Demo Client'
    });
    console.log('初始化完成');

    // 2. 创建会话
    console.log('创建新会话...');
    const session = await client.newConversation({
      cwd: process.cwd(),
      model: 'o3-mini',
      sandbox: 'workspace-write'
    });
    console.log('会话已创建:', session);

    // 3. 添加会话监听器
    console.log('添加会话监听器...');
    const subscription = await client.addConversationListener(
      session.conversation_id,
      false
    );
    console.log('监听器已添加:', subscription);

    // 监听会话事件
    client.on('notification:conversationEvent', (params) => {
      console.log('会话事件:', JSON.stringify(params, null, 2));
    });

    // 4. 发送消息
    console.log('发送用户消息...');
    await client.sendUserMessage(session.conversation_id, [
      { type: 'text', text: '你好，请介绍一下你自己。' }
    ]);

    // 等待一段时间以接收响应
    await new Promise(resolve => setTimeout(resolve, 10000));

    // 5. 清理
    console.log('清理资源...');
    await client.removeConversationListener(subscription.subscription_id);
    client.close();

  } catch (error) {
    console.error('错误:', error);
    client.close();
    process.exit(1);
  }
}

// 运行示例
if (require.main === module) {
  main().catch(console.error);
}
```

## 示例 2：交互式聊天应用

```typescript
// interactive-chat.ts
import { CodexClient } from './codex-client';
import * as readline from 'readline';

class InteractiveChatApp {
  private client: CodexClient;
  private conversationId: string | null = null;
  private subscriptionId: string | null = null;
  private rl: readline.Interface;

  constructor() {
    this.client = new CodexClient();
    this.rl = readline.createInterface({
      input: process.stdin,
      output: process.stdout
    });
  }

  async start() {
    console.log('=== Codex 交互式聊天应用 ===\n');

    // 初始化
    await this.client.initialize({
      name: 'interactive-chat',
      version: '1.0.0'
    });

    // 创建会话
    const session = await this.client.newConversation({
      cwd: process.cwd()
    });
    this.conversationId = session.conversation_id;
    console.log(`会话已创建: ${this.conversationId}\n`);

    // 添加监听器
    const subscription = await this.client.addConversationListener(
      this.conversationId
    );
    this.subscriptionId = subscription.subscription_id;

    // 处理会话事件
    this.client.on('notification:conversationEvent', (params) => {
      this.handleConversationEvent(params);
    });

    // 开始交互循环
    await this.chatLoop();
  }

  private handleConversationEvent(params: any) {
    const event = params.event;
    
    switch (event.type) {
      case 'agent_message':
        process.stdout.write(`\n助手: ${event.text}`);
        break;
      
      case 'reasoning':
        process.stdout.write(`\n[思考中] ${event.text}`);
        break;
      
      case 'command_execution':
        console.log(`\n[执行命令] ${event.command}`);
        if (event.output) {
          console.log(`输出: ${event.output}`);
        }
        break;
      
      case 'file_change':
        console.log(`\n[文件变更] ${event.path}: ${event.kind}`);
        break;
      
      case 'turn_completed':
        console.log('\n\n--- 回合结束 ---\n');
        this.prompt();
        break;
      
      case 'turn_failed':
        console.error(`\n错误: ${event.error}`);
        this.prompt();
        break;
    }
  }

  private prompt() {
    this.rl.question('你: ', async (input) => {
      const trimmed = input.trim();
      
      if (!trimmed) {
        this.prompt();
        return;
      }

      if (trimmed.toLowerCase() === 'exit' || trimmed.toLowerCase() === 'quit') {
        await this.cleanup();
        process.exit(0);
      }

      if (trimmed.toLowerCase() === 'interrupt') {
        await this.client.interruptConversation(this.conversationId!);
        console.log('已发送中断请求');
        this.prompt();
        return;
      }

      // 发送消息
      try {
        await this.client.sendUserMessage(this.conversationId!, [
          { type: 'text', text: trimmed }
        ]);
      } catch (error) {
        console.error('发送消息失败:', error);
        this.prompt();
      }
    });
  }

  private async chatLoop() {
    console.log('输入消息开始聊天，输入 "exit" 退出，"interrupt" 中断当前回合\n');
    this.prompt();
  }

  private async cleanup() {
    console.log('\n清理资源...');
    if (this.subscriptionId) {
      await this.client.removeConversationListener(this.subscriptionId);
    }
    this.client.close();
    this.rl.close();
  }
}

// 运行应用
const app = new InteractiveChatApp();
app.start().catch((error) => {
  console.error('启动失败:', error);
  process.exit(1);
});
```

## 示例 3：批量代码审查工具

```typescript
// batch-code-review.ts
import { CodexClient } from './codex-client';
import * as fs from 'fs/promises';
import * as path from 'path';

interface ReviewResult {
  file: string;
  conversation_id: string;
  rollout_path: string;
  summary: string;
}

class BatchCodeReviewer {
  private client: CodexClient;
  private results: ReviewResult[] = [];

  constructor() {
    this.client = new CodexClient();
  }

  async initialize() {
    await this.client.initialize({
      name: 'batch-code-reviewer',
      version: '1.0.0'
    });
  }

  async reviewFile(filePath: string): Promise<ReviewResult> {
    console.log(`\n审查文件: ${filePath}`);

    // 读取文件内容
    const content = await fs.readFile(filePath, 'utf-8');
    
    // 创建新会话
    const session = await this.client.newConversation({
      cwd: path.dirname(filePath),
      model: 'o3-mini'
    });

    console.log(`  会话创建: ${session.conversation_id}`);

    // 添加监听器
    await this.client.addConversationListener(session.conversation_id);

    let summary = '';
    let completed = false;

    // 监听事件
    const eventHandler = (params: any) => {
      if (params.subscription_id !== session.conversation_id) return;
      
      const event = params.event;
      if (event.type === 'agent_message') {
        summary += event.text;
      } else if (event.type === 'turn_completed') {
        completed = true;
      }
    };

    this.client.on('notification:conversationEvent', eventHandler);

    // 发送审查请求
    const prompt = `
请审查以下代码文件并提供详细反馈：

文件名: ${path.basename(filePath)}
路径: ${filePath}

代码内容:
\`\`\`
${content}
\`\`\`

请关注以下方面：
1. 代码质量和最佳实践
2. 潜在的 bug 或安全问题
3. 性能优化建议
4. 可读性和可维护性
5. 测试覆盖率建议

请以结构化的方式提供反馈。
    `.trim();

    await this.client.sendUserMessage(session.conversation_id, [
      { type: 'text', text: prompt }
    ]);

    // 等待完成
    while (!completed) {
      await new Promise(resolve => setTimeout(resolve, 500));
    }

    // 移除监听器
    this.client.off('notification:conversationEvent', eventHandler);

    const result: ReviewResult = {
      file: filePath,
      conversation_id: session.conversation_id,
      rollout_path: session.rollout_path,
      summary
    };

    this.results.push(result);
    console.log(`  审查完成！`);

    return result;
  }

  async reviewDirectory(dirPath: string, pattern: RegExp = /\.(ts|js|tsx|jsx)$/) {
    console.log(`批量审查目录: ${dirPath}`);
    
    const files = await this.findFiles(dirPath, pattern);
    console.log(`找到 ${files.length} 个文件\n`);

    for (const file of files) {
      try {
        await this.reviewFile(file);
      } catch (error) {
        console.error(`审查文件失败 ${file}:`, error);
      }
    }

    return this.results;
  }

  private async findFiles(dir: string, pattern: RegExp): Promise<string[]> {
    const results: string[] = [];
    const entries = await fs.readdir(dir, { withFileTypes: true });

    for (const entry of entries) {
      const fullPath = path.join(dir, entry.name);
      
      if (entry.isDirectory()) {
        // 跳过 node_modules 等目录
        if (!['node_modules', '.git', 'dist', 'build'].includes(entry.name)) {
          results.push(...await this.findFiles(fullPath, pattern));
        }
      } else if (entry.isFile() && pattern.test(entry.name)) {
        results.push(fullPath);
      }
    }

    return results;
  }

  async generateReport(outputPath: string) {
    const report = {
      timestamp: new Date().toISOString(),
      total_files: this.results.length,
      reviews: this.results.map(r => ({
        file: r.file,
        conversation_id: r.conversation_id,
        rollout_path: r.rollout_path,
        summary: r.summary
      }))
    };

    await fs.writeFile(outputPath, JSON.stringify(report, null, 2));
    console.log(`\n报告已生成: ${outputPath}`);
  }

  close() {
    this.client.close();
  }
}

// 使用示例
async function main() {
  const reviewer = new BatchCodeReviewer();
  
  try {
    await reviewer.initialize();
    
    const projectDir = process.argv[2] || process.cwd();
    await reviewer.reviewDirectory(projectDir);
    
    await reviewer.generateReport('./code-review-report.json');
    
    reviewer.close();
  } catch (error) {
    console.error('审查失败:', error);
    reviewer.close();
    process.exit(1);
  }
}

if (require.main === module) {
  main();
}
```

## 示例 4：CI/CD 集成

```typescript
// ci-integration.ts
import { CodexClient } from './codex-client';
import * as fs from 'fs/promises';

interface CIResult {
  success: boolean;
  conversation_id: string;
  summary: string;
  errors?: string[];
}

class CIIntegration {
  private client: CodexClient;

  constructor() {
    this.client = new CodexClient();
  }

  async runCIChecks(params: {
    checkType: 'test' | 'lint' | 'security' | 'docs';
    projectPath: string;
  }): Promise<CIResult> {
    await this.client.initialize({
      name: 'ci-integration',
      version: '1.0.0'
    });

    const session = await this.client.newConversation({
      cwd: params.projectPath,
      sandbox: 'workspace-write'
    });

    await this.client.addConversationListener(session.conversation_id);

    let summary = '';
    let errors: string[] = [];
    let success = true;

    // 根据检查类型生成提示
    const prompts = {
      test: '运行所有测试并报告结果。如果有失败的测试，分析原因并建议修复。',
      lint: '运行代码检查工具（如 ESLint）并报告所有问题。修复可以自动修复的问题。',
      security: '扫描安全漏洞并报告发现的问题。检查依赖项的已知漏洞。',
      docs: '检查文档完整性。确保所有公共 API 都有文档，且文档是最新的。'
    };

    this.client.on('notification:conversationEvent', (eventParams) => {
      const event = eventParams.event;
      
      if (event.type === 'agent_message') {
        summary += event.text;
      } else if (event.type === 'command_execution') {
        if (event.exit_code !== 0) {
          success = false;
          errors.push(`命令失败: ${event.command} (退出码: ${event.exit_code})`);
        }
      } else if (event.type === 'turn_failed') {
        success = false;
        errors.push(event.error);
      }
    });

    await this.client.sendUserMessage(session.conversation_id, [
      { type: 'text', text: prompts[params.checkType] }
    ]);

    // 等待完成
    await new Promise(resolve => setTimeout(resolve, 30000));

    this.client.close();

    return {
      success,
      conversation_id: session.conversation_id,
      summary,
      errors: errors.length > 0 ? errors : undefined
    };
  }
}

// GitHub Actions 集成示例
async function githubActionIntegration() {
  const ci = new CIIntegration();
  const checkType = (process.env.CHECK_TYPE || 'test') as 'test' | 'lint' | 'security' | 'docs';
  
  try {
    console.log(`运行 ${checkType} 检查...`);
    
    const result = await ci.runCIChecks({
      checkType,
      projectPath: process.cwd()
    });

    console.log('\n=== CI 检查结果 ===');
    console.log(`状态: ${result.success ? '✓ 通过' : '✗ 失败'}`);
    console.log(`会话 ID: ${result.conversation_id}`);
    console.log(`\n摘要:\n${result.summary}`);

    if (result.errors) {
      console.error('\n错误:');
      result.errors.forEach(err => console.error(`  - ${err}`));
    }

    // 设置 GitHub Actions 输出
    if (process.env.GITHUB_OUTPUT) {
      await fs.appendFile(
        process.env.GITHUB_OUTPUT,
        `success=${result.success}\nconversation_id=${result.conversation_id}\n`
      );
    }

    process.exit(result.success ? 0 : 1);
  } catch (error) {
    console.error('CI 检查失败:', error);
    process.exit(1);
  }
}

if (require.main === module) {
  githubActionIntegration();
}
```

## 示例 5：Web 服务器集成

```typescript
// web-server.ts
import express from 'express';
import { WebSocketServer, WebSocket } from 'ws';
import { CodexClient } from './codex-client';

const app = express();
const port = 3000;

app.use(express.json());
app.use(express.static('public'));

// WebSocket 服务器
const wss = new WebSocketServer({ noServer: true });

// 为每个 WebSocket 连接维护一个客户端实例
const clients = new Map<WebSocket, {
  codex: CodexClient;
  conversationId: string | null;
  subscriptionId: string | null;
}>();

wss.on('connection', async (ws: WebSocket) => {
  console.log('新的 WebSocket 连接');

  // 创建 Codex 客户端
  const codex = new CodexClient();
  
  try {
    await codex.initialize({
      name: 'web-client',
      version: '1.0.0'
    });

    const clientData = {
      codex,
      conversationId: null as string | null,
      subscriptionId: null as string | null
    };
    
    clients.set(ws, clientData);

    // 转发 Codex 通知到 WebSocket
    codex.on('notification:conversationEvent', (params) => {
      ws.send(JSON.stringify({
        type: 'conversation_event',
        data: params
      }));
    });

    // 处理 WebSocket 消息
    ws.on('message', async (message: string) => {
      try {
        const data = JSON.parse(message);
        
        switch (data.type) {
          case 'create_conversation':
            const session = await codex.newConversation({
              cwd: data.cwd || process.cwd()
            });
            clientData.conversationId = session.conversation_id;
            
            const subscription = await codex.addConversationListener(
              session.conversation_id
            );
            clientData.subscriptionId = subscription.subscription_id;
            
            ws.send(JSON.stringify({
              type: 'conversation_created',
              data: session
            }));
            break;

          case 'send_message':
            if (!clientData.conversationId) {
              ws.send(JSON.stringify({
                type: 'error',
                error: '请先创建会话'
              }));
              return;
            }

            await codex.sendUserMessage(clientData.conversationId, [
              { type: 'text', text: data.message }
            ]);
            break;

          case 'interrupt':
            if (clientData.conversationId) {
              await codex.interruptConversation(clientData.conversationId);
            }
            break;
        }
      } catch (error) {
        ws.send(JSON.stringify({
          type: 'error',
          error: error instanceof Error ? error.message : String(error)
        }));
      }
    });

    // 清理
    ws.on('close', () => {
      console.log('WebSocket 连接关闭');
      codex.close();
      clients.delete(ws);
    });

  } catch (error) {
    console.error('初始化失败:', error);
    ws.close();
  }
});

// HTTP 升级到 WebSocket
const server = app.listen(port, () => {
  console.log(`服务器运行在 http://localhost:${port}`);
});

server.on('upgrade', (request, socket, head) => {
  wss.handleUpgrade(request, socket, head, (ws) => {
    wss.emit('connection', ws, request);
  });
});

// REST API 端点示例
app.post('/api/analyze', async (req, res) => {
  const { code, language } = req.body;
  const codex = new CodexClient();

  try {
    await codex.initialize({
      name: 'api-client',
      version: '1.0.0'
    });

    const session = await codex.newConversation();
    await codex.addConversationListener(session.conversation_id);

    let result = '';
    codex.on('notification:conversationEvent', (params) => {
      if (params.event.type === 'agent_message') {
        result += params.event.text;
      }
    });

    await codex.sendUserMessage(session.conversation_id, [
      {
        type: 'text',
        text: `分析以下 ${language} 代码并提供反馈：\n\n\`\`\`${language}\n${code}\n\`\`\``
      }
    ]);

    // 等待响应
    await new Promise(resolve => setTimeout(resolve, 10000));

    codex.close();
    res.json({ result });
  } catch (error) {
    codex.close();
    res.status(500).json({
      error: error instanceof Error ? error.message : String(error)
    });
  }
});
```

## 前端示例（HTML + JavaScript）

```html
<!-- public/index.html -->
<!DOCTYPE html>
<html>
<head>
  <meta charset="UTF-8">
  <title>Codex Web Chat</title>
  <style>
    body {
      font-family: Arial, sans-serif;
      max-width: 800px;
      margin: 0 auto;
      padding: 20px;
    }
    #chat-container {
      border: 1px solid #ccc;
      height: 400px;
      overflow-y: auto;
      padding: 10px;
      margin-bottom: 10px;
      background: #f9f9f9;
    }
    .message {
      margin: 10px 0;
      padding: 8px;
      border-radius: 5px;
    }
    .user-message {
      background: #e3f2fd;
      text-align: right;
    }
    .assistant-message {
      background: #f1f8e9;
    }
    .system-message {
      background: #fff3e0;
      font-style: italic;
    }
    #input-container {
      display: flex;
      gap: 10px;
    }
    #message-input {
      flex: 1;
      padding: 10px;
      border: 1px solid #ccc;
      border-radius: 5px;
    }
    button {
      padding: 10px 20px;
      background: #4CAF50;
      color: white;
      border: none;
      border-radius: 5px;
      cursor: pointer;
    }
    button:hover {
      background: #45a049;
    }
    button:disabled {
      background: #ccc;
      cursor: not-allowed;
    }
  </style>
</head>
<body>
  <h1>Codex Web Chat</h1>
  
  <div id="chat-container"></div>
  
  <div id="input-container">
    <input 
      type="text" 
      id="message-input" 
      placeholder="输入消息..." 
      disabled
    />
    <button id="send-button" disabled>发送</button>
    <button id="interrupt-button" disabled>中断</button>
  </div>

  <script>
    const chatContainer = document.getElementById('chat-container');
    const messageInput = document.getElementById('message-input');
    const sendButton = document.getElementById('send-button');
    const interruptButton = document.getElementById('interrupt-button');

    let ws = null;
    let connected = false;

    function addMessage(text, type) {
      const div = document.createElement('div');
      div.className = `message ${type}-message`;
      div.textContent = text;
      chatContainer.appendChild(div);
      chatContainer.scrollTop = chatContainer.scrollHeight;
    }

    function connect() {
      ws = new WebSocket(`ws://${location.host}`);

      ws.onopen = () => {
        console.log('WebSocket 已连接');
        connected = true;
        addMessage('已连接到服务器，正在创建会话...', 'system');
        
        // 创建会话
        ws.send(JSON.stringify({
          type: 'create_conversation',
          cwd: null
        }));
      };

      ws.onmessage = (event) => {
        const data = JSON.parse(event.data);
        
        switch (data.type) {
          case 'conversation_created':
            addMessage(`会话已创建: ${data.data.conversation_id}`, 'system');
            messageInput.disabled = false;
            sendButton.disabled = false;
            interruptButton.disabled = false;
            messageInput.focus();
            break;

          case 'conversation_event':
            const eventData = data.data.event;
            if (eventData.type === 'agent_message') {
              addMessage(eventData.text, 'assistant');
            } else if (eventData.type === 'turn_completed') {
              addMessage('--- 回合结束 ---', 'system');
            }
            break;

          case 'error':
            addMessage(`错误: ${data.error}`, 'system');
            break;
        }
      };

      ws.onclose = () => {
        console.log('WebSocket 已断开');
        connected = false;
        addMessage('连接已断开', 'system');
        messageInput.disabled = true;
        sendButton.disabled = true;
        interruptButton.disabled = true;
      };

      ws.onerror = (error) => {
        console.error('WebSocket 错误:', error);
        addMessage('连接错误', 'system');
      };
    }

    function sendMessage() {
      const message = messageInput.value.trim();
      if (!message || !connected) return;

      addMessage(message, 'user');
      
      ws.send(JSON.stringify({
        type: 'send_message',
        message
      }));

      messageInput.value = '';
    }

    function interrupt() {
      if (!connected) return;
      
      ws.send(JSON.stringify({
        type: 'interrupt'
      }));
      
      addMessage('已发送中断请求', 'system');
    }

    // 事件监听
    sendButton.onclick = sendMessage;
    interruptButton.onclick = interrupt;
    messageInput.onkeypress = (e) => {
      if (e.key === 'Enter') {
        sendMessage();
      }
    };

    // 启动连接
    connect();
  </script>
</body>
</html>
```

## 运行这些示例

### 准备工作

```bash
# 1. 安装依赖
npm install

# 2. 确保 codex-app-server 可执行
# 如果需要从源码构建：
cd codex-rs/app-server
cargo build --release

# 3. 将构建的二进制文件添加到 PATH 或使用完整路径
```

### 运行示例

```bash
# 基础客户端
npx ts-node codex-client.ts

# 交互式聊天
npx ts-node interactive-chat.ts

# 批量代码审查
npx ts-node batch-code-review.ts /path/to/project

# CI 集成
CHECK_TYPE=test npx ts-node ci-integration.ts

# Web 服务器
npx ts-node web-server.ts
# 然后在浏览器中访问 http://localhost:3000
```

## 总结

这些示例展示了 `codex app-server` 的各种实际应用场景：

1. **基础客户端**：展示了完整的协议实现
2. **交互式聊天**：构建终端聊天应用
3. **批量处理**：自动化代码审查
4. **CI/CD 集成**：在持续集成流程中使用
5. **Web 集成**：通过 WebSocket 提供 Web 界面

所有示例都遵循最佳实践：
- 正确的错误处理
- 资源清理
- 超时控制
- 事件驱动架构

可以根据实际需求修改和扩展这些示例。
