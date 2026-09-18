#!/usr/bin/env bash
# Runs a command against a display that delivers GPUI frames. Linux uses Xvfb
# with twm: without a window manager the window is never reported visible and
# GPUI stops after its first frame. Other platforms run the command directly.
set -euo pipefail

if [ "$(uname -s)" != Linux ]; then
  exec "$@"
fi
if ! command -v twm >/dev/null 2>&1; then
  echo "headless.sh requires twm so Xvfb delivers frames to the GPUI window" >&2
  exit 1
fi
# shellcheck disable=SC2016
exec xvfb-run -a -s '-screen 0 2560x1440x24 -noreset' sh -c '
  LC_ALL=C twm -f scripts/linux/benchmark.twmrc &
  wm_pid=$!
  "$@"
  result=$?
  kill "$wm_pid" >/dev/null 2>&1 || true
  wait "$wm_pid" >/dev/null 2>&1 || true
  exit "$result"
' sh "$@"
