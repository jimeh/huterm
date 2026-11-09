use super::*;
use crate::{
    PaneId, PaneInfo, PaneNode, SPLIT_TOTAL, SessionId, SplitAxis, SplitChild,
    SplitId, SplitNode, WorkspaceId,
};

fn pane(id: u64, terminal: u64) -> PaneNode {
    PaneNode::Pane(PaneInfo {
        id: PaneId::new(id),
        terminal_id: TerminalId::new(terminal),
        fallback_name: "sh".into(),
        custom_name: None,
    })
}

/// One session, one workspace, and a tab split into panes 4 and 6, which
/// show terminals 5 and 7, under split 8.
fn capture() -> RuntimeCapture {
    let layout = PaneLayout::new(PaneNode::Split(SplitNode {
        id: SplitId::new(8),
        axis: SplitAxis::Columns,
        children: vec![
            SplitChild {
                weight: SPLIT_TOTAL / 2,
                node: pane(4, 5),
            },
            SplitChild {
                weight: SPLIT_TOTAL / 2,
                node: pane(6, 7),
            },
        ],
    }))
    .unwrap();
    RuntimeCapture {
        socket_name: "default".into(),
        next_id: 8,
        session_ordinal: 1,
        sessions: vec![SessionCapture {
            info: SessionInfo {
                id: SessionId::new(1),
                custom_name: None,
                automatic_name: "Session 1".into(),
            },
            workspace_ordinal: 1,
            workspaces: vec![WorkspaceCapture {
                info: WorkspaceInfo {
                    id: WorkspaceId::new(2),
                    custom_name: Some("work".into()),
                    automatic_name: "Workspace 1".into(),
                    pinned: false,
                },
                tabs: vec![TabInfo {
                    id: TabId::new(3),
                    custom_name: None,
                    layout,
                    zoomed: Some(PaneId::new(6)),
                    input_sync: true,
                }],
                directory: None,
            }],
        }],
        terminals: [5, 7]
            .into_iter()
            .map(|id| TerminalCapture {
                id: TerminalId::new(id),
                directories: vec![PathBuf::from("/tmp")],
                launch: TerminalLaunch::DefaultShell,
            })
            .collect(),
    }
}

fn tab(capture: &mut RuntimeCapture) -> &mut TabInfo {
    &mut capture.sessions[0].workspaces[0].tabs[0]
}

#[test]
fn a_consistent_capture_validates() {
    assert_eq!(capture().validate(), Ok(()));
    assert_eq!(capture().tabs().count(), 1);
    assert!(capture().terminal(TerminalId::new(7)).is_some());
}

#[test]
fn identities_must_be_nonzero_unique_across_kinds_and_within_the_watermark() {
    let mut zero = capture();
    zero.sessions[0].info.id = SessionId::new(0);
    assert_eq!(zero.validate(), Err(CaptureError::ZeroId));

    // A workspace reuses the split's numeric identity.
    let mut repeated = capture();
    repeated.sessions[0].workspaces[0].info.id = WorkspaceId::new(8);
    assert_eq!(repeated.validate(), Err(CaptureError::DuplicateId(8)));

    // A tab reuses a terminal's.
    let mut terminal = capture();
    tab(&mut terminal).id = TabId::new(5);
    assert_eq!(terminal.validate(), Err(CaptureError::DuplicateId(5)));

    let mut low = capture();
    low.next_id = 7;
    assert_eq!(low.validate(), Err(CaptureError::AboveWatermark(8)));
}

#[test]
fn every_pane_terminal_has_exactly_one_record_and_no_record_is_orphaned() {
    let mut missing = capture();
    missing.terminals.remove(1);
    assert_eq!(
        missing.validate(),
        Err(CaptureError::MissingTerminal(TerminalId::new(7)))
    );

    let mut orphan = capture();
    orphan.terminals.push(TerminalCapture {
        id: TerminalId::new(2),
        directories: Vec::new(),
        launch: TerminalLaunch::DefaultShell,
    });
    assert_eq!(
        orphan.validate(),
        Err(CaptureError::OrphanTerminal(TerminalId::new(2)))
    );

    let mut twice = capture();
    let again = twice.terminals[0].clone();
    twice.terminals.push(again);
    assert_eq!(twice.validate(), Err(CaptureError::DuplicateId(5)));
}

#[test]
fn zoom_names_and_directories_follow_the_runtime_rules() {
    let mut zoom = capture();
    tab(&mut zoom).zoomed = Some(PaneId::new(9));
    assert_eq!(
        zoom.validate(),
        Err(CaptureError::InvalidZoom(TabId::new(3)))
    );

    let mut single = capture();
    tab(&mut single).layout = PaneLayout::single(match pane(4, 5) {
        PaneNode::Pane(info) => info,
        PaneNode::Split(_) => unreachable!(),
    });
    tab(&mut single).zoomed = Some(PaneId::new(4));
    single.terminals.remove(1);
    assert_eq!(
        single.validate(),
        Err(CaptureError::InvalidZoom(TabId::new(3)))
    );

    let mut blank = capture();
    blank.sessions[0].info.custom_name = Some("  ".into());
    assert_eq!(blank.validate(), Err(CaptureError::BlankName));
    let mut automatic = capture();
    automatic.sessions[0].workspaces[0].info.automatic_name = String::new();
    assert_eq!(automatic.validate(), Err(CaptureError::BlankName));
    let mut pane_name = capture();
    tab(&mut pane_name)
        .layout
        .rename_pane(PaneId::new(4), Some(" ".into()));
    assert_eq!(pane_name.validate(), Err(CaptureError::BlankName));

    let mut many = capture();
    many.terminals[0].directories =
        ["/a", "/b", "/c", "/d"].map(PathBuf::from).to_vec();
    assert_eq!(
        many.validate(),
        Err(CaptureError::InvalidDirectories(TerminalId::new(5)))
    );
    let mut repeated = capture();
    repeated.terminals[0].directories =
        ["/a", "/a"].map(PathBuf::from).to_vec();
    assert_eq!(
        repeated.validate(),
        Err(CaptureError::InvalidDirectories(TerminalId::new(5)))
    );
}
