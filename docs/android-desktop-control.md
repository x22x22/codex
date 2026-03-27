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

`attach` targets the live **Genie** session runtime in this first pass. Agent
parent sessions remain listable and readable, but the live attach path is
implemented for active Genie child sessions because that is the long-lived
Codex runtime that performs target-app work.

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

- Add a headless exported bootstrap activity that starts the bridge server and
  seeds the bearer token supplied by the desktop CLI.
- Run the bridge on device loopback only.
- Reuse `AgentSessionController` / `AgentSessionLauncher` for create, list, and
  management actions instead of duplicating orchestration logic.
- Track active attachable runtimes in the Agent process by framework `sessionId`
  and current Codex `threadId`.
- Extend the existing framework session bridge between Agent and Genie so it can
  relay app-server JSON-RPC frames for live desktop attach in addition to the
  existing fixed-form metadata calls.

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
