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
  embedded `codexd`, including auth status and the effective model/provider.
- The Genie runtime inspects the paired target package from inside the
  target-app sandbox and feeds package metadata plus launcher intent details
  into the delegated Codex prompt.
- The hosted `codex app-server` process routes model traffic through the
  Agent-owned `codexd` abstract Unix socket, keeping network/auth Agent-owned
  even while the Genie runs inside the target-app sandbox.
- The Genie runtime exposes reusable Android capabilities to Codex as
  **dynamic tools**, not via a custom `TOOL:` text protocol.
- Non-bridge Genie questions surface through AgentSDK question flow by mapping
  `request_user_input` back into Agent-managed questions and answers.
- The Agent also attempts to answer Genie questions through the embedded
  `codexd` runtime before falling back to notification/UI escalation.
- Runtime testing on the emulator shows that the exported Agent Binder service
  is reachable from Genie execution for the current bootstrap calls.

The Rust `codexd` service/client split remains in place and is still the
existing network/auth bridge while this refactor proceeds.

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
- The current implementation hosts `codex app-server` inside the Genie sandbox.
- Kotlin is now only the host/bridge layer for:
  - framework lifecycle and result publication
  - Android dynamic tool execution
  - Agent escalation via `request_user_input`
  - runtime bootstrap from the Agent-owned Binder bridge

## First Milestone Scope

### Included now

- AgentSDK compile-time integration via
  `ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP` or `-PagentPlatformStubSdkZip=...`
- Agent app manifest/service wiring
- Genie app manifest/service wiring
- Direct session launcher in the Agent UI
- Agent-side target-package planning from installed launchable apps, with an
  optional package override
- Framework session inspection UI in the Agent app
- Question answering and detached-target attach controls
- Exported Binder bridge request handling in `CodexAgentBridgeService`
- Binder bridge request issuance in `CodexGenieService`
- Agent-owned `/internal/runtime/status` metadata for Genie bootstrap
- Target-app package metadata and launcher-intent inspection from the Genie
  sandbox, with that context included in the delegated Codex prompt
- Hosted `codex app-server` inside Genie, with model traffic routed through the
  Agent-owned `codexd` abstract socket
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
- Agent-mediated free-form answers for Genie questions, using the current
  embedded `codexd` runtime as the temporary answer engine
- Abstract-unix-socket support in the legacy Rust bridge via `@name` or
  `abstract:name`, so the compatibility transport can move off app-private
  filesystem sockets when Agent<->Genie traffic is introduced

### Not done yet

- Moving network/auth mediation from `codexd` into the Agent runtime
- Expanding the Binder control plane beyond the current fixed-form runtime
  bootstrap/status calls
- Making the Agent the default product surface instead of the legacy service app
- Replacing the remaining Agent-side use of embedded `codexd` for planning and
  auto-answering with a first-class Agent runtime
- Adding more Android-native tool surfaces and richer observation types to the
  hosted Genie runtime

## Current Code Layout

- `android/app`
  - Agent scaffold and legacy `codexd` wrapper live together for now
- `android/genie`
  - standalone Genie scaffold APK with hosted `codex app-server`
- `android/app/src/main/java/com/openai/codexd/CodexAgentService.kt`
  - framework `AgentService`
- `android/app/src/main/java/com/openai/codexd/AgentSessionController.kt`
  - Agent-side `AgentManager` orchestration helper
- `android/app/src/main/java/com/openai/codexd/MainActivity.kt`
  - Agent session UI plus existing `codexd` bridge controls
- `android/genie/src/main/java/com/openai/codex/genie/CodexGenieService.kt`
  - Genie lifecycle host for the embedded `codex app-server`
- `android/genie/src/main/java/com/openai/codex/genie/CodexAppServerHost.kt`
  - stdio JSON-RPC host for `codex app-server`, dynamic tools, and
    `request_user_input` bridging
- `android/app/src/main/java/com/openai/codexd/CodexAgentBridgeService.kt`
  - exported Binder/AIDL bridge for Genie control-plane calls
- `android/genie/src/main/java/com/openai/codex/genie/AgentBridgeClient.kt`
  - Genie-side Binder client for the Agent bridge service
- `android/app/src/main/java/com/openai/codexd/CodexdLocalClient.kt`
  - Agent-local client for the embedded `codexd` bridge

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

The Agent app depends on `just android-service-build` for the packaged
`codexd` JNI binaries. The Genie app now also depends on `just android-build`
for the packaged `codex` JNI binaries.

## Next Implementation Steps

1. Expand the Binder control plane into a fuller Agent<->Genie runtime API.
2. Split the legacy `codexd` concerns out of the Agent UI once the Agent owns
   auth and transport directly.
3. Add more Android-native tool surfaces and richer observation types to the
   hosted Genie runtime.
