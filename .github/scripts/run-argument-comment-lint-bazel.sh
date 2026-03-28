#!/usr/bin/env bash

set -euo pipefail

manual_rust_test_targets=()
cquery_stdout="$(mktemp)"
cquery_stderr="$(mktemp)"
trap 'rm -f "$cquery_stdout" "$cquery_stderr"' EXIT

ci_config=ci-linux
case "${RUNNER_OS:-}" in
  macOS)
    ci_config=ci-macos
    ;;
  Windows)
    ci_config=ci-windows
    ;;
esac

bazel_startup_args=()
if [[ -n "${BAZEL_OUTPUT_USER_ROOT:-}" ]]; then
  bazel_startup_args+=("--output_user_root=${BAZEL_OUTPUT_USER_ROOT}")
fi

cquery_args=("$@" "--config=${ci_config}" "--keep_going")
if [[ -n "${BAZEL_REPO_CONTENTS_CACHE:-}" ]]; then
  cquery_args+=("--repo_contents_cache=${BAZEL_REPO_CONTENTS_CACHE}")
fi
if [[ -n "${BAZEL_REPOSITORY_CACHE:-}" ]]; then
  cquery_args+=("--repository_cache=${BAZEL_REPOSITORY_CACHE}")
fi
if [[ -n "${BUILDBUDDY_API_KEY:-}" ]]; then
  cquery_args+=("--remote_header=x-buildbuddy-api-key=${BUILDBUDDY_API_KEY}")
fi

# Wildcard target patterns skip manual-tagged unit-test binaries, so resolve
# them explicitly to keep Bazel lint coverage aligned with Cargo --all-targets.
# Use cquery with the active Bazel flags so platform-incompatible manual tests
# are filtered out before the build, especially on Windows.
manual_rust_test_query='kind("rust_test rule", attr("tags", "manual", //codex-rs/...))'
if ! bazel "${bazel_startup_args[@]}" \
  --noexperimental_remote_repo_contents_cache \
  cquery \
  "${cquery_args[@]}" \
  --output=label \
  "$manual_rust_test_query" >"$cquery_stdout" 2>"$cquery_stderr"; then
  if [[ ! -s "$cquery_stdout" ]]; then
    cat "$cquery_stderr" >&2
    exit 1
  fi
fi

while IFS= read -r label; do
  [[ -n "$label" ]] || continue
  manual_rust_test_targets+=("$label")
done <"$cquery_stdout"

./.github/scripts/run-bazel-ci.sh \
  -- \
  build \
  "$@" \
  -- \
  //codex-rs/... \
  "${manual_rust_test_targets[@]}"
