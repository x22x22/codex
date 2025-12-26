# Langfuse Integration with Codex

本文档说明如何将 Codex 与 Langfuse 进行集成，实现 LLM 应用的可观测性和追踪。

## Langfuse 简介

[Langfuse](https://langfuse.com/) 是一个开源的 LLM 工程平台，帮助团队协作开发、监控、评估和调试 AI 应用程序。主要功能包括：

- **LLM 应用可观测性**：追踪 LLM 调用和相关逻辑（检索、嵌入、代理操作等）
- **提示词管理**：集中管理和版本控制提示词
- **评估**：支持 LLM-as-a-judge、用户反馈收集、手动标注等
- **数据集**：用于测试和基准测试的数据集管理
- **LLM Playground**：测试和迭代提示词的工具
- **全面的 API**：支持自定义 LLMOps 工作流

## Codex 的 OpenTelemetry 支持

Codex 已经内置了 OpenTelemetry (OTEL) 支持，可以导出以下事件：

- `codex.conversation_starts` - 会话开始
- `codex.api_request` - API 请求
- `codex.sse_event` - SSE 事件
- `codex.user_prompt` - 用户提示
- `codex.tool_decision` - 工具决策
- `codex.tool_result` - 工具结果

每个事件都包含丰富的元数据：
- `conversation.id` - 会话 ID
- `app.version` - 应用版本
- `auth_mode` - 认证模式
- `user.account_id` - 用户账户 ID
- `user.email` - 用户邮箱
- `terminal.type` - 终端类型
- `model` - 使用的模型
- `slug` - 模型标识

## 集成方式

Codex 与 Langfuse 的集成主要通过 OpenTelemetry 协议进行，**无需修改代码**，只需配置即可。

### 集成架构

```
Codex CLI
    |
    | (OpenTelemetry Events)
    |
    v
OTLP HTTP Exporter
    |
    | (HTTP POST /api/public/otel)
    |
    v
Langfuse Platform
    |
    v
Langfuse UI (Traces, Analytics, Evaluations)
```

### 工作原理

1. **Codex 生成事件**：Codex 在运行时生成 OpenTelemetry 日志和追踪事件
2. **OTLP 导出**：通过 OTLP HTTP 导出器将事件发送到 Langfuse
3. **Langfuse 处理**：Langfuse 接收 OTEL 数据并映射到其数据模型
4. **可视化分析**：在 Langfuse UI 中查看和分析追踪数据

## 配置步骤

### 1. 获取 Langfuse API 密钥

首先需要获取 Langfuse 的 API 密钥：

#### 选项 A：使用 Langfuse Cloud（推荐）

1. 访问 [https://cloud.langfuse.com](https://cloud.langfuse.com) 注册账户
2. 创建项目
3. 在项目设置中获取 `Public Key (pk-lf-...)` 和 `Secret Key (sk-lf-...)`

#### 选项 B：自托管 Langfuse

如果你想自己部署 Langfuse：

```bash
# 克隆 Langfuse 仓库
git clone https://github.com/langfuse/langfuse.git
cd langfuse

# 使用 Docker Compose 启动
docker compose up
```

更多部署选项请参考 [Langfuse 自托管文档](https://langfuse.com/docs/deployment/self-host)。

### 2. 配置 Codex

在 `~/.codex/config.toml` 中添加 OTEL 配置：

```toml
[otel]
# 标记追踪的环境（dev, staging, prod）
environment = "production"

# 是否在追踪中记录用户提示（默认为 false 以保护隐私）
log_user_prompt = true

# 配置 OTLP HTTP 导出器连接到 Langfuse
[otel.exporter."otlp-http"]
# Langfuse 的 OTEL 端点
# 🇪🇺 EU region (欧洲区域)
endpoint = "https://cloud.langfuse.com/api/public/otel"

# 🇺🇸 US region (美国区域) - 取消注释以使用
# endpoint = "https://us.cloud.langfuse.com/api/public/otel"

# 🏠 Local deployment (本地部署) - 取消注释以使用
# endpoint = "http://localhost:3000/api/public/otel"

# 使用 binary protobuf 协议（推荐，比 JSON 更高效）
protocol = "binary"

[otel.exporter."otlp-http".headers]
# Langfuse 使用 Basic Auth 认证
# 格式：base64(public_key:secret_key)
# 将 YOUR_AUTH_STRING 替换为你的认证字符串
"Authorization" = "Basic YOUR_AUTH_STRING"
```

### 3. 生成 Authorization 字符串

Langfuse 使用 HTTP Basic Auth，需要将 API 密钥进行 base64 编码：

```bash
# 替换为你的实际密钥
echo -n "pk-lf-your-public-key:sk-lf-your-secret-key" | base64
```

**注意**：在 GNU 系统上，如果密钥很长，可能需要添加 `-w 0` 参数：

```bash
echo -n "pk-lf-your-public-key:sk-lf-your-secret-key" | base64 -w 0
```

将生成的 base64 字符串替换配置文件中的 `YOUR_AUTH_STRING`。

### 4. 完整配置示例

```toml
# ~/.codex/config.toml

# 模型配置
model = "gpt-5.1"

# OpenTelemetry 配置
[otel]
environment = "production"
log_user_prompt = true

[otel.exporter."otlp-http"]
endpoint = "https://cloud.langfuse.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "Basic cGstbGYteW91ci1wdWJsaWMta2V5OnNrLWxmLXlvdXItc2VjcmV0LWtleQ=="
```

### 5. 使用环境变量（可选）

如果你不想在配置文件中硬编码 API 密钥，可以使用环境变量：

```toml
[otel.exporter."otlp-http"]
endpoint = "https://cloud.langfuse.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "${LANGFUSE_AUTH}"
```

然后在环境中设置：

```bash
export LANGFUSE_AUTH="Basic $(echo -n "pk-lf-...:sk-lf-..." | base64)"
```

## 启用追踪导出

默认情况下，Codex 的 OTEL 导出是禁用的（`exporter = "none"`）。配置好 Langfuse 后，需要设置导出器类型：

```toml
[otel]
environment = "production"
exporter = "otlp-http"  # 启用 HTTP 导出
log_user_prompt = true

[otel.exporter."otlp-http"]
endpoint = "https://cloud.langfuse.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "Basic YOUR_AUTH_STRING"
```

## 追踪和日志

Codex 同时支持日志和追踪的导出。如果你想分别配置日志和追踪的导出目标：

```toml
[otel]
environment = "production"
# 日志导出到 Langfuse
exporter = "otlp-http"
# 追踪也导出到 Langfuse（可以配置到不同的端点）
trace_exporter = "otlp-http"

[otel.exporter."otlp-http"]
endpoint = "https://cloud.langfuse.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "Basic YOUR_AUTH_STRING"
```

## TLS/SSL 配置

如果你的自托管 Langfuse 实例使用自签名证书或需要客户端证书：

```toml
[otel.exporter."otlp-http"]
endpoint = "https://langfuse.example.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "Basic YOUR_AUTH_STRING"

# TLS 配置
[otel.exporter."otlp-http".tls]
# 自定义 CA 证书
ca-certificate = "certs/ca.pem"
# 客户端证书（用于 mTLS）
client-certificate = "certs/client.pem"
# 客户端私钥
client-private-key = "certs/client-key.pem"
```

## 验证集成

配置完成后，运行 Codex：

```bash
codex
```

然后在 Langfuse UI 中查看追踪数据：

1. 登录 Langfuse（cloud.langfuse.com 或你的自托管实例）
2. 进入你的项目
3. 导航到 "Traces" 页面
4. 你应该能看到来自 Codex 的追踪记录

每个 Codex 会话将显示为一个追踪，包含：
- 会话开始事件
- API 请求和响应
- 用户输入
- 工具调用和结果
- Token 使用统计

## 数据映射

Langfuse 将 OpenTelemetry 的数据映射到其数据模型：

### 追踪级别属性

- **trace name**: 从根 span 的名称派生
- **userId**: 映射自 `user.account_id` 或 `langfuse.user.id`
- **sessionId**: 映射自 `conversation.id` 或 `langfuse.session.id`
- **metadata**: 包含所有 OpenTelemetry 属性
- **environment**: 映射自 OTEL 配置的 `environment`

### 观测级别属性（Observation）

- **type**: `span`, `generation`, 或 `event`
- **model**: 从 `model` 属性获取
- **input/output**: 从相关事件数据提取
- **usage**: Token 使用统计（input_tokens, output_tokens, cached_tokens）
- **metadata**: 事件特定的元数据

## 高级功能

### 1. 用户和会话追踪

Codex 自动包含用户和会话信息，在 Langfuse 中你可以：
- 按用户过滤追踪
- 查看特定会话的所有交互
- 分析用户行为模式

### 2. 成本分析

Langfuse 可以自动计算和聚合 LLM 调用的成本（基于 token 使用和模型定价）。

### 3. 评估和反馈

你可以在 Langfuse UI 中：
- 为追踪添加手动评分
- 配置自动评估规则
- 收集用户反馈

### 4. 数据集和实验

利用 Langfuse 的数据集功能：
- 从生产追踪创建测试数据集
- 运行实验比较不同配置
- 追踪改进效果

## 隐私考虑

### 用户提示日志

默认情况下，`log_user_prompt = false` 会隐藏用户输入的提示词内容。如果你想记录完整的用户输入（用于调试或分析），设置为 `true`。

```toml
[otel]
log_user_prompt = true  # 记录用户提示（注意隐私）
```

### 敏感数据

确保你的 Langfuse 配置符合你的隐私和安全要求：
- 使用自托管部署处理敏感数据
- 在记录用户输入前获得用户同意
- 定期审查和清理旧数据

## 故障排除

### 1. 追踪未出现在 Langfuse

检查：
- Codex 配置中 `exporter` 是否设置为 `"otlp-http"`（而不是 `"none"`）
- Authorization header 是否正确（base64 编码的 API 密钥）
- 网络连接是否正常（防火墙、代理）
- Langfuse 端点 URL 是否正确

### 2. 认证失败（401 错误）

- 验证 API 密钥是否正确
- 确保 base64 编码没有多余的换行符或空格
- 检查是否使用了正确的 public key 和 secret key

### 3. 不支持 gRPC

Langfuse **不支持** gRPC 协议的 OTLP 端点。必须使用 HTTP：

```toml
[otel]
exporter = "otlp-http"  # ✅ 支持

# 不要使用 otlp-grpc
# exporter = "otlp-grpc"  # ❌ Langfuse 不支持
```

### 4. 自托管版本问题

如果使用自托管的 Langfuse，确保版本 >= v3.22.0，因为 OpenTelemetry 端点是在该版本中引入的。

```bash
# 升级到最新版本
git pull
docker compose down
docker compose up
```

## 与其他工具的比较

| 功能 | Langfuse | 自定义 OTLP Collector | 云原生解决方案 |
|------|----------|----------------------|----------------|
| LLM 特定功能 | ✅ 原生支持 | ❌ 需要自定义 | 部分支持 |
| 提示词管理 | ✅ 内置 | ❌ 无 | ❌ 无 |
| 成本追踪 | ✅ 自动 | ❌ 需要自定义 | 部分支持 |
| 评估工具 | ✅ 丰富 | ❌ 无 | 有限 |
| 部署难度 | 🟢 简单 | 🟡 中等 | 🟢 简单 |
| 数据控制 | ✅ 可自托管 | ✅ 完全控制 | ❌ 云端 |
| 开源 | ✅ MIT | ✅ 依赖项目 | ❌ 专有 |

## 参考资源

### Langfuse 文档
- [Langfuse 主页](https://langfuse.com/)
- [OpenTelemetry 集成指南](https://langfuse.com/docs/integrations/opentelemetry)
- [属性映射文档](https://langfuse.com/docs/integrations/opentelemetry#property-mapping)
- [自托管部署](https://langfuse.com/docs/deployment/self-host)

### Codex 文档
- [Codex 配置文档](./config.md#observability-and-telemetry)
- [OpenTelemetry 概述](https://opentelemetry.io/)

### 示例项目
- [Langfuse GitHub](https://github.com/langfuse/langfuse)
- [Langfuse Python SDK](https://github.com/langfuse/langfuse-python)

## 总结

Codex 与 Langfuse 的集成非常简单：

1. **无需代码更改** - 纯配置集成
2. **利用现有基础设施** - 使用 Codex 的 OpenTelemetry 支持
3. **丰富的 LLM 特定功能** - Langfuse 专为 LLM 应用设计
4. **灵活部署** - 支持云端和自托管
5. **开源** - 完全可控和可定制

通过这个集成，你可以获得：
- 完整的 LLM 调用追踪和可视化
- 成本分析和优化建议
- 提示词版本管理
- 评估和实验工具
- 团队协作功能

开始使用 Langfuse，让你的 Codex 应用可观测性提升到新的水平！
