#!/bin/sh

set -eu

real_ninja="${NINJA_REAL:?NINJA_REAL must point to the downloaded ninja binary}"
staged_ninja="${TMPDIR:-/tmp}/codex-ninja-$$"

cp "${real_ninja}" "${staged_ninja}"
chmod +x "${staged_ninja}"
trap 'rm -f "${staged_ninja}"' EXIT

exec "${staged_ninja}" "$@"
