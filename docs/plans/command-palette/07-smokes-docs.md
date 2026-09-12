# Chunk 7: native smokes and user documentation

Part of [the command palette plan](../command-palette.md). Extends the
platform palette smokes to the plan's "Native smoke coverage" list and
finishes the README usage text. Requires chunks 1 to 6 merged.

Owner: Codex implementer. Dependencies: chunk 6. Last chunk before the
final dual review.

## Objective

Make `smoke:macos-palette` and `smoke:linux-palette` prove the new
interaction model end to end with real native input on both PTY engines,
and update the README palette section to describe the shipped behaviour.

Files: `scripts/check-palette.ts`,
`crates/huterm-gpui/src/desktop/palette_smoke.rs` (state string and any new
file-driven commands), `README.md` palette section, and `mise.toml` or
`.github/workflows/ci.yml` only if a new binary or step is needed (it should
not be; the same example binary drives everything).

## Invariants (AGENTS.md)

- Publish file-driven smoke commands through a temporary file and atomic
  rename; the harness already does.
- Native input smoke events must enter through `postEvent:atStart:` on
  macOS and XTest on Linux; reuse the existing `nativeKey`, `typeText`,
  `key`, and `clickOverlay` helpers.
- Use `timeout --foreground` around raw-PTY readers; the recorder shell is
  already set up that way.
- Keep `ci:smoke:build` targets aligned with `ci:smoke:run`.
- CI's macOS smoke job has no `timeout`; its Linux job has no `rg`. Use
  Bun's process timeout and string checks.

## Smoke steps to add

Read the current script first; it already covers opening, modal routing,
pointer isolation, composition, cancellation acknowledgement, rename
through core, explicit requests, stale targets, busy availability, and
deferred failures. Keep those, adjusting state expectations to the new
state string that chunk 6 publishes. Then add, in this order and for both
engines:

1. **Fuzzy search runs.** Type `tfs`, expect `selected=toggle_fullscreen`.
   Escape. (Do not execute fullscreen in the smoke; it is covered by the
   fullscreen smokes.)
2. **All-optional runs from the list.** Type `toggle q`, Enter, expect
   `w0.palette=false` and the quake window to be requested (the state
   string or a new `quake-state` command reports the registry). Then open,
   type `toggle q`, Tab, expect `stage=slots active=profile` and a picker
   row count equal to the configured profiles (the smoke config gets a
   second `[quake.profiles.logs]` table). Escape twice.
3. **Rename in one Enter.** Open, `rename tab`, Enter, type `once`, Enter,
   expect `w0.palette=false`; `core-state` shows `once`. Type a letter and
   check the terminal bytes to prove focus returned.
4. **Rename with Tab to another target.** Open a second tab with the
   `new-tab` shortcut. Open the palette, `rename tab`, Enter, type
   `other`, Tab, expect `active=tab`, Down, Enter; `core-state` shows the
   first tab named `other` and the second unchanged.
5. **Bare keybinding prompts.** Add a `[[keybinding]]` for `cmd-shift-o` /
   `ctrl-shift-o` → `select_tab` in the smoke config. Press it, expect
   `stage=slots requested=true active=tab`; type part of the other tab's
   title, Enter; the active tab changes (state `w0.active_index` or an
   equivalent chunk 6 publishes). Press it again, Escape, expect
   `w0.palette=false` and a shell acknowledgement.
6. **Backspace-on-empty pops and query is retained.** Open, type `ren`,
   Enter, Backspace, expect `stage=search query="ren"`. Escape, reopen
   within the retention window, expect `query="ren"` selected (state
   `query_selected=true`); type `x`, expect `query="x"`.
7. **Mouse.** Open, move the pointer over the third row, expect
   `hover=2 selected=0` (keyboard selection unchanged). Click the row,
   expect execution of that command (choose a query where the third row is
   harmless, for example `hide` variants are not; use `about` is a native
   prompt; prefer `scroll` so rows are scroll commands). Open again, wheel
   over the list, confirm no bytes reached the PTY. Click the scrim, expect
   `w0.palette=false`.
8. **Copy availability.** Open, type `copy`, expect
   `unavailable="no selection"`. Escape. (Creating a selection natively
   is covered by the input smoke; do not duplicate it here.)
9. **Reset name.** After step 3, open, `reset tab`, Enter, expect
   `w0.palette=false`; `core-state` shows no custom name. Open, `reset tab`,
   expect `unavailable="tab has no custom name"`.
10. **Switch to Last Tab.** With two tabs, open, `last tab`, Enter, expect
    the active tab to change; repeat, expect it to change back.
11. **Synchronous failure stays in the status line.** Reuse the existing
    `busy-on` mechanism or the deferred-failure fixture: after dispatch the
    palette is closed and `w0.status` carries the message; the palette does
    not reopen.

Every step that changes core state ends with a `core-state` assertion.
Every step that closes the palette ends with `w0.terminal_focused=true`.

## README

Rewrite the "Command palette" usage paragraph(s): opening, fuzzy search and
ranking, Enter/Tab rule, slots and chips, Backspace and Escape behaviour,
bare keybinding prompting, retained query and its config, pickers for tabs
and quake profiles, and that every palette key is a rebindable command.
Keep it to two or three paragraphs plus the existing tables that chunks 2,
3, and 5 already updated.

## Verification

```sh
mise run smoke:macos-palette      # macOS host
mise run smoke:linux-palette      # Linux host or the Docker runner
mise run check:scripts
mise run lint:docs:files -- README.md
mise run verify
```

Run the smoke at least twice on the host you have to catch timing
flakiness; report both results.

## Self-review checklist

- Every new step asserts observed state after each native event, not just
  that the event was queued.
- No step depends on wall-clock sleeps longer than the existing `waitFor`
  polling.
- The smoke config has the second quake profile and the bare
  `select_tab` binding.
- State-string keys used by the script exist in `palette_smoke.rs`.

Report: files changed, both smoke runs' tails, and deviations with reasons.
