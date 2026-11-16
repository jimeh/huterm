# Command line

Rules for `crates/huterm-cli`, the `huterm` command. The decision record is
[the CLI plan](../plans/huterm-cli.md); the user-facing behavior is in
[the README](../../README.md#running-huterm-from-a-shell).

## Two executables

- `huterm` (crate `huterm-cli`) is the command line. `huterm-gui` (the root
  `huterm` package) is the desktop app and the macOS `CFBundleExecutable`.
  They sit side by side in `target/<profile>/`, `Huterm.app/Contents/MacOS/`,
  and the Linux packages' `bin/`.
- Run without arguments, `huterm` replaces itself with the `huterm-gui` beside
  its canonicalized path, so the PID, standard streams, and exit status are the
  app's. Desktop entries, `AppRun`, and symlinks into the bundle rely on that.
- Smokes, benches, and VM runs that need the app start `huterm-gui`. The Linux
  desktop smoke and the package smokes start `huterm`, so they also cover the
  launch path.
- `huterm` must run on a host without a display. It must not depend on
  `gpui-pre`, `huterm-gpui`, or `huterm-ui` (`mise run architecture`), must
  link only `libSystem` on macOS, and needs only the C runtime libraries in
  `commandLineLibraries` on Linux. Packaging checks both.

## Adding a subcommand

The crate documentation in `crates/huterm-cli/src/lib.rs` is the source of
truth: one `Command` variant per subcommand, its arguments and `run` function
in a module under `commands/`, help on standard output with status 0, usage
errors from clap on standard error with status 2, and failures as
`huterm: <message>` with status 1.

- A subcommand never starts the desktop app as a side effect.
- Test a subcommand's parsing in the crate's unit tests and its behavior as a
  process in `crates/huterm-cli/tests/`, with `DISPLAY` and `WAYLAND_DISPLAY`
  removed.
- A subcommand that needs a new dependency also changes the shipped notices;
  run `mise run licenses:generate`, which covers `huterm-cli`'s tree.
