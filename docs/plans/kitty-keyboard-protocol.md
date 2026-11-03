# Kitty keyboard protocol

Status: approved. Delivers [#85][issue-85], parent [#78][issue-78].

Terminal applications that enable the Kitty keyboard protocol get unambiguous
keys for the flags Huterm implements: disambiguate escape codes (`0b1`) and
report alternate keys (`0b100`). Everything else stays as it is today.

## Decisions

These were settled with Jim on 2026-11-03.

- **Supported flags:** `1 | 4` only. Report event types (`2`), report all keys
  as escape codes (`8`), and report associated text (`16`) are deferred to a
  follow-up issue. Flag 2 needs key-up tracking. Flag 8 needs printable keys to
  bypass AppKit text input, which puts IME and dead keys at risk.
- **Legacy encoding:** when the effective Kitty flags are zero, every key
  Huterm encodes today keeps its current bytes exactly. Unmodified F1 to F12
  and Insert gain their standard xterm legacy bytes, because the protocol needs
  those keys for Kitty mode and they need a legacy form. Modified legacy named
  keys, such as Ctrl+Left as `CSI 1;5D`, stay out of scope.
- **Encoder:** in Kitty mode, core uses Ghostty's key encoder against
  Ghostty's own flag stack. Huterm does not add a second Kitty parser or
  encoder.
- **Key identity:** recover it from native key data before GPUI normalizes
  it. GPUI rewrites a shifted non-letter key as its shifted character and
  clears Shift on both platforms (`gpui-pre-macos` `events.rs` `parse_keystroke`,
  `gpui-pre-linux` `platform.rs` `keystroke_from_xkb`). It also leaves macOS
  Control chords without `key_char`. Spec-correct unshifted keys, shifted
  alternates, and base-layout keys need the native keycode and layout.
- **Manual QA:** Codex computer use runs in the background, on a scratch
  bundle and configuration, with `kitten show-key -m kitty` and vim 9.1 legacy
  spot checks. Nothing is installed on the host.
- **Linux real application:** the Linux input smoke runs kitty's `kitten
  show-key -m kitty` inside its container and asserts its decoded output. The
  binary is downloaded at a pinned version, with a SHA-256 for each
  architecture. kitty is GPL-3.0, but it is a test-only tool that is never
  linked or shipped, so the dependency licence policy does not apply. The PR
  states this.

## What Ghostty provides at the pin

Ghostty 1.4.0, the current pin, needs no change:

- `ghostty_terminal_get(GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS)` returns
  the active screen's current flags as a `uint8_t`.
- Ghostty parses `CSI > flags u` (push), `CSI < n u` (pop), `CSI = flags ; mode
  u` (set, or, or not), and `CSI ? u` (query). Each screen has its own 8-entry
  stack (`src/terminal/kitty/key.zig`). Ghostty answers the query natively with
  `CSI ? flags u`.
- `include/ghostty/vt/key/{encoder,event}.h` offer `ghostty_key_encoder_*`,
  `ghostty_key_encoder_setopt_from_terminal`, and `ghostty_key_event_*`. The
  encoder takes a key, modifiers, consumed modifiers, UTF-8 text, and an
  unshifted codepoint. When the flags are non-zero it encodes Kitty sequences
  (`src/input/key_encode.zig`, `fn kitty`).

## Approach

### `huterm-ghostty`

- Add the key encoder and key event functions, types, and constants to the
  bindgen allowlist in `bindgen/src/main.rs`. Regenerate with
  `mise run ghostty:bindings` and keep `ghostty:bindings:check` green.
- Add a safe wrapper with the following parts:
  - An owned `KeyEncoder` that lives with the `Terminal` on the owner thread.
  - A way to sync it from a terminal.
  - A kitty-flags override.
  - An `encode` that takes a plain-value event (key, modifiers, text, unshifted
    codepoint) and returns bytes, handling `GHOSTTY_OUT_OF_SPACE`.
  - `Terminal::kitty_keyboard_flags()`.

  Keep the `unsafe` code in `native.rs` under the crate's existing rules: an
  `// SAFETY:` comment that cites the header contract, one operation per unsafe
  block, and getter and option types from the generated `keys.rs`.
- Ownership:
  - The wrapper owns one native key event next to the encoder.
  - It sets every field (action, key, mods, consumed mods, composing, UTF-8,
    unshifted codepoint) on every encode, so nothing carries over from the
    previous key.
  - The borrowed UTF-8 must outlive the encode call, because the event does
    not own it.
  - Both handles are freed on drop.
- Extend the generated key sets and their native type probes the way
  `terminal-engines.md` describes ("Each key set declares where its header
  puts annotations ...").
- Add contract tests in `huterm-ghostty` that pin the native behaviors core
  relies on:
  - the flags getter after push, pop, set, and reset, and per screen;
  - the 8-entry stack bound;
  - the encoder's output for representative events under flags 1 and 5.

  After a future pin bump, these tests should fail before engine code
  misbehaves.

### `huterm-protocol`

Extend the key input so core can encode either way from one queued event,
using no engine types:

- `TerminalKey` gains `F1`..`F12` and `Insert`. It is already
  `#[non_exhaustive]`.
- Character chords keep their current legacy interpretation: the text with any
  Control transformation, and the `meta` flag. They also gain the key identity
  that Kitty encoding needs:
  - the unshifted base character of the key on the active layout;
  - the shifted text the layout produced, without Control or Meta
    transformation;
  - Shift, which Character currently drops;
  - optionally, the ASCII base-layout key GPUI reports when it differs from the
    layout character.
- A Character chord that legacy encoding would not claim is allowed only while
  Kitty mode is active. It carries a legacy fallback, its layout text with the
  Meta prefix when `meta` is set, which core uses if Kitty mode turns off
  before dequeue. See "Ctrl chords legacy cannot encode" below.
- `TerminalModes` gains the effective Kitty keyboard flags, masked to `1 | 4`,
  as a small dependency-neutral type. Clients may use it only as a routing hint.
  Core always encodes against the live flags.

The implementer chooses the exact type shapes, within these limits:

- `huterm-protocol` keeps an empty dependency set.
- New public enums are `#[non_exhaustive]` where the existing types are.
- Both accounting functions count every new owned string: GPUI's
  `input_queue.rs` `buffered_input_bytes` and core's `input_bytes` in
  `terminal.rs`.
- The base-layout key is carried as the US-layout character of the physical
  key, such as `a` for the key labelled `ф`. Core maps it to `GhosttyKey`, so
  no engine type crosses the protocol.

### `huterm-core`

- `TerminalEngine` owns a `KeyEncoder` and exposes its effective flags:
  Ghostty's current flags `& (1 | 4)`.
- `encode_input` runs at dequeue time in `handle_input`, as it does now. For
  `Key` and `Character`, it chooses an encoding from the live effective flags:
  - **Flags zero:** today's legacy bytes. Add F1 to F4 as `SS3 P`..`SS3 S`, F5
    to F12 as `CSI 15~`, `17~`, `18~`, `19~`, `20~`, `21~`, `23~`, `24~`, and
    Insert as `CSI 2~`. Modified F-keys and Insert keep today's
    ESC-prefix-for-Alt rule, and Ctrl and Shift do not change their bytes. A
    kitty-only chord uses its legacy fallback.
  - **Flags non-zero:**
    - A Character chord with neither Ctrl nor Meta, such as Shift+letter or a
      Linux printable key, sends its text unchanged and bypasses the encoder.
      This is what Kitty sends for unmodified and Shift-only text under flags
      1 and 4. Without the bypass, Ghostty would see Shift as an unconsumed
      binding modifier and emit `CSI 97;2u` for Shift+a.
    - Every other `Key` or `Character` goes through the encoder:
      1. Sync the encoder from the terminal.
      2. Override its Kitty flags with the masked value.
      3. Pass the client's modifiers with consumed modifiers empty. The client
         has already applied the Option policy, so `alt` means Meta. An
         Option-composed character arrives as `Text`, not as a chord.
      4. Encode the event.

    `Text` and `Paste` never pass through the key encoder. Plain text, IME
    commits, and Option-composed characters stay plain UTF-8, and paste keeps
    its bracketed-paste handling.
- Replies: `unsupported_reply` currently drops every `CSI ? … u`. Replace that
  with a rewrite: a `CSI ? <flags> u` reply leaves as `CSI ? <flags & 5> u`.
  Kitty graphics replies stay dropped.
- Synced panes already re-encode against their own modes, and Kitty encoding
  follows the same rule. Every claimed chord carries its key identity, so a
  Kitty-enabled target encodes it correctly. One limitation remains: the source
  pane's routing hint decides whether a Kitty-only Ctrl chord is claimed at all
  (see below). A legacy source therefore sends nothing structured for Ctrl+1 to
  a Kitty target. This is documented and tested, not fixed.

### `huterm-gpui`

- `keyboard::translate` maps `f1`..`f12` and `insert` to the new keys. It fills
  the key identity for Character chords, and the key input keeps Shift, so
  Shift+Enter and Shift+Tab work in Kitty mode.
- **Native key identity.** Add vendored GPUI patches, following the
  `keystroke-held-flag` precedent and the process in `docs/agents/dependencies.md`:
  - `gpui-pre`: a plain-value optional identity on `KeyDownEvent` and
    `KeystrokeEvent`, carried to interceptors. It holds:
    - the layout's unshifted character;
    - the layout's shifted character;
    - Shift as the user actually held it;
    - the physical key's US-layout character.

    Do not add a field to `Keystroke`: its equality drives binding matching.
  - `gpui-pre-macos`: fill the identity in `parse_keystroke` from the event's
    `keyCode`, using `chars_for_modified_key` with `NO_MOD` and `SHIFT_MOD` and
    the raw Shift flag. Map the keyCode to its ANSI US character.
  - `gpui-pre-linux`: fill it in `keystroke_from_xkb` from the XKB keycode:
    the level-0 and level-1 keysyms, or UTF-32, of the active layout; the raw
    Shift state; and `guess_ascii(keycode, false)` for the US character.
  - Huterm reads the identity from the interceptor event that reaches
    `handle_keystroke`.
  - Chord replay: GPUI replays pending prefix strokes as bare `Keystroke`s.
    Store each stroke's identity in `PendingShortcuts` when it is first
    intercepted, and reuse it when that stroke replays.
  - Fallback: when no identity is available, derive it from `Keystroke`
    (letters keep Shift; other keys use the reported character). Test it,
    and document that it is less precise.
- Pin the derivation with tests that use platform-produced events where
  possible: the `smoke:macos-input` and `smoke:linux-input` cases below. Add
  unit tests for the patched parsers' pure helpers. Hand-built `Keystroke`
  tests alone could pass on a derivation that real events never produce.
  Cover:
  - a lowercase letter;
  - Shift with a letter;
  - Shift with a digit or punctuation;
  - Ctrl with a letter;
  - Ctrl+Shift with a letter;
  - Ctrl+Shift with punctuation, expecting `CSI 59;6u` and, under flag 4,
    `CSI 59:58;6u`;
  - Ctrl+Space;
  - a non-ASCII layout key where it can be produced.
- **Ctrl chords legacy cannot encode.** Today, Ctrl with a key outside
  `@`..`_`, such as Ctrl+1, Ctrl+. or Ctrl+;, falls through to the platform
  text path. While the displayed snapshot's `TerminalModes` reports Kitty mode
  active, the client claims those chords as Character chords with a legacy
  fallback. Otherwise behavior is unchanged.
- Unchanged:
  - Huterm command keybindings, reserved keys, and pending sequence replay;
  - the macOS Option policy and dead-key composition;
  - native IME and marked text;
  - modifier-only events;
  - Cmd and Super chords, which stay client-owned and never reach the PTY.

## Observable success

- `kitten show-key -m kitty` in Huterm shows Kitty-protocol decoding of:
  - Escape;
  - Ctrl+letter;
  - Ctrl+Shift+letter;
  - Alt+letter under Meta;
  - Shift+Enter, Ctrl+Enter, Shift+Tab, Ctrl+Backspace;
  - modified arrows;
  - F1 to F12, Insert;
  - Ctrl+digit and Ctrl+punctuation;
  - shifted alternates under flag 4.

  Plain text, Enter, Tab, and Backspace stay legacy.
- `printf '\e[>31u\e[?u'` reads back `\e[?5u`, and Huterm then encodes only
  flags 1 and 4. `printf '\e[>2u\e[?u'` reads back `\e[?0u`, and keys stay
  legacy.
- vim 9.1 and a shell without Kitty mode see byte-identical legacy input,
  except that F1 to F12 and Insert now work.

## Failure paths and risks

- **Mode change adjacent to a keypress:** keys are encoded at dequeue time
  against the flags parsed so far, the same rule as mouse and cursor-key modes.
  A key queued before a push encodes as Kitty if the push was parsed first.
  This matches the documented `NextMessage` ordering.
- **Alternate screen:** Ghostty keeps a separate stack per screen. Entering the
  alternate screen shows its flags. Leaving it restores the primary screen's
  flags.
- **Full reset (RIS) and malformed sequences:** Ghostty handles both. Pin RIS
  clearing and the ignored malformed forms in contract tests.
- **Excessive nesting:** Ghostty's stack holds 8 entries and evicts the oldest.
  Pin this with a test, and do not add a Huterm limit.
- **Child exit:** input already closes at root exit (`Owner::queue_write`).
  No key bytes are written afterwards, so retained Kitty state is harmless. A
  restarted terminal starts with a fresh engine.
- **Focus changes:** focus reporting is unchanged. The Kitty protocol defines
  no focus-related key behavior that flags 1 and 4 need.
- **Replayed chord without stored identity:** falls back to the
  `Keystroke`-derived identity. This is bounded and documented.
- **Native identity unavailable:** an unpatched path, a synthetic event, or a
  test event falls back the same way.
- **Stale routing hint:** a Ctrl chord claimed under a stale Kitty-active hint
  falls back to its layout text if Kitty mode is off at dequeue. A chord that
  is not claimed under a stale Kitty-off hint takes today's platform path. Both
  outcomes are bounded and match legacy behavior.
- **Encoder failure:** an encoder error fails the runtime through the existing
  `RuntimeError` path, as other engine errors do. The encoder never panics
  across FFI.
- **Encoder state across syncs:** the encoder is reused, and every encode
  re-syncs it from the terminal and re-applies the override, so no option
  survives from a previous key.

## Non-goals

- Flags 2, 8, and 16: key release and repeat events, report-all, and associated
  text.
- Modifier-key reporting.
- `modifyOtherKeys`, xterm modified legacy keys, and F13 and above.
- Keypad keys, which GPUI does not distinguish.
- Kitty-only Ctrl chords synced from a legacy source pane to a Kitty target.
- A Kitty-capable future TUI client. The protocol type allows one, but only the
  GPUI client ships.
- Cmd and Super chords reaching the PTY.

## Verification

Every native check below injects keys only after barriers that prove the
relevant state has been reached. For Kitty mode those are:

- the recorder has written the push;
- core has parsed it, which a `CSI ? u` reply on the PTY proves;
- the displayed snapshot reports the flags, which a published-state readout
  proves.

Each check asserts the complete recorder output through a completion boundary,
such as a trailing sentinel key, so trailing duplicate bytes fail it. The Linux
recorder currently reads only the expected byte count (`check-linux-input.ts`);
fix that.

| Concern | Evidence |
| --- | --- |
| Ghostty flags getter, push, pop, set (all three modes), query, RIS, per-screen stack, 8-entry bound, malformed forms | `huterm-ghostty` contract tests |
| Encoder output under effective flags 1, 4, and 5 for representative events | `huterm-ghostty` contract tests. Flag 4 alone is its own case, because Ghostty selects the Kitty encoder for any non-zero flags |
| Encoder ownership: short-buffer retry on `OUT_OF_SPACE`, consecutive events with different text and no stale fields, drop frees handles | `huterm-ghostty` tests, with the test allocator where it applies |
| Bindings and key-set types match the headers | `mise run ghostty:bindings:check`; `key_tests.rs` probes for the new key sets |
| Legacy bytes unchanged | A new compact legacy matrix in `input/tests.rs`: every existing `TerminalKey` x {none, Shift, Ctrl, Alt} x application-cursor on and off, plus Character Ctrl and Meta cases, asserted against today's bytes. Write it first and run it green on the unchanged encoder |
| New legacy F1-F12 and Insert bytes | Exact-byte tests |
| Kitty bytes | Exact-byte `huterm-core` tests at effective flags 1, 4, and 5, and with raw flags that mask to 0, such as 2 and 8. Pinned expectations include: Escape `CSI 27u`; Shift+Tab `CSI 9;2u`; Shift+Enter `CSI 13;2u`; Ctrl+Backspace `CSI 127;5u`; unmodified Enter, Tab, and Backspace legacy; F1-F4 `CSI P`, `CSI Q`, `CSI 13~`, `CSI S`; Ctrl+Left `CSI 1;5D`; Ctrl+a `CSI 97;5u`; Ctrl+Shift+a `CSI 97;6u` and, under flag 4, `CSI 97:65;6u`; Ctrl+Shift+; `CSI 59;6u` and `CSI 59:58;6u`; Meta+a `CSI 97;3u`; Ctrl+1 `CSI 49;5u`; a base-layout alternate under flag 4; Shift+a plain `A` |
| Text, Paste, and Option-composed text untouched in Kitty mode | `huterm-core` tests |
| Reply masking: `CSI ? 31 u` to `5`, `2` to `0`; graphics replies still dropped | Replaces `kitty_keyboard_and_graphics_replies_never_reach_the_pty`. One output chunk holds several queries interleaved with pushes and pops, and the test asserts each captured reply, not only the final state |
| Flags read at dequeue, ordered with output | A runtime test that admits a structured key, changes flags through parsed output before that key is dequeued, and asserts the bytes match the new flags. Synchronize on observable progress, such as a parsed reply, not on sleeps |
| Alternate-screen enter and exit flag isolation | Engine test |
| Child exit writes no key bytes | Existing input-closed tests; add a `Key` or `Character` case if none covers it |
| GPUI translation: F-keys, Insert, Shift kept, identity from the native field and from the fallback, Kitty-only Ctrl chords claimed only under the hint | `keyboard` unit tests |
| Chord replay keeps its identity and stays suppressed before text | `PendingShortcuts` unit tests; the existing macOS smoke replay assertions stay green |
| Synced input: Kitty source to legacy target and the reverse, for ordinary Ctrl and Meta chords and for a Kitty-only Ctrl chord (the documented limitation) | Extend `window_tests/synced_input.rs` |
| Queue accounting counts new strings | GPUI `input_queue` and core `input_bytes` tests |
| Shortcuts, pending replay, Option policy, composition, and IME unchanged | Existing `keyboard` tests; `smoke:macos-input` and `smoke:linux-input` stay green |
| Native end to end | Extend `smoke:macos-input` and `smoke:linux-input`. The recorder pushes `CSI > 5 u`; after the barriers, posted keys (Escape, Ctrl+a, Ctrl+Shift+a, Ctrl+Shift+;, Shift+Enter, Ctrl+1, F1) must produce exact Kitty bytes. Then pop, check legacy bytes, and check F1 legacy. On Linux, check `df -h` first and clean up containers, volumes, and images afterwards |
| Real application, Linux | `kitten show-key -m kitty` inside the Linux smoke, pinned and hash-checked, with its decoded output asserted for a few keys |
| Real application, macOS | Codex computer use, in the background, on a scratch bundle, under the host smoke lock: `kitten show-key -m kitty` and a vim 9.1 legacy spot check |
| Full gate | `mise run check`, `mise run test`, `mise run test:ghostty-releasesafe`, `mise run verify`, `mise run license` |

## Documentation

- `docs/agents/terminal-engines.md`: change the "Kitty keyboard ... remain
  unadvertised" sentence and the `unsupported_reply` description. Document the
  encoder, the `1 | 4` mask, and the per-screen stack.
- `docs/agents/desktop-input.md`: add these rules:
  - the Kitty routing hint;
  - Shift is kept;
  - the F-key and Insert mapping;
  - the native key-identity field;
  - replay identity storage and its fallback.
- `third-party/vendor` README and `sources.json`, and
  `docs/agents/dependencies.md` where it lists vendored GPUI patches: record
  the new patches, why each exists, and when to drop it.
- `README.md` or `docs/configuration.md`: list supported Kitty flags only if
  either already lists terminal protocol support. Check before writing.

## Open questions

None blocking. File the deferred flags as a follow-up issue after the PR
opens.

[issue-85]: https://github.com/jimeh/huterm/issues/85
[issue-78]: https://github.com/jimeh/huterm/issues/78
