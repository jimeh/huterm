# Chunk 6: slot model and palette UI

Part of [the command palette plan](../command-palette.md). Implements the
interaction model, argument slots, pickers, mouse, pending-snapshot
handling, retained query, and presentation. Requires chunks 1 to 5 merged.

Owner: the orchestrating session (not delegated). This file exists so the
scope is explicit and so the final dual review can check the result against
it.

## Scope

Files: `crates/huterm-gpui/src/desktop/palette/mod.rs`, new
`crates/huterm-gpui/src/desktop/palette/slots.rs`,
`crates/huterm-gpui/src/ui/text_field.rs`, `crates/huterm-gpui/src/ui/picker.rs`,
the palette-specific parts of `windows.rs` (`open_palette_request`,
`handle_palette_event`, `restore_palette_focus`, the capture-phase mouse
handler clause), and the palette smoke state string in `palette_smoke.rs`.

## Deliverables

- `SlotEditor` in `slots.rs`: the state machine from the plan (`commit`,
  `pop`, `edit`, `advance`, `expand`), `Slot` records with committed,
  prefilled, sole, explicit, and requested flags, human validation
  messages, single-option skip on domain size, pending-intent generation.
  Pure; tests without a window, including a targeted perturbation on one
  transition.
- `CommandPalette::run` bodies for every `palette_*` and `text_*` id, with
  `text_delete_backward` on an empty field routed to `palette_pop`,
  `tab` as `palette_expand` in search and `palette_next_slot` in a slot,
  `open_command_palette` as an in-place no-op.
- Pickers: identity, quake profile, and `select_tab` tab rows through the
  chunk 4 `PickerMatcher`; selection follows identity; MRU order for tabs
  with the active tab last; `cmd-N` key caps where representable.
- Pending snapshot: Enter before the hierarchy lands records an intent
  tagged with the palette generation; any input cancels; snapshot failure
  shows inline.
- Mouse: capture-handler clause removed; overlay blocks motion, buttons,
  and wheel itself; row and picker `on_click`; hover state separate from
  selection; chip click reopens; scrim click cancels; scrollable list.
- Retained query: stored on `WorkspaceView` on cancel only, honoured on
  open within `palette.retain_query_seconds` when `palette.retain_query`.
- Presentation per the plan's "Presentation" section, with theme
  invalidation on reload.
- Requested stages (from chunk 5's `requested` flag): back and pop from the
  first slot close.
- Availability-driven badges: `⇥ options` when
  `spec.missing_prompted(&empty)` is empty and the command has arguments;
  `↩ prompts` otherwise.
- Smoke state string extended with `stage`, `requested`, active slot name,
  chip summary, and pending flag, keeping the existing keys so the current
  smoke steps still parse until chunk 7 rewrites them.

## Tests

Per the plan's "Pure and focused tests" list under Slots, Pending snapshot,
Picker, Retained query, and Errors. Text field: Backspace at the start of
non-empty text leaves it unchanged; word and line deletion.

## Verification

`cargo test --locked -p huterm-gpui`, `mise run check`, the existing palette
smoke on macOS, and a manual pass at ordinary and half-height quake window
sizes with screenshots kept for the final review.
