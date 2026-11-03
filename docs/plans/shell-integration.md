# Shell integration: automatic loading (#349)

Status: approved. Ships #349 only. #352 (OSC 133 command records in core)
follows in a separate pull request.

## Outcome

Huterm loads its integration when it starts bash, zsh, fish, or nushell,
without any edit to the user's startup files. The startup files still run in
their normal order. Each shell reports its working directory with OSC 7 and
the prompt, input, execution, and completion markers with OSC 133, including
the exit status. Elvish gets a module on its path and loads it with a one-line
`use`. A `[shell_integration]` table turns the integration off and selects
optional features. The only feature for now is `path`.

## Decisions

Jim settled these on 2026-11-03.

- **Huterm writes all five scripts itself**, from each shell's documentation.
  The only vendored code is bash-preexec 0.7.0 (MIT, upstream commit
  `d866eeefdb8dfce075cbd4e37dc73c4deb4b0bd2`), shipped unmodified with its
  license. Ghostty's bash and zsh scripts are GPLv3 because they derive from
  Kitty's, so the implementation must not copy from, or consult, Kitty's or
  Ghostty's bash and zsh integration files. It must not copy code from any
  other terminal's scripts either. Shell-level mechanisms such as a `ZDOTDIR`
  stub, `bash --posix` with `ENV`, `--rcfile`, and `XDG_DATA_DIRS` vendor
  directories are documented shell behavior and are fine to use.
- **Config** is `[shell_integration] mode = "detect" | "off"` (default
  `detect`) and `features = []`, a list of names. `path` is opt-in until #350
  makes `huterm` a command-line entry point.
- **Only the shell Huterm starts** loads the integration. Nested shells,
  `exec`, `su`, and `sudo` start plain.
- **New tab and new split directories do not change.** The existing
  `terminal.new_tab_directory = "inherit"` already prefers a local OSC 7
  report.
- **Apple's `/bin/bash` 3.2** ignores `ENV` in POSIX mode (verified on macOS
  27). It loads through `--rcfile`, and the script emulates the login startup
  sequence.
- **Elvish** has no autoload hook. Huterm puts its module on elvish's path, and
  the docs show a guarded `use huterm-integration` line for `rc.elv`. The pull
  request records this as a deviation from #349's "automatic" criterion.
- **Out of scope:** SSH, `sudo`, `title`, and `cursor` (#351), and the CLI
  (#350). Each script reads the enabled features once and lays out one block
  per feature, so #351 can add `ssh-env`, `ssh-terminfo`, and `sudo` blocks
  without restructuring it.

## Architecture

The injection decision lives in `huterm-core`, so a future server applies it
the same way the GPUI client does. A client only states what it wants.

### Protocol

`TerminalCommand` gains `shell_integration: ShellIntegration { mode, features }`.

- `ShellIntegrationMode` is `Detect` or `Off`.
- `ShellFeatures` is a small set of `ShellFeature`. `ShellFeature` is a
  `#[non_exhaustive]` enum whose only variant is `Path`.
- Keep the types dependency-neutral: no serde and no schemars.
  `mise run architecture` must still pass.
- Provide an "off" constant. Migrate all 18 `TerminalCommand` literals (one
  production, one native-Quit smoke, 16 fixtures). Fixtures use off.

### Core

`huterm_core::shell_integration` holds a pure planner:
`plan(&TerminalCommand, &Host) -> LaunchPlan`.

- `Host` carries the resources directory, the executable directory, the target
  OS, an environment lookup, and a resolver for executables.
- The environment lookup returns the inherited environment overlaid with
  `TerminalCommand.environment`, so a client override such as `ZDOTDIR` wins.
- `LaunchPlan` holds the program, the arguments, the variables to set, the
  variables to remove, and at most one warning.
- `pty::spawn` builds the real `Host`, applies the plan with `env` and
  `env_remove` on the builder, and prints the warning, if any, once with
  `eprintln!`, as core reports other runtime diagnostics. No UI notice.
- Unit tests build fixture hosts.

### Resource discovery

Discovery runs in core and is executable-relative, like terminfo's.

- Candidates: `Contents/Resources` in a macOS bundle, `share/huterm` beside a
  Linux package's `bin`, and, for development builds, the repository's
  `assets` directory found relative to `target/<profile>/` (including
  `examples/`).
- Use no compile-time absolute path.
- Update the macOS and Linux VM dev staging (`scripts/linux-vm.ts`,
  `scripts/macos-vm/guest.sh`) so staged binaries find `assets/shell-integration`.

### Client

`RawConfig` feeds the GPUI `Config`, which feeds `shell_command`. That function
copies `[shell_integration]` into the command. A reload affects only terminals
started afterwards.

### Shell detection

Detection runs only with the mode `detect`. Leave the command unchanged for an
unsupported shell, for `-c`, for bash `--posix`, and for nushell
`--commands`/`--lsp`.

- **Shell identity** is the invocation name: the basename of `program` as
  given. Shells change behavior with their invocation name, so a symlink named
  `sh` that points at bash is not bash.
- **Apple-bash check:** resolve a bare or relative `program` with the child's
  effective `PATH` and the launch working directory, the way portable-pty
  does, then canonicalize symlinks. The Apple path applies only when the
  invocation name is `bash`, the target is macOS, and the canonical path is
  `/bin/bash`.
- **Never change `argv[0]`.**

### Environment contract

The `HUTERM_*` names below belong to core. Before applying a plan, core
removes every inherited core-owned variable, so stale values from a Huterm
started inside Huterm cannot leak. It then sets only what the plan needs.

- **`HUTERM_RESOURCES_DIR` and `HUTERM_BIN_DIR`** are exported for every
  terminal when they resolve, including with the mode `off` and for unsupported
  shells. They are terminal identity, like `TERM_PROGRAM`, and they let users
  source the scripts by hand. "Starts exactly as without integration" means the
  same program and arguments, no other `HUTERM_*` variables, and no changes to
  `ZDOTDIR`, `ENV`, `HISTFILE`, `XDG_DATA_DIRS`, or `PATH`.
- **`HUTERM_SHELL_FEATURES`** is a comma-separated list. It is set only when
  injection applies and the list is non-empty, and removed otherwise. It stays
  exported, so a nested shell that sources a script by hand gets the same
  features.
- **Injection-only variables** carry state into the shell, and the shell
  removes each one as soon as it reads it:
  - `HUTERM_SHELL_INTEGRATION` names the shell the variables are meant for and
    admits one load.
  - `HUTERM_BASH_INJECT` records `--norc` and `--noprofile`.
  - `HUTERM_BASH_LOGIN` asks for login emulation.
  - `HUTERM_BASH_RCFILE` records the user's `--rcfile` or `--init-file`.
  - `HUTERM_ORIG_ENV`, `HUTERM_ORIG_HISTFILE`, `HUTERM_ORIG_ZDOTDIR`, and
    `HUTERM_ORIG_XDG_DATA_DIRS` save each original value. Each also records
    whether the variable was unset or empty, so restoration is exact.
  - `HUTERM_XDG_ENTRY` names the directory that was prepended.
- When `XDG_DATA_DIRS` is unset, prepend to the XDG default
  `/usr/local/share:/usr/share`, and restore it as unset afterwards.

### Injection per shell

Each shell that uses `XDG_DATA_DIRS` gets its own XDG data root, which holds
only that shell's files. A leftover entry therefore never exposes one shell's
scripts to another shell.

#### zsh

- Injection: set `ZDOTDIR` to a stub directory. Its `.zshenv` is a small
  bootstrap that restores or unsets `ZDOTDIR` exactly, sources the user's
  `.zshenv`, and then loads the integration body for interactive shells only.
- Startup order: native zsh order. `.zshenv`, then `.zprofile` for a login
  shell, then `.zshrc`, then `.zlogin` for a login shell.

#### bash (not Apple's)

- Injection: run `bash --posix` with `ENV` pointing at the bootstrap.
- Startup order: the bootstrap leaves POSIX mode, restores `ENV` and
  `HISTFILE`, then sources the startup files. A login shell gets
  `/etc/profile` and then the first of `~/.bash_profile`, `~/.bash_login`, or
  `~/.profile`. A non-login shell gets `~/.bashrc` or the user's `--rcfile`.
  `--norc` and `--noprofile` are honored.

#### bash (Apple's `/bin/bash`)

- Injection: run `bash --rcfile <bootstrap> -i`. Remove `-l`, `--login`, and
  the `l` in combined flags such as `-il`, and set `HUTERM_BASH_LOGIN=1` when
  any was present. Carry `--norc`, `--noprofile`, and `--rcfile` or
  `--init-file`.
- Startup order: the login sequence above.
- Documented caveat: `shopt login_shell` is off, `logout` refuses, and
  `~/.bash_logout` does not run.

#### fish

- Injection: prepend the fish XDG root, which holds
  `fish/vendor_conf.d/huterm.fish`.
- Startup order: native fish order. Vendor `conf.d` runs before `config.fish`.

#### nushell

- Injection: prepend the nushell XDG root, which holds
  `nushell/vendor/autoload/huterm.nu`, and add `--execute 'use huterm *'`.
- Startup order: native nushell order.
- OSC 133 remains native. When `shell_integration.osc7` is true at module
  load, replace native OSC 7 with Huterm's literal reports at each prompt and
  on PWD changes. When false, emit no OSC 7. Skip hosts or paths with controls.

Revision, 2026-11-03: nushell 0.116.1 sends unencoded `file://` paths, so `#`,
`?`, and `%` can leave core's directory metadata stale. Disable its enabled
native OSC 7 and use `kitty-shell-cwd://` hooks instead. Keep core's `file://`
parser and nushell's native OSC 133 unchanged. The bootstrap suppresses the
first native report before the body loads to preserve one OSC 7 stream.

#### elvish

- Injection: prepend the elvish XDG root, which holds
  `elvish/lib/huterm-integration.elv`, only.
- Startup order: unchanged. The user adds `use huterm-integration`.

### Bootstrap and body

Each automatic path splits into a small bootstrap and a body.

- **The bootstrap** does only what startup correctness needs:
  1. Check `HUTERM_SHELL_INTEGRATION` and consume the injection variables.
  2. Restore the user's environment exactly.
  3. Source the user's startup files.
  4. Load the body inside the shell's error boundary: a subshell syntax check
     where available, plus `{ … } always` in zsh, `source … || …` in bash,
     and `try` in nushell and elvish.
- **If the body fails** to parse or load, the bootstrap prints one line,
  `huterm: shell integration failed to load: <path>`, removes any hooks it had
  installed, and continues.
- **Hook guard:** hook functions run with their own guard, so a runtime
  failure inside a hook disables integration for that shell after one message
  rather than repeating it at every prompt.
- **A missing bootstrap** is caught by core before spawn: core starts the
  original command with the warning.
- **A broken bootstrap** may print the shell's own error, but the user's
  startup files must still run. Keep the bootstrap minimal and cover it with a
  native test. If a shell cannot meet this, record the limitation.
- **Cleanup does not depend on the body loading.** The bootstrap and the
  shell's first script both restore `XDG_DATA_DIRS` and remove the variables.
  Elvish without `use` keeps its own XDG entry, but that root holds only
  elvish files.

### Markers

- **Sequences:** OSC 133 `A` at prompt start, `B` at input start, `C` before
  execution, and `D;<status>` after completion. OSC 7 goes at each prompt and
  after each directory change. Use only sequences and options that the pinned
  libghostty-vt parses.
- **Exactly once:** a per-shell-process guard makes automatic loading plus a
  manual `source`, or a second `source`, a no-op.
- **fish 4.9.3** natively emits the full OSC 133 and OSC 7 stream (observed).
  **nushell 0.116.1** enables `shell_integration.osc133` and `osc7` by default
  (observed). Huterm leaves native OSC 133 alone and replaces enabled nushell
  OSC 7 as described in the revision above. The native tests prove exactly
  one stream. Disabling nushell's markers before module load opts out of
  those reports.
- **elvish** emits `A`, `C`, `D;status`, and OSC 7. It has no `B`: its prompt
  renders asynchronously and escapes its output. This is a documented
  limitation.
- **Prompt frameworks:** bash and zsh scripts check the prompt strings (`PS1`,
  `PROMPT`, `PS2`) at each prompt for an existing OSC 133 mark. They match
  literal `\e]133;`, `\033]133;`, and `\x1b]133;` forms as well as real ESC
  bytes. If one is found, they skip their own OSC 133 output for that cycle
  and keep OSC 7 and `path`. Markers that a framework prints from its own
  hooks cannot be detected; the docs list that limitation.

### OSC 7 format

Scripts send `kitty-shell-cwd://<host><path>`, an unencoded path that is easy
to produce in a shell.

- **Core:** add a separate literal parser beside `normalize_directory`
  (`crates/huterm-core/src/engine.rs`).
  - The host runs up to the first `/`, and the rest is the path, taken byte for
    byte with no URL parsing and no percent-decoding.
  - Apply the same size limit and the same host and locality rules as
    `file://`.
  - Reject C0, DEL, and C1 control characters, and an empty or relative path.
  - `file://` handling stays unchanged.
- **Scripts:** skip the report when the host or path contains a control
  character. Never strip characters, because a stripped path can name a
  different existing directory that new tabs would then inherit.

### `path` feature

Append `HUTERM_BIN_DIR` to `PATH` after the user's startup files run, and only
when it is not already a component.

## Configuration, docs, and packaging

**Config:** add `ShellIntegrationConfig` in `huterm-config` with
`deny_unknown_fields`.

- `mode` defaults to `detect`.
- `features` is a list of known names. An unknown name is a config error, and
  duplicates collapse.
- Add schema doc comments, then run `mise run schema:generate`.

### Settings

- `mode` needs every piece of metadata `crates/huterm-config/src/settings.rs`
  requires: label, section, group, order, enum choice labels, and
  `x-huterm-apply = "new_terminals"`.
- List `shell_integration.features` in `FILE_ONLY`
  (`crates/huterm-gpui/src/desktop/settings.rs`).
- The coverage test must stay green.

**Assets:** `assets/shell-integration/`, with one directory per shell and an
XDG root per XDG shell.

- Every file opens with a Huterm header.
- bash-preexec ships unmodified with its upstream header.
- Files are `0644` and directories searchable.

### Packaging

- macOS: add `{ src = "assets/shell-integration", target =
  "shell-integration" }` to the packager resources in `Cargo.toml`.
  `package:macos:bundle` (`mise.toml`) asserts every shipped file, including
  `.zshenv` and `bash-preexec.sh`.
- Linux: `scripts/package-linux.ts` copies the tree to
  `share/huterm/shell-integration`, verifies the same files, and keeps its
  normalized modes. Its tests cover the copy.

**Notices:** add `third-party/bash-preexec/`. It holds the MIT license, plus a
README recording the version, the commit, and the source URL. Register it in
`third-party/licenses/components.json`, then run `mise run licenses:generate`
and `mise run license`.

### Docs

- `docs/configuration.md` gets a "Shell integration" section covering:
  - modes and features;
  - each shell and how it loads;
  - manual sourcing per shell: guarded by `HUTERM_RESOURCES_DIR` for bash, zsh,
    and fish; nushell's parse-time constant path; elvish's `use`;
  - nested shells, `exec`, `su`, `sudo`, and subshells;
  - the limitations: the Apple bash caveats, elvish's missing `B` mark, an
    `/etc/zshenv` that sets `ZDOTDIR` before the stub runs, startup files that
    replace `XDG_DATA_DIRS` before fish or nushell read it, undetectable
    framework hook markers, and nushell's disabled native markers.
- `README.md`: a short mention.
- The core-runtime guide: the placement rule, the Apple bash hazard, the
  GPL-provenance rule for scripts, and the `kitty-shell-cwd` parser.

## Nested shells, exec, su, sudo, subshells

Only the shell Huterm starts is admitted, and the admission variable is
consumed on first use. A nested `zsh`, `bash`, or `fish`, an `exec zsh`, or a
shell under `su` or `sudo` therefore starts plain, even if a leftover
`XDG_DATA_DIRS` entry survives.

Subshells such as `( … )` and `$( … )` inherit functions but run no prompt
hooks, so they emit no extra markers. To get integration in nested shells, a
user sources the script from a startup file, and the exactly-once guard
prevents double hooks.

## Non-goals

- The `huterm` CLI (#350). `path` only puts the directory on `PATH`.
- SSH and `sudo` wrappers, and changing the `terminal.term` default (#351).
- Core consumption of OSC 133: command records, navigation, and notifications
  (#352, #38, #81).
- `title` and `cursor` features.
- New windows inheriting directories, and restore (#42).
- Automatic elvish loading.
- Windows.

## Verification

| Concern | Evidence |
| --- | --- |
| Planner: detection by invocation name, Apple-bash resolution (bare `bash` through `PATH`, relative paths, a symlink named `sh`), bail-outs (`-c`, `--posix`, nushell `--commands`/`--lsp`), combined login flags, `--norc`/`--noprofile`/`--rcfile`/`--init-file`, exact preservation of unset and empty `ENV`/`HISTFILE`/`ZDOTDIR`/`XDG_DATA_DIRS`, client override precedence, removal of stale inherited `HUTERM_*` values, empty features, the `off` and unsupported cases, and a missing bootstrap with its single warning | Core unit tests on `plan` with fixture hosts and resource directories |
| `kitty-shell-cwd://`: an exact path with spaces, `#`, `?`, and `%`; host and locality; rejection of controls, oversize, empty, and relative paths; `file://` behavior unchanged; a local report feeding inheritance | Core unit tests beside the existing OSC 7 tests, plus one test through `inherited_directory` |
| Config defaults, parsing, unknown features, schema, Settings metadata and coverage | `huterm-config` and Settings tests, and `mise run schema:check` |
| Per shell (zsh, bash 5, Apple bash 3.2 on macOS, fish, nushell; elvish after `use`): admitted once; startup files in their normal order for login and non-login starts; well-formed OSC 7 and OSC 133 `A`/`B`/`C`/`D;status` with zero and nonzero status (elvish has no `B`); exactly one marker stream, including after a manual re-`source`; `path` added once and only when missing; a nested same-shell start, and a fish or nushell start under elvish, coming up plain; a broken body giving one message while startup files still run; a broken bootstrap still running startup files; `off` matching a plain start; a prompt with an existing `133;A` gaining no duplicate (bash and zsh) | An in-crate native test module in `huterm-core`. It drives production `pty::spawn` with the real plan and reads raw PTY output through a test-only tap. It uses a temporary `HOME`, with a `ZDOTDIR` variant, whose startup files append their names to a log. Waits are event-driven with bounded deadlines. A failure prints the raw transcript, and every child is cleaned up |
| Shell availability | fish 4.9.3 (`aqua:fish-shell/fish-shell`) and nushell 0.116.1 (`aqua:nushell/nushell`) install through Mise on macOS. Elvish 0.21.0 installs through Mise's `http:` backend from `dl.elv.sh` (verified on darwin-arm64). Pin all three in `mise.toml` and `mise.lock` for macOS arm64, Linux x86_64, and Linux aarch64, inside the three-day policy. zsh comes from the host and from apt on Linux CI. The test skips a missing shell with a printed notice unless `HUTERM_REQUIRE_SHELLS=1`, which only the Checks jobs' Rust test step sets. No new required check |
| Packaging | `package:macos` and the Linux package verification assert every shipped file. The `package-linux` tests cover the copy |
| Licenses | `mise run license`, `mise run licenses:check`, and the regenerated `licenses.json` |
| Regression | `mise run check`, `mise run test`, `mise run architecture`, and CI, including ReleaseSafe |

A UX review is not planned: the change adds one Settings control and no new
interaction. A running-app spot check with the dev build and zsh confirms that
`cd` updates "Copy Directory Path" through OSC 7.

## Open questions

None. If a pinned shell's native markers differ from what was observed above,
the implementer records the finding and follows the exactly-one-stream rule.
