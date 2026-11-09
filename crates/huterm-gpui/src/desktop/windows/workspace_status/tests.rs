use huterm_protocol::{
    ExitStatus, PaneId, PaneInfo, PaneLayout, RuntimeId, SessionInfo, StreamId,
    TabInfo, TerminalFailure, TerminalMetadata, TerminalProgress,
    WorkspaceInfo,
};

use super::*;

const RUNTIME: RuntimeId = RuntimeId::new(3);

fn workspace(value: u64) -> WorkspaceId {
    WorkspaceId::in_runtime(RUNTIME, value)
}

fn terminal(value: u64) -> TerminalId {
    TerminalId::in_runtime(RUNTIME, value)
}

/// A one-pane tab `value` showing terminal `value + 100`, named `name`
/// when it has a custom name.
fn tab(value: u64, name: Option<&str>) -> TabInfo {
    TabInfo {
        id: TabId::in_runtime(RUNTIME, value),
        custom_name: name.map(str::to_owned),
        layout: PaneLayout::single(PaneInfo {
            id: PaneId::in_runtime(RUNTIME, value + 50),
            terminal_id: terminal(value + 100),
            fallback_name: "zsh".into(),
            custom_name: None,
        }),
        zoomed: None,
        input_sync: false,
    }
}

/// Session 1 shows workspace 10 (tabs 20 "api" and 21) and workspace 11,
/// which holds tab 22 once `projected`.
fn state(projected: bool) -> HierarchyState {
    let info = |value: u64| WorkspaceInfo {
        id: workspace(value),
        custom_name: None,
        automatic_name: format!("Workspace {value}"),
        pinned: false,
    };
    let eleven = if projected {
        vec![tab(22, None)]
    } else {
        Vec::new()
    };
    HierarchyState::from_sessions(
        StreamId::new(RUNTIME),
        1,
        [(
            SessionInfo {
                id: session(),
                custom_name: None,
                automatic_name: "Session 1".into(),
            },
            vec![
                (info(10), vec![tab(20, Some("api")), tab(21, None)]),
                (info(11), eleven),
            ],
        )],
        [],
    )
    .unwrap()
}

fn session() -> SessionId {
    SessionId::in_runtime(RUNTIME, 1)
}

fn idle() -> TerminalStatus {
    TerminalStatus::default()
}

fn read(revision: u64, statuses: &[(u64, TerminalStatus)]) -> TerminalStatuses {
    TerminalStatuses {
        revision,
        statuses: statuses
            .iter()
            .map(|(value, status)| (terminal(*value), Arc::new(status.clone())))
            .collect(),
    }
}

fn running(process: &str) -> TerminalStatus {
    TerminalStatus {
        metadata: TerminalMetadata::new(None, Some(process.into())),
        ..idle()
    }
}

#[test]
fn a_workspace_rolls_up_to_its_most_urgent_tab_and_names_tabs_by_state() {
    let state = state(true);
    let mut statuses = Statuses::default();
    let titled = TerminalStatus {
        title: "vim notes.md".into(),
        ..running("vim")
    };
    statuses.apply(read(1, &[(120, running("cargo")), (121, titled)]));
    let status = workspace_status(&state, workspace(10), &statuses, None);
    assert_eq!(
        status,
        Some(RowStatus {
            dot: Dot::Running,
            text: "Running: api, vim notes.md".into(),
        })
    );

    // Progress counts as running; an exit and a failure outrank it.
    let progress = TerminalStatus {
        progress: Some(TerminalProgress::Indeterminate),
        ..idle()
    };
    let exited = TerminalStatus {
        lifecycle: TerminalLifecycle::Exited(ExitStatus {
            code: Some(0),
            success: true,
        }),
        ..idle()
    };
    statuses.apply(read(2, &[(120, progress), (121, exited)]));
    let status = workspace_status(&state, workspace(10), &statuses, None);
    assert_eq!(
        status,
        Some(RowStatus {
            dot: Dot::Exited,
            text: "Exited: zsh · Running: api".into(),
        })
    );
    let failed = TerminalStatus {
        failures: vec![TerminalFailure {
            sequence: 1,
            message: "reader failed".into(),
        }],
        ..idle()
    };
    statuses.apply(read(3, &[(120, failed), (121, idle())]));
    assert_eq!(
        workspace_status(&state, workspace(10), &statuses, None)
            .map(|status| status.dot),
        Some(Dot::Failed)
    );
    // Idle tabs, or none known, show no dot.
    statuses.apply(read(4, &[(120, idle())]));
    assert_eq!(
        workspace_status(&state, workspace(10), &statuses, None),
        None
    );
}

#[test]
fn a_bell_marks_a_workspace_until_the_window_shows_it() {
    let state = state(true);
    let mut statuses = Statuses::default();
    let rung = |bells| TerminalStatus { bells, ..idle() };
    let mut window = BellBaselines::default();
    let mut other = BellBaselines::default();
    let follow = |bells: &mut BellBaselines, statuses: &Statuses, shown| {
        bells.follow(&state, Some(session()), Some(workspace(shown)), statuses);
    };
    let dot = |bells: &BellBaselines, statuses: &Statuses, value| {
        workspace_status(&state, workspace(value), statuses, Some(bells))
            .map(|status| status.dot)
    };
    // Bells rung before the desktop first looked count as seen.
    assert!(statuses.apply(read(1, &[(120, rung(0)), (122, rung(2))])));
    follow(&mut window, &statuses, 10);
    follow(&mut other, &statuses, 11);
    assert_eq!(dot(&window, &statuses, 11), None);

    // A later bell marks the workspace a window does not show, and only in
    // that window.
    assert!(statuses.apply(read(2, &[(120, rung(1)), (122, rung(2))])));
    follow(&mut window, &statuses, 10);
    follow(&mut other, &statuses, 11);
    assert_eq!(dot(&other, &statuses, 10), Some(Dot::Bell));
    assert_eq!(dot(&window, &statuses, 10), None, "the shown workspace");

    // Showing it sees the bell; leaving keeps it seen.
    other.see(&state, workspace(11), &statuses);
    follow(&mut other, &statuses, 10);
    follow(&mut other, &statuses, 11);
    assert_eq!(dot(&other, &statuses, 10), None);

    // A terminal that went drops its count.
    assert!(statuses.apply(read(3, &[(122, rung(2))])));
    follow(&mut other, &statuses, 11);
    assert!(!other.seen.contains_key(&terminal(120)));
}

#[test]
fn a_bell_rung_before_the_projection_lists_its_terminal_still_counts() {
    let mut statuses = Statuses::default();
    let rung = |bells| TerminalStatus { bells, ..idle() };
    let mut window = BellBaselines::default();
    // Workspace 11's tab is not projected yet when its new terminal, 122,
    // rings and the status read lists it.
    let lagging = state(false);
    assert!(statuses.apply(read(1, &[(120, rung(0))])));
    window.follow(&lagging, Some(session()), Some(workspace(10)), &statuses);
    assert!(statuses.apply(read(2, &[(120, rung(0)), (122, rung(1))])));
    window.follow(&lagging, Some(session()), Some(workspace(10)), &statuses);
    // The projection catches up with no further read.
    let state = state(true);
    window.follow(&state, Some(session()), Some(workspace(10)), &statuses);
    assert_eq!(
        workspace_status(&state, workspace(11), &statuses, Some(&window))
            .map(|status| status.dot),
        Some(Dot::Bell)
    );
    // A window that starts following later sees the bell as already rung.
    let mut later = BellBaselines::default();
    later.follow(&state, Some(session()), Some(workspace(10)), &statuses);
    assert_eq!(
        workspace_status(&state, workspace(11), &statuses, Some(&later)),
        None
    );
}

#[test]
fn a_read_older_than_the_one_applied_changes_nothing() {
    let mut statuses = Statuses::default();
    assert!(statuses.apply(read(5, &[(120, running("make"))])));
    assert!(!statuses.apply(read(4, &[(120, idle())])));
    assert_eq!(
        statuses
            .get(terminal(120))
            .unwrap()
            .metadata
            .foreground_process(),
        Some("make")
    );
    // The same revision again still applies: nothing newer is lost.
    assert!(statuses.apply(read(5, &[(120, idle())])));
}
