#!/usr/bin/env bash
# Select a compatible SDK for the pinned Zig without changing xcode-select.
set -euo pipefail

fail() {
  echo "Ghostty toolchain failed: $*" >&2
  exit 1
}

if [ "$#" -eq 0 ]; then
  fail "expected a command to run with the Ghostty toolchain"
fi

if [ "$(uname -s)" = Darwin ] && [ -z "${DEVELOPER_DIR:-}" ]; then
  selected="$(xcrun --sdk macosx --show-sdk-version)"
  if [ "${selected%%.*}" -ge 27 ]; then
    # Zig 0.15.2 cannot link its build runner against the Xcode 27 beta SDK.
    export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
    compatible="$(xcrun --sdk macosx --show-sdk-version)" ||
      fail "install Xcode 26 or set DEVELOPER_DIR to a compatible Xcode installation"
    if [ "${compatible%%.*}" != 26 ]; then
      fail "selected macOS SDK $selected is incompatible with Zig 0.15.2; set DEVELOPER_DIR to an Xcode 26 installation"
    fi
    echo "Ghostty: using macOS SDK $compatible from $DEVELOPER_DIR" >&2
  fi
fi

exec "$@"
