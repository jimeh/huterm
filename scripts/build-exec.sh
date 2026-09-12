#!/usr/bin/env bash
# Preserve the shared build entrypoint without overriding the selected Apple SDK.
# Ghostty's pinned Zig 0.16 build handles Xcode 27 headers and native linking.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "Ghostty toolchain failed: expected a command to run" >&2
  exit 1
fi

exec "$@"
