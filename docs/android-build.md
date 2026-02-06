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

## Notes

- On Android, keyring-backed credential storage is unavailable; Codex falls back to file-backed storage under `CODEX_HOME`.
