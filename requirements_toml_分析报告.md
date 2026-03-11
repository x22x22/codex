# requirements.toml 配置文件分析报告

## 概述

`requirements.toml` 是 Codex 项目中用于**系统级安全策略约束**的配置文件。它允许系统管理员在企业或组织环境中，对 Codex 的行为进行强制性限制，确保所有用户的使用符合安全规范。

## 文件位置

在 Unix/Linux/macOS 系统上：
```
/etc/codex/requirements.toml
```

在 Windows 系统上：
```
C:\etc\codex\requirements.toml
```

> **注意**：此文件为系统级配置，通常需要管理员权限才能创建和修改。

## 用途和作用

`requirements.toml` 文件的主要用途包括：

1. **限制审批策略（Approval Policy）**：控制用户可以选择哪些命令审批模式
2. **限制沙箱模式（Sandbox Mode）**：控制 Codex 执行命令时允许使用的沙箱隔离级别
3. **强制 MCP 服务器配置**：要求用户必须使用特定的 MCP（Model Context Protocol）服务器

### 与用户配置的关系

- **用户配置**：位于 `~/.codex/config.toml`，用户可以自由修改
- **系统要求**：位于 `/etc/codex/requirements.toml`，由管理员设置，优先级更高
- **约束机制**：`requirements.toml` 定义的是**允许列表**（allowlist），用户只能在允许的选项范围内进行选择

## 配置结构和字段说明

### 1. allowed_approval_policies（允许的审批策略）

控制用户可以选择的命令审批策略。

**可用值：**

| 值 | 说明 |
|---|---|
| `"untrusted"` | 仅自动批准已知安全的只读命令，其他命令需要用户确认 |
| `"on-failure"` | 所有命令自动批准，但需在沙箱中运行。如果命令失败，会询问用户是否重试 |
| `"on-request"` | 所有命令都需要用户明确批准 |
| `"never"` | 从不询问用户，所有命令自动执行（需谨慎使用） |

**配置示例：**
```toml
# 只允许用户选择 "untrusted" 或 "on-request" 模式
allowed_approval_policies = ["untrusted", "on-request"]
```

**效果：**
- 用户只能在 `untrusted` 和 `on-request` 之间选择
- 默认会使用列表中的第一个值（`untrusted`）
- 如果用户尝试设置为 `"never"` 或 `"on-failure"`，系统会拒绝并显示错误信息

### 2. allowed_sandbox_modes（允许的沙箱模式）

控制 Codex 执行命令时允许使用的沙箱隔离级别。

**可用值：**

| 值 | 说明 |
|---|---|
| `"read-only"` | 只读模式，命令只能读取文件，不能修改 |
| `"workspace-write"` | 工作区写入模式，可以在工作区目录内写入文件 |
| `"danger-full-access"` | 完全访问模式，没有沙箱限制（不推荐） |
| `"external-sandbox"` | 外部沙箱模式（高级用途） |

**重要约束：**
- **必须包含 `"read-only"`**：这是系统正常运行的基础要求
- 如果不包含 `"read-only"`，配置会被拒绝

**配置示例：**
```toml
# 允许只读和工作区写入模式
allowed_sandbox_modes = ["read-only", "workspace-write"]
```

**效果：**
- 用户可以选择只读或工作区写入模式
- 用户无法选择完全访问模式（`danger-full-access`）
- 默认使用 `read-only` 模式

### 3. mcp_servers（MCP 服务器要求）

强制要求用户使用特定的 MCP 服务器配置。

**配置格式：**
```toml
[mcp_servers.<服务器名称>.identity]
command = "<命令路径>"
# 或
url = "<服务器URL>"
```

**配置示例：**
```toml
# 强制使用本地 MCP 服务器
[mcp_servers.docs.identity]
command = "codex-mcp"

# 强制使用远程 MCP 服务器
[mcp_servers.remote.identity]
url = "https://example.com/mcp"
```

**效果：**
- 用户必须连接到指定的 MCP 服务器
- 可以同时要求多个 MCP 服务器
- 用于企业环境中统一管理和监控

## 完整配置示例

### 示例 1：严格限制（高安全）

```toml
# 只允许最安全的审批策略
allowed_approval_policies = ["on-request"]

# 只允许只读模式
allowed_sandbox_modes = ["read-only"]

# 强制使用公司 MCP 服务器
[mcp_servers.corporate.identity]
url = "https://mcp.company.com"
```

**适用场景：** 高度敏感的企业环境，需要严格控制所有操作。

### 示例 2：平衡配置（推荐）

```toml
# 允许两种审批模式
allowed_approval_policies = ["untrusted", "on-request"]

# 允许只读和工作区写入
allowed_sandbox_modes = ["read-only", "workspace-write"]
```

**适用场景：** 大多数企业环境，在安全性和便利性之间取得平衡。

### 示例 3：宽松配置（开发环境）

```toml
# 允许所有审批策略
allowed_approval_policies = ["untrusted", "on-failure", "on-request", "never"]

# 允许所有沙箱模式
allowed_sandbox_modes = ["read-only", "workspace-write", "danger-full-access"]
```

**适用场景：** 开发测试环境，需要较高灵活性。

## 配置加载顺序

Codex 按以下优先级加载配置约束：

1. **MDM 管理偏好设置**（仅 macOS）- 最高优先级
2. **系统 requirements.toml** (`/etc/codex/requirements.toml`)
3. **传统 managed_config.toml**（向后兼容，已弃用）

**合并规则：**
- 优先级高的配置源定义的约束不能被低优先级覆盖
- 未设置的字段会从低优先级配置源填充
- 这确保了管理员设置的约束始终有效

## 如何使用

### 创建配置文件

1. 以管理员身份创建文件：
   ```bash
   sudo mkdir -p /etc/codex
   sudo nano /etc/codex/requirements.toml
   ```

2. 填写配置内容（参考上述示例）

3. 保存文件并设置适当权限：
   ```bash
   sudo chmod 644 /etc/codex/requirements.toml
   ```

### 验证配置

通过 Codex 的 API 接口查看当前生效的约束：

```bash
# 使用 configRequirements/read API
# 如果配置正确，会返回当前的约束设置
# 如果没有配置约束，会返回 null
```

### 测试配置效果

1. 尝试设置用户配置为不允许的值
2. Codex 会拒绝该设置并显示错误消息
3. 错误消息会明确指出：
   - 哪个字段违反了约束
   - 尝试设置的值
   - 允许的值列表
   - 约束来源（如 `/etc/codex/requirements.toml`）

## 常见问题

### Q1: 用户能否绕过这些限制？
**A:** 不能。`requirements.toml` 的约束在代码层面强制执行，用户无法通过修改自己的配置文件来绕过。

### Q2: 如果不创建此文件会怎样？
**A:** 如果该文件不存在，Codex 不会施加任何系统级约束，用户可以自由选择任何审批策略和沙箱模式。

### Q3: 可以为不同用户设置不同的约束吗？
**A:** 不能直接实现。`requirements.toml` 是系统级配置，对所有用户生效。如需不同约束，可以考虑：
- 使用 MDM（仅 macOS）针对不同设备推送不同配置
- 在不同系统/容器中部署不同的配置

### Q4: 配置更新后何时生效？
**A:** Codex 在启动时读取配置。已运行的实例需要重启才能应用新配置。

### Q5: allowed_sandbox_modes 为什么必须包含 "read-only"？
**A:** `read-only` 是最基本的沙箱模式，是 Codex 安全执行的基础。其他高级模式都是在此基础上扩展的。强制包含它可以确保系统始终有一个可用的安全执行模式。

## 错误处理

### 常见错误 1：空的允许列表

```toml
allowed_approval_policies = []
```

**错误信息：**
```
Empty constraint list for field 'allowed_approval_policies'
```

**解决方法：** 至少包含一个值。

### 常见错误 2：allowed_sandbox_modes 缺少 "read-only"

```toml
allowed_sandbox_modes = ["workspace-write"]
```

**错误信息：**
```
Invalid value for 'allowed_sandbox_modes': must include 'read-only'
```

**解决方法：** 添加 `"read-only"` 到列表中。

### 常见错误 3：拼写错误

```toml
allowed_approval_policies = ["untrustd"]  # 拼写错误
```

**错误信息：**
```
Unknown variant: expected one of untrusted, on-failure, on-request, never
```

**解决方法：** 使用正确的值名称。

## 与 MDM 集成（macOS）

在 macOS 环境中，可以通过 MDM（移动设备管理）系统推送配置，无需直接创建文件：

1. 在 MDM 中配置托管偏好设置
2. 使用 `com.codex` 作为域名
3. 将 requirements.toml 内容编码为 base64 并推送
4. Codex 会自动读取并应用这些约束

**优先级：** MDM 配置的优先级高于 `/etc/codex/requirements.toml`

## 安全建议

1. **最小权限原则**：只开放必要的审批策略和沙箱模式
2. **定期审查**：定期检查和更新配置，确保符合当前安全政策
3. **文件权限**：确保 requirements.toml 只能由管理员修改（`chmod 644` 或更严格）
4. **监控合规性**：通过日志和监控系统验证用户是否在允许的约束范围内运行
5. **测试验证**：在生产环境部署前，在测试环境中验证配置的正确性

## 技术实现细节

### 数据结构

```rust
pub struct ConfigRequirementsToml {
    pub allowed_approval_policies: Option<Vec<AskForApproval>>,
    pub allowed_sandbox_modes: Option<Vec<SandboxModeRequirement>>,
    pub mcp_servers: Option<BTreeMap<String, McpServerRequirement>>,
}
```

### 约束验证

- 配置加载时，系统会将 `requirements.toml` 转换为带约束的配置对象
- 每个字段都会创建一个验证函数，检查用户设置是否在允许列表中
- 违反约束时，会生成包含详细信息的错误消息

### 错误消息格式

错误消息包含：
- 字段名称
- 用户尝试设置的值
- 允许的值列表
- 约束来源（文件路径或 MDM 信息）

## 总结

`requirements.toml` 是 Codex 项目中强大的企业级安全配置工具：

- **功能定位**：系统级安全约束，不可被用户绕过
- **配置灵活**：支持三大类约束（审批策略、沙箱模式、MCP 服务器）
- **易于管理**：TOML 格式简单直观，易于理解和维护
- **企业友好**：支持 MDM 集成，适合大规模部署
- **安全优先**：强制执行约束，确保合规性

通过合理配置 `requirements.toml`，组织可以在保证安全性的同时，让开发人员充分利用 Codex 的强大功能。
