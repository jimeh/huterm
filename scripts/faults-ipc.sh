#!/usr/bin/env bash
# Runs the seeded fault-injection harness over the real IPC server and
# client in release: `faults:ipc [SEEDS]` runs every scenario over SEEDS
# seeds (200 by default), and `faults:ipc SCENARIO SEED` replays one run.
set -euo pipefail

case "$#" in
  0) ;;
  1) export HUTERM_FAULT_SEEDS="$1" ;;
  2) export HUTERM_FAULT_REPLAY="$1 $2" ;;
  *)
    echo "usage: faults:ipc [SEEDS] | faults:ipc SCENARIO SEED" >&2
    exit 2
    ;;
esac

output=$(bash scripts/build-exec.sh cargo test --release --locked -p huterm-server --lib \
  faults::tests::fault_injection_long_run -- --ignored --nocapture --exact 2>&1) || {
  printf '%s\n' "$output"
  exit 1
}
printf '%s\n' "$output"
# A renamed or moved harness would otherwise run no test and pass.
case "$output" in
  *"test result: ok. 1 passed"*) ;;
  *) echo "faults:ipc: fault_injection_long_run ran no test" >&2; exit 1 ;;
esac
