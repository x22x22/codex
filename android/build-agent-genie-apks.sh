#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Build the packaged Codex binary plus the Android Agent and Genie APKs.

Usage:
  build-agent-genie-apks.sh [--agent-sdk-zip PATH] [--variant debug|release] [--skip-lto]

Options:
  --agent-sdk-zip PATH Path to android-agent-platform-stub-sdk.zip.
                       Defaults to $ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP.
  --variant VALUE      APK variant to build: debug or release. Defaults to debug.
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
variant="debug"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --agent-sdk-zip)
      shift
      [[ $# -gt 0 ]] || fail "--agent-sdk-zip requires a path"
      stub_sdk_zip="$1"
      ;;
    --variant)
      shift
      [[ $# -gt 0 ]] || fail "--variant requires a value"
      variant="$1"
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
[[ "$variant" == "debug" || "$variant" == "release" ]] || fail "--variant must be debug or release"

gradle_task_variant="${variant^}"
agent_apk="$script_dir/app/build/outputs/apk/$variant/app-$variant.apk"
genie_apk="$script_dir/genie/build/outputs/apk/$variant/genie-$variant.apk"
if [[ "$variant" == "release" ]]; then
  agent_apk="$script_dir/app/build/outputs/apk/$variant/app-$variant-unsigned.apk"
  genie_apk="$script_dir/genie/build/outputs/apk/$variant/genie-$variant-unsigned.apk"
fi

echo "Building packaged Codex binary"
(
  cd "$repo_root"
  just android-build
)

echo "Building Android Agent and Genie APKs"
(
  cd "$script_dir"
  ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP="$stub_sdk_zip" \
    ./gradlew ":app:assemble$gradle_task_variant" ":genie:assemble$gradle_task_variant" \
    -PagentPlatformStubSdkZip="$stub_sdk_zip"
)

cat <<EOF
Build complete.

Agent APK:
  $agent_apk

Genie APK:
  $genie_apk
EOF
