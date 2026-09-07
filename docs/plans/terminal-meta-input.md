# Terminal Meta input and menu shortcut verification

Status: proposed implementation plan. Left/right Option support is explicitly
deferred by agreement. No implementation is included in this change.

Deliver [#55: Option/Alt-as-Meta][issue-55] with
[#57: native menu shortcut verification][issue-57] in one keyboard-focused PR.
Unbound Alt chords should work in Emacs, tmux, and readline; Huterm bindings
should consume their chords and display the correct macOS menu shortcuts.

## Scope and defaults

- Add `terminal.macos_option_as_alt` with string values `off` and `both`.
  Default to `off` to preserve existing macOS typing. Document `both` as the
  setting for users who want Option chords sent as Meta.
- On Linux, unbound Alt chords use Meta without requiring this macOS setting.
- When Meta applies, encode one ESC prefix followed by the intended character,
  including Shift and supported Control combinations. Preserve existing
  special-key encoding rather than introducing a new keyboard protocol.
- With macOS Meta disabled, preserve Option text and dead-key composition.
  Keep the existing Alt behavior for special keys such as arrows; document
  this distinction from printable character composition.
- Apply successful reloads to subsequent keystrokes in existing and new views.
  Already queued input retains its interpretation. Invalid reloads preserve
  the working configuration and report the error.
- Preserve effective keybinding precedence, conditional matching, chord
  reservation, and unbinding. A consumed shortcut never reaches the shell.

Left/right Option, native modifier tracking, Kitty keyboard protocol,
`modifyOtherKeys`, fullscreen, and global hotkeys are outside this delivery.
Do not modify personal configuration as part of the repository implementation.

## Current implementation and constraints

`TerminalView::handle_keystroke` in `crates/huterm-gpui/src/desktop.rs`
forwards printable `key_char` as `TerminalInput::Text`. Control chords also
become text without retaining Alt. `crates/huterm-core/src/input.rs` writes
text unchanged and already ESC-prefixes supported Alt special keys.

The pinned GPUI 0.2.2 macOS converter exposes one Alt flag. Its `key_char`
includes Option composition, while `key` represents the base key. Shifted
ASCII letters retain Shift; shifted punctuation is already resolved into
`key` and clears Shift. Translation must respect this normalization rather
than applying a US keyboard punctuation table.

The existing keymap compiler and real GPUI matcher tests cover shortcut
reservation and menu-facing binding order. Native menu installation is shared
by startup and reload. The existing AppKit bridge and macOS quit smoke provide
the integration pattern for reading real menu items.

## Implementation sequence

1. **Define and test character translation.** Extract a focused translation
   function from the production handler with explicit platform and Option
   policy inputs. Add failing cases for Option-r, Alt-r, Shift letters and
   punctuation, and Control+Alt. Preserve ordinary text, special keys, and
   unsupported-key handling. Inspect the pinned Linux converter and macOS
   composition event path before assuming every input arrives as one keydown.

2. **Carry Meta intent through the shared input path.** Keep character Meta
   encoding in core, using a dependency-neutral structured input representation.
   Prefer a small explicit character-input variant carrying text and Meta
   intent; leave paste and composed text semantics intact. Update desktop and
   runtime queue byte accounting for payload size and the ESC prefix. Enqueue
   prefix and character as one input so other input cannot interleave them.
   Cover exact bytes and absence of double prefixes in focused core tests.

3. **Wire configuration and reload.** Add the validated enum and default,
   propagate it to retained terminal views through the existing reload path,
   and document the setting and platform behavior in the README and starter
   configuration. Reject `left`, `right`, and unknown values clearly. Test
   omission, both supported values, invalid reload, and a queued input followed
   by reload and a later keystroke.

4. **Verify shortcut interaction through production dispatch.** Extend the
   real matcher coverage for a bound Alt chord, an unbound chord, and a
   conditional binding whose predicate is false. Prove that matching commands
   consume the input and unmatched chords follow the Option policy. Exercise
   the shared encoder through a raw PTY fixture for both engines, asserting
   exact received bytes rather than relying only on displayed characters.

5. **Add the AppKit menu smoke.** Keep unsafe menu inspection in
   `native_quit.rs`, with a safe entry point for the smoke. Boot a real GPUI
   application using the production keymap and menu installation path. Assert
   that rebinding Reload Configuration to `cmd-r` yields the actual AppKit
   key equivalent `r` and Command modifier, and that an untouched default still
   displays correctly. Rebind and reinstall through the reload path, then
   assert the new equivalent. Include a conditional binding to preserve the
   ordering case described in #57. Expose `smoke:macos-menus` through Mise and
   macOS CI, using the SDK build wrapper and an explicit Linux skip. Reverse
   binding order once to prove failure at the shortcut assertion, then restore
   it and rerun successfully.

6. **Complete native verification and review.** Run the checks below, inspect
   the final diff, and record completed evidence separately from outstanding
   manual checks in the PR. Keep #57 in this PR unless its harness exposes an
   unrelated blocker; split it out if necessary to avoid delaying usable Meta
   input. Fullscreen #40 and quake #41 follow as separate deliveries.

## Verification and acceptance

Use focused Cargo tests while iterating, through the repository's native build
wrapper after Ghostty preparation. Confirm new tests are collected and fail at
their intended assertions before the relevant fix where practical.

- Translation and encoder tests cover Option off/both, Linux Alt, Shift,
  digits, punctuation, Control+Alt, Unicode text, existing special keys, and
  paste remaining unchanged. Queue checks cover the new representation's
  payload accounting and ordering.
- Configuration and matcher tests cover reload timing, invalid settings,
  consumed shortcuts, conditional fallthrough, and explicit unbinding.
- Both-engine PTY fixtures prove identical Meta bytes. Native macOS checks
  use Emacs or a byte-reporting program to verify Option-r, shifted keys,
  arrows, a bound chord, and toggling the policy by reload. With Meta off,
  verify Option-r composition and Option-e followed by e on a suitable layout.
  Record the keyboard layout and distinguish synthetic input evidence from
  physical typing and composition evidence.
- Verify Alt-r and representative modified input on Linux. Headless matcher
  tests alone do not prove native keyboard conversion or composition.
- Run `mise run smoke:macos-menus` on macOS, `mise run check:scripts` for smoke
  tooling changes, and `mise run verify` before implementation handoff. Run
  the existing macOS quit smoke if shared native bridge changes affect it.
  CI supplies platform evidence unavailable locally; report any remaining
  native checks explicitly rather than treating a green unit suite as proof.

Success means unbound Meta input works with either engine, ordinary macOS
composition still works with the policy off, bound shortcuts do not leak,
reload changes only later input, and actual AppKit shortcuts track the keymap.

## Open questions

No product decisions block implementation. Native composition behavior through
the pinned GPUI event path remains an implementation verification question.
Resolve it early; if preserving composition needs a broader platform change,
bring back the evidence and scope tradeoff before expanding this PR.

[issue-55]: https://github.com/jimeh/huterm/issues/55
[issue-57]: https://github.com/jimeh/huterm/issues/57
