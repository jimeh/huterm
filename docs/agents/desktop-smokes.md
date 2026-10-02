# Desktop smokes and benchmarks

Rules for desktop smoke fixtures, their harnesses, and benchmarks. Discover the
tasks with `mise tasks`; [the development guide](development.md) describes what
each smoke covers and how to run it through Docker or Tart.

## Benchmarks

- `mise run bench:renderer` drives the release renderer under Xvfb and reports
  opt-in CPU preparation and paint-encoding timings.
- `mise run bench:renderer-scenarios` times production `prepare` and `paint`
  against synthetic snapshots, one scenario per process. Pass
  `-- --output <file>` to record a report and `-- --compare <file>` to diff
  against one. It takes one prepare or paint sample per frame, so Linux needs
  `twm`.
- `mise run bench:output-latency` reports the delay from a parsed invalidation
  to an applied snapshot for keystroke-paced output; pass `-- flood` to check
  that sustained output stays paced. `ci:benchmarks` runs the `echo` mode with
  `HUTERM_OUTPUT_LATENCY_APPLIED_BUDGET_US=5000`, the only automated check of
  the activity-driven snapshot path: without it the median waits for the
  16 ms refresh pump. `-- keys` types through GPUI's window dispatch and times
  each key to its painted echo; `ci:benchmarks` fails it above 1.5 snapshots
  per key. Input must not request a snapshot unless it leaves history: one
  started before the echo exists spends the frame's allowance and delays the
  echo by a frame. On macOS its keys reach the terminal only while the window
  stays active; the driver skips keys and prints `window_active=false` when
  another application takes focus.
- `mise run bench:scroll` drives production scroll inputs against 10,000 rows
  and enforces snapshot elapsed-time, wakeup, offset, and bounded-queue budgets.
  Linux runs it under Xvfb; macOS runs it natively and also enforces paint
  elapsed time, reuse, and input latency when the host delivers enough frames.

Headless Xvfb does not deliver continuous GPUI frames after the initial
presentation. Keep Linux scroll gates based on completion-time snapshot and
queue evidence; require frame-bound paint and input-latency evidence only on a
host that actually delivers enough frames.

Benchmark durations measured with `Instant` include scheduler preemption; label
them as elapsed time, not per-thread CPU time.

Initialize scroll benchmark display scale from the attached native window.
Async tab creation may finish after Xvfb's only frame; snapshot benchmark startup
must not depend on TerminalView rendering. Render updates the scale when frames
are available, but Linux snapshot gates must work without them.

Headless benchmarks must use the explicit `scripts/linux/benchmark.twmrc` and
run twm with `LC_ALL=C`. Default manual placement can grab the X server and
block Huterm startup, producing zero samples; missing host fontsets can also
leave twm stuck during cleanup. RandomPlacement and fixed core fonts avoid both.
The scroll benchmark must collect three minimum-size snapshot windows plus
queue-coalescing evidence before stopping Huterm. Parsing the initial 10,000
history rows varies enough across CI hosts that a fixed process lifetime can
leave too few samples. One 25-snapshot window can make median wakeup hinge on a
short callback-delay burst immediately after the release build.

## Harness processes

Run Xvfb with `-noreset` in desktop harnesses. A last-client disconnect otherwise
resets the server and sends another SIGUSR1 to xvfb-run, which can interrupt its
cleanup wait and return exit 5 despite successful file removal. Keep application
exit checks and benchmark budgets strict.

The renderer smoke's initial paint can finish before GPUI starts the Linux event
loop. Gate completion on every expected panel, then cross that startup boundary
before requesting platform quit; a fixed post-launch readiness timer is not paint
evidence.

Use Bun's process timeout and output checks for portable smoke runners. CI's
macOS smoke job has no `timeout`, and its Linux smoke job has no `rg`.

## Synchronizing with the app

Smoke state is sampled from the model, which can run ahead of the drawn frame.
Before a smoke presses on a newly opened overlay, wait for a field set during
render or paint, such as `dialog_drawn_groups` or `menu_rect`, not only for the
model flag.

Use `timeout --foreground` around raw-PTY readers in desktop smoke fixtures.
Without it, GNU timeout puts the reader outside the terminal foreground process
group, so accepted terminal input never reaches the fixture reader.

Publish file-driven smoke commands and polled value files, including XDND
actions and finished status, with a temporary file and atomic rename. A polling
native process can read a newly created command before writeFile has filled
it, consuming an empty or partial command; in the fullscreen smoke this showed
up as a spurious window-index error. Readers must never observe partial
contents.

Integration fixture native-command acknowledgments mean NSEvents were queued,
not dispatched. Assert observed state transitions after each press/release;
check independent Right release and link ownership in the same published state.

The XDND source publishes terminal acknowledgements before closing Xlib and
exiting. Await successful process exit and clear its fixture handle after
drop/leave; otherwise a follow-up action can target a source already exiting.

The AppKit witness publishes readiness before its first state snapshot. Wait for
that snapshot or a command acknowledgement before reading its activation state.
Before summoning from an external AppKit witness, wait for both the exact
frontmost process and the quake window's inactive observation. NSWorkspace can
publish the new frontmost process before NSApplication clears its active state;
sampling only the former can falsely satisfy a new activation request.

## Native input fixtures

The native input smoke command protocol is tab-separated, so a Tab keystroke's
characters are written as the two-character escape `\t`; `native::post`
unescapes it. Send DEL (`\x7f`) as Backspace's characters: GPUI names the key
from that character, and `\x08` produces a keystroke no binding matches while
a following replace-on-type hides the miss. Palette result and picker rows are
exactly `ROW_HEIGHT` (54) points tall and the list caps at `VISIBLE_ROWS`;
the first row centres near 117, and hover and click fixtures use that
geometry.

Native input smoke events must enter NSApplication through `postEvent:atStart:`;
calling NSView.keyDown: directly does not establish `currentEvent` for Option
composition. Mouse moves are the exception: GPUI windows refuse window-level
moved events and take them from a tracking area that only real pointer motion
feeds, so the fixture calls `mouseMoved:` on the view under the point. Fake
`NSDraggingInfo` fixtures must answer every selector GPUI sends, including
`draggingSource`; an unrecognized selector aborts inside the AppKit callback.
Use printable Option prefixes and held printable suffixes in replay regressions:
control-only prefixes can pass even when replay suppression breaks.

Conditional fallback tests must change selection after the prefix starts and
before resolution; GPUI may dispatch a conditional short binding immediately
when that condition was already true at prefix start.

Use XTest for `smoke:linux-input`: xdotool's `--window` path uses XSendEvent and
does not exercise the server's XKB modifier state. The smoke explicitly unbinds
Alt-3 because Linux reserves Alt-1 through Alt-9 for tab selection.

## AppKit fixtures

`smoke:macos-palette` drives the title strip with real HID events from
`target/debug/hid-pointer`: the window server decides title-bar drags and
double-clicks, which NSEvents posted inside the app never reach. The Tart
guest's display is 1024x768, and a login-time "App Background Activity" banner
takes presses in its top-right corner, so the check first places the window
through Accessibility. The window server can drop part of an app-started
synthetic drag, so assert direction and a lower bound, not the exact delta.

AppKit's mouseEventWithType constructor leaves buttonNumber at zero for Right
events. Rebuild fixture Right events from their CGEvent with the button-number
field set to one; GPUI routes by buttonNumber rather than NSEvent type.

Quake close/Quit cancellation fixtures need a live child, not just an idle shell.
macOS correctly closes idle shells without confirmation. Keep the fixture child
alive through the matrix and require the prompt plus a post-cancel shell ACK.

`smoke:macos-updater` assembles a disposable app with a fixture-only Sparkle
key and feed, isolates its user defaults, and exercises the production updater
command without requiring production signing material. Sparkle uses the
presence of `SUEnableAutomaticChecks`, not just its boolean getter, to suppress
the second-launch consent prompt. Always call its setter for an explicit
`updates.automatic_checks` value, including `false`; omitted config calls no
setter.

After emitting every ordered updater marker, the disposable smoke process exits
explicitly. An active Sparkle network check can otherwise keep its AppKit helper
alive beyond the harness deadline; graceful application shutdown is outside
this smoke's contract.

## X11 fixtures

Start Openbox with --sm-disable and wait for its --startup command before
launching integration fixtures.
Openbox publishes _NET_SUPPORTING_WM_CHECK before completing startup.
Poll visible windows without xdotool --sync, with a bounded deadline and app/WM
liveness checks; an empty search is pending, but process or X11 errors must fail.
Keep stderr and bounded PID-window/map/parent diagnostics on discovery failure.
Finish fixture pointer positioning with a PTY ACK before enabling AllMotion;
keep reporting enabled through the entire native drag assertion.

Bound the native XDND fixture by idle time, resetting its deadline after
selection-handshake progress and serviced actions. A valid drag can exceed ten
seconds overall while each individual phase stays within its timeout.
