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
  The only vendored code was to be bash-preexec 0.7.0 (MIT, upstream commit
  `d866eeefdb8dfce075cbd4e37dc73c4deb4b0bd2`), shipped unmodified with its
  license; review removed it (see "Revisions during review"). Ghostty's
  bash and zsh scripts are GPLv3 because they derive from
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
- Files are `0644` and directories searchable.

### Packaging

- macOS: add `{ src = "assets/shell-integration", target =
  "shell-integration" }` to the packager resources in `Cargo.toml`.
  `package:macos:bundle` (`mise.toml`) asserts every shipped file, including
  `.zshenv`.
- Linux: `scripts/package-linux.ts` copies the tree to
  `share/huterm/shell-integration`, verifies the same files, and keeps its
  normalized modes. Its tests cover the copy.

**Notices:** none. The first implementation vendored bash-preexec with notices
in `third-party/bash-preexec/`; review removed both.

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

## Revisions during review

Recorded on 2026-11-04 from the dual review of PR #385 and its first CI run.

- **bash-preexec removed.** It drops `ignorespace` from `HISTCONTROL` and turns
  `ignoreboth` into `ignoredups`, so commands typed with a leading space were
  saved. The bash body now marks execution through `PS0` on bash 4.4 and later,
  and through a `DEBUG` trap on bash 3.2 that still runs any `DEBUG` trap the
  user had installed. It never reads history. Huterm now vendors no shell code.
- **Hook order.** Exit status is captured before user prompt hooks run, but
  marker decisions happen after the prompt is built. Bash appends its
  decoration as the last `PROMPT_COMMAND` entry. Zsh schedules decoration
  through `zsh/sched`, after `precmd` hooks and before the single prompt
  expansion. It uses no ZLE widget or reset-prompt. This survives Debian and
  oh-my-zsh replacing `zle-line-init`, as well as later widget registrations.
  Reordering `precmd_functions` during iteration only affects the next cycle
  on zsh 5.9, so an array-only reordering cannot fix the first prompt.
- **Bash hook redesign.** Bash 4.4 and later install no Huterm DEBUG trap.
  `PS0` contains only prompt escapes, including with `promptvars` off, and
  `\#` through parameter prompt expansion tracks command cycles independently
  of history. The 3.2 fallback installs its trap at shell scope without a
  traced installer. It restores `$_` and the incoming status before running
  the original trap verbatim on every user simple command, preserving its
  `extdebug` result and hiding Huterm function internals from that trap.
- **Existing bash-preexec.** First-prompt installation runs after lazy
  installers, then adds execution/status callbacks to their arrays. Decoration
  remains a final prompt-command entry because existing prompt-command themes
  can run after bash-preexec's callback invoker. This deliberately differs
  from callback-only decoration: that approach cannot decorate those themes.
  Huterm leaves bash-preexec's trap intact and calls its interactive-mode
  function with `force` after decoration. Version 0.7 otherwise refuses to
  re-arm from a nested function; 0.5 accepts the argument. Native fixtures
  emulate this ordering independently; temporary checks cover both releases.
- **Jim's PIPESTATUS decision, 2026-11-04.** Keep the DEBUG fallback on Apple
  bash 3.2 without bash-preexec. Any DEBUG trap on that version clobbers
  `PIPESTATUS`, including bash-preexec's, so only the last command's status
  remains. This is the accepted exception to shell-state preservation; all
  other preservation requirements still apply. Bash 4.4 and later retain the
  full pipeline status through their trap-free PS0 path.
- **Standalone bash 3.2 subshells.** `( false )` still has no C/D marks: the
  parent receives no DEBUG event for the compound command, and child trap
  state does not propagate to the parent. Function tracing would change user
  DEBUG firing and cannot communicate completion state back. This remains
  outside the complete marker coverage of the modern PS0 path.
- **Zsh body reads.** The bootstrap requires `/bin/cat` to read the body
  successfully before its in-shell parse check. A missing body therefore
  produces only the generic load diagnostic, and restricted PATHs still work.
- **History.** When `HISTFILE` was not exported, bash ends startup with its
  native unexported default, so history persists as in a plain start.
- **`XDG_DATA_DIRS`.** fish, nushell, and elvish remove only Huterm's entry, and
  restore the saved original only while the value is still exactly what core
  injected. A change the user's startup files made survives. An empty inherited
  value counts as unset when Huterm builds the injected value.
- **Syntax checks.** fish checks with `status fish-path` and zsh parses the body
  inside the running shell, so neither depends on `PATH`.
- **Development resources.** Discovery searches the executable's ancestors, with
  a bound, for `assets/shell-integration`, so nested target directories such as
  ReleaseSafe's find the scripts.
- **CI shells.** macOS requires Apple bash 3.2. Bash 5 is required only on
  Linux, which covers the `--posix` and `ENV` path, login starts included.
- **Smaller fixes.** The missing-bootstrap warning prints once per process.
  Non-interactive detection follows each shell's option syntax and skips script
  operands. An explicit `--rcfile ""` stays distinct from no rcfile. A failed
  nushell body restores the hooks and settings it changed. The package verifier
  checks and reads each file through one handle.

### Correction round, 2026-11-04

Jim decided that Claude implements one more correction round of the bash and
zsh hook layers, and that permissively licensed (MIT) integration scripts may
be read and adapted with recorded provenance. Kitty's, Ghostty's bash/zsh, and
iTerm2's scripts stay off limits. Claude read VS Code's scripts and
bash-preexec 0.5.0 and 0.7.0 for design and interoperability and copied no
code; `assets/shell-integration/README.md` lists the revisions. Jim also
accepted that a standalone subshell such as `( false )` on Apple bash 3.2 has
no C/D marks. Fish, nushell, and elvish were not changed. These bullets
supersede the "Hook order", "Bash hook redesign", and "Existing bash-preexec"
revisions above.

- **Bash completion status.** Bash restores `$?`, `$_`, and `PIPESTATUS` after
  `PROMPT_COMMAND`, so with `promptvars` on, bash 4.4 and later put `D;$?` in
  the decorated prompt. No Huterm command runs before user entries, which see
  the plain state, including the full `PIPESTATUS`. The capture entry remains
  first only on bash 3.2 and with `promptvars` off, where it restores `$?` and
  `$_` and uses `&&` so a failing status triggers neither `errexit` nor `ERR`.
- **Empty input.** `PS0`'s expansion sets a flag through a zero-length
  substring, so bash 4.4 and later detect executed commands without counting.
  With `promptvars` off, a rise in `\#` marks a command; bash 5.3 reports its
  first `${var@P}` read, before any prompt, one higher (per its `parse.y`),
  which is the cause of the 5.2/5.3 difference that broke the fixed offset.
- **`PROMPT_COMMAND` layout.** Each prompt removes Huterm's entries wherever
  they are, including text glued onto them, and re-adds them on lines or
  elements of their own: decoration last, before a trailing
  `__bp_interactive_mode`, and any capture first, after a leading
  `__bp_precmd_invoke_cmd`. When bash-preexec installs itself earlier in the
  same prompt-command run, Huterm re-arms it with `__bp_interactive_mode
  force`. Huterm no longer registers bash-preexec callbacks.
- **DEBUG traps.** Huterm's trap runs on bash 3.2, and on any version where a
  user trap exists after startup. It ignores commands whose text or call stack
  belongs to Huterm, so `set -T` and `extdebug` no longer turn Huterm's own
  statements into C marks, and preexec-style user traps never see Huterm's
  entries. The bootstrap sets the user's trap aside after the startup files
  (two visible statements) and installs Huterm's trap last, at shell scope.
- **Marks.** Bash writes the semicolons in its marks as `\073` and zsh ends its
  marks with ST, so each decoration removes exactly its own marks, including
  copies saved by commands such as a virtualenv's `activate`, and never a
  framework's. An ST backslash before `PS0`'s expansion escaped it, so bash
  marks use BEL.
- **Zsh hooks.** A status hook stays first and a decoration hook last; both
  are restored before each command, and at the first prompt after startup
  files that assigned the hook arrays. The scheduled command decorates only
  when hooks follow the decoration hook. It restores `$?` and `$_` through its
  return value and last argument and cannot restore `pipestatus`, which then
  shows only `$?`. Jim accepted that remaining case, mainly the first prompt
  after startup files add theme hooks, on 2026-11-04. He also accepted the
  bash first-prompt `$_` and `PIPESTATUS`, the bootstrap statements a user
  `DEBUG` trap sees, and the marks a full readline redraw resends, which #352
  must handle by ignoring a D without a preceding C. Commands see the prompt
  without Huterm's marks. With `prompt_percent` unset, zsh prints A and omits
  B.
- **Bootstrap state.** The bash bootstrap restores the startup files' status
  at the first prompt. `$_` and `PIPESTATUS` there come from sourcing the last
  startup file, which the injected launch cannot avoid.
- **C1 controls.** Both shells compare paths against the 32 UTF-8 C1
  sequences as literal substrings, independent of locale and of zsh's
  metafication.

### Final round, 2026-11-04

A comparison of Ghostty, Kitty, VS Code, and iTerm2 found that none meets all
of Huterm's invariants, and that each remaining defect has a general shell
mechanism. Jim decided on 2026-11-04 to run one targeted final round with
Huterm-authored scripts (no GPL route), to switch bash 4 and later from
`--posix` with `ENV` to `--rcfile` with login emulation, and to hold the final
review to no P1 or P2 finding in common supported use, documenting exotic
cases instead of fixing them. These bullets supersede the earlier bash launch
sections ("bash (not Apple's)", "bash (Apple's `/bin/bash`)") and the Apple
resolution rule in "Shell detection".

- **One bash launch.** Every injected bash runs as `bash --rcfile <bootstrap>
  -i`, with `-i` after the user's leading long options because bash accepts
  long options only before short ones. Login flags become
  `HUTERM_BASH_LOGIN=1`, and the bootstrap emulates the login sequence on every
  platform, so the `login_shell`, `logout`, and `.bash_logout` caveats apply to
  all bash login shells. Core no longer resolves the program to detect Apple
  bash and no longer sets `ENV`, `HISTFILE`, `HUTERM_ORIG_ENV`, or
  `HUTERM_ORIG_HISTFILE`. Bash keeps its native `HISTFILE` default, and its
  `shopt` and `set -o` state matches a plain start except `login_shell`;
  `inherit_errexit` and imports of exported functions with non-POSIX names no
  longer differ.
- **Inherited prompt state.** Each `PROMPT_COMMAND` entry is a `case` on
  `${_huterm_bash_loaded-}:$?`, which calls Huterm only in the process that
  loaded it. Elsewhere it keeps `$_`, keeps `$?` without a command for 0, uses
  `[[ -z 1 ]] && :` for 1, and forks `( exit $? ) && :` above 1. The `case ...
  in` header has its own line so bash-preexec recognizes the entry. Huterm's
  `PS1` and `PS0` additions expand Huterm's arrays, which bash never exports;
  `D` comes from `${_huterm_bash_d[$?]-}`, a table of 256 marks. With
  `promptvars` off, the prompts keep static marks, a documented exception.
- **Armed `PS0`.** `PS0`'s C is `${_huterm_bash_c[${_huterm_bash_armed-0}]-}`,
  and its expansion clears the flag without forking. Only the prompt hook sets
  the flag, so a re-sourced `.bashrc` that replaces `PROMPT_COMMAND` leaves no
  C on later commands, and the hook knows a command ran when the flag is clear.
  Huterm restores `PS1` and `PS0` only from the exact values it set and
  otherwise decorates the current value after removing its own marks.
- **bash-preexec 0.7 installer.** Its scalar sanitizer takes seconds when its
  installer's `:` placeholder sits between longer lines, so Huterm keeps a
  scalar's pending `__bp_install "$_"` line last. On bash before 5.1, a
  `precmd` theme that rebuilds `PS1` then leaves the first prompt unmarked, a
  documented exception.
- **Bash 3.2 C.** The `DEBUG` trap writes C to `/dev/tty` and falls back to
  standard output, because a first command inside a redirected group, such as
  `{ ...; } > file`, otherwise captured the mark.
- **Directory check.** Bash compares against literal C0, DEL, and C1 lists
  without `local LC_ALL=C`, which made an uninstalled `LC_ALL` warn at every
  prompt.
- **Zsh ownership.** Zsh restores the undecorated prompt only while `PROMPT`
  still equals the decorated value Huterm set, and no longer strips marks by
  pattern, so a framework's ST- or BEL-terminated marks survive and suppress
  Huterm's OSC 133 stream.
- **Documented exceptions added.** A wrapped user `DEBUG` trap sees
  `PIPESTATUS` holding only the previous status on every version, which can
  change an `extdebug` skip decision. Turning `promptvars` off mid-session
  loses that command's D status. With `promptvars` off and a scalar
  `PROMPT_COMMAND`, prompt hooks see only the previous status in `PIPESTATUS`.
  Zsh re-sourcing a `.zshrc` that assigns `precmd_functions` loses one prompt
  cycle. A full readline redraw, including a terminal resize, resends the
  prompt's marks, D included, so #352 must ignore a D without a new C. Nested
  shells inheriting exported prompt state see `PIPESTATUS` reduced to `$?`
  after a failing command.

### Finishing fix, 2026-11-04

Jim approved a small finishing fix after the final dual review.

- **`--norc`.** Debian-family bash reads its system bashrc for any rcfile
  start, so an injected `--norc` cannot match a plain start. Bash started with
  `--norc` now runs without integration. The double system-bashrc read of an
  emulated login shell on those distributions is documented; Huterm starts
  non-login bash on Linux.
- **ERR traps and errexit.** Huterm's two status restores, at the end of the
  bootstrap and in the wrapper around a user `DEBUG` trap, now run inside an
  `&&` list, so a restored nonzero status no longer fires an ERR trap, and the
  `DEBUG` wrapper no longer exits a `set -e` shell. A startup file that sets
  `set -e` and ends nonzero still exits the shell through the bootstrap's
  `source`, and a user `DEBUG` trap sees the final `: "$_"`; both are
  documented P3 limitations from the verification review.
- **Tests.** The empty-rcfile test waits for Huterm's input marker, since
  Ubuntu's system bashrc replaces the fixture's prompt even in a plain start.
