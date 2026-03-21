# Orchestrator and Executor Split

## Goal

Codex should be able to route tool calls through a remote executor without
teaching every tool implementation about remote filesystems, RPC retries, or
model-specific defaults.

For unified exec, the main rule should be:

- the executor advertises the `exec_command` and `write_stdin` tool contracts
- the orchestrator forwards tool calls plus any orchestrator-owned overrides
- the executor owns host-local execution details, session ids, and process
  lifecycle
- the executor does not invent model-dependent policy that only the
  orchestrator can know

## Current Ownership Leak

Today the unified exec path is split across several layers:

- `core/src/tools/handlers/unified_exec.rs` parses model args and normalizes
  approval-related fields
- `core/src/unified_exec/process_manager.rs` still derives execution env,
  approval requirements, and other execution inputs from `TurnContext`
- `core/src/tools/runtimes/unified_exec.rs` still rewrites commands for shell
  snapshot and PowerShell handling
- `core/src/tools/spec.rs` still statically advertises unified exec from the
  orchestrator side instead of learning it from the executor

That makes the executor boundary fuzzy. A future remote executor would need to
know more than "handle this tool call" because some request fields are still
hidden behind local helpers and the tool itself is not yet executor-owned.

## Ownership Rule

The orchestrator owns:

- model sampling
- tool routing and dynamic tool registration
- model-facing tool-call parsing
- shell selection and final argv construction
- working-directory resolution
- execution environment construction
- approval-policy evaluation
- output-policy selection, including max output truncation for the tool result

The executor owns:

- advertising host-local tool specs such as unified exec
- spawning and tracking the process
- streaming and buffering raw output
- minting unified-exec session ids
- applying the already selected sandbox attempt
- returning the requested output slice for the current poll
- filesystem and host-local resource access that should happen on the executor
  host

## Unified Exec Wire Contract

The unified exec boundary should be expressed as an executor-advertised tool
contract plus explicit orchestrator overrides that can be serialized and sent
over a future executor transport.

`UnifiedExecExecCommandRequest`

- `command`: final argv vector
- `cwd`: absolute working directory
- `env`: fully constructed environment for the command
- `tty`: whether to allocate a PTY
- `yield_time_ms`: initial poll window
- `max_output_tokens`: tool-output truncation budget selected by the
  orchestrator
- `sandbox_permissions`: already normalized sandbox request
- `additional_permissions`: already normalized additional permissions
- `additional_permissions_preapproved`: whether those permissions are already
  sticky-approved
- `justification`: optional user-facing approval reason
- `exec_approval_requirement`: resolved approval plan for this request

The executor's `exec_command` tool result should include:

- `process_id`: stable unified-exec session id chosen by the executor when the
  session stays open after the initial poll
- no `process_id` for short-lived commands that exit before the initial poll
  completes

`UnifiedExecWriteStdinRequest`

- `process_id`: existing unified-exec session id
- `input`: bytes to write, expressed as text for the tool path
- `yield_time_ms`: poll window for this write or empty poll
- `max_output_tokens`: tool-output truncation budget selected by the
  orchestrator

These are intentionally higher level than `codex-exec-server`'s low-level
`process/start`, `process/read`, and `process/write` methods. Unified exec is a
tool contract owned by the executor; the executor transport can map that
contract onto lower-level process RPCs internally.

## Phase Plan

1. Let the executor advertise unified exec tool specs to the orchestrator,
   similar to MCP `listTools`.
2. Define protocol-owned unified-exec request types for executor-owned tool
   calls plus orchestrator-owned overrides.
3. Make the executor side mint unified-exec session ids and return them in tool
   results.
4. Move the host-local unified-exec implementation behind the executor boundary.
5. Reuse the same request types for a remote executor transport.

## Crate Split

- `codex-executor-protocol`: shared executor transport types with minimal
  dependencies
- `codex-executor`: executor-side receive loop and future transport adapter
- `codex-cli`: `codex executor` subcommand that launches the executor process

## Follow-ups

This work does not fully remove every host-local concern yet. In particular:

- remote network-proxy binding is still local to the executor path
- zsh-fork backend setup remains local because it depends on executor-host
  binaries and inherited file descriptors
- shell snapshot capture and executor bootstrapping still need a broader
  context-gathering protocol
- unified exec is still statically registered by the orchestrator today rather
  than advertised by the executor

Those should be follow-up steps, but they should build on the same principle:
the orchestrator chooses only the policy and routing inputs that are truly
orchestrator-owned, while the executor owns host-local tool behavior.
