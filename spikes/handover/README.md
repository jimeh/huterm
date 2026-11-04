# Server handover spike harness (#394)

Throwaway prototype. It lives only on the `spike/394-handover-harness` branch
and is never merged. Findings: `docs/research/server-handover-spike.md` on
`main`.

The harness runs a small terminal server that owns PTYs and hands them to a
new server image while the shells keep running:

- **A, exec in place** (`src/approach_a.rs`): pause readers, write a manifest
  to an unlinked temporary file, clear `FD_CLOEXEC` on the masters, the
  control listener, and the state file, and `execve` the new binary. A build
  that rejects the state execs the rollback copy taken at startup. The
  `check-handover` subcommand is the dry run.
- **B, descriptor passing** (`src/approach_b.rs`, `src/shim_core.rs`): spawn
  the new server, two-phase handshake (`validated`, `ready`, `committed`,
  `owned`), `SCM_RIGHTS` in batches of 64, and a per-terminal shim that is the
  shell's parent and reports the raw wait status as one 16-byte record.

Shared parts are `src/server.rs` (terminals, reader pause and drain, control
socket, nudge, restore) and `src/manifest.rs`. Each terminal's capture file
stands in for the emulator; restore is level 1 (fresh emulator plus a redraw
nudge). A spawns through the vendored portable-pty; B's shim reimplements its
child setup.

## Running

```sh
cargo build --release
cargo build --profile shim --bin handover-shim   # the dedicated tiny shim
./target/release/handover-spike driver scenario all --approach a
./target/release/handover-spike driver scenario all --approach b
./run-all.sh "$PWD/target/release" [path/to/huterm]   # full matrix into results/
```

`driver scenario` accepts `jobcontrol`, `tui`, `heavy`, `exit`, `close`,
`failure`, or `all`. `driver bench` measures cutover for 1, 16, and 128
terminals; `driver rss` measures shim memory. `scripts-fuse.sh` runs the
AppImage-like FUSE case on Linux in Docker with `/dev/fuse` and `SYS_ADMIN`.
`HS_KEEP=1` keeps a run directory for inspection.

The `--pty-shim-idle` switch in the repository's `src/main.rs` on this branch
idles the real Huterm binary before any GPUI setup, to measure the main-binary
shim's memory.

`results/` holds the raw JSON from the runs the findings cite.
