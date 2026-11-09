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

/// Session 1 shows workspace 10 (tabs 20 "api" and 21) and workspace 11
/// (tab 22).
fn state() -> HierarchyState {
    let info = |value: u64| WorkspaceInfo {
        id: workspace(value),
        custom_name: None,
        automatic_name: format!("Workspace {value}"),
        pinned: false,
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
                (info(11), vec![tab(22, None)]),
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
    let state = state();
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
    let state = state();
    let mut statuses = Statuses::default();
    let rung = |bells| TerminalStatus { bells, ..idle() };
    let mut window = BellBaselines::default();
    let mut other = BellBaselines::default();
    // Bells rung before the desktop first saw a terminal count as seen;
    // the first read's previous counts are empty, so it starts at zero.
    let previous = statuses
        .apply(read(1, &[(120, rung(0)), (122, rung(2))]))
        .unwrap();
    window.follow(
        &state,
        Some(session()),
        Some(workspace(10)),
        &statuses,
        &previous,
    );
    other.follow(
        &state,
        Some(session()),
        Some(workspace(11)),
        &statuses,
        &previous,
    );
    let dot = |bells: &BellBaselines, statuses: &Statuses, value| {
        workspace_status(&state, workspace(value), statuses, Some(bells))
            .map(|status| status.dot)
    };
    assert_eq!(dot(&window, &statuses, 11), Some(Dot::Bell));

    // A later bell marks the workspace the window does not show, and only
    // in that window.
    let previous = statuses
        .apply(read(2, &[(120, rung(1)), (122, rung(2))]))
        .unwrap();
    window.follow(
        &state,
        Some(session()),
        Some(workspace(10)),
        &statuses,
        &previous,
    );
    other.follow(
        &state,
        Some(session()),
        Some(workspace(11)),
        &statuses,
        &previous,
    );
    assert_eq!(dot(&other, &statuses, 10), Some(Dot::Bell));
    assert_eq!(dot(&window, &statuses, 10), None, "the shown workspace");

    // Showing it sees the bell; leaving keeps it seen.
    let previous = statuses
        .apply(read(3, &[(120, rung(1)), (122, rung(2))]))
        .unwrap();
    other.follow(
        &state,
        Some(session()),
        Some(workspace(10)),
        &statuses,
        &previous,
    );
    other.follow(
        &state,
        Some(session()),
        Some(workspace(11)),
        &statuses,
        &previous,
    );
    assert_eq!(dot(&other, &statuses, 10), None);

    // A terminal that went drops its baseline.
    let previous = statuses.apply(read(4, &[(122, rung(2))])).unwrap();
    other.follow(
        &state,
        Some(session()),
        Some(workspace(11)),
        &statuses,
        &previous,
    );
    assert!(!other.0.contains_key(&terminal(120)));
}

#[test]
fn a_read_older_than_the_one_applied_changes_nothing() {
    let mut statuses = Statuses::default();
    assert!(statuses.apply(read(5, &[(120, running("make"))])).is_some());
    assert!(statuses.apply(read(4, &[(120, idle())])).is_none());
    assert_eq!(
        statuses
            .get(terminal(120))
            .unwrap()
            .metadata
            .foreground_process(),
        Some("make")
    );
    // The same revision again still applies: nothing newer is lost.
    assert!(statuses.apply(read(5, &[(120, idle())])).is_some());
}
