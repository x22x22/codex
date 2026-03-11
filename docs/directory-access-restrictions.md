# Directory Access Restriction Analysis

This document analyzes how the Codex project restricts directory access, with a particular focus on behavior in "yolo mode."

## Overview

Codex uses multi-layered sandbox policies to control agent access to the file system. These policies are implemented using different underlying technologies on various operating systems.

## Sandbox Policy Types

### 1. ReadOnly
- **Restrictions**: Read-only access to the entire file system, no write operations allowed
- **Network Access**: Disabled
- **Use Case**: Safest mode, suitable for code analysis and query operations
- **Implementation**:
  - macOS: Uses Seatbelt (sandbox-exec)
  - Linux: Uses Landlock + seccomp
  - Windows: Uses Restricted Token

### 2. WorkspaceWrite
- **Restrictions**:
  - Can read the entire file system
  - Can only write to the current working directory (cwd)
  - Optional: Add additional writable directories via `--add-dir`
  - By default includes `/tmp` and user temporary directory (can be excluded via configuration)
- **Network Access**: Configurable (disabled by default)
- **Use Case**: Daily development work, allows file modifications within specific directories
- **Implementation**: Specifies list of writable root directories through sandbox configuration

### 3. ExternalSandbox
- **Restrictions**: Assumes process is already running in an external sandbox, allows full disk access
- **Network Access**: Configurable
- **Use Case**: When running in containers or virtual machines
- **Characteristics**:
  - Does not apply Codex internal sandbox
  - Relies on isolation provided by external environment
  - `--add-dir` flags are ignored (since full access is already granted)

### 4. DangerFullAccess (also known as yolo mode)
- **Restrictions**: No restrictions whatsoever
- **Network Access**: Allowed
- **Use Case**: Intended solely for running in externally sandboxed environments
- **Risk**: Extremely dangerous, completely bypasses all security checks
- **CLI Flags**: `--yolo` or `--dangerously-bypass-approvals-and-sandbox`
- **Characteristics**:
  - Does not apply any sandbox technology
  - Completely bypasses directory access restrictions
  - `--add-dir` flags are ignored (since full access is already granted)

## Directory Access Restriction Implementation

### Platform-Specific Implementations

#### macOS (Seatbelt)
```
File Path: codex-rs/core/src/seatbelt.rs
Technology: /usr/bin/sandbox-exec

Implementation:
1. Generate Seatbelt policy file
2. For WorkspaceWrite mode:
   - Convert cwd and additional directories to absolute paths (canonicalize)
   - Generate (allow file-write* (subpath ...)) rules
   - Add (require-not ...) rules for subpaths that should remain read-only
3. Execute command using sandbox-exec
```

#### Linux (Landlock + seccomp)
```
File Path: codex-rs/linux-sandbox/src/landlock.rs
Technology: Landlock LSM + seccomp

Implementation:
1. Serialize SandboxPolicy to JSON
2. Pass to codex-linux-sandbox via --sandbox-policy parameter
3. codex-linux-sandbox parses policy and:
   - Configures Landlock rules to restrict filesystem access
   - Configures seccomp rules to restrict system calls
4. Execute command in restricted environment
```

#### Windows (Restricted Token)
```
File Path: codex-rs/windows-sandbox-rs/src/lib.rs
Technology: Windows Restricted Token

Implementation:
1. Create restricted token
2. Apply restrictions in-process
3. Control file access through ACL checks
```

### Handling of `--add-dir` Flag

The `--add-dir` flag allows users to specify additional writable directories, but is only effective in **WorkspaceWrite** mode:

```rust
// In codex-rs/core/src/config/mod.rs
if let SandboxPolicy::WorkspaceWrite { writable_roots, .. } = &mut sandbox_policy {
    for path in additional_writable_roots {
        if !writable_roots.iter().any(|existing| existing == &path) {
            writable_roots.push(path);
        }
    }
}
```

For other modes:
- **ReadOnly**: Displays a warning, ignores `--add-dir`
- **DangerFullAccess**: Silently ignores (since full access is already granted)
- **ExternalSandbox**: Silently ignores (assumes external sandbox handles it)

## Behavior in YOLO Mode

### Current Implementation

YOLO mode (`--yolo` or `--dangerously-bypass-approvals-and-sandbox`) maps to `SandboxMode::DangerFullAccess`:

```rust
// In codex-rs/tui/src/lib.rs
} else if cli.dangerously_bypass_approvals_and_sandbox {
    (
        Some(SandboxMode::DangerFullAccess),
        Some(AskForApproval::Never),
    )
}
```

### Limitations in YOLO Mode

In YOLO mode:

1. **No Directory Access Restrictions**
   - `has_full_disk_write_access()` returns `true`
   - `get_writable_roots_with_cwd()` returns empty list
   - No sandbox technology is applied

2. **Ignores Security Flags**
   - `--add-dir` is ignored
   - Cannot limit access scope

3. **Bypasses Approval Process**
   - Automatically sets `AskForApproval::Never`
   - All operations proceed without user confirmation

### Design Intent of YOLO Mode

According to code comments and CLI help text:

```rust
/// Skip all confirmation prompts and execute commands without sandboxing.
/// EXTREMELY DANGEROUS. Intended solely for running in environments that are externally sandboxed.
```

The design intent of YOLO mode is:
- **Use only in externally sandboxed environments** (e.g., containers, virtual machines)
- External environment is responsible for all security isolation
- Codex imposes no additional restrictions to maximize flexibility

## Possible Approaches to Restrict Directory Access in YOLO Mode

If you need to restrict directory access in a yolo-like mode, here are several approaches:

### Approach 1: Use WorkspaceWrite + Never Approval

```bash
codex --sandbox workspace-write --ask-for-approval never --add-dir /path/to/dir1 --add-dir /path/to/dir2
```

**Pros**:
- Maintains sandbox restrictions
- Can specify allowed directories
- No code modification needed

**Cons**:
- Still has approval prompts (for some operations)
- May not be as "free" as YOLO mode

### Approach 2: Extend ExternalSandbox to Support Directory Restrictions

Modify `SandboxPolicy::ExternalSandbox` to support writable roots:

```rust
ExternalSandbox {
    network_access: NetworkAccess,
    writable_roots: Vec<AbsolutePathBuf>,  // New field
}
```

**Pros**:
- Clearer semantics (explicitly indicates external sandbox)
- Can record allowed directories without applying internal sandbox

**Cons**:
- Requires modifying protocol and all related code
- Won't actually enforce restrictions (relies on external environment)

### Approach 3: Add New Sandbox Mode

Create a new `ConstrainedFullAccess` mode:

```rust
ConstrainedFullAccess {
    allowed_roots: Vec<AbsolutePathBuf>,
    network_access: bool,
}
```

**Pros**:
- Clear semantics
- Can enforce or serve as documentation

**Cons**:
- Requires substantial code changes
- Increases system complexity

### Approach 4: Use External Sandbox Tools

Restrict access at the external environment level:

```bash
# Using Docker
docker run -v /path/to/allowed:/workspace codex --yolo

# Using firejail
firejail --whitelist=/path/to/allowed codex --yolo
```

**Pros**:
- No Codex modification needed
- True enforcement
- Aligns with YOLO mode's design intent

**Cons**:
- Requires external tools
- More complex configuration

## Recommendations

For scenarios requiring directory access restrictions in yolo mode:

1. **Short-term solution**: Use `WorkspaceWrite` mode with `--add-dir` - this is currently the safest and most usable approach
2. **Medium-term solution**: Use external sandbox tools (Docker, Podman, firejail) with yolo mode
3. **Long-term solution**: If there's strong demand, consider extending `ExternalSandbox` mode to support directory whitelist configuration

## Code References

### Key Files

- `codex-rs/protocol/src/protocol.rs` - SandboxPolicy definition
- `codex-rs/core/src/sandboxing/mod.rs` - Sandbox management
- `codex-rs/core/src/seatbelt.rs` - macOS implementation
- `codex-rs/core/src/landlock.rs` - Linux implementation
- `codex-rs/windows-sandbox-rs/` - Windows implementation
- `codex-rs/tui/src/cli.rs` - CLI argument definitions
- `codex-rs/tui/src/additional_dirs.rs` - `--add-dir` handling

### Key Functions

- `SandboxPolicy::get_writable_roots_with_cwd()` - Get list of writable root directories
- `SandboxPolicy::has_full_disk_write_access()` - Check if full write permission exists
- `create_seatbelt_command_args()` - Generate macOS sandbox arguments
- `create_linux_sandbox_command_args()` - Generate Linux sandbox arguments

## Summary

Codex's directory access restrictions are implemented through multi-layered policies:

1. **ReadOnly**: Most restrictive, read-only access to all files
2. **WorkspaceWrite**: Balances security and flexibility, restricts write scope
3. **ExternalSandbox**: Assumes external isolation, provides full access
4. **DangerFullAccess (yolo)**: No restrictions, intended only for externally sandboxed environments

In yolo mode, **directory access restrictions are neither supported nor intended**, because:
- The design intent is to completely trust the external environment
- The external environment should be responsible for all security isolation
- Avoids misleading users into thinking there are security protections

If you need to balance flexibility and security, use `WorkspaceWrite` mode with `--add-dir` flags.
