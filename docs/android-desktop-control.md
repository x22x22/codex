# Android Desktop Control

This document tracks the desktop-to-device Codex control surface for the
Android Agent/Genie stack.

## Goal

Allow this machine to control the Android-installed Agent and Genie sessions
from the `codex` CLI with parity for the session-management actions currently
available in the device UI:

- list sessions
- create draft sessions without starting them
- optionally start draft sessions with a prompt
- inspect session state and diagnostics
- answer questions, cancel sessions, and attach detached targets
- attach a desktop Codex/TUI client to a live Android session runtime

## Chosen architecture

- Transport is `adb` only for the first version.
- The Android **Agent app** is the single desktop-facing endpoint.
- The desktop bootstraps a loopback-only bridge inside the Agent app, forwards a
  local TCP port with `adb forward`, and authenticates with a short-lived bearer
  token.
- The Agent bridge exposes:
  - a control websocket for Android session management RPCs
  - session websocket paths that proxy the standard `codex app-server`
    websocket protocol for live session attachment
- Shell-level `codex android ...` commands ship first.
- Future in-session Codex commands should reuse the same bridge RPCs instead of
  creating a second device-control backend.

## Scope for the first implementation

- `codex android sessions list`
- `codex android sessions create`
- `codex android sessions start`
- `codex android sessions read`
- `codex android sessions answer`
- `codex android sessions cancel`
- `codex android sessions attach-target`
- `codex android sessions attach`

`attach` now works for both:

- live **Genie** child runtimes
- live **Agent planner** runtimes for direct AGENT parent sessions

Attach lifecycle semantics:

- When an attached turn completes, the attached runtime stays alive so the same
  desktop TUI can send follow-up prompts.
- When an attached planner session finishes planning and spawns child Genie
  sessions, the planner stays attached instead of closing.
- Child Genie sessions spawned while the planner is attached now start in idle
  desktop-attach mode instead of immediately running their delegated prompt.
- Those child sessions receive the same bridge-provisioned setup state they
  would have had for an immediate start, but their first turn is paused until
  the attached desktop client sends a prompt.
- The planner-computed delegated objective is staged inside the Genie runtime as
  task context. If the planner later detaches before the user manually starts
  the child, the Genie falls back to auto-starting that staged objective.
- Detaching the planner releases that hold, allowing completed child sessions to
  finalize and the parent roll-up to settle normally.
- Cancelling a direct AGENT parent session is tree-scoped: it cancels the
  parent and all child Genie sessions from both the desktop bridge and the
  on-device UI.
- Recoverable hosted I/O failures are intentionally non-terminal:
  - if a Genie is currently attached, the current desktop attach closes, the
    session records a recoverable framework error, and the Genie restarts into a
    fresh attachable idle thread with staged recovery context
  - if a Genie is running unattached, it first retries automatically with staged
    recovery context; if automatic retries are exhausted, it also pauses into an
    attachable idle recovery thread instead of failing immediately

## CLI shape

The desktop entrypoint is:

- `codex android sessions list [--serial SERIAL]`
- `codex android sessions read [--serial SERIAL] SESSION_ID`
- `codex android sessions create [--serial SERIAL] [--target-package PACKAGE] [--model MODEL] [--reasoning-effort EFFORT]`
- `codex android sessions start [--serial SERIAL] SESSION_ID --prompt "..."`
- `codex android sessions answer [--serial SERIAL] SESSION_ID --answer "..."`
- `codex android sessions cancel [--serial SERIAL] SESSION_ID`
- `codex android sessions attach-target [--serial SERIAL] SESSION_ID`
- `codex android sessions attach [--serial SERIAL] SESSION_ID`
  The attach flow forces `disable_paste_burst=true` for that TUI session so scripted
  prompt injection remains submit-safe when text and Enter arrive in a tight burst.
  Attached planner and Genie sessions also mirror live framework events into the
  TUI as `thread/frameworkEvent` notifications so child-session ids, framework
  transport traces, questions, results, and framework errors are visible inline.

`attach` opens the existing remote app-server-backed Codex TUI against a
session websocket path returned by the device bridge, so the desktop attaches
to the live Android-backed runtime instead of starting a second independent
session.

Draft-session behavior:

- HOME-scoped drafts map directly to framework `STATE_CREATED` sessions created
  with `createAppScopedSession(...)` and started later with
  `startGenieSession(...)`.
- Direct AGENT drafts are created as parent AGENT sessions and remain
  unplanned until `sessions start` begins planner execution.
- `sessions attach` on a CREATED draft now bootstraps an idle runtime instead of
  requiring a prior prompt:
  - HOME drafts start Genie with an internal idle-bootstrap sentinel so the
    app-server thread exists before the first turn.
  - direct AGENT drafts start an idle planner app-server thread inside the
    Agent process.
- After attaching to a draft, the first prompt is entered from the attached
  TUI. Do not call `sessions start` for that same draft afterward.
- `sessions start` remains the non-interactive path for draft launch when the
  caller wants to provide the first prompt on the command line instead of
  through an attached TUI.
- HOME-scoped drafts require a session-UI lease while they remain
  `STATE_CREATED`; the desktop bridge holds that lease on behalf of the CLI
  until idle attach, `sessions start`, or `sessions cancel`.

## Android-side design

- Add an exported bootstrap broadcast receiver that starts the bridge server and
  seeds the bearer token supplied by the desktop CLI without surfacing the
  Agent UI task.
- Run the bridge on device loopback only.
- Reuse `AgentSessionController` / `AgentSessionLauncher` for create, list, and
  management actions instead of duplicating orchestration logic.
- Track active attachable runtimes in the Agent process by framework `sessionId`
  and current Codex `threadId`.
- Extend the existing framework session bridge between Agent and Genie so it can
  relay app-server JSON-RPC frames for live desktop attach in addition to the
  existing fixed-form metadata calls.
- Keep planner-spawned child Genie sessions under an inspection hold while the
  planner desktop attach is active, and release that hold when the planner
  detaches.

## Desktop-side design

- Add `codex android sessions ...` subcommands in `codex-cli`.
- Each command:
  - resolves the target adb device, defaulting to the adb default device
  - bootstraps the Agent bridge with a short-lived bearer token
  - ensures local port forwarding
  - talks to the Agent bridge control websocket
- `codex android sessions attach <session-id>` requests a session websocket path
  from the Agent bridge, then launches the existing remote app-server-backed TUI
  against that path using the same bearer token.

## Deliberate non-goals

- No direct desktop connection to the Genie package
- No LAN-exposed bridge in the first version
- No separate desktop-only orchestration protocol
- No replacement of the framework-owned `/responses` transport
