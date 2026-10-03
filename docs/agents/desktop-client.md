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
as the
shared source of terminal bounds for painting, mouse input, scrollbars, and PTY
resizing.

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

The terminal activity drain queues shell-exit transitions once for assessed tab
close.
Keep automatic exit requests separate from the merged manual close intent, so
canceling one prompt does not lose inactive siblings. Config reload changes only
future exit events. Exited tabs retain history and local selection/scrolling;
close their input queue and reset mouse ownership without emitting releases.

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
conditional, so it reserves no terminal key. A repeated close shortcut while
`confirming` must not confirm. Menus and dialogs return focus to the terminal
when they close; the menu button never keeps it, or terminal bindings stop
matching until the user clicks back in.

Each open menu carries a `MenuKind` and pointer anchor; derive button state
and pick targets from them, not from whether any menu is open. A terminal
right-click's link rides on the next snapshot request and opens the menu when
it answers or after a bounded wait. `MouseState::down` records a local press as
held, so sample `owns_pointer_gesture` before it; a right press during another
gesture must not open a menu. Emulator-side edits such as Clear Scrollback's
`CSI 3 J` reach the emulator only while the native stream is at ground.
`EscapeHint` ignores UTF-8 state and must not gate injected bytes. Ghostty's
full reset, like its RIS, keeps OSC color overrides.

Clickable controls pair their hover style with an `.active` pressed style:
`Swatch::pressed`, `accent_pressed`, or `selection_pressed` in overlays and
`TabColors::control_pressed` in the tab bar and title row. `Swatch` is passed
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
and reveal, widens on hover while shown, jumps on a track press, and drags
through the window-level capture that also serves tab reordering. It sits
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
