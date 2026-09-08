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

if [ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ]; then
  supports_arm64() {
    [ -f "$1/usr/lib/libSystem.tbd" ] || return 1
    # Later embedded TBD documents may support arm64 even when libSystem
    # itself does not. Inspect only the first document's target list.
    while IFS= read -r line; do
      case "$line" in
        install-name:*) break ;;
        *arm64-macos*) return 0 ;;
      esac
    done < "$1/usr/lib/libSystem.tbd"
    return 1
  }
  sdk="${HUTERM_ZIG_SDKROOT:-$(xcrun --sdk macosx --show-sdk-path)}"
  if ! supports_arm64 "$sdk"; then
    [ -z "${HUTERM_ZIG_SDKROOT:-}" ] || fail "HUTERM_ZIG_SDKROOT lacks arm64-macos system stubs: $sdk"
    selected_sdk="$sdk"
    sdk=""
    for candidate in "$(dirname "$selected_sdk")"/MacOSX*.sdk /Library/Developer/CommandLineTools/SDKs/MacOSX*.sdk; do
      if supports_arm64 "$candidate"; then
        sdk="$candidate"
        break
      fi
    done
    [ -n "$sdk" ] || fail "Zig 0.15.2 needs an SDK with arm64-macos system stubs; install a compatible SDK and set HUTERM_ZIG_SDKROOT"
    echo "Ghostty: using SDK stubs from $sdk (selected SDK lacks arm64-macos)" >&2
  fi
  # Keep Xcode's tools (including Metal), but route Zig's SDK path query to
  # compatible stubs. New SDKs may expose only arm64e, which Zig cannot use.
  export HUTERM_ZIG_SDKROOT="$sdk"
  export PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/zig-sdk-bin:$PATH"
fi

exec "$@"
