# Unified drags

Status: in progress. Refs [issue #27][issue-27]. Second of three stacked
changes: [retained workspace views](retained-workspace-views.md) (R1), this
one (R2), then D, which adds drops on sidebar workspace rows,
spring-loaded rows, and workspace tear-off.

## Decision

The maintainer asked on 2026-11-09 to unify the drag behaviour and code.
Before this change the desktop client had four drag implementations:

| Drag | Record | Own pieces |
| --- | --- | --- |
| Tab reorder, with pane mode that nests a tab beside a pane | `TabReorder` in `windows.rs` | axis projection, pane mode, its own capture branch |
| Pane drag, from a title bar or the modifier grab | `PaneDrag` in `pane_drag.rs` | target resolver, spring-load, autoscroll |
| Sidebar row drags | `RowDrag<K>` in `sidebar_rows.rs` | threshold, row slots, list edge scroll |
| Cross-window drags | `Payload`, `Drag`, `Incoming`, `Transfer` in `cross_window.rs` | a second target resolver and operation mapping |

R2 replaces them with one model and keeps every user-visible behaviour.
It adds no drop target, no chip on row drags, and no cross-window row drag;
D adds those with mockups.

Out of scope: divider, sidebar-resize, tab-column-resize, and scrollbar
drags, and OS file drops. They keep their own branches in the capture
listeners, ahead of and behind the unified gesture as today.

## Shape

- `windows/drag.rs` holds the pure model:
  - `Payload`: `Tab`, `Pane`, `Workspace`, or `Session`.
  - `Press`: origin, pointer, and the shared threshold.
  - `DropTarget`: a pane zone, a tab-bar slot, the title row, a spring
    key, or a row slot. D adds a workspace row and a new window as
    variants.
  - `SpringKey` (`Tab` today; D adds `Workspace`) and `SpringLoad` over it,
    plus `SpringBack`.
  - `ListScroll`: the sidebar list's edge autoscroll.
  - `Operation` and `operation`, the one table from a payload, a target,
    and the destination to a Mux operation, or `None` for a refusal.
- `Gesture` is the one source-side record (`WorkspaceView::drag`). It holds
  the payload, the press, the resolved target, the hover state
  (spring-load, spring-back, tab-strip autoscroll), whether the pointer
  left the window, and the macOS remap flag. `cross_window::Incoming`, the
  destination preview of another window's gesture, shares the hover state.
- One resolver (`WorkspaceView::resolve_drop`) returns a `DropTarget` for a
  payload at a pointer, as the source or as a destination. The per-payload
  rules that exist today stay where they are decided: a lone pane offers
  no slots, a tab reorder projects onto the bar outside pane mode, and
  centres never swap across tabs or windows.
- The capture listeners, Escape, blur, opening menus, switches, and the
  drag cursor go through `update_drag`, `finish_drag`, and `cancel_drag`.
  Escape keeps its per-payload rule: a tab press cancels before the
  threshold, every other payload only once it drags.
- Commits: local operations use the existing paths (`commit_pane_move`,
  the reorder worker, `move_workspace`, `move_session`); cross-window
  operations go through `dispatch_transfer` and `tear_off` as before, now
  with the same `Operation` type.

## Stages

1. Introduce the model, and move the sidebar row drags onto it.
2. Move pane drags onto `Gesture` and the shared resolver and table.
3. Move tab reorder and pane mode.
4. Move cross-window previews and transfers onto the shared payload,
   resolver, table, and hover state.
5. Route capture, cancellation, and the cursor through the gesture; update
   the agent guides.

Each stage keeps the existing tests green.

## Verification

- The existing drag tests are the safety net: `window_tests/pane_drags.rs`,
  `pane_grabs.rs`, `cross_window.rs` and `cross_window/drag_preview.rs`,
  the row drag tests in `workspace_panel.rs` and `session_control.rs`, and
  the tab reorder tests. Field renames in them are mechanical; no assertion
  changes meaning.
- New unit tests cover the operation table, the shared threshold, the
  row slot resolver, and the generalised spring key.
- `mise run check`, `mise run test`, `smoke:macos-input`, and
  `smoke:linux-input` (Docker).

## Behaviour changes

None intended. Any small inconsistency the unification removes is listed in
the pull request.

[issue-27]: https://github.com/jimeh/huterm/issues/27
