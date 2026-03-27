# Android Agent/Genie

This file applies to the Android subtree under `android/`. It is developer-facing
context for working on the Android Agent/Genie implementation in this repo.

Do not confuse this with the runtime guidance asset at
`android/bridge/src/main/assets/AGENTS.md`, which is copied into Codex homes on
device for live Agent/Genie sessions.

## Module layout

- `android/app`: Agent app
- `android/genie`: Genie app
- `android/bridge`: shared Android bridge/runtime compatibility layer
- `android/build-agent-genie-apks.sh`: helper for building both APKs
- `android/install-and-provision-agent-genie.sh`: helper for adb install, role
  assignment, and auth seeding

## Default SDK input

When building the Android APKs, use the branch-local Android Agent Platform stub
SDK at:

`$HOME/code/io/ci/aosp/artifacts/aosp/android16-qpr2-release/android-agent-platform-stub-sdk.zip`

The Android build already accepts this through either:

- environment: `ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP`
- script flag: `android/build-agent-genie-apks.sh --agent-sdk-zip ...`
- Gradle property: `-PagentPlatformStubSdkZip=...`

Treat this path as the default SDK source unless the user explicitly says
otherwise.

## Authoritative design references

Read these first when recovering Android Agent/Genie context:

- local architecture/status doc:
  `docs/android-agent-genie-refactor.md`
- SDK docs inside the stub SDK zip:
  - `README.md`
  - `AGENT_GENIE_DESIGN.md`
  - `CONSUMER_GUIDE.md`

Useful inspection commands:

```bash
unzip -p "$HOME/code/io/ci/aosp/artifacts/aosp/android16-qpr2-release/android-agent-platform-stub-sdk.zip" README.md | sed -n '1,220p'
unzip -p "$HOME/code/io/ci/aosp/artifacts/aosp/android16-qpr2-release/android-agent-platform-stub-sdk.zip" AGENT_GENIE_DESIGN.md | sed -n '1,260p'
unzip -p "$HOME/code/io/ci/aosp/artifacts/aosp/android16-qpr2-release/android-agent-platform-stub-sdk.zip" CONSUMER_GUIDE.md | sed -n '1,260p'
```

## Key platform contract to preserve

The current Android work in this repo assumes the same contract described by the
stub SDK docs and the local refactor doc:

- Agent and Genie are separate APKs.
- The framework-managed per-session bridge is the app-private Agent<->Genie
  control plane.
- The framework-owned streaming HTTP exchange is the transport for active
  `/responses` traffic in both top-level Agent planner sessions and live Genie
  child sessions.
- Genie is headless and should not depend on direct internet access.
- Detached target handling must use framework-authoritative presentation/runtime
  state and typed detached-target recovery APIs rather than guessed relaunches.
- App-scoped HOME drafts are real framework `STATE_CREATED` sessions created
  before `startGenieSession(...)`; if you expose that flow outside the on-device
  UI, remember that provisional HOME sessions are expected to hold a
  session-UI lease until they are started or cancelled.
- Desktop draft attach is now implemented by bootstrapping an idle app-server
  runtime before the first turn:
  - HOME drafts start Genie with an internal idle-bootstrap sentinel and become
    attachable immediately after bridge bootstrap.
  - direct AGENT drafts spin up an idle planner app-server host inside the
    Agent process.
  - the first prompt can then be typed in the attached desktop TUI instead of
    being supplied to `sessions start`.

## External reference implementations

There are standalone stub apps outside this repo that are useful for
understanding the intended Android API usage:

- Agent stub root:
  `$HOME/code/omix/AgentStub`
- Genie stub root:
  `$HOME/code/omix/GenieStub`

Especially useful files:

- `$HOME/code/omix/AgentStub/src/com/example/agentstub/ValidationAgentService.java`
- `$HOME/code/omix/AgentStub/src/com/example/agentstub/AgentOrchestrationService.java`
- `$HOME/code/omix/AgentStub/src/com/example/agentstub/SessionActivity.java`
- `$HOME/code/omix/AgentStub/README-standalone.md`
- `$HOME/code/omix/GenieStub/src/com/example/geniestub/ValidationGenieService.java`
- `$HOME/code/omix/GenieStub/README-standalone.md`

Use these as contract/reference implementations for session lifecycle, detached
target control, question flow, and framework HTTP exchange usage.

## Recovery checklist

When returning to Android Agent/Genie work after interruption:

1. Read `docs/android-agent-genie-refactor.md` for the current architecture and
   recent implementation status.
2. Re-read the three markdown files inside the stub SDK zip if the framework
   contract matters for the change.
3. Check `git log --oneline -- android docs/android-agent-genie-refactor.md` to
   see the latest Android-specific changes.
4. If behavior is ambiguous, compare against the AgentStub/GenieStub reference
   implementations before changing repo code.
