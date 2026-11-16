# SSH and sudo with Huterm's terminal identity (#351)

Status: approved, after a Codex plan review. Plans [issue #351][issue-351]
(parent [#37][issue-37]), on top of shell integration ([#349][issue-349])
and the `huterm` command ([#350][issue-350]).

## Outcome

`ssh` and `sudo` from Huterm's integrated shells keep a working line editor
when `TERM` is `xterm-huterm`, so `terminal.term = "auto"` becomes the default
again. Configs that set `term` keep their value.

- `huterm ssh` runs the system `ssh` with the user's arguments. When `TERM` is
  `xterm-huterm` and the destination is not known to have the entry, it
  connects with `TERM=xterm-256color`. It can forward `COLORTERM`,
  `TERM_PROGRAM`, and `TERM_PROGRAM_VERSION`, and it can install the entry on
  the remote host.
- `huterm ssh-cache` lists, adds, removes, prunes, and clears the record of
  destinations known to have the entry.
- `huterm sudo` runs the system `sudo`. When `TERM` is `xterm-huterm` and no
  system terminfo directory has the entry, it runs it with
  `TERM=xterm-256color`.
- bash, zsh, fish, and nushell define `ssh` and `sudo` wrappers that call
  these subcommands when the matching features are on.

## Decisions

Jim settled the sudo mechanism on 2026-11-16. The rest follow from the code and
the issue.

- **sudo downgrades `TERM`; it does not preserve `TERMINFO_DIRS`.** sudo drops
  `TERMINFO` and `TERMINFO_DIRS` (they are on its built-in delete list).
  `--preserve-env=TERMINFO,TERMINFO_DIRS` works only under a sudoers rule that
  allows setting variables, such as `ALL`; a restricted rule such as
  `NOPASSWD: /usr/bin/apt` makes sudo refuse the whole command. Downgrading
  works under every policy and loses only `Tc` and `pairs#32767`, the entry's
  only additions to `xterm-256color`. This departs from the issue's
  "keeps `TERMINFO` and `TERMINFO_DIRS`".
- **Feature names and defaults.** `[shell_integration] features` gains
  `ssh_env` (on), `ssh_terminfo` (off, because it writes to `~/.terminfo` on
  remote hosts), and `sudo` (on). Names are snake_case like the existing
  serde enums. The default list becomes `["path", "ssh_env", "sudo"]`.
- **`SendEnv`, not `SetEnv`.** OpenSSH uses only the first `SetEnv` line it
  reads, so `-o SetEnv=…` on the command line silently replaces the user's
  configured `SetEnv` list (verified with `ssh -G` on OpenSSH 10.3). `SendEnv`
  accumulates across sources and sends the local values, which Huterm sets.
  Works with any OpenSSH since 3.9. The remote `AcceptEnv` still decides.
- **Downgrade is intrinsic to `huterm ssh`.** Its flags only add behavior:
  `--forward-env` (the `ssh_env` feature) and `--terminfo` (the
  `ssh_terminfo` feature). The wrapper exists when either feature is on.
- **Logic lives in Rust, wrappers stay one-liners.** Argument parsing,
  destination resolution, the cache, and the install are unit-testable in
  `huterm-cli` and behave the same in every shell.
- **Core sets `TERM_PROGRAM_VERSION`** to the package version beside the
  existing `COLORTERM` and `TERM_PROGRAM` (`crates/huterm-core/src/pty.rs`).
- **Elvish gets no automatic wrapper.** Its integration is already a manual
  `use`, and a module cannot safely replace a REPL function. The docs show a
  one-line `fn ssh` that calls `huterm ssh`.

## `huterm ssh [--forward-env] [--terminfo] [ssh arguments…]`

Huterm's own options are recognized only as leading arguments. Everything after
the first other argument reaches `ssh` unchanged, byte for byte: `--`, `-h`,
`--help`, `-V`, and non-UTF-8 bytes included. No help flag on the subcommand;
`huterm help ssh` documents it. OpenSSH has no long options, so the two flags
cannot collide with an ssh argument.

### Flow

1. **Parse** the ssh arguments with OpenSSH's option grammar (the getopt
   string from the pinned OpenSSH man page; clusters such as `-4p22` and
   `-vvv`; value options taking the rest of the cluster or the next argument;
   options after the destination, as OpenSSH accepts them; the first
   non-option after the destination starts the remote command; `--`). An
   unknown option or a malformed list means "unparsed": skip resolution and
   the install, and downgrade when `TERM` is `xterm-huterm`.
2. **Resolve** with `ssh -G <the user's arguments>`, stdin and stderr null,
   stdout captured up to a size cap, killed and reaped after 10 seconds
   (`Match exec` and canonicalization can run commands or block). Read `user`,
   `hostname`, `sessiontype`, `forkafterauthentication`, `stdinnull`,
   `requesttty`, and `remotecommand`. The cache key is `user@hostname`:
   `HostName` applied, the alias ignored, and `ProxyJump` irrelevant because
   it does not change the destination. A failed or timed-out `ssh -G` behaves
   like "unparsed".
3. **Decide `TERM`.** Only when `TERM` is `xterm-huterm`:
   - a cached key keeps `xterm-huterm`;
   - else, with `--terminfo` and an interactive session, install (below);
     success caches the key and keeps `xterm-huterm`, failure prints one line
     to stderr and downgrades;
   - else downgrade to `xterm-256color`.
   Install-eligible means: no command-line remote command; none of `-W`,
   `-O`, `-G`, `-V`, `-Q`, `-S`, `-M` (the last two would override Huterm's
   master options); and the effective `sessiontype default`,
   `forkafterauthentication no`, `stdinnull no`, `remotecommand none`, and
   `requesttty` other than `no` (read from `ssh -G`, which already applies
   `-N`, `-f`, `-n`, `-s`, `-t`, and `-T` in their order).
4. **Exec** `ssh` (found on `PATH`) with Huterm's options first, then the
   user's arguments unchanged, and the chosen `TERM` in its environment.
   `--forward-env` adds `-o SendEnv=COLORTERM`, `-o SendEnv=TERM_PROGRAM`,
   and `-o SendEnv=TERM_PROGRAM_VERSION`. OpenSSH sends configured `SetEnv`
   values after `SendEnv` ones, so a user's `SetEnv` of the same name wins;
   document that. After an install through Huterm's
   own master, the session adds `-o ControlPath=<socket> -o
   ControlMaster=no`, so it reuses the authenticated connection and falls
   back to a normal connection when the master is gone. `exec` keeps the
   process ID, streams, signals, and exit status. A failure to start `ssh`
   follows the CLI convention: `huterm: <message>`, exit 1.

### Remote install

- **One authentication, always through Huterm's own master.** The install
  runs with `-o ControlMaster=yes -o ControlPath=<socket> -o
  ControlPersist=10` prepended, which take precedence over the user's config
  (OpenSSH keeps the first value; `-S` and `-M` on the command line would not,
  so they make the session install-ineligible). The session then adds `-o
  ControlPath=<socket> -o ControlMaster=no`, so it multiplexes over that
  connection, and connects normally if the master is gone. A user's own
  multiplexing is bypassed for this one first connection; documented.
- **Socket path:** a per-user directory under `XDG_RUNTIME_DIR` when set and
  absolute, else the platform temporary directory (`TMPDIR` on macOS is per
  user), else `/tmp`. Create it `0700`; before use, check it is a directory,
  not a symlink, owned by the effective user, with mode `0700` (reuse
  `huterm-socket`'s checks if they fit), and refuse it otherwise. The socket
  name is unique per invocation (process ID plus random suffix). OpenSSH
  first binds `<path>.<16 random characters>`, so the final path must fit
  within `sun_path` minus 18 bytes: 86 bytes on macOS, 90 on Linux. Try the
  candidates in order until one fits; when none does, the install is
  skipped as ineligible. ControlPersist removes the socket when the master
  exits; a stale socket from a crashed master makes the install fail, which
  falls back.
- **Install command:** Huterm's control options, `-T`, `-o
  ClearAllForwardings=yes`, the user's arguments with every `-t` removed (from
  clusters too), and a remote command; eligibility already excludes the
  settings that would discard stdin, fork, or reject a command. Stdin carries
  Huterm's terminfo source, embedded in the binary with `include_str!` from
  `assets/terminfo/xterm-huterm.terminfo`, so no local `infocmp` or bundle
  lookup is needed. Stdout is captured up to a small cap (the detached
  master redirects its own stdio to `/dev/null`, so it does not hold the
  pipe); stderr is inherited so prompts, banners, and connection errors show.
  Before starting, print `huterm: installing the xterm-huterm terminfo entry
  on <key>` to stderr, so a slow or hanging remote is visible.
- **Remote command:** one line, no single quotes, run as `sh -c '<script>'`
  so non-POSIX login shells (fish, nushell, tcsh) pass it through to `sh`.
  The script:
  - succeeds without writing when `infocmp xterm-huterm` already works;
  - otherwise requires `tic` and `mktemp`, writes stdin to a temporary file
    (removed by an `EXIT` trap, also on interruption), runs `tic -x -o
    "$HOME/.terminfo"` on it with output discarded, and checks that
    `$HOME/.terminfo/x/xterm-huterm` or `78/xterm-huterm` exists (a remote
    without `infocmp` still installs); `-o` keeps a root login from writing
    the system database;
  - on success prints an acknowledgement computed by the remote shell from a
    per-invocation nonce, such as `huterm-terminfo-$((N+1))`. Only that exact
    value counts as success, so a forced command that exits 0 or echoes
    `SSH_ORIGINAL_COMMAND` cannot mark a host as installed.
- **Cancellation:** during preflight and install, `huterm` keeps the default
  signal dispositions. Ctrl-C reaches the foreground process group, so it
  stops both `huterm` and its child `ssh`, and no session starts. There is no
  install deadline, because authentication is interactive.
- **Failure** of any kind (connection, authentication, forced commands such as
  Git hosts, missing `tic`, `sh`, or `mktemp`, an unwritable
  `~/.terminfo`, a remote without `xterm-256color` for the source's `use=`,
  a missing acknowledgement) prints `huterm: could not install the
  xterm-huterm terminfo entry on <key>; using xterm-256color` and continues
  with a downgraded session. That session may prompt again; documented. A
  cache write error after a successful install prints a warning and keeps
  `xterm-huterm`.

## `huterm ssh-cache`

| Command | Behavior |
| --- | --- |
| `list` | One line per entry, sorted: `user@host`, a tab, the install time as UTC `YYYY-MM-DDTHH:MM:SSZ`. Nothing for an empty cache |
| `add <user@host>…` | Adds or refreshes entries with the current time |
| `remove <user@host>…` | Removes entries; an absent one fails with `huterm: <key> is not in the SSH cache`, exit 1, after removing the rest |
| `prune <age>` | Removes entries older than `<age>`: an integer with `s`, `m`, `h`, `d`, or `w` |
| `clear` | Removes every entry |

- **File:** `<state root>/ssh-cache`, using `huterm_state::state_root`
  (`HUTERM_STATE_DIR`, then `XDG_STATE_HOME`, then `~/.local/state/huterm`).
  Lines are `user@host<TAB><unix seconds>`. Reads are capped (1 MiB) and
  skip malformed lines. The directory is created `0700`; an existing
  directory or cache file must be owned by the effective user and not be a
  symlink, or the command fails (and `huterm ssh` treats the cache as empty
  and skips writing). Writes create a unique temporary file in the same
  directory with `create_new` and mode `0600`, then rename it over the old
  one. Concurrent writers can lose an entry, which costs one later
  reinstall; no lock.
- **Keys:** `user@host`, split at the last `@`, both parts non-empty, no
  whitespace or control characters. A bad key is a usage error (exit 2).

## `huterm sudo [sudo arguments…]`

All arguments reach `sudo` unchanged (no help flag; `huterm help sudo`
documents it). When `TERM` is `xterm-huterm` and none of the system terminfo
directories (`/etc/terminfo`, `/lib/terminfo`, `/usr/share/terminfo`,
`/usr/lib/terminfo`, in both the `x/` and `78/` layouts) has the entry, exec
`sudo` with `TERM=xterm-256color`; otherwise exec it with the environment
unchanged.

## Shell wrappers

- **bash, zsh, and fish: decided once, at the first prompt**, after every
  startup file. A per-process flag makes the decision once, so a user who
  later removes or replaces the wrapper keeps their choice. The wrapper is
  defined only when the user has no `ssh` or `sudo` function of their own at
  that point (bash `declare -F`, zsh `$+functions`, fish `functions -q`,
  which also sees autoloaded files and fish aliases). Definitions use the
  `function name { … }` form in bash and zsh, which alias expansion does not
  touch, so an existing `alias ssh='ssh -A'` neither breaks the definition
  nor stops composing with it.
- **Body:** when `$HUTERM_BIN_DIR/huterm` is executable, run it with `ssh`
  plus `--forward-env` and/or `--terminfo` per `HUTERM_SHELL_FEATURES`, then
  the arguments; otherwise `command ssh` (fish `command`, nushell `^ssh`).
  The `sudo` wrapper does the same with `huterm sudo`.
- **Bypass:** `command ssh` and `command sudo` (nushell `^ssh`, `^sudo`).
- **nushell:** the module that `use huterm *` imports (defined in
  `nushell/xdg/nushell/vendor/autoload/huterm.nu`, not the body sourced from
  the first-prompt hook, whose definitions would not persist) exports
  `def --wrapped ssh` and `sudo`, which check the features at call time and
  run `^ssh` or `^sudo` when off. `use huterm *` runs after `config.nu` and
  the autoload directories, so Huterm's definitions replace a user's own
  `def ssh` or `def sudo`; the docs say to turn the feature off to keep one.
- **elvish:** documented one-liners only.
- Scripts stay Huterm-authored; never consult Ghostty's or Kitty's bash or zsh
  integration (GPL), per the shell integration plan.

## Plumbing

- `huterm-config`: `ShellFeature::{SshEnv, SshTerminfo, Sudo}`, the new
  default list, `TerminalIdentity` default `Auto` in every owner: the enum's
  `#[default]`, `TerminalConfig::default`, and `RawTerminal::default`
  (`crates/huterm-config/src/lib.rs`); schema doc comments; `mise run
  schema:generate`. The generated starter config
  (`crates/huterm-gpui/src/config.rs`) writes `term = "auto"` with an updated
  comment, and the default assertions in
  `crates/huterm-gpui/src/config/tests.rs` change with it.
  `shell_integration.features` is already file-only in Settings, so no new
  control.
- `huterm-protocol`: matching `ShellFeature` variants.
- `huterm-wire`: feature bits 2, 4, and 8. Minor version 0 is unreleased, so
  update its records and vectors in place; keep rejecting unknown bits.
- `huterm-core`: `HUTERM_SHELL_FEATURES` lists every enabled feature
  (`path,ssh_env,ssh_terminfo,sudo` order), and `TERM_PROGRAM_VERSION`.
- `huterm-cli`: modules under `commands/` for `ssh`, `ssh_cache`, and `sudo`;
  dependency on `huterm-state` (and `huterm-socket` if used). Run
  `mise run licenses:generate`. The CLI must stay GPUI-free and keep its
  `libSystem`-only macOS linkage.

## Docs

- `docs/configuration.md`: each new feature, its default, the wrapper rules and
  bypass, user functions and aliases, nushell's precedence, elvish's manual
  line, remote requirements (installs need `sh`, `tic`, `mktemp`, a writable
  `~/.terminfo`, and an `xterm-256color` entry; forwarding needs `AcceptEnv
  COLORTERM TERM_PROGRAM TERM_PROGRAM_VERSION`), `SetEnv` precedence, the
  bypassed user multiplexing on an install's first connection, what is not
  wrapped
  (`scp`, `sftp`, Git, scripts, `mosh`, tools that call `ssh` themselves),
  the cache, and the `term` default change with how to opt out.
- `README.md`: the `huterm ssh`, `ssh-cache`, and `sudo` commands, and the
  `auto` default.
- `docs/plans/terminal-capabilities-and-queries.md` header note and the
  terminal engines guide: `auto` is the default again since #351.
- `docs/agents/cli.md` and `crates/huterm-cli/src/lib.rs`: wrapper
  subcommands pass arguments through without a help flag.

## Non-goals

- Wrapping `scp`, `sftp`, `rsync`, Git, or `mosh`; they get the downgrade only
  if the user's `TERM` is not `xterm-huterm` or they never allocate a tty.
- Installing over a session that is not interactive.
- Locking the cache file, cache expiry without `prune`, and keying by port.
- Preserving `TERMINFO`/`TERMINFO_DIRS` through sudo.
- Automatic elvish wrappers; Windows.

## Verification

| Concern | Evidence |
| --- | --- |
| ssh argument grammar: clusters, value options attached and detached, options after the destination, command start, `--`, unknown options, interactive detection, install argument filtering | `huterm-cli` unit tests |
| `ssh -G` parsing, cache key, user-managed `controlpath` detection | Unit tests on captured `-G` text |
| `ssh -G` eligibility: each effective setting that makes a session install-ineligible, `-S`/`-M`, `-t` removal from clusters | Unit tests on captured `-G` text and argument lists |
| `huterm ssh` as a process: arguments byte for byte (incl. `--`, `--help`, non-UTF-8), `TERM` downgrade and keep, cached key keeps `xterm-huterm`, `SendEnv` options only with `--forward-env`, exit status and both streams passed through, same PID (exec), install success caches and keeps `TERM`, a wrong or missing acknowledgement and a failing install warn and downgrade, a hanging `ssh -G` is killed and falls back, Ctrl-C (SIGINT to the process group) during preflight stops both processes and starts no session | Integration tests in `crates/huterm-cli/tests/` with a fake `ssh` script first on `PATH` that answers `-G` and records argv, env, stdin, and PID; `HUTERM_STATE_DIR` scratch |
| Socket directory and cache file checks: wrong owner or mode, symlink, length budget | Unit tests with scratch directories |
| Remote install script: no-op when `infocmp` already knows the entry, installs with `tic`, fails without `tic`, acknowledgement only on success, temporary file removed; and the command string passes through bash, zsh, fish, and nushell as the outer `-c` shell | Unit test that runs the command locally in a scratch `HOME` through each available shell's `-c` (skipping missing ones unless `HUTERM_REQUIRE_SHELLS=1`), skipped when `tic` is missing |
| `huterm ssh-cache` list, add (incl. refresh), remove (incl. absent), prune, clear, malformed lines, bad keys, atomic replace | Unit tests on the store; one integration test per operation through the binary |
| `huterm sudo`: arguments unchanged, downgrade only for `xterm-huterm` without a system entry | Unit test for the decision with fixture directories; integration test with a fake `sudo` |
| Wrappers: per feature combination, the flags passed; `command ssh`/`^ssh` bypass; fallback when `huterm` is missing; available at the first usable prompt. bash, zsh, fish: not over a user function, an existing `alias ssh='ssh -A'` composes, a later user removal or replacement sticks. nushell: replaces a user `def ssh` as documented | Native shell tests in `huterm-core` with fake `ssh`, `sudo`, and `huterm` executables that record their arguments |
| Feature config, defaults, schema, wire bits | `huterm-config` tests, `mise run schema:check`, `huterm-wire` round-trip and vector tests |
| Over the fallback, a remote zsh with zsh-autosuggestions keeps a working line editor: assert rendered cells and cursor after typing, accepting a suggestion, and editing. A control run with `command ssh` fails at that assertion (proves the check) | Linux-only native test (below) |
| Install over a real connection: one authentication for install plus session (count `Accepted publickey` lines in the sshd log), cached key skips the install, a remote without `tic` falls back, the master's disappearance falls back to a normal connection | Same gated test |
| `scp`, `sftp`, and `git ls-remote` over SSH from an integrated shell behave as before | Same gated test |
| Regression | `mise run check`, `mise run test`, `mise run architecture`, CI including ReleaseSafe and packaging |

**The gated sshd fixture.** A non-root `sshd` can only log in its own user,
so the test runs only where that account's home is disposable: the Linux
Docker container (whose `/home/huterm` lives in the container layer) and CI
runners. `HUTERM_SSH_TESTS=1` enables it; the Linux Checks job and
documented `mise run linux:exec` runs set it; it never runs on macOS, even
with the gate set. With the gate set, a missing `sshd` or zsh-autosuggestions
is a failure, not a skip. The fixture: `sshd -D -e -f <scratch config>` on
`127.0.0.1` and a free port, with a scratch host key, `AuthorizedKeysFile`,
`PidFile`, `UsePAM no`, `PermitUserRC no`, `StrictModes no`, `LogLevel
VERBOSE` to a scratch log, `SetEnv HOME=<scratch home>`, and a `ForceCommand`
script that runs `sh -c "$SSH_ORIGINAL_COMMAND"` when a command was given
and otherwise `exec zsh -il` (the account's login shell is bash). The client
uses `-F <scratch config>` with a scratch identity, `IdentitiesOnly`,
`UserKnownHostsFile`, and `StrictHostKeyChecking accept-new`. Add
`openssh-server` and `zsh-autosuggestions` to the CI apt list and the Docker
image. Authentication count is proven for key authentication only; one
password prompt is the hand check.

Hand check for Jim: `ssh` from a Huterm terminal to one of his own hosts with
each feature on and off (with `ssh_terminfo`, one password or passphrase
prompt for a new host, none for a cached one), and `sudo less` with the
`sudo` feature on and off.

[issue-37]: https://github.com/jimeh/huterm/issues/37
[issue-349]: https://github.com/jimeh/huterm/issues/349
[issue-350]: https://github.com/jimeh/huterm/issues/350
[issue-351]: https://github.com/jimeh/huterm/issues/351
