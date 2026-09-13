# Ghostty upgrade for upstream clipboard handling

Issue #118 later made this Ghostty integration the only terminal engine. The
upgrade evidence and clipboard behavior recorded here remain applicable.

Upgrade the Rust bindings to upstream revision
`5988a0b78b4aa804d1c12e66bbfe662bd97d81c0` and its native Ghostty pin
`22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018`. This brings upstream OSC allocation
bounds and clipboard callback safety fixes into Huterm. Keep Alacritty as the
default and leave clipboard delivery implementation to issue #80.

1. Vendor the published wrapper archive and record the upstream source delta.
   Update the existing sys crate through a named vendor patch, preserving CPU
   baseline, source verification, private build staging, and the memset fix.
2. Refresh native archive and dependency hashes and license notices. Keep Zig
   0.16 unless the matched upstream source requires otherwise.
3. Adapt the Ghostty integration to the matched API, preserving current history
   retention and snapshot behavior. Test empty/binary clipboard callbacks and
   oversized OSC recovery without connecting them to the OS clipboard.
4. Run vendor/native reproduction checks, focused engine and runtime tests,
   Linux input smoke and scroll benchmark, then the broad verification task.
   Native macOS behavior requires a macOS host and remains a separate check.
5. Record the observed result and any compatibility gaps. Revise the OSC 52
   plan to use upstream parsing limits if this upgrade passes.

## Observed result

Both crates now contain the exact upstream source at the selected revision,
reproduced as patches over published 0.2.1 archives. The sys crate retains only
local rebuild-path, MIT packaging, and private-source staging fixes. Upstream
supplies CPU targeting, Zig 0.16 support, OSC bounds, and clipboard safety.

The adapter uses `Terminal::new(columns, rows)` and then
`set_scrollback_max_bytes(Some(16 * 1024 * 1024))`, preserving its history budget.
No default-engine or OS clipboard integration change is included.

- Vendor reproduction, pristine native-source verification, build staging tests,
  license audit, and full `mise run verify` pass.
- The core suite passes 103 tests, with 3 ignored. Two new regressions cover
  empty/binary callbacks and oversized OSC 52/1337 recovery.
- Linux native input smoke passes for both engines with exact expected bytes.
- The Ghostty release scroll benchmark passes its budgets: p95 snapshot elapsed
  time is 683 microseconds, with one maximum queued request. These are host
  elapsed-time measurements, not a comparison with the previous revision.
- The first broad run timed out in both engines' existing final-output tests;
  the unchanged full retry passed. No timing workaround was added.
- Native macOS validation requires a macOS host. Upstream wrapper dead-code
  warnings remain when optional graphics support is disabled.

## Unresolved questions

Making Ghostty the default remains a separate decision after native macOS
validation. Clipboard delivery and user-facing permission remain issue #80.
