# PowerShell REPL (`ps_repl`)

`ps_repl` runs PowerShell in a persistent `pwsh`-backed kernel.

## Feature gate

`ps_repl` is disabled by default and only appears when:

```toml
[features]
ps_repl = true
```

The initial rollout stage is under development.

## PowerShell runtime

`ps_repl` requires PowerShell 7 or newer.

Runtime resolution order:

1. `CODEX_PS_REPL_PATH` environment variable
2. `ps_repl_path` in config/profile
3. `pwsh` discovered on `PATH`

You can configure an explicit runtime path:

```toml
ps_repl_path = "/absolute/path/to/pwsh"
```

If only Windows PowerShell (`powershell.exe`) is available, `ps_repl` stays disabled and emits a startup warning telling you to install PowerShell 7.

## Usage

- `ps_repl` is a freeform tool: send raw PowerShell source text.
- Optional first-line pragma:
  - `# codex-ps-repl: timeout_ms=15000`
- Variables, functions, aliases, imported modules, environment changes, and `$LASTEXITCODE` persist across calls.
- Use `ps_repl_reset` to clear the kernel state.

## Helper APIs inside the kernel

`ps_repl` exposes these helpers:

- `$CodexTmpDir`: per-session scratch directory path.
- `Invoke-CodexTool -Name <string> -Arguments <object|string>`: executes a normal Codex tool call from inside `ps_repl`.
- `$Codex.TmpDir`: alias for the scratch directory.
- `$Codex.Tool(<name>, <args>)`: thin alias to `Invoke-CodexTool`.

`Invoke-CodexTool` returns the raw tool output object. Nested tool calls can also return multimodal content such as `view_image` results.

To share generated images with the model, write a file under `$CodexTmpDir`, call:

```powershell
Invoke-CodexTool -Name view_image -Arguments @{ path = "/absolute/path" }
```

Then delete the file.

## Output and transport

`ps_repl` uses a JSON-line transport over stdio.

- Safe output forms: pipeline output, `Write-Output`, `Write-Host`, `Write-Verbose`, `Write-Warning`
- Avoid: direct `[Console]::Write*`, raw StdOut/StdErr writes, or other host-level output that bypasses PowerShell streams

Bypassing PowerShell streams can corrupt the transport between the Rust host and the persistent kernel.
