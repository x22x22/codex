# Android Agent + Genie Refactor

## Status

This document tracks the Android Agent Platform refactor that moves Codex from a
single Android wrapper process to a framework-native Agent/Genie architecture.

The current repo now contains these implementation slices:

- `android/app` is a buildable **Agent** app scaffold.
- `android/genie` is a buildable **Genie** app scaffold.
- The Agent app can:
  - register `AgentService`
  - plan target packages for a user objective
  - bridge framework session APIs into the hosted Agent Codex runtime
  - launch direct parent + child framework sessions
  - start one Genie session per selected target package
  - display framework session state and event timelines
  - answer waiting Genie questions
  - attach detached targets
- The Genie app now hosts a real `codex app-server` subprocess, packaged inside
  the Genie APK as `libcodex.so`.
- The first internal Agent<->Genie control plane uses the framework-managed
  **per-session bridge** returned by `AgentManager.openSessionBridge(...)` on
  the Agent side and `GenieService.Callback.openSessionBridge(...)` on the
  Genie side, not framework question/answer events.
- The older direct cross-app bind/socket bridge experiments have been removed
  from the app code; the framework session bridge is now the only supported
  Agent<->Genie control plane.
- The current session bridge exposes small fixed-form calls, and the Genie
  runtime already uses it to fetch Agent-owned runtime metadata from the
  hosted Agent Codex runtime, including auth status and configured model/provider.
- The Android host layer now copies a checked-in `AGENTS.md` asset into the
  Agent Codex home at bootstrap, and each Genie session copies that installed
  Agent file into its per-session Codex home over the framework session bridge.
- Target-package planning now relies on the hosted Agent Codex runtime using
  standard Android shell tools already available on-device (`cmd package`, `pm`,
  `am`) instead of Kotlin-side app discovery wrappers.
- Direct child-session launch now runs through a dedicated hosted Agent
  framework-session bridge, with Kotlin reduced to the framework/session host
  layer.
- The Genie runtime now relies on the hosted Codex shell/tool path for target
  package inspection, activity launch, input injection, and UI dumping instead
  of host-side Kotlin wrappers for those operations.
- The hosted `codex app-server` process now routes `/v1/responses` traffic over
  the existing app-server JSON-RPC channel to the Android host, and the Android
  host forwards that traffic to the Agent over the framework session bridge.
  This keeps network/auth Agent-owned without depending on target-sandbox local
  sockets or direct cross-app IPC.
- The session bridge now exposes a **narrow Responses transport** owned by the
  Agent app itself, so Genie model traffic no longer depends on any separate
  sidecar socket service.
- The Genie runtime now keeps host dynamic tools limited to framework-only
  detached-target controls and frame capture, while standard Android shell and
  device commands stay in the normal Codex tool path.
- Non-bridge Genie questions surface through AgentSDK question flow by mapping
  `request_user_input` back into Agent-managed questions and answers.
- The Agent also attempts to answer Genie questions through its hosted Codex
  runtime before falling back to notification/UI escalation, and now submits
  those answers through the same framework-session bridge instead of a separate
  Kotlin-only path.
- The Agent now records an explicit per-child final presentation policy
  (`ATTACHED`, `DETACHED_HIDDEN`, `DETACHED_SHOWN`, or `AGENT_CHOICE`) and
  uses the framework-authoritative `AgentSessionInfo.getTargetPresentation()`
  state to verify whether a completed child actually satisfied it.
- Parent roll-up now uses the new presentation state. If a child was required
  to finish `ATTACHED` but completes detached, the Agent requests `attachTarget`
  before rolling the parent up to success. Detached shown/hidden mismatches are
  treated as real completion errors instead of silent success.
- Runtime testing on the emulator showed that direct cross-app `bindService`
  and raw local-socket access from the live Genie runtime are not a stable
  contract because the runtime executes under the paired target sandbox
  identity. The framework session bridge is the correct transport boundary.
- Runtime testing on the updated image shows that self-targeted shell activity
  launch is viable from the paired Genie sandbox when the command shape is
  correct, for example `cmd activity start-activity --user 0 ...` or
  `am start --user 0 ...`.

The Android app now owns auth, runtime status, and Genie Responses forwarding
directly through the hosted Agent runtime. The older standalone
service/client split has been removed from the repo and is no longer part of
the Android Agent/Genie flow.

## Fixed Architecture Decisions

- There is one **Agent** runtime.
- There is one installed **Genie APK**.
- The framework launches **distinct Genie session instances**, each bound to one
  target app.
- A running Genie session instance is the unit of target pairing, sandboxing,
  and Android tool access.
- Genie is **headless** and must not access the internet directly.
- The Agent<->Genie product contract is free-form text:
  - Agent -> Genie: free-form objective / instructions
  - Genie -> Agent: free-form result / progress / follow-up question
  - Agent decides whether to answer directly or escalate to the user
- The user interacts only with the **Agent**.
- The Agent decides which target package(s) should receive child Genie sessions.
- Each child Genie decides its own local tool usage inside the paired sandbox.
- The Agent is the only runtime that owns:
  - auth
  - outbound network access
  - upstream provider selection
  - orchestration of parent + child sessions
- Internal Agent<->Genie coordination now splits into:
  - framework per-session bridges for fixed-form control/data RPC
  - AgentSDK session events for free-form product dialogue
- hosted `codex app-server` inside Genie for the actual Codex execution loop
- Genie-local transport termination between the hosted `codex` child process
  and the framework session bridge
- Agent-owned Responses transport termination between the framework session bridge
  and the upstream model backend

## Runtime Model

### Agent

- Framework-facing Android app with:
  - `AgentService`
  - `HANDLE_SESSION` activity
  - session orchestration via `AgentManager`
- Responsible for:
  - user interaction
  - task planning and target-package selection
  - direct parent session creation
  - child session creation per target app
  - starting Genie sessions
  - answering Genie questions
  - aggregating child progress/results into a parent task
  - acting as the eventual network/auth proxy for Genie traffic

### Genie

- Single APK exposing `GenieService`
- Framework launches one Genie execution per target app session
- Responsible for:
  - driving the paired target app
  - publishing trace, question, result, and error events
  - requesting detached target actions when appropriate
- The current implementation hosts `codex app-server` inside both the Agent and Genie sandboxes.
- Kotlin is now only the host/bridge layer for:
  - framework lifecycle and result publication
  - Android dynamic tool execution
  - Agent escalation via `request_user_input`
  - runtime bootstrap from the framework session bridge
  - forwarding hosted `codex` `/v1/responses` traffic onto the framework session bridge

## First Milestone Scope

### Included now

- AgentSDK compile-time integration via
  `ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP` or `-PagentPlatformStubSdkZip=...`
- Agent app manifest/service wiring
- Genie app manifest/service wiring
- Direct session launcher in the Agent UI
- Agent-side target-package planning with an optional package override
- Hosted Agent planning via standard Android shell tools already available on-device
- Dedicated framework-session bridge tools for direct Genie-session launch and question resolution
- Framework session inspection UI in the Agent app
- Question answering and detached-target attach controls
- Explicit per-child final target presentation policy in planning/session
  launch, backed by framework-authoritative presentation state in session
  observation and diagnostics
- Framework session bridge request handling in `AgentSessionBridgeServer`
- Framework session bridge request issuance in `CodexGenieService`
- Agent-hosted runtime metadata for Genie bootstrap
- Shell-first Genie execution for package inspection, activity launch, input injection, and UI dumping
- Hosted `codex app-server` inside Genie, with model traffic routed through the
  app-server request/response channel and then over the Agent framework session bridge
- Agent-owned `/v1/responses` proxying in
  `android/app/src/main/java/com/openai/codex/agent/AgentResponsesProxy.kt`
- Framework-only Android dynamic tools registered on the Genie Codex thread with:
  - detached target show/hide/attach/close
  - detached frame capture
- `request_user_input` bridged from hosted Codex back into AgentSDK questions
- Agent-owned question notifications for Genie questions that need user input
- Agent-mediated free-form answers for Genie questions, using the hosted Agent
  Codex runtime as the temporary answer engine
- Agent planning can now use `request_user_input` to ask the user clarifying
  questions before launching child Genie sessions
- Abstract-unix-socket support in the legacy Rust bridge via `@name` or
  `abstract:name`, so the compatibility transport can move off app-private
  filesystem sockets when Agent<->Genie traffic is introduced

### Not done yet

- Expanding the framework session bridge beyond the current fixed-form runtime
  bootstrap/status calls
- Adding more Android-native tool surfaces and richer observation types to the
  hosted Genie runtime

## Current Code Layout

- `android/app`
  - Agent scaffold and hosted Agent Codex runtime
- `android/genie`
  - standalone Genie scaffold APK with hosted `codex app-server`
- `android/app/src/main/java/com/openai/codex/agent/CodexAgentService.kt`
  - framework `AgentService`
- `android/app/src/main/java/com/openai/codex/agent/AgentSessionController.kt`
  - Agent-side `AgentManager` orchestration helper
- `android/app/src/main/java/com/openai/codex/agent/AgentFrameworkToolBridge.kt`
  - hosted Agent bridge for framework session APIs
- `android/app/src/main/java/com/openai/codex/agent/MainActivity.kt`
  - Agent session UI, Agent clarification dialogs, and Agent-native auth controls
- `android/app/src/main/java/com/openai/codex/agent/AgentUserInputPrompter.kt`
  - Android dialog bridge for hosted Agent `request_user_input` calls
- `android/genie/src/main/java/com/openai/codex/genie/CodexGenieService.kt`
  - Genie lifecycle host for the embedded `codex app-server`
- `android/genie/src/main/java/com/openai/codex/genie/CodexAppServerHost.kt`
  - stdio JSON-RPC host for `codex app-server`, framework-only dynamic tools,
    `request_user_input` bridging, and `/v1/responses` forwarding
- `android/app/src/main/java/com/openai/codex/agent/AgentSessionBridgeServer.kt`
  - Agent-side server for the framework-managed per-session bridge
- `android/app/src/main/java/com/openai/codex/agent/AgentResponsesProxy.kt`
  - Agent-owned Responses transport used by Genie model traffic
- `android/genie/src/main/java/com/openai/codex/genie/AgentBridgeClient.kt`
  - Genie-side client for the framework-managed session bridge
- `android/app/src/main/java/com/openai/codex/agent/AgentCodexAppServerClient.kt`
  - hosted Agent `codex app-server` client for planning, orchestration, auto-answering, runtime metadata, and narrow Agent tool calls

## Build

Set the Agent Platform stub SDK zip path:

```bash
export ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP=/path/to/android-agent-platform-stub-sdk.zip
```

Build both Android binaries first:

```bash
just android-build
```

Build both Android apps:

```bash
cd android
./gradlew :genie:assembleDebug :app:assembleDebug
```

The Agent app and Genie app both depend on `just android-build` for the
packaged `codex` JNI binaries.

## Next Implementation Steps

1. Route more Agent orchestration through the hosted runtime while keeping the
   host bridge limited to framework session APIs.
2. Add more Android-native tool surfaces and richer observation types to the
   hosted Genie runtime.
3. Later, replace the framework-session host bridge with command-line
   equivalents once those are stable enough to rely on.
