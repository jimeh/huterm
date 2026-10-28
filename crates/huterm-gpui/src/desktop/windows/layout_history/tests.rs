use super::*;
use huterm_core::{Mux, MuxError};
use huterm_protocol::{
    CellSize, GridSize, SplitDirection, TabId, TerminalCommand, WorkspaceId,
};

use RestoreDirection::{Redo, Undo};

fn idle() -> TerminalCommand {
    TerminalCommand {
        program: "/bin/sh".into(),
        arguments: vec!["-c".into(), "read value".into()],
        working_directory: std::env::current_dir().unwrap(),
        environment: Vec::new(),
        grid_size: GridSize::clamped(40, 8),
        cell_size: CellSize {
            width: 8,
            height: 16,
        },
        presentation: huterm_protocol::TerminalPresentation::default(),
        scrollback: huterm_protocol::ScrollbackLimit::DEFAULT,
        scrollback_compression: false,
    }
}

fn entry(label: &'static str, change: LayoutChange, seq: u64) -> HistoryEntry {
    HistoryEntry {
        label: label.into(),
        focus: None,
        change,
        seq,
    }
}

/// A tab of two panes.
fn split_tab(mux: &mut Mux, workspace: WorkspaceId) -> (TabId, PaneId, PaneId) {
    let tab = mux.open_tab(workspace, &idle()).unwrap().tab;
    let left = tab.first_pane().id;
    let right = mux
        .split_pane(workspace, tab.id, left, SplitDirection::Right, &idle())
        .unwrap()
        .pane
        .id;
    (tab.id, left, right)
}

fn projection(mux: &mut Mux) -> HierarchyState {
    mux.subscribe_hierarchy().0
}

fn label(history: &LayoutHistory, direction: RestoreDirection) -> Option<&str> {
    history.peek(direction).map(|entry| entry.label.as_ref())
}

#[test]
fn a_new_operation_replaces_the_undo_entry_and_clears_redo() {
    let mut history = LayoutHistory::default();
    assert!(history.record(entry("Swap Panes", LayoutChange::default(), 1)));
    let undone = history.take(Undo).unwrap();
    history.finish(Undo, undone, 2);
    assert_eq!(label(&history, Undo), None);
    assert_eq!(label(&history, Redo), Some("Swap Panes"));
    assert!(history.record(entry("Move Pane", LayoutChange::default(), 3)));
    assert_eq!(label(&history, Undo), Some("Move Pane"));
    assert_eq!(label(&history, Redo), None);
    // A redo files its entry back as the undo entry.
    let undone = history.take(Undo).unwrap();
    history.finish(Undo, undone, 4);
    let redone = history.take(Redo).unwrap();
    assert_eq!((label(&history, Undo), label(&history, Redo)), (None, None));
    history.finish(Redo, redone, 5);
    assert_eq!(label(&history, Undo), Some("Move Pane"));
}

#[test]
fn an_operation_that_committed_before_the_newest_entry_is_ignored() {
    let mut history = LayoutHistory::default();
    assert!(history.record(entry("Move Pane", LayoutChange::default(), 4)));
    assert!(!history.record(entry("Balance", LayoutChange::default(), 3)));
    assert_eq!(label(&history, Undo), Some("Move Pane"));
    // An undo filed at a later sequence also outranks it.
    let undone = history.take(Undo).unwrap();
    history.finish(Undo, undone, 6);
    assert!(!history.record(entry("Balance", LayoutChange::default(), 5)));
    assert_eq!(label(&history, Redo), Some("Move Pane"));
    assert!(history.record(entry("Balance", LayoutChange::default(), 7)));
    assert_eq!(label(&history, Undo), Some("Balance"));
    // The newest sequence outlives its entry: with sequence 7 taken for an
    // undo still in flight, an older completion is still ignored.
    let _in_flight = history.take(Undo).unwrap();
    assert!(!history.record(entry("Move Pane", LayoutChange::default(), 6)));
    assert_eq!(label(&history, Undo), None);
}

#[test]
fn pruning_drops_only_entries_the_runtime_would_refuse() -> Result<(), MuxError>
{
    let mut mux = Mux::default();
    let session = mux.create_session(None)?;
    let workspace = mux.create_workspace(session, None)?;
    let (one, a, b) = split_tab(&mut mux, workspace);
    let (two, c, d) = split_tab(&mut mux, workspace);
    let mut history = LayoutHistory::default();
    let change = mux.swap_panes(workspace, one, a, b)?;
    history.record(entry("Swap Panes", change, 1));
    let swapped = mux.swap_panes(workspace, two, c, d)?;
    mux.restore_tabs(&swapped, Undo)?;
    history.finish(Undo, entry("Swap Panes", swapped, 1), 2);

    // A rename and a zoom change no structure, so both entries stay.
    mux.rename_tab(one, Some("renamed"))?;
    mux.set_pane_zoom(workspace, one, Some(a))?;
    assert!(!history.prune(&projection(&mut mux)));
    assert!(history.peek(Undo).is_some() && history.peek(Redo).is_some());
    // A split changes tab one's shape: its undo entry goes, and the redo
    // entry on tab two stays.
    mux.split_pane(workspace, one, a, SplitDirection::Down, &idle())?;
    let state = projection(&mut mux);
    assert!(!history.peek(Undo).unwrap().restorable(Undo, &state));
    assert!(history.prune(&state));
    assert!(history.peek(Undo).is_none());
    assert!(history.peek(Redo).unwrap().restorable(Redo, &state));
    // Moving the workspace into another session changes no tab, but the
    // runtime refuses a restore across sessions, so the entry goes.
    let other = mux.create_session(None)?;
    mux.move_workspace(session, workspace, other, None)?;
    assert!(history.prune(&projection(&mut mux)));
    assert!(history.peek(Redo).is_none());
    mux.shutdown()?;
    Ok(())
}
