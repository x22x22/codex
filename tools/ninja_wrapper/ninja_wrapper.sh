#!/bin/sh

set -eu

real_ninja="${NINJA_REAL:?NINJA_REAL must point to the downloaded ninja binary}"
staged_dir="$(mktemp -d /var/tmp/codex-ninja.XXXXXX)"
staged_ninja="${staged_dir}/ninja"

cp "${real_ninja}" "${staged_ninja}"
chmod +x "${staged_ninja}"
trap 'rm -rf "${staged_dir}"' EXIT

exec "${staged_ninja}" "$@"
