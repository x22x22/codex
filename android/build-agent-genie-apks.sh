#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Build the packaged Codex binary plus the Android Agent and Genie debug APKs.

Usage:
  build-agent-genie-apks.sh [--stub-sdk-zip PATH] [--skip-lto]

Options:
  --stub-sdk-zip PATH  Path to android-agent-platform-stub-sdk.zip.
                       Defaults to $ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP.
  --skip-lto           Set CODEX_ANDROID_SKIP_LTO=1 for faster local builds.
  -h, --help           Show this help text.
EOF
}

fail() {
  echo "error: $*" >&2
  exit 1
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
stub_sdk_zip="${ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --stub-sdk-zip)
      shift
      [[ $# -gt 0 ]] || fail "--stub-sdk-zip requires a path"
      stub_sdk_zip="$1"
      ;;
    --skip-lto)
      export CODEX_ANDROID_SKIP_LTO=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
  shift
done

[[ -n "$stub_sdk_zip" ]] || fail "set ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP or pass --stub-sdk-zip"
[[ -f "$stub_sdk_zip" ]] || fail "stub SDK zip not found: $stub_sdk_zip"

echo "Building packaged Codex binary"
(
  cd "$repo_root"
  just android-build
)

echo "Building Android Agent and Genie APKs"
(
  cd "$script_dir"
  ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP="$stub_sdk_zip" \
    ./gradlew :app:assembleDebug :genie:assembleDebug -PagentPlatformStubSdkZip="$stub_sdk_zip"
)

cat <<EOF
Build complete.

Agent APK:
  $script_dir/app/build/outputs/apk/debug/app-debug.apk

Genie APK:
  $script_dir/genie/build/outputs/apk/debug/genie-debug.apk
EOF
