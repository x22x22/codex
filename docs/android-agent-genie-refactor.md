# Android Agent + Genie Refactor

## Status

This document tracks the Android Agent Platform refactor that moves Codex from a
single `codexd` foreground-service wrapper to a framework-native Agent/Genie
architecture.

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
- The first internal Agent<->Genie control plane uses an exported
  **Binder/AIDL service** in the Agent app, not framework question/answer
  events.
- The current Binder bridge exposes small fixed-form calls, and the Genie
  runtime already uses it to fetch Agent-owned runtime metadata from the
  hosted Agent Codex runtime, including auth status and configured model/provider.
- Target-package planning now relies on the hosted Agent Codex runtime using
  standard Android shell tools already available on-device (`cmd package`, `pm`,
  `am`) instead of Kotlin-side app discovery wrappers.
- Direct child-session launch now runs through a dedicated hosted Agent
  framework-session bridge, with Kotlin reduced to the framework/session host
  layer.
- The Genie runtime now relies on the hosted Codex shell/tool path for target
  package inspection, activity launch, input injection, and UI dumping instead
  of host-side Kotlin wrappers for those operations.
- The hosted `codex app-server` process now talks to a **Genie-local loopback
  HTTP proxy** inside the Genie app. That proxy forwards HTTP traffic to the
  Agent over Binder/AIDL, keeping network/auth Agent-owned without assuming the
  Genie child process can reach the Agent's abstract socket directly.
- The Binder bridge now exposes a **narrow Responses transport** owned by the
  Agent app itself, so Genie model traffic no longer depends on the legacy
  `codexd` socket service.
- The Genie runtime now keeps host dynamic tools limited to framework-only
  detached-target controls and frame capture, while standard Android shell and
  device commands stay in the normal Codex tool path.
- Non-bridge Genie questions surface through AgentSDK question flow by mapping
  `request_user_input` back into Agent-managed questions and answers.
- The Agent also attempts to answer Genie questions through its hosted Codex
  runtime before falling back to notification/UI escalation, and now submits
  those answers through the same framework-session bridge instead of a separate
  Kotlin-only path.
- Runtime testing on the emulator shows that the exported Agent Binder service
  is reachable from Genie execution for the current bootstrap calls, while
  direct cross-app access to the Agent-owned abstract socket is not a valid
  assumption.

The Rust `codexd` service/client split remains in place only for the legacy
foreground-service auth/status surface while this refactor proceeds.

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
- The first milestone keeps the current local CLI/socket bridge internally so
  the Rust runtime can migrate incrementally.
- Internal Agent<->Genie coordination now splits into:
  - Binder/AIDL for fixed-form control/data RPC
  - AgentSDK session events for free-form product dialogue
- hosted `codex app-server` inside Genie for the actual Codex execution loop
- Genie-local transport termination between the hosted `codex` child process
  and the Binder control plane
- Agent-owned Responses transport termination between the Binder control plane
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
  - runtime bootstrap from the Agent-owned Binder bridge
  - local proxying of hosted `codex` HTTP traffic onto Binder

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
- Exported Binder bridge request handling in `CodexAgentBridgeService`
- Binder bridge request issuance in `CodexGenieService`
- Agent-hosted runtime metadata for Genie bootstrap
- Shell-first Genie execution for package inspection, activity launch, input injection, and UI dumping
- Hosted `codex app-server` inside Genie, with model traffic routed through a
  Genie-local proxy backed by the Agent Binder bridge
- Agent-owned `/v1/responses` proxying in
  `android/app/src/main/java/com/openai/codexd/AgentResponsesProxy.kt`
- Android dynamic tools registered on the Genie Codex thread with:
  - `android.package.inspect`
  - `android.intent.launch`
  - detached target show/hide/attach/close
  - detached frame capture
  - UI hierarchy dump
  - shell-backed input injection helpers (`tap`, `text`, `key`)
  - bounded waits
- `request_user_input` bridged from hosted Codex back into AgentSDK questions
- Agent-owned question notifications for Genie questions that need user input
- Agent-mediated free-form answers for Genie questions, using the hosted Agent
  Codex runtime as the temporary answer engine
- Abstract-unix-socket support in the legacy Rust bridge via `@name` or
  `abstract:name`, so the compatibility transport can move off app-private
  filesystem sockets when Agent<->Genie traffic is introduced

### Not done yet

- Expanding the Binder control plane beyond the current fixed-form runtime
  bootstrap/status calls
- Making the Agent the default product surface instead of the legacy service app
- Consolidating the remaining auth/status responsibilities out of the legacy
  `codexd` foreground service
- Adding more Android-native tool surfaces and richer observation types to the
  hosted Genie runtime

## Current Code Layout

- `android/app`
  - Agent scaffold, hosted Agent Codex runtime, and legacy `codexd` bridge live together for now
- `android/genie`
  - standalone Genie scaffold APK with hosted `codex app-server`
- `android/app/src/main/java/com/openai/codexd/CodexAgentService.kt`
  - framework `AgentService`
- `android/app/src/main/java/com/openai/codexd/AgentSessionController.kt`
  - Agent-side `AgentManager` orchestration helper
- `android/app/src/main/java/com/openai/codexd/AgentFrameworkToolBridge.kt`
  - hosted Agent bridge for framework session APIs
- `android/app/src/main/java/com/openai/codexd/MainActivity.kt`
  - Agent session UI plus existing `codexd` bridge controls
- `android/genie/src/main/java/com/openai/codex/genie/CodexGenieService.kt`
  - Genie lifecycle host for the embedded `codex app-server`
- `android/genie/src/main/java/com/openai/codex/genie/CodexAppServerHost.kt`
  - stdio JSON-RPC host for `codex app-server`, framework-only dynamic tools,
    and `request_user_input` bridging
- `android/genie/src/main/java/com/openai/codex/genie/GenieLocalCodexProxy.kt`
  - Genie-local loopback HTTP proxy that forwards hosted `codex` HTTP traffic to
    the Agent Binder bridge
- `android/app/src/main/java/com/openai/codexd/CodexAgentBridgeService.kt`
  - exported Binder/AIDL bridge for Genie control-plane calls
- `android/app/src/main/java/com/openai/codexd/AgentResponsesProxy.kt`
  - Agent-owned Responses transport used by Genie model traffic
- `android/genie/src/main/java/com/openai/codex/genie/AgentBridgeClient.kt`
  - Genie-side Binder client for the Agent bridge service
- `android/app/src/main/java/com/openai/codexd/AgentCodexAppServerClient.kt`
  - hosted Agent `codex app-server` client for planning, orchestration, auto-answering, runtime metadata, and narrow Agent tool calls
- `android/app/src/main/java/com/openai/codexd/CodexdLocalClient.kt`
  - Agent-local client for the legacy `codexd` foreground-service surface

## Build

Set the Agent Platform stub SDK zip path:

```bash
export ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP=/path/to/android-agent-platform-stub-sdk.zip
```

Build both Android binaries first:

```bash
just android-build
just android-service-build
```

Build both Android apps:

```bash
cd android
./gradlew :genie:assembleDebug :app:assembleDebug
```

The Agent app depends on both `just android-build` and
`just android-service-build` for the packaged `codex` and `codexd` JNI
binaries. The Genie app depends on `just android-build` for the packaged
`codex` JNI binaries.

## Next Implementation Steps

1. Route more Agent orchestration through the hosted runtime while keeping the
   host bridge limited to framework session APIs.
2. Split the remaining legacy `codexd` auth/status concerns out of the Agent UI.
3. Add more Android-native tool surfaces and richer observation types to the
   hosted Genie runtime.
4. Later, replace the framework-session host bridge with command-line
   equivalents once those are stable enough to rely on.
