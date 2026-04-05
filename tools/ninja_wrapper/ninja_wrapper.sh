#!/bin/sh

set -eu

real_ninja="${NINJA_REAL:?NINJA_REAL must point to the downloaded ninja binary}"

chmod +x "${real_ninja}"

exec "${real_ninja}" "$@"
