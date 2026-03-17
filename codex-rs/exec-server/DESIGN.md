# exec-server design notes

This document sketches a likely direction for integrating `codex-exec-server`
with unified exec without baking the full tool-call policy stack into the
server.

The goal is:

- keep exec-server generic and reusable
- keep approval, sandbox, and retry policy in `core`
- preserve the unified-exec event flow the model already depends on
- support retained output caps so polling and snapshot-style APIs do not grow
  memory without bound

## Unified exec today

Today the flow for LLM-visible interactive execution is:

1. The model sees the `exec_command` and `write_stdin` tools.
2. `UnifiedExecHandler` parses the tool arguments and allocates a process id.
3. `UnifiedExecProcessManager::exec_command(...)` calls
   `open_session_with_sandbox(...)`.
4. `ToolOrchestrator` drives approval, sandbox selection, managed network
   approval, and sandbox-denial retry behavior.
5. `UnifiedExecRuntime` builds a `CommandSpec`, asks the current
   `SandboxAttempt` to transform it into an `ExecRequest`, and passes that
   resolved request back to the process manager.
6. `open_session_with_exec_env(...)` spawns the process from that resolved
   `ExecRequest`.
7. Unified exec emits an `ExecCommandBegin` event.
8. Unified exec starts a background output watcher that emits
   `ExecCommandOutputDelta` events.
9. The initial tool call collects output until the requested yield deadline and
   returns an `ExecCommandToolOutput` snapshot to the model.
10. If the process is still running, unified exec stores it and later emits
    `ExecCommandEnd` when the exit watcher fires.
11. A later `write_stdin` tool call writes to the stored process, emits a
    `TerminalInteraction` event, collects another bounded snapshot, and returns
    that tool response to the model.

Important observation: the 250ms / 10s yield-window behavior is not really a
process-server concern. It is a client-side convenience layer for the LLM tool
API. The server should focus on raw process lifecycle and streaming events.

## Proposed boundary

The clean split is:

- exec-server server: process lifecycle, output streaming, retained output caps
- exec-server client: `wait`, `communicate`, yield-window helpers, session
  bookkeeping
- unified exec in `core`: tool parsing, event emission, approvals, sandboxing,
  managed networking, retry semantics

If exec-server is used by unified exec later, the boundary should sit between
step 5 and step 6 above: after policy has produced a resolved spawn request, but
before the actual PTY or pipe spawn.

## Suggested process API

Start simple and explicit:

- `process/start`
- `process/write`
- `process/closeStdin`
- `process/resize`
- `process/terminate`
- `process/wait`
- `process/snapshot`

Server notifications:

- `process/output`
- `process/exited`
- optionally `process/started`
- optionally `process/failed`

Suggested request shapes:

```rust
enum ProcessStartRequest {
    Direct(DirectExecSpec),
    Prepared(PreparedExecSpec),
}

struct DirectExecSpec {
    process_id: String,
    argv: Vec<String>,
    cwd: PathBuf,
    env: HashMap<String, String>,
    arg0: Option<String>,
    io: ProcessIo,
}

struct PreparedExecSpec {
    process_id: String,
    request: PreparedExecRequest,
    io: ProcessIo,
}

enum ProcessIo {
    Pty { rows: u16, cols: u16 },
    Pipe { stdin: StdinMode },
}

enum StdinMode {
    Open,
    Closed,
}

enum TerminateMode {
    Graceful { timeout_ms: u64 },
    Force,
}
```

Notes:

- `processId` remains a protocol handle, not an OS pid.
- `wait` is a good generic API because many callers want process completion
  without manually wiring notifications.
- `communicate` is also a reasonable API, but it should probably start as a
  client helper built on top of `write + closeStdin + wait + snapshot`.
- If an RPC form of `communicate` is added later, it should be a convenience
  wrapper rather than the primitive execution model.

## Output capping

Even with event streaming, the server should retain a bounded amount of output
per process so callers can poll, wait, or reconnect without unbounded memory
growth.

Suggested behavior:

- stream every output chunk live via `process/output`
- retain capped output per process in memory
- keep stdout and stderr separately for pipe-backed processes
- for PTY-backed processes, treat retained output as a single terminal stream
- expose truncation metadata on snapshots

Suggested snapshot response:

```rust
struct ProcessSnapshot {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    terminal: Vec<u8>,
    truncated: bool,
    exit_code: Option<i32>,
    running: bool,
}
```

Implementation-wise, the current `HeadTailBuffer` pattern used by unified exec
is a good fit. The cap should be server config, not request config, so memory
use stays predictable.

## Sandboxing and networking

### How unified exec does it today

Unified exec does not hand raw command args directly to the PTY layer for tool
calls. Instead, it:

1. computes approval requirements
2. chooses a sandbox attempt
3. applies managed-network policy if needed
4. transforms `CommandSpec` into `ExecRequest`
5. spawns from that resolved `ExecRequest`

That split is already valuable and should be preserved.

### Recommended exec-server design

Do not put approval policy into exec-server.

Instead, support two execution modes:

- `Direct`: raw command, intended for orchestrator-side or already-trusted use
- `Prepared`: already-resolved spawn request, intended for tool-call execution

For tool calls from the LLM side:

1. `core` runs the existing approval + sandbox + managed-network flow
2. `core` produces a resolved `ExecRequest`
3. the exec-server client sends `PreparedExecSpec`
4. exec-server spawns exactly that request and streams process events

For orchestrator-side execution:

1. caller sends `DirectExecSpec`
2. exec-server spawns directly without running approval or sandbox policy

This gives one generic process API while keeping the policy-sensitive logic in
the place that already owns it.

### Why not make exec-server own sandbox selection?

That would force exec-server to understand:

- approval policy
- exec policy / prefix rules
- managed-network approval flow
- sandbox retry semantics
- guardian routing
- feature-flag-driven sandbox selection
- platform-specific sandbox helper configuration

That is too opinionated for a reusable process service.

## Optional future server config

If exec-server grows beyond the current prototype, a config object like this
would be enough:

```rust
struct ExecServerConfig {
    shutdown_grace_period_ms: u64,
    max_processes_per_connection: usize,
    retained_output_bytes_per_process: usize,
    allow_direct_exec: bool,
    allow_prepared_exec: bool,
}
```

That keeps policy surface small:

- lifecycle limits live in the server
- trust and sandbox policy stay with the caller

## Mapping back to LLM-visible events

If unified exec is later backed by exec-server, the `core` client wrapper should
keep owning the translation into the existing event model:

- `process/start` success -> `ExecCommandBegin`
- `process/output` -> `ExecCommandOutputDelta`
- local `process/write` call -> `TerminalInteraction`
- `process/exited` plus retained transcript -> `ExecCommandEnd`

That preserves the current LLM-facing contract while making the process backend
swappable.
