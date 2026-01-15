# 目录访问限制分析

本文档分析 Codex 项目如何限制对目录的访问，特别是在 yolo 模式下的行为。

## 概述

Codex 使用多层沙箱策略（Sandbox Policy）来控制代理对文件系统的访问权限。这些策略在不同操作系统上使用不同的底层技术实现。

## 沙箱策略类型

### 1. ReadOnly（只读模式）
- **限制**：只允许读取整个文件系统，不允许任何写入操作
- **网络访问**：禁止
- **使用场景**：最安全的模式，适合代码分析和查询操作
- **实现**：
  - macOS: 使用 Seatbelt (sandbox-exec)
  - Linux: 使用 Landlock + seccomp
  - Windows: 使用 Restricted Token

### 2. WorkspaceWrite（工作区写入模式）
- **限制**：
  - 可读取整个文件系统
  - 只能写入当前工作目录（cwd）
  - 可选：通过 `--add-dir` 添加额外的可写目录
  - 默认包含 `/tmp` 和用户临时目录（可通过配置排除）
- **网络访问**：可配置（默认禁止）
- **使用场景**：日常开发工作，允许在特定目录内修改文件
- **实现**：通过沙箱配置指定可写根目录列表

### 3. ExternalSandbox（外部沙箱模式）
- **限制**：假设进程已在外部沙箱中运行，允许完全磁盘访问
- **网络访问**：可配置
- **使用场景**：在容器或虚拟机中运行时使用
- **特点**：
  - 不应用 Codex 内部沙箱
  - 依赖外部环境提供的隔离
  - `--add-dir` 标志被忽略（因为已有完全访问权限）

### 4. DangerFullAccess（完全访问模式，又称 yolo 模式）
- **限制**：无任何限制
- **网络访问**：允许
- **使用场景**：仅用于在外部已沙箱化的环境中运行
- **风险**：极度危险，完全绕过所有安全检查
- **CLI 标志**：`--yolo` 或 `--dangerously-bypass-approvals-and-sandbox`
- **特点**：
  - 不应用任何沙箱技术
  - 完全绕过目录访问限制
  - `--add-dir` 标志被忽略（因为已有完全访问权限）

## 目录访问限制实现

### 平台特定实现

#### macOS (Seatbelt)
```
文件路径：codex-rs/core/src/seatbelt.rs
技术：/usr/bin/sandbox-exec

实现方式：
1. 生成 Seatbelt 策略文件
2. 对于 WorkspaceWrite 模式：
   - 将 cwd 和额外目录转换为绝对路径（规范化）
   - 生成 (allow file-write* (subpath ...)) 规则
   - 为需要保持只读的子路径添加 (require-not ...) 规则
3. 使用 sandbox-exec 执行命令
```

#### Linux (Landlock + seccomp)
```
文件路径：codex-rs/linux-sandbox/src/landlock.rs
技术：Landlock LSM + seccomp

实现方式：
1. 将 SandboxPolicy 序列化为 JSON
2. 通过 --sandbox-policy 参数传递给 codex-linux-sandbox
3. codex-linux-sandbox 解析策略并：
   - 配置 Landlock 规则限制文件系统访问
   - 配置 seccomp 规则限制系统调用
4. 在受限环境中执行命令
```

#### Windows (Restricted Token)
```
文件路径：codex-rs/windows-sandbox-rs/src/lib.rs
技术：Windows Restricted Token

实现方式：
1. 创建受限令牌
2. 在进程内应用限制
3. 通过 ACL 检查控制文件访问
```

### `--add-dir` 标志的处理

`--add-dir` 标志允许用户指定额外的可写目录，但仅在 **WorkspaceWrite** 模式下有效：

```rust
// 在 codex-rs/core/src/config/mod.rs 中
if let SandboxPolicy::WorkspaceWrite { writable_roots, .. } = &mut sandbox_policy {
    for path in additional_writable_roots {
        if !writable_roots.iter().any(|existing| existing == &path) {
            writable_roots.push(path);
        }
    }
}
```

对于其他模式：
- **ReadOnly**：会显示警告，忽略 `--add-dir`
- **DangerFullAccess**：静默忽略（因为已有完全访问权限）
- **ExternalSandbox**：静默忽略（因为假设外部沙箱已处理）

## YOLO 模式的行为

### 当前实现

YOLO 模式（`--yolo` 或 `--dangerously-bypass-approvals-and-sandbox`）映射到 `SandboxMode::DangerFullAccess`：

```rust
// 在 codex-rs/tui/src/lib.rs 中
} else if cli.dangerously_bypass_approvals_and_sandbox {
    (
        Some(SandboxMode::DangerFullAccess),
        Some(AskForApproval::Never),
    )
}
```

### YOLO 模式的限制

在 YOLO 模式下：

1. **无目录访问限制**
   - `has_full_disk_write_access()` 返回 `true`
   - `get_writable_roots_with_cwd()` 返回空列表
   - 不应用任何沙箱技术

2. **忽略安全标志**
   - `--add-dir` 被忽略
   - 无法限制访问范围

3. **绕过审批流程**
   - 自动设置 `AskForApproval::Never`
   - 所有操作无需用户确认

### YOLO 模式的设计意图

根据代码注释和 CLI 帮助文本：

```rust
/// Skip all confirmation prompts and execute commands without sandboxing.
/// EXTREMELY DANGEROUS. Intended solely for running in environments that are externally sandboxed.
```

YOLO 模式的设计意图是：
- **仅用于外部已沙箱化的环境**（如容器、虚拟机）
- 外部环境负责提供所有安全隔离
- Codex 不施加额外限制以最大化灵活性

## 限制 YOLO 模式目录访问的可能方案

如果需要在类似 YOLO 的模式下限制目录访问，有以下几种方案：

### 方案 1：使用 WorkspaceWrite + Never 审批

```bash
codex --sandbox workspace-write --ask-for-approval never --add-dir /path/to/dir1 --add-dir /path/to/dir2
```

**优点**：
- 保持沙箱限制
- 可指定允许访问的目录
- 无需修改代码

**缺点**：
- 仍有审批提示（对某些操作）
- 可能不如 YOLO 模式"自由"

### 方案 2：扩展 ExternalSandbox 支持目录限制

修改 `SandboxPolicy::ExternalSandbox` 以支持可写根目录：

```rust
ExternalSandbox {
    network_access: NetworkAccess,
    writable_roots: Vec<AbsolutePathBuf>,  // 新增字段
}
```

**优点**：
- 语义更清晰（明确表示外部沙箱）
- 可在不应用内部沙箱的同时记录允许的目录

**缺点**：
- 需要修改协议和所有相关代码
- 不会真正强制执行限制（依赖外部环境）

### 方案 3：添加新的沙箱模式

创建新的 `ConstrainedFullAccess` 模式：

```rust
ConstrainedFullAccess {
    allowed_roots: Vec<AbsolutePathBuf>,
    network_access: bool,
}
```

**优点**：
- 明确的语义
- 可以强制执行或仅作为文档

**缺点**：
- 需要大量代码改动
- 增加系统复杂度

### 方案 4：使用外部沙箱工具

在外部环境级别限制访问：

```bash
# 使用 Docker
docker run -v /path/to/allowed:/workspace codex --yolo

# 使用 firejail
firejail --whitelist=/path/to/allowed codex --yolo
```

**优点**：
- 不需要修改 Codex
- 真正的强制执行
- 与 YOLO 模式的设计意图一致

**缺点**：
- 需要外部工具
- 配置较复杂

## 建议

对于需要在 yolo 模式下限制目录访问的场景：

1. **短期方案**：使用 `WorkspaceWrite` 模式配合 `--add-dir`，这是当前最安全且可用的方式
2. **中期方案**：使用外部沙箱工具（Docker, Podman, firejail）配合 yolo 模式
3. **长期方案**：如果有强烈需求，可考虑扩展 `ExternalSandbox` 模式支持目录白名单配置

## 代码参考

### 关键文件

- `codex-rs/protocol/src/protocol.rs` - SandboxPolicy 定义
- `codex-rs/core/src/sandboxing/mod.rs` - 沙箱管理
- `codex-rs/core/src/seatbelt.rs` - macOS 实现
- `codex-rs/core/src/landlock.rs` - Linux 实现
- `codex-rs/windows-sandbox-rs/` - Windows 实现
- `codex-rs/tui/src/cli.rs` - CLI 参数定义
- `codex-rs/tui/src/additional_dirs.rs` - `--add-dir` 处理

### 关键函数

- `SandboxPolicy::get_writable_roots_with_cwd()` - 获取可写根目录列表
- `SandboxPolicy::has_full_disk_write_access()` - 检查是否有完全写权限
- `create_seatbelt_command_args()` - 生成 macOS 沙箱参数
- `create_linux_sandbox_command_args()` - 生成 Linux 沙箱参数

## 总结

Codex 的目录访问限制通过多层策略实现：

1. **ReadOnly**：最严格，只读所有文件
2. **WorkspaceWrite**：平衡安全和灵活性，限制写入范围
3. **ExternalSandbox**：假设外部隔离，提供完全访问
4. **DangerFullAccess (yolo)**：无限制，仅用于外部已沙箱环境

在 yolo 模式下，**当前不支持也不打算支持**目录访问限制，因为：
- 设计意图是完全信任外部环境
- 外部环境应负责所有安全隔离
- 避免误导用户以为有安全保护

如需在灵活性和安全性之间取得平衡，应使用 `WorkspaceWrite` 模式配合 `--add-dir` 标志。
