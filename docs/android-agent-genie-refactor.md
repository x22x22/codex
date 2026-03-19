# Android Agent + Genie Refactor

## Status

This document tracks the Android Agent Platform refactor that moves Codex from a
single `codexd` foreground-service wrapper to a framework-native Agent/Genie
architecture.

The current repo now contains the first implementation slice:

- `android/app` is a buildable **Agent** app scaffold.
- `android/genie` is a buildable **Genie** app scaffold.
- The Agent app can:
  - register `AgentService`
  - plan target packages for a user objective
  - launch direct parent + child framework sessions
  - start a Genie session for a target package
  - display framework session state and event timelines
  - answer waiting Genie questions
  - attach detached targets
- The Genie app currently validates framework lifecycle, detached-target
  requests, question flow, and result publication with a placeholder executor.
- The first internal Agent<->Genie control plane now uses an exported
  **Binder/AIDL service** in the Agent app, not framework question/answer
  events.
- The current Binder bridge exposes small fixed-form calls, and the Genie
  runtime already uses it to fetch Agent-owned runtime metadata from the
  embedded `codexd`, including auth status and the effective model/provider.
- The Genie runtime now also inspects the paired target package from inside the
  target-app sandbox and feeds package metadata plus launcher intent details
  into the bridged model prompt.
- The Genie scaffold now issues one real **streaming `/v1/responses`** request
  through that bridge after the user answer, proving that model traffic can
  stay Agent-owned even while the Genie runs inside the target-app sandbox.
- The Genie runtime now supports a first generic structured tool loop with
  reusable Android capabilities instead of app-specific hardcoded behavior.
- Non-bridge Genie questions now surface through an Agent-owned notification,
  which gives the Agent a concrete user-escalation path without making the
  Genie the user-facing surface.
- The Agent now also attempts to answer non-bridge Genie questions through the
  embedded `codexd` runtime before falling back to that notification path.
- Runtime testing on the emulator now shows that the exported Agent Binder
  service is reachable from Genie execution for the current control-plane calls.

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
- The current implementation is a placeholder executor. It proves:
  - framework start/cancel behavior
  - question/answer flow
  - detached-target requests
  - result publication
  - Agent-mediated Binder bridge requests across the Agent/Genie boundary

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
- Generic small HTTP request/response envelopes over the Binder bridge, with
  the Genie using the real `codexd` HTTP response bodies
- Agent-owned `/internal/runtime/status` metadata for Genie bootstrap
- Target-app package metadata and launcher-intent inspection from the Genie
  sandbox, with that context included in the bridged model prompt
- One real streaming proxied `/v1/responses` request from Genie through the
  Agent-owned bridge after the user answer
- A generic structured tool loop inside Genie with:
  - `android.package.inspect`
  - `android.intent.launch`
  - detached target show/hide/attach/close
  - detached frame capture
  - UI hierarchy dump
  - shell-backed input injection helpers (`tap`, `text`, `key`)
  - bounded waits
- Agent-owned question notifications for non-bridge Genie questions
- Agent-mediated free-form answers for non-bridge Genie questions, using the
  current embedded `codexd` runtime as the temporary answer engine
- Abstract-unix-socket support in the legacy Rust bridge via `@name` or
  `abstract:name`, so the compatibility transport can move off app-private
  filesystem sockets when Agent<->Genie traffic is introduced

### Not done yet

- Replacing the placeholder Genie executor with a real Codex runtime
- Moving network/auth mediation from `codexd` into the Agent runtime
- Expanding the Binder control plane beyond the current fixed-form runtime-status
  and proxied-HTTP calls
- Wiring Android-native target-driving tools into the Genie runtime
- Making the Agent the default product surface instead of the legacy service app

## Current Code Layout

- `android/app`
  - Agent scaffold and legacy `codexd` wrapper live together for now
- `android/genie`
  - standalone Genie scaffold APK
- `android/app/src/main/java/com/openai/codexd/CodexAgentService.kt`
  - framework `AgentService`
- `android/app/src/main/java/com/openai/codexd/AgentSessionController.kt`
  - Agent-side `AgentManager` orchestration helper
- `android/app/src/main/java/com/openai/codexd/MainActivity.kt`
  - Agent session UI plus existing `codexd` bridge controls
- `android/genie/src/main/java/com/openai/codex/genie/CodexGenieService.kt`
  - placeholder Genie executor
- `android/app/src/main/java/com/openai/codexd/CodexAgentBridgeService.kt`
  - exported Binder/AIDL bridge for Genie control-plane calls
- `android/genie/src/main/java/com/openai/codex/genie/AgentBridgeClient.kt`
  - Genie-side Binder client for the Agent bridge service
- `android/genie/src/main/java/com/openai/codex/genie/CodexAgentBridge.kt`
  - Genie-side request/response helpers for bridged model traffic
- `android/app/src/main/java/com/openai/codexd/CodexdLocalClient.kt`
  - Agent-local client for the embedded `codexd` bridge

## Build

Set the Agent Platform stub SDK zip path:

```bash
export ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP=/path/to/android-agent-platform-stub-sdk.zip
```

Build both Android apps:

```bash
cd android
./gradlew :genie:assembleDebug :app:assembleDebug
```

The Agent app still depends on `just android-service-build` for the packaged
`codexd` JNI binaries.

## Next Implementation Steps

1. Move the placeholder Genie session executor to a real Codex runtime role.
2. Expand the Binder control plane into a fuller Agent<->Genie runtime API.
3. Split the legacy `codexd` concerns out of the Agent UI once the Agent owns
   auth and transport directly.
4. Add Android-native tool surfaces to Genie for target inspection and control.
