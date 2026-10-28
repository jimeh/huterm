# GPUI desktop client

Rules for `huterm-gpui` outside input handling and window presentation:
threading, the window model, frame scheduling, close and Quit, rendering, the
keymap and menus, overlays, and configuration. Input lives in
[desktop input](desktop-input.md); fullscreen, Quake, and the title row live in
[window presentation](window-presentation.md).

## Threading and the window model

Desktop structural commands serialize through the Mux mutex on background
workers. Never acquire it from rendering or input callbacks. Terminal clients
send directly to their own runtime, so sibling input and snapshots continue
while another workspace spawns or closes. TerminalView destruction only detaches;
window commands use assessed attachment close to detach or delete the final
view's session.

Global action callbacks run while the dispatching window is borrowed. Defer
Quit routing before updating a native window handle; a synchronous update of
the active window fails with `window not found` even though it remains open.
Reading any window handle, or the root view entity of the dispatching window,
from inside that window's action handler panics inside GPUI with `attempted
to read a window that is already on the stack`, and the panic aborts because
AppKit's selector callback cannot unwind. Cross-window and application reads
of window facts, such as quake profile rows, Quit dialog titles, and Quit
capture, go through the client window model in `Desktop::windows`, which
never touches the window stack. `views::broadcast` is only for updates that
reach every window; never read another window's view entity or
`AnyWindowHandle` for facts. The model owns each window's attachment,
workspace, active tab, and history. `WorkspaceView` keeps its tab entities
aligned with the model through `push_tab_view`, `apply_tab_view_order`, and
`drop_tab_views`.

Each `TabView` holds one `PaneView` per pane of its layout, with the
window's own focused pane and focus history, so two windows showing one tab
focus its panes independently. Reconcile installs views for tabs a window did
not spawn, such as a tab another window opened in a shared workspace, and for
panes another client split into a tab the window shows
(`windows/view_install.rs`). It drops pane views the projected layout no
longer holds, except while the window is busy or terminating, and moves focus
to the most recently focused remaining pane, else the first in traversal
order: tab views keep their panes in layout order. Losing the focused pane
reactivates the tab as losing the active tab does, so keyboard focus follows.
A finished install reconciles again, since reconcile skips a tab whose
install is in flight.

A pane that moves between two tabs a window shows keeps its view:
reconcile adopts the `PaneView`, with its viewer, scroll, and selection,
into the destination `TabView` before removal could drop it, building that
tab's view when the window had none. An emptied source tab view goes, and
the receiving tab takes over as active when the source was. An adopted view
keeps the visibility of the tab it left, so after any adoption the window
reselects its active tab without moving focus, even when the active tab and
pane did not change. Reconcile first sorts every tab's pane views into the
projected traversal order, because a swap or a move within a tab changes
order without changing membership, and a pane that adoption or removal
then takes away must hand focus to the first pane in the new order. A
pane's activity drain finds the pane by ID, so it needs no restart after a
move. A pane close follows its pane: `CloseTarget::Pane` resolves its tab
from the views each time it is requested, and an assessment refused with
`UnknownPane` or `UnknownTab` for a pane this window now shows in another
tab waits for the projection and assesses again there. When the pane left
the window instead, only that close ends, without a notice: an exit-close
returns to owed, and closes queued behind it still run.

Layout history (`windows/layout_history.rs`) lives in the window record, so
it dies with the window and needs no view to prune it. Every rearrangement
completion records through `record_layout_change`, which keeps the commit's
hierarchy sequence and ignores a completion older than the newest entry.
`reconcile` prunes every window's entries against the projection after
every non-empty batch, not only in the windows `windows_to_visit` returns:
an entry can name tabs in workspaces its window does not show, and a
workspace moved to another session touches none of its tabs. A window that
loses an entry is notified, so an open palette or menu stops offering it.
Undo and redo take their entry off the history while the restore runs, so a
batch that arrives in between has nothing to prune. A restored pane that
comes back without a view keeps `restore_focus` until its install
finishes, then takes the focus. The runtime checks the restore again under
its lock, so a lagging projection costs a notice, never a wrong layout.

What a window draws for a tab comes from `shown_layout` and `shown_zoom`. The
layout is the projection's, with the window's divider drag and the resizes it
committed but the projection does not hold yet applied. The zoom is the tab
view's own under `[panes] zoom = "view"` and the tab's runtime zoom under
`"shared"`. `sync_zoom`, run from the view's self-observer, applies a zoom
change once: the zoomed pane takes the tab's focus and the other panes hide.
Batch reconcile notifies every window showing a tab whose layout or shared
zoom changed, since a resize elsewhere changes no structure but changes
every pane's requested grid. It skips a busy window, whose own
spawn holds `busy` until it pushes its view, a window without an attachment,
and teardown. A worker subscribes the viewers under one Mux lock, the
completion waits for the projection to reach the worker's sequence, and the
window then pushes only the tabs and panes it still shows no view of, without
activating them unless it has no active tab. A failed install waits for a
later batch that touches the workspace. A window that gains an attachment runs
its own full reconcile, because attaching emits no hierarchy event. A view
installed after its pane exited shows the exit from status but announces
nothing: only the exit transition raises a notice or records an exit-close.

Resize commands, divider releases, and the layout commands (`arrange_panes`,
the cycle, `balance_panes`) share one ordered layout commit queue per window
(`windows/pane_layouts.rs`), so a balance pressed after a resize lands after
it. A layout command fits its arrangement first, in this view's content
bounds unzoomed, with `pane_geometry::fit`, whose oracle is the same
`pane_grids` and `fits_minimum` a split preflight uses; the runtime knows
weights, not pixels. A fitted arrangement equal to `committed_layout` commits
nothing, unzooms nothing, and records nothing. Otherwise the view clears only
its own zoom, as a move does, and never sends `set_pane_zoom`: the runtime
clears a shared zoom inside the arrangement's own event, so undo keeps it.
Arrangements never set `busy`, which would drop keystrokes and make reconcile
defer membership changes an arrangement does not make. A per-window count of
arrangements in flight instead makes `check_available` refuse, as `busy`
does and refuses divider presses and pane drags, and leaves terminal input
and reconcile alone. Binding paths that skip `check_available` check the
count themselves: a layout, undo, resize, pane focus, or zoom key, or a
title bar zoom, pressed while one is in flight does nothing, quietly, so a
held key raises no notices. A commit the runtime refuses or finds empty
gives the view back the zoom it cleared, unless focus or the zoom moved
meanwhile. The real grids decide `fit`: an ideal arrangement that passes
is kept, and minimum extents are exact rather than whole physical pixels,
because a fractional scale leaves dividers off the pixel grid. Each
completion waits for the projection to reach its commit's sequence before
recording through `record_layout_change`, which ignores a
completion older than the window's newest entry, because a drop from a drag
that started earlier can complete first. Tier 2 tests order other work
against `worker_hooks::Stage::Layout`, before each queue job's runtime call,
and `Stage::LayoutCommitted`, after it.

A view whose viewer reports revocation while its tab stays in the window's
workspace is replaced in place by the same worker and sequence wait. The tab's
activity drain runs `reconcile_own` on seeing it, because a revocation emits
no hierarchy event, and replacements share `PendingInstalls` with installs.
The new viewer starts unfocused with the view's requested geometry and
presentation. Registering it focused would make it the newest focus gain under
the `latest` policy and take control even if its window blurred meanwhile.
Swapping it in discards queued input and mouse ownership, re-sends geometry
and presentation that changed since the replacement started, sends the focus
the view last reported (`reported_focus`) through the input queue, since GPUI
raises no focus event for a view that stays focused, restarts
`ScrollController`, ignores replies to the old viewer by `viewer_epoch` (a
late `Revoked` would otherwise latch the scroll failure), and replaces the
tab's activity task, which waits on the old viewer's wake. A view that a
poll showed running (`seen_running`) treats an exit it first sees from status
as the exit transition, so a shell that exits while the replacement is in
flight still closes or announces. A revoked viewer's `poll` still reports the
exit until its slot is finalized. A failed replacement removes the view.

## Activity tasks and frame scheduling

Each TerminalView holds one `TerminalViewer`, its own registration with the
terminal (see [the terminal viewers plan](../plans/terminal-viewers.md); the
runtime's rules are in [the core guide](core-runtime.md#terminal-viewers)).
Each tab owns one cancellable activity task that waits on that viewer's wake
and polls it through its WorkspaceView; the presentation pump must not also
poll. Budget exhaustion schedules a continuation without waiting for another
producer wake. The desktop shows no notice for `Revoked`. Focus changes travel
in the view's InputQueue as ordered entries, exempt from its limits and from
exit closure; geometry stays latest-wins in `pending_resize`. Only visible terminal
views request snapshots. All request paths pass the shared admission gate; one
weak callback per window replenishes per-view frame allowances. It does not
request idle redraws. Keep in-flight scroll and dirtiness separate from frame
allowance, including on config reload and snapshot completion. GPUI throttles
next-frame callbacks in unfocused windows to `inactive_frame_interval`, 30 fps
by default; desktop windows set it to `None`. With the throttle, Linux
`bench:scroll` p95 input-to-paint latency rose from about 6.5 ms to 35 ms.

Animations share that window callback and one earliest-deadline timer. Report
frame work and deadline work separately from presentation changes: a static hold
can require a future wake without a redraw. Entity notifications arm animations;
render-time layout changes must also rearm them, including resizes within one
grid cell. Release removes their weak registrations. Keep runtime retries and
lifecycle progress independent of display frames. Each terminal owns a bounded
pending-work wake and cancellable task; signal queued input even when admission
returns no presentation change. Retry timers run only while work remains.

Workspace notifications reconcile close and palette state. Config and bounds
changes mark terminal geometry pending and synchronize it without waiting for a
frame, including hidden-tab font and padding changes. Pointer-only updates must
not scan every tab for geometry. Reuse an armed earlier deadline when a hold
extends; do not allocate a timer per interaction. Interaction
endings must renew holds explicitly because settled drags and hovers have no ticks.
Do not suppress invalidation by comparing content generations: presentation
updates invalidate without advancing the content generation. Keep ChromeLayout
as the shared source of terminal bounds for painting, mouse input, scrollbars,
and PTY resizing. `WorkspaceView::terminal_geometry` derives each view's
`TerminalGeometry`, its bounds and scrollbar track margins, from it, and every
site that changes chrome pushes that value into the views; `TerminalView`
never computes chrome itself, and a view no workspace hosts fills its window.
Pushing does not resize: views resize where they did before. A shown view
resizes on render. Hidden views resize through the layout sync, when the
layout is pending, the scale or cell size changed, the geometry differs, or
the tab bar appeared or hid; through the fullscreen sync; and otherwise on
activation. Inside those bounds, `TerminalLayout`
computes the requested grid, which the view reports and the PTY follows while
the view controls; `requested_grid` is never what the view draws.
`GridGeometry` maps between window positions and the displayed grid, the
snapshot's `size`.

## Close and Quit

GPUI close callbacks must return false while asynchronous checks and cleanup run.
Carry close intent across pending operations, and defer application quit until
all pending spawns have published or failed. Foreground checks belong to the PTY
owner, and confirmation must move focus away from terminal action handlers.

Keep a synchronous `on_app_quit` cleanup callback for native termination. AppKit
exits without returning from `Application::run`, and GPUI grants quit futures
only 100 ms. This terminal-only hook is the exception to asynchronous desktop
cleanup: set the shared termination gate before locking Mux, reject queued spawns
after they acquire that lock, and drain existing runtimes before returning.

Quit captures every session plus window navigation and geometry before cleanup;
retain the first capture through repeated shutdown. The native AppKit bridge
adds only applicationShouldTerminate: to GPUI's existing delegate and vetoes
until assessment, consent, and cleanup finish. Keep unsafe Objective-C calls in
native_quit.rs and native_updater.rs; run mise run smoke:macos-quit on macOS for
real terminate, cancel, retry, and allow coverage. An on_app_quit callback alone
cannot cancel Dock Quit.

Allow no-PTY Quit confirmation hosts while quitting, including when a queued
Application request outlives the final Window close. Only new shell windows
remain blocked. Use commit_close_with for capture and termination-gate changes:
validate once before side effects, then capture before teardown. A second
freshness check after capture can strand a half-accepted Quit.

`Desktop::exit_closes` owns exit-closes, one record per pane, and the window
close pipeline is the only actor that assesses and commits them. The first
view to report a pane's exit transition records it, owed under that window's
`close_on_exit` and settled otherwise, so a reload changes only future exits.
An idle window that shows an owed pane takes it in `next_request` as a
`CloseTarget::Pane`, after queued user closes, and starts its assessment at
once; any window showing the pane may take it, and a window holds one at a
time. Core closes a tab with its last pane. Keep automatic exit
requests separate from the merged manual close intent, so canceling one prompt
does not lose inactive siblings. A close that never reached Mux for its pane
(cancelled, merged into a wider target that left the tab, or found gone on a
stale retry) returns to owed and is offered to the other windows through
`cx.defer` and window handles, never by reading their entities from inside an
update. Once its commit starts, the commit's completion records the result by
pane in application context, so closing the window cannot let a second window
close the same pane. Removing a window returns a close it took but had not
started committing. A failed assessment or commit settles with one notice and
is not retried. Tier 2 tests hold and fail both stages through
`windows/worker_hooks.rs`, which compiles only for tests with `test-support`;
`Stage::Assessed` holds a returned assessment before the window sees it, and
`Desktop::close_failures` counts close failure notices, so a "no notice" claim
stays observable after the window is gone.
Exited tabs retain history and local selection/scrolling;
close their input queue and reset mouse ownership without emitting releases.

A window whose last tab view goes away takes an emptied close
(`CloseTarget::Emptied`, core `CloseRequest::EmptiedWindow`) on three edges:
reconcile removed its last view, its own tab close or exit-close removed its
last tab (`remove_closed_tabs`), or a batch that touched its workspace found
its attached session gone. Reconcile only marks the window
(`EmptiedClose::Owed`) and never removes it, so no assessment or commit
continuation runs after its window is gone. Core decides under its lock and
refuses with `ViewNotEmpty` while the workspace holds tabs, so a tab that is
still installing, or that another window just opened, is never terminated; a
check against the projection cannot tell an arriving tab from one moved to a
sibling workspace. A refused window stays marked (`Refused`) and asks again
only from a batch that touches its workspace or an install completion that
installed nothing, and only while it shows no view and the projection holds no
tab it would install; a pushed view clears the mark. The emptied close's
assessment carries the hierarchy sequence read under the Mux lock, and a
`ViewNotEmpty` waits for the projection to reach it before deciding: batches
that arrived while the window was busy only kept the mark owed, so an
unconditional `Refused` would strand a window whose last installing tab or
session went away after core refused. After the wait, a session gone removes
the window, a window with nothing to install asks again, and otherwise it is
refused. `UnknownAttachment` removes the window without a notice, for a
`CloseTarget::Window` close as well as an emptied one: the session closed
elsewhere and nothing is left to close. Any other emptied assessment failure
raises one notice and clears the mark without cancelling queued closes, which
run next.

A window removed with a queued Quit (`quit_after`) continues through the
internal `request_quit`, deferred outside the window update. Dispatching the
`quit` command instead is refused by `app_command_availability` while
`quitting` is set, which abandons the Quit with `quitting` stuck.
`quit_host` picks the host, the active window and then the first open one,
skipping a removed window the platform still names as active and every
window marked `closing`: on macOS a closing window's removal is already
scheduled and would drop the Quit with it. A refused emptied close whose
sequence wait is cancelled by teardown decides nothing.

Automatic closes, exit-closes and emptied closes, start only from
`next_request`: after queued user closes, while the pipeline is idle (not busy
and no confirmation showing), and never in a closing window or during
teardown. A Window or Application close absorbs a pending emptied close. A
window that shows, assesses, or commits a Quit therefore stays open when its
session closes underneath it: the Quit's commit goes stale and assesses again,
and a cancelled Quit clears `quitting` before the window takes its emptied
close. A tab confirmation whose tabs all left the window is dismissed, since
its commit could only go stale. `CloseState::dismiss_confirmation` clears only
the confirmation, its assessment, and `current`, and moves the generation;
queued closes, such as a second tab's close, stay queued. User Cancel and
Escape still cancel everything through `cancel_close`.

The close dialog's Confirm listener captures the target and
`CloseState::generation` at render, and `confirm_confirmation` acts only while
`can_confirm` still sees that confirmation at that generation. Every cancel
and every new check moves the generation. GPUI dispatches a click against the
last drawn frame, so without the guard a click drawn before Cancel finds no
assessment, and `finish_close` assesses again and commits without a prompt once
the jobs end. Cancel and Escape check `can_cancel_confirmation`, and the
keyboard confirm reads the current confirmation. A tier 2 test produces the
stale click by cancelling and calling `Window::dispatch_event` in one update,
before the end-of-update draw drops the dialog.

The Settings window (`windows/settings_window.rs`) has no session and no
`WindowModel` record; `Desktop::settings` holds its handle. Code that walks
`cx.windows()` must downcast to `WorkspaceView` and skip what is not one, and
Quit hosts only a window with a model record, or its confirmation would land
in Settings and Quit would do nothing. Committing Quit closes Settings.

## Rendering

GPUI's `ShapedLine` contains a large inline decoration buffer. Retained terminal
rendering should cache `Arc<LineLayout>` from `layout_line`, not `ShapedLine`,
and apply colors and decorations during paint. Use the generic `monospace` font
family on Linux; requesting macOS-only Menlo repeatedly exercises GPUI's
missing-font fallback path.

Derive the terminal's grid geometry once through `GridMetrics`. GPUI
reports raw font descent as negative on macOS and Linux, so normalize it to a
positive distance before calculating cell height, baseline, or decorations.
Round grid dimensions in physical pixels using the window scale, then convert
back to logical points. Keep unrounded font measurements for scale changes;
rounding up to logical points makes Menlo 12's grid 14% too wide on Retina.

Built-in terminal graphics live in `renderer/builtin`, grouped by Unicode
range and pinned to the Ghostty reference recorded in
`third-party/terminal-graphics/README.md`. Match standalone scalars only; leave
combining sequences on the font path. Use physical-pixel cell geometry and
invalidate the geometry cache when metrics change. Round dimensions again after
converting logical points back to physical pixels: floating-point residue before
`floor` can move stroke centers by one pixel at fractional scales.
GPUI `paint_layer` controls
scene ordering, not clipping; use `with_content_mask` for diagonal overshoot.
Every primitive painted outside a layer costs a bounds-tree insertion, so paint
per-cell primitives inside grid-sized layers. Inside one layer GPUI draws all
quads, then paths, underlines, and sprites, so a quad that must cover glyphs,
such as the block cursor, needs a later layer or an unlayered paint after it.
Tessellate built-in paths once per geometry, never per paint.

Run `mise run smoke:renderer` for production preparation/paint coverage and
inspect the held fixture when changing geometry. Keep its license notice in
the packaged resources independently of the Ghostty VT engine notice.

## Keymap, commands, and menus

GPUI normalizes shifted punctuation to its resulting symbol and clears Shift.
Bind reload as `cmd-<` / `ctrl-<`, not `cmd-shift-,` / `ctrl-shift-,`; test
the real `KeyBinding` matcher as well as terminal-input reservation.
Validate `when` with `KeyBindingContextPredicate::parse` before building a
`KeyBinding`; `KeyBinding::new` unwraps its predicate parse. Reserved
keystrokes derive from the compiled keymap and compare modifiers plus `key`,
never `key_char`, so `alt-r` stays reserved when macOS reports `®`. Multi-key
bindings reserve only their first stroke; GPUI owns pending sequences and matched
final strokes. Later strokes typed alone must reach terminal input. A conditional
single-key binding reserves nothing, or `ctrl-c` with `when = "selection"` would
swallow interrupts when the predicate is false. Per-scope
wrapper actions (`InvokeApp`, `InvokeWindow`, `InvokeTerminal`) route catalog
commands; anything that needs a window, such as about and open_settings, must
be Window scope because global action handlers receive only `App`. Runtime
rename commands fill omitted tab/workspace targets on the UI thread and resolve
the session under the Mux lock on the worker. Rebuild menus on reload so macOS
shortcut display follows user bindings. GPUI menus show the earliest binding
for an action and equal-depth dispatch ties go to the latest, so the compiled
keymap lists unconditional user bindings first and conditional ones last.
GPUI's `Not` predicate checks every ancestor context, so
`Terminal && !confirming` works across the Workspace and Terminal stack.
Pane layout contexts (`split`, `pane_zoomed`, `has_pane_*`) live in
TerminalView's own key context, because an identifier matches only the
innermost context. `sync_tab_layout` pushes them into every pane view as
`PanePlacement`, alongside geometry. `has_pane_*` must call
`TabView::neighbor`, the lookup `focus_pane_*` runs: a context that disagrees
with its command swallows the key without effect. Tier 2 tests read the
rendered contexts through `Window::context_stack`.

Text arguments accept a blank value: the core executor maps a blank rename name
to `None`, which clears the custom name, so there are no reset-name commands.
The palette prefills a rename's current custom name, selected, whenever the name
slot is untouched. Palette, text-field, and other UI keyboard operations are
`Palette`-scope catalog commands with a required context compiled into every
binding, never hardcoded GPUI component bindings. `validate_supplied` guards
bindings, menus, and interactive requests; only executors run the full
`validate`, so a binding may omit prompted arguments and the palette collects
them. Keep the keybinding schema aligned: only unprompted `Always` arguments
are schema-required.

`app_command_availability` lists every Application command; a new one it
omits is refused as unknown from every shortcut and menu, while a test that
calls the handler directly still passes. Settings binds Space and the arrows
under `SettingsControl`, which only its controls and section rows carry: a
binding under `Settings` matched inside a focused text field too, because the
field's `Palette` context binds no Space, and ate the space. The roving
groups (the sidebar's sections, the theme cards, the modifier chips, and
the layout cycle's switches) give each item a `SettingsGroup` context nested
inside a `SettingsControl` wrapper around the group, or around each cycle
row, and the catalog puts `settings_next` and
`settings_previous` under `SettingsGroup`, where the arrows bind to them.
GPUI ranks a binding by the depth its predicate matches, then by later
index, so the group bindings win over any `SettingsControl` binding on the
same key whatever their order: a user arrow bound to a stepping command
leaves the groups alone, and the reverse. Both contexts on one node would
tie on depth, and the later user binding would win. The wrapper keeps the
`SettingsControl` commands (toggle, reset) reaching a group's items; wrap
only the group, since a text field under a `SettingsControl` ancestor would
lose Space to `settings_toggle`. `SettingsView::run` routes the moves to
`move_group`, which tries each group in turn, and routes `settings_increase`
and `settings_decrease` to the shortcut filter (`step_filter`) before
`step`, which finds only settings and would drop it; `step_setting` gives a
switch no steps. Page Up, Page Down,
Home, and End bind under `SettingsControl && control == slider`, which only
a slider's `SettingsControl control=slider` context matches, so the keys stay
unbound, not swallowed, on every other control. A number field's context is
`Palette` alone, so no `SettingsControl` binding reaches it; a
`SettingsControl` wrapper around it would still lose to `Palette`'s deeper
`pageup` and `pagedown`. `run_palette` pages a focused number field from
`palette_page_up` and `palette_page_down` instead, as it steps one from
`palette_select_previous` and `palette_select_next`.

A command whose arguments are all unprompted, such as `resize_pane_left` or
`close_tab`, has no palette slot. `SlotEditor::new` returns `None` for it, and
the palette runs it through `unprompted_invocation`; an editor always holds a
slot. Indexing the first slot of an empty editor once aborted the app from the
palette. The palette, menus, tab strip, pane geometry, pane drags and moves,
and the Licenses view deny `clippy::indexing_slicing` outside tests, because
user input picks positions there: look positions up with `get`, and keep an
`#[expect]` with its reason only where a constructor proves the bound. The
slot unit tests build every catalog command, so a new command of either shape
is covered.

GPUI's native menu matcher uses a fixed Workspace/Pane/Editor context. Menu
ordering fixtures need a predicate true there, such as `!confirming`; `Terminal`
never participates and cannot detect conditional bindings moving ahead of defaults.

GPUI still passes literal `enter` and `tab` strings to NSMenuItem instead of
AppKit's Return and Tab characters. Normalize those two key equivalents after
`set_menus` at startup and reload; leave the configured GPUI keys unchanged.
AppKit derives both shortcut display and activation from `keyEquivalent`.
Keep the native menu smoke assertions for these characters when upgrading GPUI;
remove the workaround once upstream converts them correctly.

## Overlays, focus, and notices

Give confirmation dialogs an explicit viewport-clamped width before measuring
wrapped text. GPUI's w_full/max_w combination can measure a shorter height and
let buttons escape the panel; keep text and button containers nonshrinking.

Focus listeners (`on_focus`, `on_blur`) run inside a draw, where a
`cx.notify()` or `window.refresh()` is lost until the next input event. Work
whose result must show, such as validating a field on blur, goes through
`cx.defer_in`. Keys match only along the focus path of the drawn tree, so a
focus left on a control that went away leaves every binding dead. The
Settings window records each control it draws (`action_stop` marks it, and
a pane layout's template field marks its path) and moves focus to the window
when the focused control was not drawn, or when focus stays across a frame
outside the view's tree. A pane layout or cycle row that went away hands
focus to the same control of a neighbouring row instead (`layout_neighbour`,
from the rows the last two frames drew): a user layout's to the next row,
else New layout; a cycle row's to the next row, else the previous one. No
recovery runs while a dialog is open: a dialog opening over a control that
just went away takes focus on its first frame, after the recovery was
scheduled, and the recovery would take it back out of the dialog.

GPUI dispatches input against the last drawn frame. A press that lands before
the frame showing a dialog's scrim still reaches the covered terminal, whose
mouse-down listener must `prevent_default` to stop `track_focus` from focusing
it. The terminal's `on_focus` hand-back to the dialog cannot cover that case:
GPUI raises focus events by comparing drawn frames. When the dialog takes focus
and a stale press returns it to the terminal before the next draw, no event
fires and the terminal keeps focus. Keys that arrive before the terminal is in
a drawn frame are dropped: after focusing a window, input smokes repeat a probe
key until the fixture reads one, then send a release byte, instead of typing
immediately or sleeping.

Advance tab-overlay animation through the shared window frame clock;
call GPUI's `request_animation_frame` only while rendering. Despite its wording,
the GPUI method requires a current view and panics from a timer callback.
Terminal gestures retain move/release ownership beneath an overlay until the
deferred pointer reconciliation hides it; overlay hit bounds alone must never
discard that release.
Non-modal overlays over the terminal, such as toasts and the scroll pill, stop
presses but never releases: a drag that started in the terminal must still end
there, and the terminal ignores releases it does not own.

Overlays share the palette's grammar: every keyboard action is a catalog
command bound under a key context. The close dialog and the About panel use
`confirming`, menus `menu`, and focused toasts `notices`; each binding is
conditional, so it reserves no terminal key. The About panel's Licenses view
wraps its dialog content, the search field, list, and buttons, in the
`Palette` context, so `Input` editing and the list's keys work from any of
them without new commands; `WorkspaceView` sends Palette-scope commands to
it before the palette while it is open. That wrapper is deeper than the
dialog panel's `confirming`, so its Escape, Enter, and Tab bindings win.
A repeated close shortcut while `confirming` must not confirm. Menus and
dialogs return focus to the terminal when they close; the menu button never
keeps it, or terminal bindings stop matching until the user clicks back in.
Focus a toast gives up while a dialog shows goes back to the control the
dialog opened on (`restore_tab_focus`), since the terminal's hand-back
misses a change made between two frames.

GPUI's `on_hover` also fires when a layout change puts an element under a
resting pointer, so a menu that opens beneath the pointer would select the
row there. Menu rows select on `on_mouse_move`, which needs real motion, and
a select's menu opens on the value its control shows (`Menu::select_item`).
A GPUI test needs a second frame (`window.refresh()`) before hover settles.

Each open menu carries a `MenuKind` and pointer anchor; derive button state
and pick targets from them, not from whether any menu is open. A terminal
right-click's link rides on the next snapshot request and opens the menu when
it answers or after a bounded wait. `MouseState::down` records a local press as
held, so sample `owns_pointer_gesture` before it; a right press during another
gesture must not open a menu. Emulator-side edits such as Clear Scrollback's
`CSI 3 J` reach the emulator only while the native stream is at ground.
`EscapeHint` ignores UTF-8 state and must not gate injected bytes. Ghostty's
full reset, like its RIS, resets the palette but keeps dynamic OSC color
overrides.

Clickable controls pair their hover style with an `.active` pressed style:
`Swatch::pressed`, `accent_pressed`, or `selection_pressed` in overlays.
Title-row and tab-bar buttons are `Button`s in `TabColors::control()`, which
carries the bar's control hover and pressed overlays. Pane title-bar buttons
are not `Button`s yet and still use `TabColors::control_hover` and
`control_pressed` directly. `Swatch` is passed
by value and must stay under Clippy's 256-byte limit, so derive further shades
through methods rather than fields.

Every drawn shortcut goes through `key_hint::KeyHint`: macOS modifier
symbols, Linux modifier words, and Lucide icons for Return, Tab, Backspace,
and the arrows on both. Add icons through `scripts/ui-icons.ts`, never as
hand-drawn glyph boxes. `window.shortcut_hints` gates key caps on the scroll
pill, dialog buttons, and the About panel; the palette and menus always show
theirs. The tab position default differs by platform, so the schema describes
it instead of stating a default, and the config template leaves it commented.
Pin `position` in smoke fixtures whose geometry assumes one.

GPUI's wgpu shader draws zero-blur shadows, such as the focus ring, as sharp
shapes on Linux, as Metal does (zed#57685). Xvfb captures of GPUI windows are
black, so check Linux visuals in `vm:linux:dev`.

Palette ranking ties fall back to catalog order. A new command whose title
shares a prefix with an existing one, such as Copy Tab Directory beside Copy,
must come after it in the catalog, or the palette smoke's `copy` query selects
the new command.

Notices replace both status strings. Configuration and keymap diagnostics are
persistent notices keyed by source, and every reload replaces that source, so
a still-failing file raises them again after dismissal. Command and terminal
failures expire on the window's single earliest-deadline timer; persistent
notices schedule nothing. Terminal failures reach the window through the tab's
activity drain, including hidden tabs. Root-shell exit is not a failure: only a
tab kept by `close_on_exit = false` announces it. Smoke state serializes the
stack as `w0.notices=<n>` and newest-first `w0.notice<i>=<severity>|<source>|<message>`.

A reported command failure with no live reporter, such as one from a global
hotkey, goes to the active window. `Desktop.latched` holds it only while no
window can show it, and the next window to open or become active takes it
rather than copying it. A failure left latched after a window showed it is
shown a second time, in the next window to open or become active.
`WorkspaceView::can_show_notices` decides which windows can show one: a
closing window and a hidden Quake window cannot, although the platform can
still name either as the active window. A new window takes the latched
failures as it opens, except a Quake window whose setup failed: it is removed
while still hidden. A Quake window whose shell fails to start after it opened
has already taken them, and they close with it. An existing window takes the
latched failures only while it is active and passes that check. Every window
does so when it becomes active. A Quake window also does so whenever its
visibility is synchronized, because a show that reverses a hide, or a
recovery from a failed transition, leaves it active with no activation to
follow. A successful configuration reload clears the failures still latched,
so a stale hotkey conflict does not outlive the configuration that caused it;
an unrelated failure waiting there is dropped with it.

## UI components

Generic components live in `crates/huterm-ui`, one file per component under
`src/components/`, each opening with its contract; primitives (tokens, focus,
accessibility, scrollbars) sit at the crate root. The plan is
[UI components](../plans/ui-components.md). The crate may depend only on GPUI
and `unicode-segmentation`, so it takes colors as `huterm_ui::tokens::Tokens`
and exposes methods rather than catalog commands. `desktop/overlay.rs`
re-exports `Tokens` as `Swatch` and builds it with `tokens(theme)` and
`overlay_colors(theme)`; the orphan rule rules out `from_theme` constructors on
the crate's types.

`Tokens` corrects arbitrary themes toward WCAG's minimums, and the `contrast`
test in `desktop/overlay.rs` checks a table of the (text, fill) pairs Huterm
draws in every bundled theme; add a pair there when a new one appears.
`scripts/ui-components.test.ts` rejects raw `accent`, `warning`, `danger`,
`bg`, or `dim` used directly as a text color; a color chosen in a branch
still needs a reviewer. Draw
text on a solid fill with `on(fill)` or `on_accent()`, never `bg`, and shade
its hover and pressed fills with `shade`; draw secondary text on a selected
row with `muted_on(selection)`, colored text with `accent_text()` and its
siblings, and focus rings through `focus_ring()`. Draw key caps and tags on a
row or button, which hovers and presses, outlined with `cap_on` rather than
filled, so no chip fill stacks on those fills. `dim` is for disabled text
only; placeholders use `muted`.

`pub` items of a library are always live, so `#[expect(dead_code)]` cannot
mark a component that lands before its caller: the expectation is never met
and Clippy fails. A component's first consumer is its entry in
`desktop/gallery.rs` (`cargo run -p huterm-gpui --example
component_gallery`), and `scripts/ui-components.test.ts` fails when a
component file has none.

Draw a focus ring through `huterm_ui::focus::ring`, which applies the shadow
to the element, never GPUI's `.focus()`, `.in_focus()`, or `focus_visible`
styles, and mount `focus::tracker()` once in the window's root view.
`focus_visible` rechecks GPUI's input modality every frame, and any pointer
move switches it to mouse, so its ring vanished when the pointer was nudged
after Tab. The tracker remembers whether the last press was a key or a pointer
button (pointer motion is not a press) and decides a focus change by it; a
pointer press hides the ring, because pressing the already-focused control
does not change focus. Key listeners on an element see only keys dispatched
along the focus path, so the tracker notes keys with an app-wide keystroke
interceptor. It records during paint, after lists that render rows while
prepainting. The same script test rejects those styles and `focus_ring`
references in `huterm-ui` outside `focus.rs`, and in the terminal-window files
whose buttons migration 3b moved; other `huterm-gpui` files keep their own
until the migrations replace them. In the Settings window it counts the GPUI
focus styles left for later migrations, so lower the count when one moves.

Settings builds its buttons, links, switches, segmented controls, tri-states,
modifier chips, tags, and counts from `huterm-ui`: `action_button` makes a
`Button` that runs an action key, and `SettingsView::listen` adapts a
component callback to the view, as `cx.listener` does for element events. A
read-only control passes its reason (`read_only_reason`, `lock_reason`,
`keys_lock_reason`) to the component's `disabled`, which shows it as a
tooltip and accessible description and refuses a press's focus; the command
paths (`step`, `toggle`) refuse a read-only control themselves, because a
control can turn read-only while it has focus. Tests click these components
through `simulate_a11y_action` from the window, not inside a view update, since
the click updates the view.

The terminal window's dialog, title-row, tab-bar, and toast buttons are
`huterm_ui::components::button::Button`s; tabs, pane title-bar buttons, and
the scroll pill are not. An owner sizes, places, and restyles a `Button`
through `Styled`, which applies over its variant, adds overlays such as the
menu button's notice dot as children, and gives a button on another surface's
palette its own `Colors`. Its click action takes no event, so
`WorkspaceView::button_action` adapts a view method. Chrome buttons take no
focus handle, so they never hold focus, and `stop_presses` keeps their
presses from the title row's move gesture and the tab strip. The close
dialog, About, and Licenses buttons take focus inside their dialog's
`Modal` cycle, so their ring follows `focus::ring` and the terminal window
mounts `focus::tracker()` too; no terminal-window button uses
`Button::current` any more. A click on a button focuses it, as a browser
does. Licenses moves between its buttons with Tab and Shift-Tab only,
because the arrows stay with its list and search. A dialog opened from the
native app menu right after a key press can show its ring at once, because
AppKit menu clicks reach GPUI as no pointer event. A
button with a tooltip shows it below itself while its ring shows and it is
not clipped (`tooltip::focused`), an expanded button shows none, and an
icon-only button's label is its tooltip.

Text fields are `huterm_ui::components::input::Input` entities, and
`huterm_ui::text` holds the UTF-16 range conversions that `Input` and
`desktop/composition.rs` share. `ui/input.rs` is the adapter that maps the
catalog's `text_*` commands onto `Input` methods; route a field's commands
through `crate::ui::input::run`. `Input` refuses typing, composition, paste,
cut, and deletion while read-only or disabled, but the owner's `set_text` and
`replace` always apply. The palette and Licenses draw their own frames
around `Chrome::Bare` fields until their migrations; Settings fields are
`Chrome::Framed`, and a field inside an `InputGroup`, such as Settings'
searches, uses `Chrome::Grouped`. `NumberField` and `Slider` step along
`huterm_ui::steps::Steps`, whose grid clamps every user change to the range.
Settings builds a number setting's `Steps` with `values::steps` from the
schema's bounds, an exclusive minimum as the first grid value above it, an
unsigned integer `format`'s limit when there is no `maximum`, and
`x-huterm-step`; an open end gets a far stand-in bound, so an empty field
steps as `values::empty_step` says, from the default or to the minimum, never
to that stand-in. `x-huterm-control = "slider"` picks a `Slider` instead of a
field; without `x-huterm-unit` its readout is a percent, so the model refuses
a unitless slider whose range leaves 0 to 1. Row controls take a fixed width:
a full-width control squeezes the row's description to a character per line.
A locked Settings field is `Input`'s disabled state with the lock
reason, not read-only, so a press takes no focus. Settings commits number
fields itself (`with_commit_on_blur(false)`), so a conflicted draft survives
focus moving to Use mine or Use file, and routes every `NumberField` change
through `commit_field`; keep text that Settings shows in a number field going
through `show_text`, which keeps the field's value in step.
GPUI sends focus and blur events only while a window is active, so a test of
blur behavior, such as `NumberField`'s commit, activates its window first.

`Combobox` and `Command` drive a list from a field that the owner keeps and
draws with any chrome, and focus never leaves that field. Mount a `Combobox`
entity as the last child of a relatively positioned wrapper around its
field: it measures the wrapper and draws its suggestions in its own
non-modal `Popover`, mounting that popover's guard itself, since a non-modal
guard never stops a press. A non-modal guard also closes on a wheel outside
the content and lets the wheel scroll what it is over, and an open
popover's trigger probe asks for another frame when the trigger moves, so a
list placed from the field's last paint follows it. GPUI's test mode
delivers no frames, so a test of that follow, or of a list revealing its
highlight after a parent resized it, pumps `Window::simulate_next_frame`,
after first draining the frames already pending, or another request
redraws in its place. The filter runs once per `Edited`, which `Input`
emits before `Changed` for the user's edits alone, so an owner's `set_text`,
such as Escape putting back a value, opens nothing. The filter reads its own
data, never the owner's entity: it also runs inside `open`, `toggle`,
`select_next` and `select_previous` on a closed list, and `refresh`, which
an owner usually calls from its own action handler mid-update. `Command`
takes the items its owner filters and ranks through `set_options`,
typically from `Changed`, and draws them inline; turn off its pointer
highlight where the highlight shows details, as Licenses does. A picker
whose Enter must never reach another row, as the palette's identity
pickers, takes `HighlightFallback::Clear`, and an owner that restarts at
the first row on each query calls `select_first` after `set_options`. Route
list commands to their methods: a combobox's return `false` when the key is
the owner's, such as a second Escape or Enter with nothing highlighted. Both
set their field's `ListState`, so it reports an `EditableComboBox` whose
description is the highlight, because the list draws outside the focused
field, where GPUI ignores an active descendant. A press on a row prevents
the default focus change, and a combobox's list also occludes and stops it,
so the field never blurs before a pick. Test such a press under a focusable
root, as a window's root view is, or no blur could happen without them.
GPUI blurs the focused field when its window deactivates, which closes a
combobox's list. GPUI also reports a notification made during a draw to no
observer, so a combobox cannot observe a field its owner disables while
rendering, as Settings does; it checks its field when it draws instead.

Settings has two comboboxes. A keybinding's `when` field suggests context
names with `with_auto_highlight(true)`, so Return completes the name before
the caret; `Keys::completions` holds each list by its field's entity, which
keeps it as the field moves between rows. The font family field suggests
the installed families with no highlight, so Enter saves the typed name, and
a pick saves the family through `commit_field`, as Enter does. The GPUI test
platform lists no installed fonts, so font tests set `set_families` before
the Appearance section first draws. `run_list` routes the `Palette` field
commands to the focused field's list first; the field's own behavior gets
the keys the list leaves alone.

Open menus and selects in a `huterm_ui::components::popover::Popover` and
mount its `guard` in the owner's root view on every frame, open or not. The
guard's capture-phase press listener reads the live open state, so it also
stops a press that arrives before the popover's first frame, when no backdrop
is drawn yet. GPUI runs capture listeners in paint order, and the guard
stops the presses it consumes, so the terminal window mounts it after its own
window-level listeners, which every press must reach. The window, tab, and
terminal menus share that window's one popover and return focus to the
active terminal. GPUI also blurs a focused menu when its window deactivates.
The activation observer runs first and closes the menu onto the terminal;
the deferred blur close also skips an inactive window, so it can never strand
focus on a dropped menu. Opening a menu drops a right-click still waiting for
its link lookup, because the press that dismisses the menu no longer reaches
the terminal to cancel it. Component
click listeners ignore `ClickEvent::Keyboard` through
`huterm_ui::click::from_keyboard`, so a removed Enter or Space binding really
removes the key.
GPUI sends that implicit click only to the focused element: a test of the
filter must focus the element that holds the listener, or it passes without it.
A composite such as a single-choice `ToggleGroup` keeps focus on its container,
so its items never receive the implicit click. GPUI clicks only on the key-up
that follows the same key's key-down with focus unchanged, so a test of the
filter sends both, for a key nothing binds there.

`Tabs` and `RadioGroup` are roving groups instead: each item takes its own
focus handle and draws its own ring, and only one item is a tab stop
(`huterm_ui::roving::stop`, which a multiple `ToggleGroup` shares). The
owner routes its move commands through `roving::move_focus` and its
activation command through `roving::activate`, passing a `roving::Group`
built from its state when the command arrives: the handles, the selection,
and whether the group is enabled now. Both return the index to select, and
`None` for a disabled group, a group without focus, or the selected item.
A disabled group keeps the focus it had, so its keys still reach the owner;
`collapsible::toggled` applies the same rule to a trigger. A new
`FocusHandle` is not a tab stop until it says so, so a test's plain
focusable stand-in needs `tab_stop(true)` for `Window::focus_next` to reach
it. GPUI clips a child's outer ring to a container with `overflow_hidden`,
such as a `Card` or a scrolled sidebar, so full-width rows (vertical tabs
and collapsible triggers) draw `focus::ring_inset` instead of `focus::ring`.
`ScrollArea::reveal` takes `ContentBounds` the owner recorded during
paint, such as through a `canvas` probe in the focused row, because GPUI
has no query for an element's bounds by focus handle. They are in content
coordinates, so a second reveal before the repaint moves nothing; reveal
only sets the offset, so the owner redraws when it returns true.
`focus::ring` and `ring_inset` add such a probe to the control whose ring
they draw, and `focus::ring_bounds` reports where it last painted; a
control that draws its focus some other way, such as Settings' `actionable`
recorders, adds `focus::probe` itself, or Tab to it never scrolls.

Open a dialog in a `huterm_ui::components::dialog::Modal`, draw its `Dialog`
or `AlertDialog` as the last child of the area it covers while it is open, and
mount `Modal::guard` on every frame, as for a popover. The guard stops presses
and wheel events on the scrim outside the panel, and reports such a press to
the owner, never for an alert. It lets presses and wheel events outside the
scrim through unreported, so what lies around the dialog's area, such as a
client-drawn frame's resize edges, keeps working. Before the dialog's first
frame no scrim has painted, so the guard stops a press anywhere. The panel
carries the `confirming` context: route `dialog_focus_next` and
`dialog_focus_previous` to `Modal::focus_next` and `focus_previous`, which
cycle through the dialog's
declared controls and its enabled buttons, and give its buttons no
`SettingsControl` context, which would win for Enter, Space, and the arrows.
Give each button `keymap::DIALOG_BUTTON_CONTEXT` (`confirming control=button`)
instead: Space's `dialog_confirm` default has `when = "control == button"`,
and a binding's command context and `when` must match on one context node, so
both names sit on the button and Space never reaches a field in the dialog.
The Licenses buttons carry no such context, since Space there types into the
search.
A text field in a dialog carries the `Palette` context, as Settings' fields
do, so Left and Right edit it rather than run `dialog_focus_*`. Its owner
then routes `palette_expand` and `palette_previous_slot` (Tab and Shift-Tab
in the field) to `Modal::focus_next` and `focus_previous`, `palette_confirm`
to the dialog's confirm action, and `palette_back` to `Dismissal::Escape`,
so Tab never leaves the dialog. GPUI falls through to an outer context's
binding when a deeper context's action has no handler, so an unhandled
`palette_expand` still reaches the dialog's Tab, but a handler that consumes
it without moving focus traps Tab in the field. Focus moves in on the
dialog's first frame, and `Modal::close` returns it to the target given to
`open`; with every control disabled it enters on the first button. Window
deactivation and owner changes are the owner's to route; a dialog normally
stays open while its window is inactive. A body that can outgrow the window
puts its long content in a `ScrollArea` made with `fill()`, which shrinks
with the body, since a plain `ScrollArea` grows to its content's height and
never scrolls there; the body shrinks and clips so the footer stays inside
the panel, where the guard lets presses through. The
guard stops presses on the scrim outside the panel, so a popover opened from a
dialog's body does not work yet. A simulated press in a GPUI test draws a
notified window first, so a stale-frame test opens without notifying,
presses, and notifies afterwards. In the desktop harness, `simulate_click`
still draws after an update through the Settings window's handle, so its
stale-frame test dispatches the press with `Window::dispatch_event`.

The Settings layout is built from the step 6 components. The sidebar is a
vertical `Tabs` with the section handles, one tab stop that the group
commands move and select along; the footer's links take `tab_index(1)`
(`footer_stop`), so Tab from the sidebar enters the section and Shift-Tab
from its first control returns. The theme cards are a card `RadioGroup` whose
moves choose the theme. A setting's row is a `Field` (`render_row`), with its
conflict under it as a warning `Alert`; the rows that only lead to another
section are `Item`s, as are the `unbind` rows, whose key caps draw through
`Item::title_element` while the key's text names the row; a command search
result is a link `Item` (`Item::link`) on its `kgo:` action stop, so it is
one focusable, named `Link` that `settings_toggle` and a click run; a
command row is a
`Collapsible::item` whose shortcut summary is the item's action, outside the
trigger; groups of rows are titled `Card`s (`group_card`, which uppercases
the title); the invalid-file banner is a danger `Alert`; and an empty global
shortcut list is an `Empty`. `render` notes a focus that moved since the last
frame and defers `reveal_focused`, which scrolls the pane only when the ring
shows (`focus::visible`, so a pointer press scrolls nothing), the ring
painted inside the pane's columns, and no dialog or menu is open. A row whose
label sits apart from its controls, such as a user layout's name above its
template, Rename, and Delete, draws `reveal_group` over itself with their
handles, and the reveal shows the row with the ring. Smoke state reports
`settings.scroll` and `settings.in_view` for it.

The terminal window has one dialog slot too, `WorkspaceView::dialog`. The
top dialog draws in it: the close confirmation, else Licenses, else About.
Every change of top dialog goes through `sync_dialog`, which opens the slot
again for the new dialog, keyed by the confirmation's generation so each
new confirmation opens on its primary button, and render calls it too
before it mounts the guard, for a change that skipped it. Focus that
returns to the dialog from elsewhere, such as a toast, goes to the control
that last held it (`Modal::last_focused`), so Cancel stays Cancel, and else
to the one it opens on. Closing returns focus to the window's handle,
unless focus already left the dialog, and the caller then focuses the
terminal. The close confirmation is a bare
`AlertDialog` that opens on its primary button (`AlertDialog::initial_focus`);
About and Licenses are bare `Dialog`s, whose content draws its own header
and footer, and `LicensesView` draws its own with a clone of the slot's
`Modal`. The slot draws through `modal_layer`, so inside a client-drawn frame
the scrim covers only the content, rounded at its top corners
(`Dialog::scrim_corners`), and the frame's resize edges still work. None
closes on an outside press or on deactivation, so the guard's callback
ignores the reason. In Licenses, a press that focuses nothing
returns focus to the search, and so do Escape that clears the search and a
selection or search change that hides the focused Website or Copy button.
The `Palette` context wraps the buttons, so the list keys and text commands
work from a button. While Website, Copy, or Done has focus, a canvas over
the search field registers the search `Input`'s handler for that button with
`window.handle_input`, wrapped in `SearchFromButton`. Unbound keys then reach
the search through the platform's ordinary text input, so dead keys, IME
composition, and Linux's Shift-only rule behave as on the focused search,
and Space, which no `Palette` binding claims, types a space. The wrapper
focuses the search before text, marked text, or a paste applies, and the
composition continues there under the search's own handler, because the
`Input` holds the marked text. GPUI's tests cannot send marked text through
the window, so the composition test calls the view's handler directly. A
forwarded text command focuses the search before it edits, except Copy,
which changes nothing to show.
Smoke state reads `dialog_focus`, `about_focus`, and `licenses_focus` from
real focus.

The Settings window has one dialog slot, in `settings_window/dialog.rs`. A
section adds a `Prompt` variant, opens it with `open_dialog`, and draws it
from `render_dialog` with buttons from `dialog_button`. Each button answers
with an action key that `act` runs, and `dialog_cancel` runs the prompt's
Cancel key. `run` routes the `dialog_*` commands to the slot and, while a
dialog is open, ignores every other command except the window's close,
minimize, zoom, and fullscreen. App commands such as Quit and Settings… go
through the application's handler, not `run`, so they still work and need
no gate.

A prompt with a text field opens with `open_field_dialog`, which gives the
slot its `Input`, and its renderer draws the field inside a `Field`, wrapping
the `InputGroup` in a `div` with the `Palette` context, declares the field's
handle with `Dialog::focusable` and `initial_focus`, and names the key Enter
runs in `Prompt::confirm`. `run_palette` calls `run_dialog_field` first: it
routes `palette_expand` and `palette_previous_slot` to the modal's focus
cycle, `palette_confirm` to the confirm key, `palette_back` to the prompt's
Cancel, and every other palette command to the field, and while a dialog is
open it takes them all. Route Shift-Tab to the modal, not the window: the
field is the dialog's first control, so `window.focus_prev` leaves the
dialog. The New profile and Rename profile dialogs (`quake.rs`,
`render_name_dialog`) check the typed name on every frame, show the message
under the field, and disable the primary button, which then leaves the Tab
cycle. Settings ignores an outside press on its dialogs, as a macOS sheet
does, so a typed name survives a stray click. A save of several patches,
such as a rename's, goes through `saving.rs`'s `Batch`: `submit_batch`
queues it, and they go to `ConfigFile::apply` together, so a failed save
writes nothing. When its turn comes, the values it moves are read from the
file as the window last read or wrote it, so an edit made elsewhere to them
conflicts, but the entries it rewrites (`Batch::current_patches`) are found
in the file as it is when the batch applies, since a reference added
elsewhere would otherwise survive a successful rename. `batch_outcome` lists
the values `pending` shows meanwhile, including each value a rename copies
under the new name, so an edit made there before the batch lands takes the
copied value as its base and builds on the batch instead of conflicting;
`batch_baseline` makes a field there sync against that value, not the file,
until the batch lands, so a draft typed over it is not taken for a conflict
when an earlier save lands. `batch_refused` says why it failed and drops the
queued and pending edits under the new name, which written alone would create
it; `batch_started` and `batch_settled` follow a batch outside the window. A
rename records itself in the window model when its write starts
(`quake_rename_started`) rather than moving the Quake window's association: a
reload that read the file before the rename can apply after it, and its
`reconcile` would turn the moved window into an ordinary one. The save can
also replace the file, and a reload read and apply it, before the save
settles on the main thread. The save's continuation settles the record
before `written_batch`, even when Settings has closed: noted when the write
landed, dropped when it wrote nothing. Each reload takes
`quake_renames_noted` before it reads, and `reconcile_quake` moves a window
along the running and noted renames to the first name the file it read has,
dropping the noted renames that read already shows. Another rename adds a
`Batch` variant, as `RenameLayout` (`layouts.rs`) does for pane layouts;
nothing outside the window follows a layout, so its `batch_started` records
nothing. A layout's deletion is a batch too (`DeleteLayout`): its
`current_patches` refuses it, writing nothing, when an active keybinding or
the cycle in the file as it is then names the layout, since one added
elsewhere or pointed there by a rename queued ahead of it would otherwise
name a missing layout; `Batch::fresh` applies it to that read. Because it
reads afresh, a deletion queued behind a write that would have made the
layout, a rename to its name or a creation, goes when that write does not
land (`drop_queued_deletes`); otherwise it would delete a layout of that name
another editor made. Whether a failed write was a creation comes from the
change (`expected` was empty), not from the window's read, which an earlier
save in the queue can have updated. Every conflict and error must sit on a
drawn row, since `request_close` refuses while any conflict exists and
nothing else shows it: a rename clears the old name's, and
`forget_gone_layouts` drops those of a layout a load or save shows gone,
with its field's draft. A user pane layout's template is an ordinary
Settings text field
over a setting `layouts::template_setting` makes for its path, as a quake
profile's fields are, so commits, Escape, conflicts, and a rename's
baselines work unchanged. `commit_field` refuses a template that does not
parse before any write, so the message under the field is the parser's,
not the validator's.

GPUI reports an accessibility node only for an element with both an `id` and a
`role`; set disabled, read-only, invalid, modal, and live states with
`huterm_ui::a11y::States`, since GPUI has no setters for them. GPUI's
`aria_position_in_set` and `aria_size_of_set` pass AccessKit's values through
unchanged, and AccessKit differs from ARIA: a position is zero-based, and the
AT-SPI adapter adds one and reads the size from the set's container, never
from the item. The macOS adapter reads neither. States are per node too: a
disabled container does not disable its items, so set them on each. The vendored
GPUI's `test-window-accessibility` patch activates accessibility in test
windows: tests read `Window::debug_a11y_tree_json`, which spells element ids
in their `Debug` form (`Name("id")`), and act through
`Window::simulate_a11y_action`. A missing tree must fail a test, not skip it.
GPUI performs an accessibility `Click` on a node with no action listener as a
pointer press and release at the node's center, so it arrives as
`ClickEvent::Mouse` and passes the keyboard filter; tests click child-id items
that way through `testing::a11y_send`.

## Tab bar layout

The README describes the tab bar's options. These are the layout mechanics
that rendering, hit testing, and smoke geometry depend on.

Vertical tabs stay 32 pixels tall and fill the sidebar width.
Drag the sidebar's inner edge to resize it between 140 and 400 logical pixels,
capped at half the window width. Each window keeps
its preferred width for its lifetime, including through temporary window
shrinking. The vertical new-tab button is a row with the same insets as the
tabs; it follows the last tab and stays visible at the bottom when tabs
overflow. Horizontal bars reserve a 32-point slot for it and center a 26-point
button across the bar, matching the pill inset, so it never touches the bar
border. Strip bars are 32 points tall; Pill bars are 34 so 26-point pills keep
4 points on every side.
Pill strips also start with a leading margin so the first pill's visible edge
sits at that same inset, and hovering a pill never hides its dividers.
`pill_accent = true` widens every pill's left padding to keep titles aligned,
and the close-button slot stays reserved whatever `close_button` shows, so tab
widths never change. Fullscreen auto-hide reserves no bar space; a top bar can
be revealed from the macOS notch-height region and appears below the notch, and
repeated reveal activity restarts its one-second hold.

Left and right placement follow `style` too: Strip rows span the column,
merge into the terminal, put the accent line on the window edge, and outline the
active row above and below; a first active row at the strip's start joins the
terminal's top border instead and squares the rounded corner. Pill rows are
rounded, inset in the column, and carry the same optional accent bar. With a
titlebar above vertical tabs, a border also runs along the terminal's top edge,
joining the bar's edge so the titlebar and bar form one surface around the
terminal. The corner where they meet is rounded by the smaller window padding,
capped at 12 points, so the arc never covers a cell. Under a notch in non-native
fullscreen there is no titlebar: the column and its edge run to the top of the
screen, its rows stay below the safe area, and the strip beside it keeps the
terminal background. While a top bar is visible or revealing, the macOS titlebar
and the non-native fullscreen safe area above a notch use the tab bar
background; a titlebar above a left or right bar does too; otherwise, including
for a bottom bar, they use the terminal background. `[tabs].notch = "left"`
(the default) or `"right"` places a top bar in the auxiliary area beside the
notch in non-native fullscreen, read from `NSScreen`'s auxiliary top areas,
and `"off"` keeps the bar below the
notch: the bar keeps its
height, sits at the bottom of that area, and the terminal starts directly under
the safe area, so auto-hide leaves it in place. Fullscreen quake profiles read
the same shelves from their own window. Without a notch, in windowed mode, or in
native fullscreen the key is ignored.

Render measures titles and
caches their widths; `TabStrip` uses the cached widths for layout, reveal, and
drop slots. Vertical tabs ignore `width`.

Trackpad and wheel scrolling move the strip without selecting a tab. Horizontal
bars show floating arrows that indicate hidden content and animate scrolling
when clicked, plus a slim `Scrollbars` indicator along their bottom edge that
never expands or shows a track but still drags and jumps. Vertical columns use
the same overlay on their right edge instead of arrows: it appears on scrolling
and reveal, widens once the pointer rests on it (200 ms) or at once on a
press, jumps on a track press, and drags through the window-level capture
that also serves tab reordering. It sits
inboard of the sidebar resize handle by the handle's width so the two never
overlap, and both tab bar indicators fade after 700 ms rather than the
terminal's two seconds. Explicit selection or creating a tab reveals it;
ordinary redraws preserve manual scroll. Drag a tab to reorder it within its
window, horizontally or vertically. The preview and insertion position remain
constrained to the bar when the pointer leaves it; release commits and Escape
cancels. During a drag, hovering an overflow edge scrolls continuously without
changing the active terminal. Reordering preserves terminal processes, focus,
selection, and scroll position. Cross-window moves and tear-out remain deferred.

## Configuration and schemas

Theme reload must invalidate prepared row colors even when the terminal snapshot
is unchanged. Font changes must also invalidate glyph layouts and update PTY cell
pixel dimensions even if the row/column count stays the same. Keep bundled theme
licenses in the packaged resources.

Treat malformed TOML and invalid legacy engine values as fatal at startup.
Omitted and explicit `ghostty` values are silent; explicit `alacritty` launches
Ghostty with the migration warning. For valid TOML, fallback from unrelated
settings errors must preserve clipboard policy and the migration diagnostic.

Fatal snapshot errors set the runtime closing gate; latch client snapshot
failure so pending scroll or invalidation cannot create an immediate retry loop.

`desktop/config_watch.rs` owns the `notify` watcher on its own worker thread,
which starts, reconfigures, and drops it: `FSEvents` blocks while starting and
joins a thread when dropped, so the GPUI thread only sends `Settings` and
receives `Report`s. Watch directories non-recursively, never files, and a
missing directory through its nearest existing ancestor; keep the ancestor
watched until the descendant's registration succeeds, and keep it when that
registration fails. The ancestor's non-recursive watch misses edits inside the
failed directory, so a reconcile that ends with a registration failed raises
the `Unavailable` warning and reports watching off. A reconcile that ends with
every registration held reports watching on, which takes the warning back from
every window and the latch and re-arms it. Record each registered
directory's identity and register it again when the identity changes or an event
removes or renames the directory itself: inotify drops a removed directory's
watch and keeps following a renamed one, so a directory replaced at the same
path is otherwise silently unwatched. Match events against each input as given,
with only its directory canonicalized, and fully resolved: native events name a
symlink through its real directory, so resolving the link itself misses a
retarget when the configured path is not canonical, such as under macOS's
`/tmp`. Ignore every `Access` event: fingerprinting opens the files, so reacting
to inotify's open and close events loops. Every load and reload records
`config::Inputs`, a fingerprint of the bytes it consumed plus the theme files it
read or probed; reconcile compares the current fingerprint with the last
*attempted* reload's, not the last successful one, so restoring good content
after a broken save reloads. Each registration change schedules one catch-up
reconciliation for changes made while no watch covered them, including the
stream restart `FSEvents` performs on every `watch` call. With no events the
worker blocks on its channel: no timer, no reads. On the desktop,
`config_watch::Reloads` admits one reload at a time; a report during a reload
marks it dirty even when the content matches, and one follow-up runs when it
finishes. Every finished reload sends the watcher fresh `Settings`, including
after a failure, which keeps the previous `watch_config`. An automatic reload
that finds the file missing is skipped silently, since editors remove it while
saving. An editor that truncates the file and writes it more than `QUIET_PERIOD`
(150 ms) later can briefly apply the empty file, that is the defaults, until the
write's event reloads the full content; this risk is accepted. An automatic
reload may start until Quit is committed, while the confirmation is open and
while teardown runs, since Cancel would otherwise leave a change saved meanwhile
unapplied; none starts once Quit is committed. Committing Quit drops the
watcher's handle, and a reload that finishes afterwards applies nothing, since
applying would register the global shortcuts Quit just removed.

The `huterm_config::document` patch service edits `config.toml` by splicing
byte ranges, never by re-serializing, and refuses any splice whose reparsed
value tree or comments differ from what the operation asked for. `apply`
resolves every entry locator once, against the file as it is when the call
starts, and keeps each entry's identity through the call's own edits and
removals; resolving again after an earlier patch can land on an identical
twin. Its desktop
validator, `config::validator`, must stay free of side effects: it compiles the
keymap and global shortcuts but registers nothing.

Configuration schemas are generated from `huterm-config` with its `schema`
feature; that crate must remain independent of GPUI and terminal engines.
`mise run schema:generate` updates both committed files; `schema:check` compares
bytes without writing. Keep selected/named theme projections and catalog-derived
keybinding constraints aligned with application parsing through the shared
`schemas/fixtures.json` cases. Do not stamp release versions into schema files;
release packaging verifies their bytes against the selected checkout.
Taplo 0.10.0 skips properties beside a schema's `allOf` during completion.
Keep common keybinding fields in its first member, with command constraints in
later `if`/`then` members. This preserves command completion and specific argument
diagnostics; it does not provide command-specific argument completion.
`mise run smoke:schema-editor` drives the pinned Taplo language server against
the committed schema, and CI's Policy job runs it. Prompted arguments are
optional in the schema, so its argument checks use constraints such as
`select_tab.index` bounds rather than required properties.

Constraints, descriptions, and Settings metadata live on the config types:
`#[schemars(range(...))]`, `regex`, doc comments, and `extend` with the
`x-huterm-*` keys that `huterm_config::settings` reads, including
`x-huterm-step` and `x-huterm-control` for number settings. `schema.rs` keeps only
what no single type owns. Every schema property needs `x-huterm-label` unless
it is a namespace object, or `SettingsModel::load` fails, and the coverage
test in `desktop/settings.rs` fails until a new setting has a generic control,
an override, or a file-only entry. Doc comments become schema descriptions
with their line breaks; editors render them as soft wraps, and the model joins
each paragraph's lines.
