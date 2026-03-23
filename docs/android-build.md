# Android Native Build (codex)

## Plan (Implemented)

- Standardize TLS on `rustls` so Android builds do not depend on system OpenSSL.
- Treat keyring storage as unsupported on Android and use file-backed storage instead.
- Add a `just android-build` helper that uses `cargo-ndk` to build `codex` for `arm64-v8a` and `x86_64` (API 26).
- Document build and run steps for pushing the binary to a device.
- Build the Android Agent/Genie apps directly on `codex`.
- Have Gradle rebuild the packaged Android `codex` binary automatically instead
  of silently reusing a stale prebuilt artifact.

## Prerequisites

- Install [Android NDK](https://developer.android.com/studio/projects/install-ndk) r29 (recommended)
    - set `ANDROID_NDK_HOME` accordingly, e.g.:
```bash
set export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/<version>/
```

- `cargo-ndk` (`cargo install cargo-ndk`).
- Rust target: `rustup target add aarch64-linux-android`
- Rust target: `rustup target add x86_64-linux-android`

## Build

From the repo root:

```bash
just android-build
```

For faster local iteration, you can skip Android release LTO:

```bash
CODEX_ANDROID_SKIP_LTO=1 just android-build
```

The Agent and Genie APKs now package only `codex`.

Build the Android Agent/Genie prototype APKs with the Android Agent Platform
stub SDK:

```bash
export ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP=/path/to/android-agent-platform-stub-sdk.zip
cd android
./gradlew :genie:assembleDebug :app:assembleDebug
```

Or use the helper script from `android/`:

```bash
cd android
./build-agent-genie-apks.sh --agent-sdk-zip /path/to/android-agent-platform-stub-sdk.zip --variant debug
```

If you prefer the system Gradle install, use `gradle :app:assembleDebug` from
`android/`.

The Agent/Genie prototype modules also require
`ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP` (or `-PagentPlatformStubSdkZip=...`) so
Gradle can compile against the stub SDK jar. The Agent APK and Genie APK both
package the Android `codex` binary as `libcodex.so`. Gradle now rebuilds that
native binary automatically before `:app:assembleDebug` and
`:genie:assembleDebug`, so plain APK builds no longer silently package a stale
`libcodex.so`.

To install both APKs, assign the AGENT/GENIE roles, grant notifications, and
optionally seed `auth.json` into the Agent sandbox:

```bash
cd android
./install-and-provision-agent-genie.sh --serial emulator-5554 --variant debug
```

For release APKs:

```bash
cd android
./build-agent-genie-apks.sh --agent-sdk-zip /path/to/android-agent-platform-stub-sdk.zip --variant release
```

The default release outputs are the standard Gradle unsigned artifacts
(`app-release-unsigned.apk` and `genie-release-unsigned.apk`). Sign them before
calling `install-and-provision-agent-genie.sh --variant release`, or override
the install paths with `--agent-apk` and `--genie-apk`.

If `cargo-ndk` cannot find your NDK, set:

```bash
export ANDROID_NDK_HOME="$HOME/Library/Android/sdk/ndk/<version>"
```

Build outputs:

- `target/android/aarch64-linux-android/release/codex`
- `target/android/x86_64-linux-android/release/codex`

`just android-build` is still useful when you want the standalone Android CLI
binary for `adb push`, but it is no longer required as a manual prerequisite for
building the Agent and Genie APKs.
## Run On Device

Example for `arm64-v8a`:

```bash
adb push target/android/aarch64-linux-android/release/codex /data/local/tmp/codex-bin
adb shell chmod +x /data/local/tmp/codex-bin
adb shell /data/local/tmp/codex-bin --help
```

## Authentication on Android

There are two reliable approaches when running the CLI from `adb shell`:

1) ChatGPT login via device code (recommended)

```bash
adb shell /data/local/tmp/codex-bin --device-auth
```

The Agent app now owns authentication directly through the hosted Agent runtime.

2) MCP OAuth login via host browser + `adb forward`

This flow uses a local callback server on the device. Forward the callback port
from host to device so the redirect can reach the device.

```bash
# Forward host -> device:
adb forward tcp:8765 tcp:8765

# Start the login; the URL will be printed.
adb shell /data/local/tmp/codex-bin mcp login <server_name> --callback-port 8765 --host-browser
```

Open the printed URL on your host. When the provider redirects to
`http://127.0.0.1:8765/...`, the port forward sends it to the device and the
login completes. Remove the forward when done:

```bash
adb forward --remove tcp:8765
```

## Notes

- On Android, keyring-backed credential storage is unavailable; Codex falls back to file-backed storage under `CODEX_HOME`.
- If `CODEX_HOME` is not set, Codex defaults to `/data/local/tmp/codex` on Android.
- Avoid naming the binary `/data/local/tmp/codex`; it collides with the default `CODEX_HOME`. Use `/data/local/tmp/codex-bin` or set `CODEX_HOME`.
- The Android Agent app stores auth in its own `files/codex-home/auth.json` and
  uses that state for Genie traffic forwarded over the framework session bridge.
