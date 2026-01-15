# Codex 项目 Reasoning 输出语言问题分析报告

## 问题描述

在 Codex 项目中，即使任务提示(prompt)中明确要求使用中文输出，reasoning（推理过程）部分仍然以英文输出。本报告深入分析该问题的根本原因。

## 技术架构概述

Codex CLI 是 OpenAI 的本地运行编码助手，其核心技术栈包括：

- **后端**: Rust 实现 (位于 `codex-rs/` 目录)
- **API 客户端**: 与 OpenAI API 通信的封装层
- **系统提示**: 预定义的 Markdown 格式指令文件
- **模型支持**: 支持多种 GPT 模型，包括支持 reasoning 功能的模型

## 问题根源分析

### 1. 系统指令(System Instructions)的语言固定为英文

**核心发现**: 所有系统级指令文件均以英文编写，没有多语言支持机制。

**相关文件**:
- `codex-rs/core/prompt.md` - 基础系统提示 (24,230 字节)
- `codex-rs/core/gpt_5_codex_prompt.md` - GPT-5 Codex 专用提示
- `codex-rs/core/gpt_5_1_prompt.md` - GPT-5.1 提示
- `codex-rs/core/gpt_5_2_prompt.md` - GPT-5.2 提示
- `codex-rs/core/gpt-5.1-codex-max_prompt.md` - GPT-5.1 Codex Max 提示
- `codex-rs/core/gpt-5.2-codex_prompt.md` - GPT-5.2 Codex 提示

**代码证据** (`codex-rs/core/src/models_manager/model_info.rs`):

```rust
pub const BASE_INSTRUCTIONS: &str = include_str!("../../prompt.md");
const GPT_5_CODEX_INSTRUCTIONS: &str = include_str!("../../gpt_5_codex_prompt.md");
const GPT_5_1_INSTRUCTIONS: &str = include_str!("../../gpt_5_1_prompt.md");
const GPT_5_2_INSTRUCTIONS: &str = include_str!("../../gpt_5_2_prompt.md");
// ...等
```

这些指令在编译时被硬编码到程序中，**没有运行时语言选择机制**。

### 2. OpenAI Chat Completions API 的语言行为机制

**关键点**: OpenAI API 的 `reasoning` 输出语言**主要由系统消息(system message)的语言决定**，而非用户消息。

**API 请求构建** (`codex-rs/codex-api/src/requests/chat.rs` 第58-60行):

```rust
pub fn build(self, _provider: &Provider) -> Result<ChatRequest, ApiError> {
    let mut messages = Vec::<Value>::new();
    messages.push(json!({"role": "system", "content": self.instructions}));
    // ... 用户消息在后面添加
}
```

**API payload 结构** (第293-299行):

```rust
let payload = json!({
    "model": self.model,
    "messages": messages,
    "stream": true,
    "tools": self.tools,
});
```

**重要发现**: 
1. OpenAI API **没有提供** `language` 或 `locale` 参数来控制输出语言
2. `reasoning` 字段的语言跟随系统指令的语言
3. 即使用户消息使用中文，系统消息仍然是英文，导致模型倾向于用英文进行推理

### 3. Reasoning 内容的处理流程

**Reasoning 数据流**:

1. **生成阶段**: OpenAI API 根据系统指令语言生成 reasoning 内容
2. **传输阶段**: 通过 SSE (Server-Sent Events) 流式传输
3. **本地处理**: Codex 接收并显示 reasoning 内容

**代码证据** (`codex-rs/codex-api/src/requests/chat.rs` 第98-147行):

```rust
if let ResponseItem::Reasoning {
    content: Some(items),
    ..
} = item
{
    let mut text = String::new();
    for entry in items {
        match entry {
            ReasoningItemContent::ReasoningText { text: segment }
            | ReasoningItemContent::Text { text: segment } => {
                text.push_str(segment)
            }
        }
    }
    // reasoning 内容被附加到 assistant 消息上
}
```

Reasoning 内容被**原样处理**，没有翻译或语言转换逻辑。

### 4. 模型选择与指令绑定

**模型配置逻辑** (`codex-rs/core/src/models_manager/model_info.rs`):

不同模型使用不同的系统指令，但**所有指令文件都是英文**：

```rust
pub(crate) fn find_model_info_for_slug(slug: &str) -> ModelInfo {
    if slug.starts_with("o3") || slug.starts_with("o4-mini") {
        model_info!(
            slug,
            base_instructions: BASE_INSTRUCTIONS_WITH_APPLY_PATCH.to_string(),
            supports_reasoning_summaries: true,
            // ...
        )
    } else if slug.starts_with("gpt-5.2-codex") {
        model_info!(
            slug,
            base_instructions: GPT_5_2_CODEX_INSTRUCTIONS.to_string(),
            // ...
        )
    }
    // ... 更多模型配置
}
```

每个模型的 `base_instructions` 字段都指向英文指令文件。

## 为什么用户的中文要求无效

### 场景重现

假设用户输入：
```
请用中文完成以下任务：实现一个排序算法
```

### 实际 API 请求结构

```json
{
  "model": "gpt-5-codex",
  "messages": [
    {
      "role": "system",
      "content": "You are a coding agent running in the Codex CLI... (24KB 英文指令)"
    },
    {
      "role": "user", 
      "content": "请用中文完成以下任务：实现一个排序算法"
    }
  ],
  "stream": true,
  "tools": [...]
}
```

### 模型行为分析

1. **系统指令权重高**: OpenAI 模型在生成 reasoning 时，系统消息的影响力**远大于**用户消息
2. **语言一致性倾向**: 模型倾向于使用与系统指令相同的语言进行内部推理
3. **输出与推理分离**: 模型可能会用中文输出最终答案，但推理过程(reasoning)仍然用英文

## 当前代码中没有语言控制机制

### 检索结果

在整个代码库中搜索语言相关配置：

```bash
grep -rn "language\|locale\|zh\|CN\|中文" codex-rs/
```

**发现**: 
- 唯一的 `locale` 相关代码仅用于**数字格式化** (`protocol/src/num_format.rs`)
- **没有任何**针对输出语言的配置选项
- **没有**多语言系统指令的支持

## 问题的深层原因

### 1. 架构设计层面

Codex CLI 的设计假设：
- **单一语言环境**: 默认所有用户使用英文
- **系统指令不变**: 指令文件在编译时固定
- **无国际化需求**: 没有 i18n (国际化) 框架

### 2. OpenAI API 限制

OpenAI Chat Completions API **本身不提供**:
- `language` 参数
- `locale` 参数  
- `reasoning_language` 参数

语言完全依赖于提示内容本身。

### 3. Reasoning 的特殊性

Reasoning (推理过程) 是模型的**内部思考过程**，它：
- 在生成最终输出之前产生
- 受系统指令语言影响最大
- 不像最终输出那样容易被用户指令改变

## 潜在解决方案(技术分析)

虽然本报告不实现代码，但从技术角度分析可能的解决方案：

### 方案1: 多语言系统指令文件

**实现思路**:
1. 创建中文版本的系统指令文件 (如 `prompt_zh-CN.md`)
2. 在配置中添加 `language` 选项
3. 根据配置选择对应语言的指令文件

**优点**: 
- 彻底解决问题
- reasoning 和输出都会是中文

**缺点**:
- 需要维护多份指令文件
- 需要修改核心架构

### 方案2: 动态指令注入

**实现思路**:
在系统指令末尾动态添加语言要求：
```
... (原有英文指令)

IMPORTANT: All reasoning and output must be in Simplified Chinese (简体中文).
```

**优点**:
- 改动较小
- 不需要翻译整个指令文件

**缺点**:
- 效果可能不如方案1稳定
- reasoning 可能仍然部分英文

### 方案3: Post-processing 翻译

**实现思路**:
接收到 reasoning 后，调用翻译 API 转换

**优点**:
- 不影响模型行为

**缺点**:
- 增加延迟和成本
- 可能影响 reasoning 的准确性
- 实现复杂度高

## 结论

### 核心问题总结

**Codex 项目中 reasoning 部分输出英文的根本原因是**:

1. **系统指令文件全部为英文**，且在编译时硬编码
2. **OpenAI API 的 reasoning 语言主要由系统消息决定**，而非用户消息
3. **没有语言选择机制**，项目未实现国际化支持
4. **用户的中文要求**只影响最终输出文本，对 reasoning 过程影响很小

### 技术债务

这是一个**架构层面的国际化缺失**问题，不是简单的 bug。要彻底解决需要：

1. 引入 i18n 框架
2. 翻译所有系统指令文件
3. 实现语言配置机制
4. 可能需要修改构建流程

### 优先级评估

从项目角度：
- **当前状态**: 项目主要面向英文用户，符合设计预期
- **影响范围**: 仅影响非英文用户的使用体验
- **实现成本**: 中到高（需要架构调整）

## 附录

### 关键代码文件清单

| 文件路径 | 作用 | 关键点 |
|---------|------|--------|
| `codex-rs/core/prompt.md` | 基础系统指令 | 24KB 英文内容 |
| `codex-rs/core/src/models_manager/model_info.rs` | 模型配置与指令绑定 | 硬编码英文指令 |
| `codex-rs/codex-api/src/requests/chat.rs` | API 请求构建 | 系统消息优先级最高 |
| `codex-rs/protocol/src/openai_models.rs` | Reasoning 配置定义 | 无语言参数 |

### 相关 OpenAI 文档

- OpenAI Reasoning API: https://platform.openai.com/docs/guides/reasoning
- Chat Completions API: https://platform.openai.com/docs/api-reference/chat

---

**报告生成时间**: 2026-01-15  
**分析范围**: Codex 项目完整代码库  
**分析方法**: 静态代码分析 + API 行为推断
