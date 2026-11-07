# Terminal notifications, progress, and bell attention

Status: implemented on 2026-11-07, in the pull request for #81.

Issue: [#81: Surface terminal bells, notifications, and progress][issue81].
Parent: [#78: Terminal protocol compatibility][issue78].

## Outcome

- **Progress.** A program's OSC 9;4 progress report shows as a ring in its
  tab's and pane title's status slot and, unless `[terminal] progress_bar =
  false`, as a 2-point bar across the top of its pane.
- **Notifications.** An OSC 9 or OSC 777 notification appears exactly once.
  While Huterm is in the background it is a system notification. While
  Huterm is in front it is a notice, offering Show Tab when the terminal is
  not the one in front. When system notifications cannot show, a notice
  stands in for them and the window asks for attention.
- **Bell attention.** A bell in a window that is not active asks for
  attention without taking focus, under `[terminal.bell] attention`.

Bell presentation (flash, tab mark, macOS sound) landed earlier and is
unchanged, apart from the tab mark now also showing unseen notifications.

## Decisions

### Which channel carries each signal

Since #208, a terminal's status is revisioned latest-value state that each
viewer reads when it wakes, and a host effect is a bounded,
single-recipient delivery (see [the core guide][core-viewers]).

| Signal | Channel | Why |
| --- | --- | --- |
| Progress | `TerminalStatus::progress` | Every view of the terminal presents it, a late or reconnecting viewer should see the current value, and only the latest value matters. |
| Notification | `HostEffect::Notification` | It has an OS-visible side effect that must happen once, on one client, and must never replay to a late viewer. |
| Bell attention | Client presentation of `TerminalStatus::bells` | Bells are already counted in status; asking for attention is the window's presentation of a bell, like its tab mark. |

Progress is never terminal history: it is not in the grid, and session
capture records no status, so restore cannot replay it.

### Engine contract

Ghostty's C API already parses both sequences, so there is no second
parser. `huterm-ghostty` installs `GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION`
and `GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT` and copies each payload out as a
plain `Effect`. Its contract tests pin what the engine relies on:

- OSC 9 has no title. OSC 777 `notify;title;body` keeps later semicolons in
  its body.
- Both use Ghostty's fixed 2,048-byte OSC capture: a body of 2,047 bytes
  arrives, and a longer sequence is dropped whole.
- CAN and SUB cancel a sequence. A chunk split delivers the sequence with
  its terminator. Bytes arrive raw, including invalid UTF-8.
- Ghostty removes C0 bytes such as tab from an OSC payload. An ESC ends it.
- A ConEmu command Ghostty does not implement, such as `9;1`, produces
  nothing. A malformed one, such as `9;4;9`, arrives as a notification.
- Progress percentages are clamped to 0 through 100. A full reset (RIS)
  reports a removal; a soft reset (DECSTR) does not.

### Normalization and limits

Core treats notification text as untrusted (`notifications.rs`):

- Invalid UTF-8 becomes U+FFFD. Control characters, C1 included, become
  spaces. Bidirectional formatting characters are removed, so text cannot
  read in a different order than it was written.
- Whitespace collapses, the title keeps 128 characters and the body 512,
  and each ends in an ellipsis when truncated.
- An untitled body that starts with digits and a semicolon is a malformed
  ConEmu command and is dropped. Ghostty's callback does not say which
  sequence it parsed, so an untitled OSC 777 that starts that way is dropped
  too; a body of only digits is kept.
- Each terminal may send 4 notifications at once, then one every 3 seconds.
  The desktop adds an app-wide limit of 8 at once, then one a second.
  Notifications over a limit are dropped, never deferred.
- Clients present text only as plain text. Linux escapes the markup that
  notification daemons parse. The only action is Huterm's own Show Tab.

Progress expires 15 seconds after the last report, matching Ghostty, so a
program that crashed cannot leave a stale indicator. The runtime checks the
expiry at the top of every turn, so neither continuous output nor a stream of
controls can hold it, and waits for it while idle or backed up. A repeated,
unchanged report extends the expiry without publishing a revision. The root's
exit clears progress, and output parsed after the exit cannot report it again.
Reset Terminal clears it too, since Ghostty's C reset reports no removal.

### Delivery and eligibility

Host effects now have kinds (`HostEffectKind::Clipboard` and
`Notification`). A recipient is eligible for an effect only while it
permits that kind, and each kind has its own permission and generation:
revoking notifications cancels queued and popped notifications, not
clipboard writes. Selection keeps the existing rule: the eligible recipient
with the latest activity ordinal, chosen at admission and never redirected.
This matches [runtime IPC][ipc]'s "client-routed host effects", where a
recipient is eligible only for kinds its client negotiated, and its
`HostEffect` frame carries a kind.

| Situation | Behavior |
| --- | --- |
| `[terminal] notifications = "deny"` | The recipient does not permit the kind; nothing is admitted. |
| No client attached, or none eligible | Admission fails and the notification is dropped. A later client gets no replay. |
| Client disconnects or the view is removed | Its recipient is revoked and its queue cleared. Nothing moves to another client. |
| Local text client (TUI) or remote client | `LocalText` and `Remote` recipients stay ineligible for every kind until #48 and #131 define their policies. |
| Reconnect or session restore | Notifications are never stored. Progress is status, so a new viewer sees the current value, and restore starts with none. |
| macOS permission denied or undecided, a build outside an app bundle, or no Linux notification daemon | The background case raises a notice in the terminal's window and asks for attention. An undecided macOS permission, or an availability the first probe has not answered yet, still posts, which asks the user. |

A notification's heading falls back to the tab or pane label, which a
program can set through its terminal title, so the label passes the same
cleaning and limit. A system notification's title carries the label after a
program's own title, so a banner always names its terminal. Each system
notification has its own tag, so a pane's newer notification does not
replace an earlier one in the notification center.

`native_notifications.rs` learns availability without blocking: on macOS
it reads the authorization status (`UNUserNotificationCenter`, guarded by
the bundle check GPUI uses), and on Linux it asks the daemon for its server
information on a background thread. It refreshes at startup, when a window
becomes active, and after each post.

### Presentation

Jim chose these from HTML mockups on 2026-11-07:

- **Progress.** Option C: a ring in the status slot plus a pane bar, which
  `[terminal] progress_bar` turns off. The ring takes the running dot's
  place; failed, exited, and the attention mark outrank it. Determinate and
  indeterminate progress use the tab accent, paused progress the ANSI yellow
  of the running dot, and errors the ANSI red, filling the ring or bar. A
  split tab shows its focused pane's progress. Indeterminate motion steps
  every 125 ms on the window's earliest-deadline timer, not every frame,
  only while a window shows it, and stands still under
  `App::reduce_motion`. A pane title's accessible name gains the state,
  such as "progress 42%".
- **Notifications.** Option N2:

  "Huterm in front" means any Huterm window is active, the Settings window
  included; each window is asked, because a platform's main window can
  outlive activation.

  | Huterm | Terminal | Presentation |
  | --- | --- | --- |
  | Background | Any | System notification and tab mark; clicking it reveals the pane. |
  | Front | In another tab or window | Notice in the active window with Show Tab, and tab mark. |
  | Front | In front | Notice only. |

  The notice is an expiring Info notice titled with the program's title, or
  the tab or pane label when it sent none, as OSC 9 never does. Its source is
  `NoticeSource::Program`, so each tab keeps only its newest notification
  notice, and a terminal failure in that tab does not replace it. Show Tab
  and a system notification click summon a hidden Quake window and clear a
  zoom that hides the pane.
- **Linux delivery.** GPUI's Linux backend waits on a thread and a
  session-bus connection per notification until it closes. The vendored
  `bounded-notification-waits` patch lets at most 16 wait; later ones show
  without waiting, so their click does nothing. The availability probe runs
  one at a time.
- **Bell attention.** `window.request_attention()` from a window that is not
  active. macOS bounces the Dock icon once and only while Huterm is in the
  background. X11 sets the urgency hint until the window is focused.
  GPUI's Wayland backend has no attention request, so Wayland shows
  nothing; that is the documented gap.

## What #407 can reuse

[#407][issue407] makes notices accessible. Notification notices are ordinary
notices, so they become accessible with every other notice. They are
`Severity::Info` and expire, which bears on #407's open question of which
severities announce assertively.

## Verification

- `huterm-ghostty` contract tests for the cases under
  [engine contract](#engine-contract).
- Core unit tests: normalization, the limiter, per-kind permission and
  generation, recipient selection by kind, ineligible origins, progress
  expiry and exit, and engine tests from bytes to host effect and status.
  A runtime test drives both through a real PTY.
- GPUI tests: the routing table, notice content, tags, Linux escaping, and
  window tests through a real terminal for progress in the tab and pane,
  each notification route, Show Tab, the deny setting, and bell attention.
- Native macOS checks on scratch bundles with their own bundle IDs, through
  computer use, on 2026-11-07:
  - The ring on active and inactive tabs and the pane bar, determinate,
    paused, error, and indeterminate (moving), then cleared.
  - Background notices from OSC 9 and OSC 777, hostile text shown plainly,
    and the bell mark with Show Tab revealing its tab.
  - An in-front notice with no Show Tab.
  Computer use may not operate the macOS permission prompt or the Dock, so
  system banners, the permission choice, clicking a banner, and the Dock
  bounce need a manual check.
- Linux: `mise run lint` and the focused GPUI tests in the Docker container,
  including markup escaping.

## Unresolved questions

- None blocking. Command-completion notifications from OSC 133 (#37)
  remain separate work and must stay distinguishable from a program's own
  request. The notification center is #215.

[issue81]: https://github.com/jimeh/huterm/issues/81
[issue78]: https://github.com/jimeh/huterm/issues/78
[issue407]: https://github.com/jimeh/huterm/issues/407
[core-viewers]: ../agents/core-runtime.md#terminal-viewers
[ipc]: runtime-ipc.md#client-routed-host-effects
