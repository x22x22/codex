# Android Native Build (codex)

## Plan (Implemented)

- Standardize TLS on `rustls` so Android builds do not depend on system OpenSSL.
- Treat keyring storage as unsupported on Android and use file-backed storage instead.
- Add a `just android-build` helper that uses `cargo-ndk` to build `codex` for `arm64-v8a` and `x86_64` (API 26).
- Document build and run steps for pushing the binary to a device.

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

Build the Android Agent/Genie prototype APKs with the Android Agent Platform
stub SDK:

```bash
export ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP=/path/to/android-agent-platform-stub-sdk.zip
just android-service-build
cd android
./gradlew :genie:assembleDebug :app:assembleDebug
```
The Agent/Genie prototype modules require
`ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP` (or `-PagentPlatformStubSdkZip=...`) so
Gradle can compile against the stub SDK jar.
If `cargo-ndk` cannot find your NDK, set:

```bash
export ANDROID_NDK_HOME="$HOME/Library/Android/sdk/ndk/<version>"
```

Build outputs:

- `target/android/aarch64-linux-android/release/codex`
- `target/android/x86_64-linux-android/release/codex`

## Run On Device

Example for `arm64-v8a`:

```bash
adb push target/android/aarch64-linux-android/release/codex /data/local/tmp/codex
adb shell chmod +x /data/local/tmp/codex
adb shell /data/local/tmp/codex --help
```

## Authentication on Android

There are two reliable approaches when running the CLI from `adb shell`:

1) ChatGPT login via device code (recommended)

```bash
adb shell /data/local/tmp/codex --device-auth
```

This prints a URL and code. Open the URL on your host and enter the code.

2) MCP OAuth login via host browser + `adb forward`

This flow uses a local callback server on the device. Forward the callback port
from host to device so the redirect can reach the device.

```bash
# Forward host -> device:
adb forward tcp:8765 tcp:8765

# Start the login; the URL will be printed.
adb shell /data/local/tmp/codex mcp login <server_name> --callback-port 8765 --host-browser
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
