#!/bin/bash
# Runs the cargo-fuzz targets in fuzz/ on the nightly that
# fuzz/rust-toolchain.toml pins. With no arguments, runs every target for
# HUTERM_FUZZ_SECONDS (default 60) each; otherwise runs one target and passes
# the remaining arguments to libFuzzer, as in
# `mise run fuzz:wire -- messages -max_total_time=600`.
set -euo pipefail

cd "$(dirname "$0")/../fuzz"
toolchain="$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)"
if [ -z "$toolchain" ]; then
  echo "fuzz/rust-toolchain.toml names no channel" >&2
  exit 1
fi
# Mise exports the workspace's stable toolchain; the fuzz crate needs its own.
export RUSTUP_TOOLCHAIN="$toolchain"
rustup toolchain install --profile minimal "$toolchain"
# cargo-fuzz defaults to the triple it was built for, which a prebuilt
# binary may not share with this machine's toolchain.
host="$(rustc -vV | sed -n 's/^host: //p')"

if [ "$#" -gt 0 ]; then
  target="$1"
  shift
  exec cargo fuzz run --target "$host" "$target" -- "$@"
fi
seconds="${HUTERM_FUZZ_SECONDS:-60}"
for target in $(cargo fuzz list); do
  echo "fuzzing $target for ${seconds}s"
  cargo fuzz run --target "$host" "$target" -- -max_total_time="$seconds"
done
