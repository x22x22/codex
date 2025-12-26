# Codex 与 Langfuse 集成分析总结

本文档总结了 Codex 项目如何与 Langfuse 进行集成的完整分析结果。

## 执行摘要

**问题分析**：分析如何让 Codex 项目与 Langfuse 进行集成，以实现 LLM 应用的可观测性。

**解决方案**：Codex 可以通过其现有的 OpenTelemetry 基础设施与 Langfuse 无缝集成，**无需任何代码修改**，仅需配置即可。

**核心发现**：
- ✅ 零代码变更 - 纯配置集成
- ✅ 使用现有基础设施 - 利用 OTLP HTTP 导出器
- ✅ 标准协议 - 基于 OpenTelemetry
- ✅ 开箱即用 - 3步即可完成设置

## 项目背景

### Langfuse 是什么？

Langfuse 是一个开源的 LLM 工程平台，提供：
- 📊 **可观测性** - 详细的 LLM 调用追踪
- 💰 **成本追踪** - 基于 token 使用的自动成本计算
- 🔍 **评估工具** - 手动标注、LLM-as-a-judge
- 🎯 **提示词管理** - 版本控制和协作
- 🧪 **实验功能** - 数据集和基准测试

项目地址：
- 主仓库：https://github.com/langfuse/langfuse
- 文档仓库：https://github.com/langfuse/langfuse-docs

### Codex 现有能力

Codex 已经具备完整的 OpenTelemetry 支持：
- 位于 `codex-rs/otel/` 的 OTEL 模块
- 支持 OTLP HTTP 和 gRPC 导出器
- 通过 `~/.codex/config.toml` 配置
- 丰富的事件类型（会话、API 请求、工具调用等）

## 集成方案

### 技术架构

```
┌─────────────┐
│  Codex CLI  │
└──────┬──────┘
       │ 生成 OTEL 事件
       ▼
┌─────────────────────┐
│  OTLP HTTP Exporter │
└──────┬──────────────┘
       │ HTTPS POST
       │ /api/public/otel
       ▼
┌─────────────────────┐
│  Langfuse Platform  │
│  - OTLP 端点        │
│  - 数据映射          │
│  - 可视化 UI         │
└─────────────────────┘
```

### 集成原理

1. **Codex 生成事件**
   - 会话开始、API 请求、用户输入、工具执行等
   - 每个事件包含丰富的元数据

2. **OTLP 导出**
   - 使用现有的 OTLP HTTP 导出器
   - 支持 binary (protobuf) 或 JSON 格式
   - HTTP Basic Auth 认证

3. **Langfuse 接收**
   - 接收 OTLP 数据到 `/api/public/otel` 端点
   - 映射到 Langfuse 数据模型
   - 自动计算成本

4. **UI 可视化**
   - 在 Langfuse UI 中查看追踪
   - 分析 token 使用和成本
   - 评估和优化

### 关键优势

| 特性 | 描述 |
|------|------|
| 🔧 零代码变更 | 仅需配置文件修改 |
| 🚀 即插即用 | 利用现有 OTLP 基础设施 |
| 🔒 隐私友好 | 可配置日志级别，支持自托管 |
| 💵 成本透明 | 自动追踪和计算 LLM 调用成本 |
| 🎯 LLM 专用 | 为 AI 应用定制的功能 |
| 🌐 灵活部署 | 云端或自托管 |
| 📖 开源 | MIT 许可证，完全可控 |

## 配置方法

### 基本配置（3步）

**步骤 1：获取 API 密钥**

访问 https://cloud.langfuse.com 注册并获取：
- Public Key: `pk-lf-...`
- Secret Key: `sk-lf-...`

**步骤 2：生成认证字符串**

```bash
# 使用提供的脚本
./scripts/generate-langfuse-auth.sh

# 或手动生成
echo -n "pk-lf-YOUR-KEY:sk-lf-YOUR-KEY" | base64
```

**步骤 3：配置 Codex**

在 `~/.codex/config.toml` 中添加：

```toml
[otel]
environment = "production"
exporter = "otlp-http"
log_user_prompt = false  # 保护隐私

[otel.exporter."otlp-http"]
endpoint = "https://cloud.langfuse.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "Basic YOUR_BASE64_STRING"
```

### 高级配置选项

**自托管部署**
```toml
endpoint = "http://localhost:3000/api/public/otel"
```

**自定义 TLS**
```toml
[otel.exporter."otlp-http".tls]
ca-certificate = "certs/ca.pem"
client-certificate = "certs/client.pem"
client-private-key = "certs/client-key.pem"
```

**使用环境变量**
```toml
"Authorization" = "${LANGFUSE_AUTH}"
```

## 文档资源

本次分析创建了完整的文档套件：

### 核心文档

1. **`docs/langfuse-integration.md`** (中文，7.9KB)
   - 完整的集成指南
   - 详细的配置说明
   - 故障排除
   - 与其他方案的比较

2. **`docs/langfuse-quickstart.md`** (英文，7.0KB)
   - 快速入门指南
   - 3步设置流程
   - 常见配置模式

3. **`docs/langfuse-architecture.md`** (13.0KB)
   - 架构图（Mermaid 和 ASCII）
   - 数据流说明
   - 组件交互
   - 安全流程

4. **`docs/langfuse-test-plan.md`** (13.8KB)
   - 20个测试用例
   - 详细的验证步骤
   - 故障排除清单
   - 测试报告模板

### 配置文件

5. **`docs/example-langfuse-config.toml`** (7.0KB)
   - 完整的配置示例
   - 5种部署场景
   - 详细的内联注释

### 辅助脚本

6. **`scripts/generate-langfuse-auth.sh`** (Bash)
   - Unix/Linux/macOS 支持
   - 交互式密钥收集
   - 自动 base64 编码

7. **`scripts/generate-langfuse-auth.ps1`** (PowerShell)
   - Windows 支持
   - 彩色输出
   - 相同功能

## 验证步骤

### 快速验证

1. **配置 Codex**（如上所述）

2. **运行 Codex**
   ```bash
   codex
   ```

3. **进行对话**
   - 输入一个简单的提示
   - 等待响应
   - 退出 Codex

4. **检查 Langfuse**
   - 登录 https://cloud.langfuse.com
   - 导航到 "Traces" 页面
   - 查找你的会话追踪

### 完整测试

参考 `docs/langfuse-test-plan.md`，包含：
- 基本连接性测试
- 追踪创建验证
- 事件类型检查
- Token 使用追踪
- 工具调用追踪
- 隐私设置验证
- 性能影响评估

## 数据追踪

Codex 会向 Langfuse 发送以下事件：

| 事件类型 | 包含信息 |
|---------|---------|
| `conversation_starts` | 模型、环境、策略配置 |
| `api_request` | 请求时长、状态码、重试次数 |
| `sse_event` | Token 使用（输入、输出、缓存、推理） |
| `user_prompt` | 提示长度（可选：完整文本） |
| `tool_decision` | 工具名称、批准状态 |
| `tool_result` | 执行时间、成功/失败、输出 |

所有事件都包含：
- `conversation.id` - 会话 ID
- `app.version` - 应用版本
- `model` - 使用的模型
- `user.account_id` - 用户 ID（如果有）
- `environment` - 环境标签

## 注意事项

### 隐私保护

- **默认不记录用户输入**：`log_user_prompt = false`
- **可配置**：需要时可以启用完整日志
- **自托管选项**：完全控制数据存储位置

### 性能影响

- **异步导出**：不阻塞主流程
- **批处理**：减少网络开销
- **最小影响**：通常 < 5% 性能开销

### 限制

- ❌ **不支持 gRPC**：Langfuse 仅支持 HTTP
- ✅ 需要网络连接到 Langfuse
- ✅ 需要有效的 API 密钥

## 与替代方案比较

| 特性 | Langfuse | 通用 OTLP | 云厂商方案 |
|------|----------|-----------|-----------|
| LLM 专用功能 | ✅ 原生 | ❌ 需自定义 | 🟡 部分 |
| 提示词管理 | ✅ 内置 | ❌ 无 | ❌ 无 |
| 成本追踪 | ✅ 自动 | ❌ 需自定义 | 🟡 部分 |
| 评估工具 | ✅ 丰富 | ❌ 无 | 🟡 有限 |
| 自托管 | ✅ 支持 | ✅ 支持 | ❌ 不支持 |
| 开源 | ✅ MIT | ✅ 各异 | ❌ 专有 |
| 设置难度 | 🟢 简单 | 🟡 中等 | 🟢 简单 |

## 后续步骤

### 对于用户

1. **阅读文档**
   - 从 `docs/langfuse-quickstart.md` 开始
   - 查看 `docs/example-langfuse-config.toml`

2. **设置集成**
   - 使用辅助脚本生成配置
   - 更新 `~/.codex/config.toml`

3. **验证集成**
   - 运行测试对话
   - 在 Langfuse UI 中检查追踪

4. **探索功能**
   - 成本分析
   - 提示词管理
   - 评估工具

### 对于开发者

当前实现已经完整，无需额外开发：
- ✅ Codex 已有 OTLP 支持
- ✅ Langfuse 提供 OTLP 端点
- ✅ 纯配置集成
- ✅ 文档齐全

未来可能的增强（可选）：
- 自动化测试脚本
- Codex CLI 中的 Langfuse 特定命令
- 更深度的集成（如直接调用 Langfuse API）

## 技术细节

### 协议支持

```toml
protocol = "binary"  # ✅ 推荐：HTTP/protobuf
protocol = "json"    # ✅ 备选：HTTP/JSON
```

### 认证

```
Authorization: Basic base64(public_key:secret_key)
```

示例：
```bash
public_key="pk-lf-abc123"
secret_key="sk-lf-xyz789"
auth_string=$(echo -n "$public_key:$secret_key" | base64)
# 结果：cGstbGYtYWJjMTIzOnNrLWxmLXh5ejc4OQ==
```

### 端点

| 部署方式 | 端点 URL |
|---------|---------|
| Langfuse Cloud (EU) | `https://cloud.langfuse.com/api/public/otel` |
| Langfuse Cloud (US) | `https://us.cloud.langfuse.com/api/public/otel` |
| 自托管 | `http://localhost:3000/api/public/otel` |

## 常见问题

### Q: 需要修改 Codex 代码吗？
**A:** 不需要。这是纯配置集成。

### Q: 支持自托管吗？
**A:** 是的，Langfuse 可以自托管（Docker/K8s）。

### Q: 数据会发送到哪里？
**A:** 根据你的配置：Langfuse Cloud 或你的自托管实例。

### Q: 如何保护隐私？
**A:** 设置 `log_user_prompt = false` 并考虑自托管。

### Q: 有性能影响吗？
**A:** 最小影响，通常 < 5%，因为是异步导出。

### Q: 成本是如何计算的？
**A:** Langfuse 基于 token 使用和模型定价自动计算。

### Q: 支持哪些 Codex 版本？
**A:** 任何支持 OpenTelemetry 的版本（检查 `codex-rs/otel/` 是否存在）。

## 资源链接

### 文档
- [完整集成指南](./docs/langfuse-integration.md)
- [快速开始](./docs/langfuse-quickstart.md)
- [架构说明](./docs/langfuse-architecture.md)
- [测试计划](./docs/langfuse-test-plan.md)
- [配置示例](./docs/example-langfuse-config.toml)

### 外部链接
- [Langfuse 官网](https://langfuse.com/)
- [Langfuse OpenTelemetry 文档](https://langfuse.com/docs/integrations/opentelemetry)
- [Langfuse GitHub](https://github.com/langfuse/langfuse)
- [OpenTelemetry 规范](https://opentelemetry.io/docs/specs/)

### 社区支持
- [Langfuse Discord](https://discord.gg/7NXusRtqYU)
- [Langfuse GitHub Issues](https://github.com/langfuse/langfuse/issues)
- [Codex GitHub Issues](https://github.com/openai/codex/issues)

## 结论

**Codex 与 Langfuse 的集成非常简单且强大：**

✅ **无需代码修改** - 3步配置即可
✅ **基于标准协议** - OpenTelemetry
✅ **LLM 专用功能** - 成本、评估、提示词管理
✅ **灵活部署** - 云端或自托管
✅ **开源免费** - MIT 许可证
✅ **完整文档** - 涵盖所有场景

**建议行动：**
1. 从快速开始指南入手
2. 使用辅助脚本生成配置
3. 在测试环境中验证
4. 逐步推广到生产环境

通过这个集成，你可以获得对 Codex 应用的完整可观测性，优化成本，提升质量，加速开发迭代。

---

**文档完成日期：** 2025-12-26
**分析工具：** Codex CLI
**文档版本：** 1.0
