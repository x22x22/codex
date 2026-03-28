#!/usr/bin/env bash

set -euo pipefail

all_manual_rust_test_targets=()
compatible_manual_rust_test_targets=()
incompatible_manual_rust_test_targets=()
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
compatibility_cquery_args=("$@" "--config=${ci_config}")
if [[ -n "${BAZEL_REPO_CONTENTS_CACHE:-}" ]]; then
  cquery_args+=("--repo_contents_cache=${BAZEL_REPO_CONTENTS_CACHE}")
  compatibility_cquery_args+=("--repo_contents_cache=${BAZEL_REPO_CONTENTS_CACHE}")
fi
if [[ -n "${BAZEL_REPOSITORY_CACHE:-}" ]]; then
  cquery_args+=("--repository_cache=${BAZEL_REPOSITORY_CACHE}")
  compatibility_cquery_args+=("--repository_cache=${BAZEL_REPOSITORY_CACHE}")
fi
if [[ -n "${BUILDBUDDY_API_KEY:-}" ]]; then
  cquery_args+=("--remote_header=x-buildbuddy-api-key=${BUILDBUDDY_API_KEY}")
  compatibility_cquery_args+=("--remote_header=x-buildbuddy-api-key=${BUILDBUDDY_API_KEY}")
fi

# The generated unit-test binaries all end in `-unit-tests-bin`. Enumerate
# those labels explicitly so the final Bazel build can subtract them from the
# wildcard target set and then add back only the compatible subset on Windows.
manual_rust_test_query='kind("rust_test rule", filter("-unit-tests-bin$", //codex-rs/...))'
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
  incompatible_manual_rust_test_targets+=("$label")
done < <(
  sed -n 's/^Target \(\/\/[^ ]*\) is incompatible and cannot be built, but was explicitly requested\.$/\1/p' "$cquery_stderr"
)

is_incompatible_manual_rust_test_target() {
  local candidate="$1"
  local incompatible_label
  for incompatible_label in "${incompatible_manual_rust_test_targets[@]}"; do
    if [[ "$candidate" == "$incompatible_label" ]]; then
      return 0
    fi
  done
  return 1
}

manual_rust_test_target_is_compatible() {
  local candidate="$1"
  local compatibility_stdout
  local compatibility_stderr
  compatibility_stdout="$(mktemp)"
  compatibility_stderr="$(mktemp)"
  if bazel "${bazel_startup_args[@]}" \
    --noexperimental_remote_repo_contents_cache \
    cquery \
    "${compatibility_cquery_args[@]}" \
    --output=label \
    "$candidate" >"$compatibility_stdout" 2>"$compatibility_stderr"; then
    if grep -Fq "Target ${candidate} is incompatible and cannot be built, but was explicitly requested." "$compatibility_stderr"; then
      rm -f "$compatibility_stdout" "$compatibility_stderr"
      return 1
    fi

    if [[ ! -s "$compatibility_stdout" ]]; then
      cat "$compatibility_stderr" >&2
      rm -f "$compatibility_stdout" "$compatibility_stderr"
      exit 1
    fi

    rm -f "$compatibility_stdout" "$compatibility_stderr"
    return 0
  fi

  if grep -Fq "Target ${candidate} is incompatible and cannot be built, but was explicitly requested." "$compatibility_stderr"; then
    rm -f "$compatibility_stdout" "$compatibility_stderr"
    return 1
  fi

  cat "$compatibility_stderr" >&2
  rm -f "$compatibility_stdout" "$compatibility_stderr"
  exit 1
}

while IFS= read -r label; do
  [[ -n "$label" ]] || continue
  # cquery emits configured target labels as `//pkg:target (abcdef0)`. Strip
  # the configuration hash before passing the label back to `bazel build`.
  label="${label%% (*}"
  all_manual_rust_test_targets+=("$label")
  if is_incompatible_manual_rust_test_target "$label"; then
    continue
  fi
  if [[ "${RUNNER_OS:-}" == "Windows" ]] && ! manual_rust_test_target_is_compatible "$label"; then
    continue
  fi
  compatible_manual_rust_test_targets+=("$label")
done <"$cquery_stdout"

for label in "${incompatible_manual_rust_test_targets[@]}"; do
  all_manual_rust_test_targets+=("$label")
done

excluded_manual_rust_test_targets=()
for label in "${all_manual_rust_test_targets[@]}"; do
  excluded_manual_rust_test_targets+=("-${label}")
done

./.github/scripts/run-bazel-ci.sh \
  -- \
  build \
  "$@" \
  -- \
  //codex-rs/... \
  "${excluded_manual_rust_test_targets[@]}" \
  "${compatible_manual_rust_test_targets[@]}"
