//! The hierarchy invariants that keep every runtime state encodable:
//! [`MAX_NAME_BYTES`] and [`MAX_HIERARCHY_ITEMS`].

use super::*;
use crate::mux::tests::command;
use huterm_protocol::{Arrangement, SPLIT_TOTAL, SplitAxis};

fn name(bytes: usize) -> String {
    "n".repeat(bytes)
}

#[test]
fn custom_names_longer_than_the_name_limit_are_invalid_everywhere() {
    let mut mux = Mux::default();
    let at_limit = name(MAX_NAME_BYTES);
    let over = name(MAX_NAME_BYTES + 1);
    assert!(matches!(
        mux.create_session(Some(&over)),
        Err(MuxError::InvalidName)
    ));
    let session = mux.create_session(Some(&at_limit)).unwrap();
    assert!(matches!(
        mux.create_workspace(session, Some(&over)),
        Err(MuxError::InvalidName)
    ));
    let workspace = mux.create_workspace(session, Some(&at_limit)).unwrap();
    let tab = mux
        .open_test_tab(workspace, &command("read value"))
        .unwrap()
        .tab;
    let pane = tab.first_pane().id;
    let revision = mux.revision;
    assert!(matches!(
        mux.rename_session(session, Some(&over)),
        Err(MuxError::InvalidName)
    ));
    assert!(matches!(
        mux.rename_workspace(workspace, Some(&over)),
        Err(MuxError::InvalidName)
    ));
    assert!(matches!(
        mux.rename_tab(tab.id, Some(&over)),
        Err(MuxError::InvalidName)
    ));
    assert!(matches!(
        mux.rename_pane(pane, Some(&over)),
        Err(MuxError::InvalidName)
    ));
    assert_eq!(mux.revision, revision);
    mux.rename_tab(tab.id, Some(&at_limit)).unwrap();
    mux.rename_pane(pane, Some(&at_limit)).unwrap();
    assert_eq!(mux.tab(tab.id).unwrap().custom_name(), Some(&*at_limit));
    assert_eq!(
        mux.tab(tab.id).unwrap().first_pane().custom_name.as_deref(),
        Some(&*at_limit)
    );
    mux.shutdown().unwrap();
}

#[test]
fn fallback_names_truncate_at_a_character_boundary_within_the_limit() {
    // Two bytes per character, so the limit falls inside the 129th.
    let long = "é".repeat(MAX_NAME_BYTES);
    let even = TerminalCommand {
        program: format!("/usr/bin/{long}").into(),
        ..command("true")
    };
    assert_eq!(fallback_name(&even), "é".repeat(MAX_NAME_BYTES / 2));
    let odd = TerminalCommand {
        program: format!("/usr/bin/x{long}").into(),
        ..command("true")
    };
    let truncated = fallback_name(&odd);
    assert_eq!(truncated.len(), MAX_NAME_BYTES - 1);
    assert!(truncated.starts_with('x'));
    let short = TerminalCommand {
        program: "/bin/zsh".into(),
        ..command("true")
    };
    assert_eq!(fallback_name(&short), "zsh");
}

/// A Mux holding one session, one workspace, and one tab of three panes
/// side by side in one split, filled with attachments until `free` items
/// remain below [`MAX_HIERARCHY_ITEMS`].
struct Full {
    mux: Mux,
    session: SessionId,
    workspace: WorkspaceId,
    tab: TabId,
    panes: [PaneId; 3],
}

impl Full {
    fn new(free: usize) -> Self {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let script = command("read value");
        let tab = mux.open_test_tab(workspace, &script).unwrap().tab;
        let first = tab.first_pane().id;
        let second = mux
            .split_pane(
                workspace,
                tab.id,
                first,
                SplitDirection::Right,
                &script,
            )
            .unwrap()
            .pane
            .id;
        let third = mux
            .split_pane(
                workspace,
                tab.id,
                second,
                SplitDirection::Right,
                &script,
            )
            .unwrap()
            .pane
            .id;
        // A session, a workspace, a tab, three panes, and one split.
        assert_eq!(mux.hierarchy_items(), 7);
        while mux.hierarchy_items() < MAX_HIERARCHY_ITEMS - free {
            mux.attach_session(session).unwrap();
        }
        Self {
            mux,
            session,
            workspace,
            tab: tab.id,
            panes: [first, second, third],
        }
    }

    fn layout(&self) -> PaneLayout {
        self.mux.tab(self.tab).unwrap().layout().clone()
    }

    /// The first pane beside a row stack of the other two: one more split.
    fn nested(&self) -> Arrangement {
        let [first, second, third] = self.panes;
        let half = SPLIT_TOTAL / 2;
        Arrangement::Split {
            axis: SplitAxis::Columns,
            children: vec![
                (half, Arrangement::Pane(first)),
                (
                    half,
                    Arrangement::Split {
                        axis: SplitAxis::Rows,
                        children: vec![
                            (half, Arrangement::Pane(second)),
                            (half, Arrangement::Pane(third)),
                        ],
                    },
                ),
            ],
        }
    }

    /// Asserts that `refused` fails with `LimitExceeded` and changes
    /// nothing: no revision, structure, terminal, or hierarchy event.
    fn refuses<T: std::fmt::Debug>(
        &mut self,
        refused: impl FnOnce(&mut Mux) -> Result<T, MuxError>,
    ) {
        let (before, subscription) = self.mux.subscribe_hierarchy();
        let revision = self.mux.revision;
        let terminals = self.mux.terminal_count();
        let next_id = self.mux.next_id;
        let result = refused(&mut self.mux);
        assert!(matches!(result, Err(MuxError::LimitExceeded)), "{result:?}");
        assert_eq!(self.mux.revision, revision);
        assert_eq!(self.mux.next_id, next_id);
        assert_eq!(self.mux.terminal_count(), terminals);
        assert_eq!(self.mux.subscribe_hierarchy().0.digest(), before.digest());
        assert!(matches!(
            subscription.try_recv(),
            Err(HierarchyRecvError::Empty)
        ));
    }
}

#[test]
fn creations_refuse_to_pass_the_hierarchy_item_limit_and_may_reach_it() {
    let mut full = Full::new(1);
    let (session, workspace, tab) = (full.session, full.workspace, full.tab);
    let [first, _, third] = full.panes;
    let script = command("read value");
    // A tab and a split that forms a split each need two items.
    full.refuses(|mux| mux.open_tab(workspace, &script));
    full.refuses(|mux| {
        mux.split_pane(workspace, tab, first, SplitDirection::Down, &script)
    });
    // One item remains, so a creation of one reaches the limit exactly.
    full.mux.create_session(None).unwrap();
    assert_eq!(full.mux.hierarchy_items(), MAX_HIERARCHY_ITEMS);
    full.refuses(|mux| mux.create_session(None));
    full.refuses(|mux| mux.create_workspace(session, None));
    full.refuses(|mux| mux.attach_session(session));
    // A split beside a sibling on its axis needs one item.
    full.refuses(|mux| {
        mux.split_pane(workspace, tab, third, SplitDirection::Right, &script)
    });
    // Freeing an item makes room again.
    let attachment = *full.mux.attachments.keys().next().unwrap();
    full.mux.detach_session(attachment).unwrap();
    full.mux
        .split_pane(workspace, tab, third, SplitDirection::Right, &script)
        .unwrap();
    assert_eq!(full.mux.hierarchy_items(), MAX_HIERARCHY_ITEMS);
    full.mux.shutdown().unwrap();
}

#[test]
fn rearrangements_moves_and_restores_refuse_to_grow_past_the_limit() {
    let mut full = Full::new(1);
    let (workspace, tab) = (full.workspace, full.tab);
    let [first, _, third] = full.panes;
    let arrangement = full.nested();
    let expected = full.layout();
    // Arranging into one more split takes the last item.
    let change = full
        .mux
        .arrange_panes(workspace, tab, &expected, &arrangement)
        .unwrap();
    assert_eq!(full.mux.hierarchy_items(), MAX_HIERARCHY_ITEMS);
    // Undo shrinks, so it is admitted at the limit, and frees the item.
    let undone = full
        .mux
        .restore_tabs(&change, RestoreDirection::Undo)
        .unwrap();
    assert!(!undone.is_empty());
    full.mux.attach_session(full.session).unwrap();
    assert_eq!(full.mux.hierarchy_items(), MAX_HIERARCHY_ITEMS);
    full.refuses(|mux| mux.restore_tabs(&change, RestoreDirection::Redo));
    let expected = full.layout();
    full.refuses(|mux| {
        mux.arrange_panes(workspace, tab, &expected, &arrangement)
    });
    // Moving the third pane below the first forms a split while its old
    // split keeps two children.
    full.refuses(|mux| {
        mux.move_pane(
            PaneRef {
                workspace,
                tab,
                pane: third,
            },
            MoveDestination::Beside(Beside {
                target: PaneRef {
                    workspace,
                    tab,
                    pane: first,
                },
                side: SplitDirection::Down,
            }),
        )
    });
    // Extracting a pane into a new tab adds the tab.
    full.refuses(|mux| {
        mux.move_pane(
            PaneRef {
                workspace,
                tab,
                pane: third,
            },
            MoveDestination::NewTab {
                workspace,
                before: None,
            },
        )
    });
    full.mux.shutdown().unwrap();
}
