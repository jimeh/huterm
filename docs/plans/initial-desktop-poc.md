# Initial desktop proof-of-concept plan

Status: portable core tests, repository checks, Linux native tests, and the
Linux Xvfb smoke pass locally. CI enforces the same suite on Apple Silicon
macOS and Linux x86_64 as the revision-bound PR gate; manual visual/input
validation remains pending.

## Outcome

Build a native Huterm development application for the
`aarch64-apple-darwin` target, with `x86_64-unknown-linux-gnu` as an additional
development target. Running the application opens one GPUI window
with one interactive local shell. Huterm owns the shell's PTY and canonical
Alacritty terminal state. GPUI renders a Huterm-defined snapshot and sends
structured input, resize commands, and snapshot requests to the runtime.

This milestone proves the terminal data path and the boundary between the
runtime and a client. It does not attempt to prove the future daemon transport
or multiplexer user experience.

## User-visible scope

The first build provides:

- One native window titled `Huterm`.
- One session, one tab, one pane, and one terminal runtime.
- The user's `$SHELL`, falling back to `/bin/zsh` on macOS or `/bin/sh` on
  Linux, launched in Huterm's working directory with the inherited environment.
- Printable text and the common control and navigation keys needed to use a
  shell and basic full-screen terminal programs.
- Foreground and background colors, basic text decorations, a visible cursor,
  UTF-8 text, and wide terminal cells.
- Scrollback and mouse-wheel scrolling.
- PTY and terminal-grid resize when the window changes size.
- Native platform fullscreen through GPUI.
- A visible exited state when the shell terminates.
- Clean shutdown of the child process when Huterm closes the terminal.

The initial build may use a fixed monospace font, color palette, shell-launch
policy, and scrollback limit in code. It does not need configuration files or
settings UI.

## Settled decisions

- Use the published upstream crates rather than Git dependencies or Zed's
  Alacritty fork.
- Pin `gpui` 0.2.2, `alacritty_terminal` 0.26.0, and `portable-pty` 0.9.0 for the
  first implementation. Upgrade only through an explicit dependency and
  license review.
- Use `portable-pty` for platform PTY mechanics. Huterm still owns spawning,
  I/O scheduling, resize policy, attachment lifetime, and child shutdown.
- Use `alacritty_terminal::Term` and its VTE processor for canonical terminal
  state. Do not use Alacritty's PTY implementation or PTY event loop.
- Keep GPUI and Alacritty types out of Huterm's client messages and shared
  terminal representation.
- Represent the future client boundary with typed in-process messages. Do not
  add serialization, sockets, daemonization, or protocol compatibility logic
  in this milestone.
- Use full immutable viewport snapshots for the first renderer. Add changed-row
  deltas before moving the connection out of process.
- Use GPUI's separately licensed crate and examples. Do not copy or depend on
  GPL-covered Zed application or terminal-view code.
- Treat Hubris as a reference for PTY ownership and detach behavior, not as a
  dependency. Unlike Hubris's browser client, Huterm keeps the canonical
  emulator grid in the runtime and sends semantic snapshots to clients.
- License Huterm-owned source under MIT once the repository receives its
  `LICENSE` file. Preserve notices for Apache-2.0 dependencies.

## Architecture

```text
portable-pty reader thread
          |
          | ordered bytes
          v
  TerminalRuntime task <---- structured client commands
          |
          | owns Alacritty Term and parser
          |
          +----> serialized PTY writer queue
          |
          +----> terminal snapshots and lifecycle events
                            |
                            v
                       GPUI client
```

One terminal-runtime task owns all mutable emulator state. Blocking PTY read,
write, and child-wait operations run outside that task and communicate through
bounded channels. The runtime processes output, input, resize, and shutdown
messages in a defined order rather than sharing `Term` behind a mutex between
the PTY and UI threads.

The GPUI client never reads the Alacritty grid directly. It requests a viewport
and receives an immutable snapshot containing Huterm-owned rows, cells, styles,
terminal modes, cursor state, grid size, and generation number. The request does
not change the canonical terminal or another client's viewport.

### Initial workspace shape

```text
Cargo.toml
src/main.rs                  Huterm executable
crates/
  huterm-protocol/           Client commands, events, IDs, snapshots
  huterm-core/               PTY, emulator, terminal runtime, minimal mux model
  huterm-gpui/               GPUI window, renderer, input, viewport handling
```

`huterm-core` starts with internal `pty`, `terminal`, and `mux` modules. Separate
them into crates only if independent consumers or dependency constraints make
that split useful.

### Minimal domain model

The proof of concept creates all four objects even though each collection has
one item:

```text
Session
└── Tab
    └── Pane
        └── Terminal
```

Use opaque IDs in commands and events. Do not special-case away the IDs because
there is only one terminal. Do not implement split-tree mutation or tab
management yet.

### Runtime invariants

- PTY output reaches the parser in read order without dropped bytes.
- Client input and parser-generated terminal replies share one ordered PTY
  writer queue.
- Resize updates the PTY dimensions and Alacritty grid through the terminal
  runtime rather than from the GPUI thread.
- Snapshot generations increase monotonically. The client never applies an
  older snapshot over a newer one.
- Each client owns its scroll offset and requests the corresponding snapshot;
  scrolling one client never changes another client's view.
- Client disconnection and child exit are separate events. Closing the GPUI
  window requests terminal shutdown; losing a future attachment will not.
- No public Huterm type contains a GPUI, Alacritty, or `portable-pty` type.
- Public core and protocol APIs contain no platform-specific types. Private PTY
  plumbing may use narrowly scoped `cfg` implementations. The first desktop
  build supports macOS and Linux; this milestone does not claim Windows
  support.

## Initial client contract

The exact Rust spelling can follow implementation constraints, but the first
contract needs these operations:

```text
CreateTerminal(command, cwd, environment, grid_size)
SendInput(terminal_id, input)
ResizeTerminal(terminal_id, grid_size, pixel_size)
CloseTerminal(terminal_id)
ReadSnapshot(terminal_id, viewport)
```

`ReadSnapshot` is an in-process query for this milestone, not a mutation of the
terminal. Its viewport identifies the requested rows using a client-owned
bottom offset. It can become a request-response protocol message when local IPC
is introduced.

Input distinguishes composed text, terminal keys, paste, mouse input, and focus
changes. The proof of concept only needs composed text and common terminal keys,
but the enum should leave the other cases explicit rather than treating every
input as an unlabelled byte string.

The runtime emits:

```text
TerminalReady(terminal_id, snapshot)
TerminalInvalidated(terminal_id, generation)
TitleChanged(terminal_id, title)
Bell(terminal_id)
TerminalExited(terminal_id, exit_status)
TerminalFailed(terminal_id, error)
```

After invalidation, the in-process client reads the latest immutable snapshot
for its current viewport. Repeated invalidations may coalesce while GPUI has a
frame pending. The runtime must still parse every PTY byte.

## Terminal and PTY behavior

The shell starts with at least:

```text
TERM=xterm-256color
COLORTERM=truecolor
TERM_PROGRAM=Huterm
```

The implementation should inherit the remaining environment and start in the
application's working directory. Login-shell behavior, shell profiles, command
configuration, and environment overrides belong to a later milestone.

The input encoder must inspect terminal modes held by Alacritty before encoding
cursor keys, keypad keys, focus, mouse, or bracketed paste. For the initial
build, support printable text, Enter, Tab, Backspace, Escape, arrow keys,
Home, End, Page Up, Page Down, Delete, and common Control-modified characters.
Function keys, the Kitty keyboard protocol, IME composition, and mouse reporting
can follow once the basic path works.
Xterm-style modifier encodings for Control- or Shift-modified navigation keys
are also deferred; this milestone only applies mode-sensitive cursor encoding.

On resize, GPUI calculates both cell dimensions and pixel dimensions. The
runtime clamps invalid sizes, resizes the PTY, updates the Alacritty grid, and
publishes a new snapshot.

## GPUI renderer

Render the terminal through a focused GPUI element rather than a tree containing
one element per cell. Per-cell UI entities would add allocation and event costs
to the high-volume path.

For each visible frame:

1. Paint cell backgrounds in batches grouped by color.
2. Shape and paint text runs with fixed terminal-cell positioning.
3. Paint underline, strikeout, and other decorations required by the current
   snapshot.
4. Paint the cursor last.

The renderer must respect Alacritty's cell width and wide-character spacer
flags. Ligatures, complex font fallback policy, inline images, hyperlinks,
selection, and accessibility nodes are outside this milestone.

Use a single coalesced redraw request when terminal output arrives. A bounded
output stress test must not create one queued GPUI task per PTY read.

## Risks to retire early

- Confirm that the published GPUI crate builds a focused window, receives text
  and key input, and toggles native fullscreen on Apple Silicon macOS before
  building the full renderer.
- Keep Linux's XKB link dependencies and Vulkan runtime prerequisites explicit;
  a type check alone does not prove the GPUI binary links or starts.
- Compile the pinned `alacritty_terminal` API behind Huterm's adapter before
  designing around undocumented internals.
- Prove that shutdown can unblock every PTY reader, writer, and child-wait path;
  a window that closes while leaving a task or shell behind fails the milestone.
- Measure full-viewport snapshot conversion and rendering under bounded output.
  Snapshot deltas are deferred only while full snapshots remain comfortably
  within the frame budget.
- Audit the resolved dependency graph, not only each direct crate's declared
  license.

## Implementation sequence

1. Create the Cargo workspace, pin the Rust toolchain and initial dependencies,
   and open a minimal focused GPUI window on the target Mac. Confirm text and
   key events, native fullscreen, and the resolved dependency licenses. Add
   formatting, Clippy, tests, and license checks as Mise tasks.
2. Define Huterm IDs, input types, events, cell styles, cursor state, and full
   terminal snapshots in `huterm-protocol`. Add serialization derives only when
   local IPC work starts.
3. Build the terminal engine wrapper around `alacritty_terminal::Term` and its
   VTE processor. Feed recorded byte sequences into it and convert renderable
   content into Huterm snapshots.
4. Build the PTY runtime around `portable-pty`. Spawn the shell, read output,
   serialize writes, resize the PTY, observe child exit, and guarantee cleanup
   after partial startup failures.
5. Combine the PTY and emulator inside one terminal-runtime owner. Route
   parser-generated replies to the writer queue, coalesce invalidations, and
   expose the in-process client contract.
6. Add the minimal session, tab, pane, and terminal ownership model without tab
   or split commands.
7. Open the GPUI window and implement fixed-grid measurement, rendering,
   terminal focus, input translation, scrolling, window resize, and native
   fullscreen.
8. Exercise real shell and alternate-screen programs on macOS, fix ordering and
   shutdown failures found through runtime testing, and capture the validation
   results in the implementation handoff.

## Verification strategy

### Automated checks

- Unit-test terminal parsing and snapshot conversion with fixtures covering
  text, ANSI colors, cursor movement, erase operations, wide cells, alternate
  screen, and resize.
- Unit-test input encoding against normal and application cursor modes.
- Unit-test the minimal mux model's ownership and close behavior.
- Integration-test PTY spawn, input/output, resize, child exit, explicit close,
  and cleanup after a failed spawn stage with a deterministic helper process.
- Test that repeated runtime invalidations coalesce without losing terminal
  output or publishing snapshots out of generation order.
- Run `cargo fmt --check`, workspace Clippy with warnings denied, workspace
  tests, and the configured license audit.

### Manual macOS validation

Launch the application with `cargo run -p huterm` and verify:

1. The configured shell prompt appears and accepts commands.
2. `printf` output demonstrates standard colors, true color, bold, underline,
   UTF-8 text, and at least one double-width character.
3. Arrow keys, common Control shortcuts, Backspace, Tab, Home, End, and Delete
   behave correctly in the shell.
4. `vim` or another alternate-screen application opens, redraws, receives
   navigation input, and restores the shell screen on exit.
5. Resizing the window changes the values reported by `stty size` and redraws
   the active application without stale cells.
6. Scrollback remains usable after bounded high-volume output.
7. Native fullscreen enters and exits without losing focus or corrupting the
   terminal grid.
8. Exiting the shell reports the terminal's exited state.
9. Closing Huterm with a live shell leaves no child process behind.

## Acceptance criteria

The milestone is complete when:

- A fresh checkout can build and run the application on Apple Silicon macOS
  using documented Mise tasks.
- A Linux checkout with the documented system packages can build the actual
  GPUI client and keep its event loop live through the Xvfb smoke window.
- The manual shell, alternate-screen, resize, scrollback, fullscreen, and
  shutdown checks pass.
- Automated terminal, PTY, runtime, and mux tests pass and are confirmed to run.
- The GPUI client depends only on Huterm protocol and client-facing core APIs,
  not Alacritty or `portable-pty` types.
- The dependency audit reports no GPL or AGPL packages in the distributed
  dependency graph.
- The implementation does not copy or import Zed application or terminal-view
  code.

## Explicitly deferred

- Windows build validation.
- A background server, daemon lifecycle, Unix sockets, Windows named pipes, and
  protocol serialization.
- Detach, reattach, multiple clients, terminal-size arbitration, and remote
  connections.
- Multiple tabs, split-pane commands, tab-bar orientation, and multiple windows.
- The TUI and command-line clients.
- Non-native borderless macOS fullscreen.
- Configuration files, settings UI, themes, runtime font selection, and custom
  keybindings.
- Selection, copy, paste UI, search, hyperlinks, shell integration, IME,
  accessibility, notifications, and inline images.
- Terminal-state persistence across server restart.
- A `libghostty-vt` backend.

## Next milestone

After this proof of concept, add a real split tree and multiple tabs to the core
model, then introduce versioned local IPC and move the runtime into a background
server. Build a minimal TUI client against that connection before adding remote
transport. This order proves that sessions and terminal state do not depend on
GPUI while keeping the first milestone small enough to finish.
