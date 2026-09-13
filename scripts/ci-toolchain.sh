#!/bin/bash
set -euo pipefail

# Resolve the repository pin even before Mise has installed Rust or exported it.
RUSTUP_TOOLCHAIN="$(mise tool rust --requested)"
export RUSTUP_TOOLCHAIN

if mise install --locked --jobs=1 rust && mise run verify:toolchain; then
  exit 0
fi

echo "Rust installation or verification failed; reinstalling it once" >&2
# The failed install may not have added Rustup to Mise's environment yet.
# Prefer the same Cargo home used by the installer, including isolated CI homes.
rustup_bin="${CARGO_HOME:-$HOME/.cargo}/bin/rustup"
if [ -x "$rustup_bin" ]; then
  "$rustup_bin" toolchain uninstall "$RUSTUP_TOOLCHAIN"
else
  # An installer failure before Rustup exists has no toolchain to remove.
  echo "Rustup was not installed; retrying bootstrap once" >&2
fi
mise install --locked --force --jobs=1 rust
mise run verify:toolchain
