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

# Wildcard target patterns skip manual-tagged unit-test binaries, so resolve
# them explicitly to keep Bazel lint coverage aligned with Cargo --all-targets.
# Use cquery with the active Bazel flags so platform-incompatible manual tests
# are filtered out before the build, especially on Windows.
manual_rust_test_query='kind("rust_test rule", attr("tags", "manual", //codex-rs/...))'
if ! bazel cquery "$@" --config="${ci_config}" --keep_going --output=label "$manual_rust_test_query" >"$cquery_stdout" 2>"$cquery_stderr"; then
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
