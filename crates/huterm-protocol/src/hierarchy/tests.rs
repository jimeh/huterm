use super::*;

const RUNTIME: RuntimeId = RuntimeId::new(7);
const STREAM: StreamId = StreamId::new(RUNTIME);

fn session_id(value: u64) -> SessionId {
    SessionId::in_runtime(RUNTIME, value)
}

fn workspace_id(value: u64) -> WorkspaceId {
    WorkspaceId::in_runtime(RUNTIME, value)
}

fn tab_id(value: u64) -> TabId {
    TabId::in_runtime(RUNTIME, value)
}

fn session(value: u64) -> SessionInfo {
    SessionInfo {
        id: session_id(value),
        custom_name: None,
        automatic_name: format!("Session {value}"),
    }
}

fn workspace(value: u64) -> WorkspaceInfo {
    WorkspaceInfo {
        id: workspace_id(value),
        custom_name: None,
        automatic_name: format!("Workspace {value}"),
    }
}

fn tab(value: u64) -> TabInfo {
    TabInfo {
        id: tab_id(value),
        pane_id: PaneId::new(value + 100),
        terminal_id: TerminalId::new(value + 200),
        custom_name: None,
        fallback_name: "sh".into(),
    }
}

/// Two sessions; session 1 holds workspaces 10 and 11, workspace 10
/// holds tabs 20 and 21, and workspace 11 holds tab 22.
fn seeded(seq: u64) -> HierarchyState {
    HierarchyState::from_sessions(
        STREAM,
        seq,
        [
            (
                session(1),
                vec![
                    (workspace(10), vec![tab(20), tab(21)]),
                    (workspace(11), vec![tab(22)]),
                ],
            ),
            (session(2), vec![]),
        ],
    )
    .unwrap()
}

fn touched(names: bool, workspaces: &[u64], tabs: &[u64]) -> ApplyOutcome {
    ApplyOutcome::Applied(Touched {
        everything: false,
        names,
        workspaces: workspaces.iter().copied().map(workspace_id).collect(),
        tabs: tabs.iter().copied().map(tab_id).collect(),
    })
}

fn next(state: &HierarchyState, event: HierarchyEvent) -> HierarchyEnvelope {
    HierarchyEnvelope {
        stream: STREAM,
        seq: state.seq() + 1,
        event,
    }
}

fn apply(state: &mut HierarchyState, event: HierarchyEvent) -> ApplyOutcome {
    let envelope = next(state, event);
    state.apply(envelope)
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "walk every event kind through one evolving hierarchy"
)]
fn events_apply_in_order_and_summarize_what_they_touched() {
    let mut state = HierarchyState::new(STREAM);
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::SessionCreated {
                session: session(1),
                index: 0,
            },
        ),
        touched(true, &[], &[])
    );
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::SessionCreated {
                session: session(2),
                index: 0,
            },
        ),
        touched(true, &[], &[])
    );
    assert_eq!(state.sessions(), [session_id(2), session_id(1)]);
    for (value, index) in [(10, 0), (11, 0)] {
        assert_eq!(
            apply(
                &mut state,
                HierarchyEvent::WorkspaceCreated {
                    session: session_id(1),
                    workspace: workspace(value),
                    index,
                },
            ),
            touched(true, &[value], &[])
        );
    }
    assert_eq!(
        state.session_workspaces(session_id(1)).unwrap(),
        [workspace_id(11), workspace_id(10)]
    );
    for (value, index) in [(20, 0), (21, 1), (22, 1)] {
        assert_eq!(
            apply(
                &mut state,
                HierarchyEvent::TabOpened {
                    workspace: workspace_id(10),
                    tab: tab(value),
                    index,
                },
            ),
            touched(false, &[10], &[value])
        );
    }
    assert_eq!(
        state.workspace_tabs(workspace_id(10)).unwrap(),
        [tab_id(20), tab_id(22), tab_id(21)]
    );
    assert_eq!(state.tab(tab_id(22)).unwrap().display_name("vim"), "vim");

    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::TabRenamed {
                tab: tab_id(22),
                custom_name: Some("logs".into()),
            },
        ),
        touched(false, &[], &[22])
    );
    assert_eq!(state.tab(tab_id(22)).unwrap().display_name("vim"), "logs");
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::WorkspaceRenamed {
                workspace: workspace_id(10),
                custom_name: Some("work".into()),
            },
        ),
        touched(true, &[], &[])
    );
    assert_eq!(
        state.workspace(workspace_id(10)).unwrap().display_name(),
        "work"
    );
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::SessionRenamed {
                session: session_id(2),
                custom_name: Some("spare".into()),
            },
        ),
        touched(true, &[], &[])
    );
    // An unchanged name still consumes its sequence but touches nothing.
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::SessionRenamed {
                session: session_id(2),
                custom_name: Some("spare".into()),
            },
        ),
        touched(false, &[], &[])
    );
    assert_eq!(
        state.session(session_id(2)).unwrap().display_name(),
        "spare"
    );

    // Same-workspace reorder to the final position.
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::TabMoved {
                tab: tab_id(20),
                workspace: workspace_id(10),
                index: 2,
            },
        ),
        touched(false, &[10], &[20])
    );
    assert_eq!(
        state.workspace_tabs(workspace_id(10)).unwrap(),
        [tab_id(22), tab_id(21), tab_id(20)]
    );
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::TabMoved {
                tab: tab_id(21),
                workspace: workspace_id(11),
                index: 0,
            },
        ),
        touched(false, &[10, 11], &[21])
    );
    assert_eq!(state.tab_workspace(tab_id(21)), Some(workspace_id(11)));
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::WorkspaceMoved {
                workspace: workspace_id(11),
                session: session_id(2),
                index: 0,
            },
        ),
        touched(true, &[11], &[])
    );
    assert_eq!(
        state.workspace_session(workspace_id(11)),
        Some(session_id(2))
    );
    assert_eq!(
        state.session_workspaces(session_id(1)).unwrap(),
        [workspace_id(10)]
    );
    assert_eq!(
        apply(&mut state, HierarchyEvent::TabClosed { tab: tab_id(22) },),
        touched(false, &[10], &[22])
    );
    assert!(state.tab(tab_id(22)).is_none());
    assert_eq!(
        state.workspace_tabs(workspace_id(10)).unwrap(),
        [tab_id(20)]
    );

    // Closing a workspace cascades to its tabs.
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::WorkspaceClosed {
                workspace: workspace_id(11),
            },
        ),
        touched(true, &[11], &[21])
    );
    assert!(state.workspace(workspace_id(11)).is_none());
    assert!(state.tab(tab_id(21)).is_none());
    assert_eq!(state.session_workspaces(session_id(2)).unwrap(), []);

    // Closing a session cascades to its workspaces and their tabs.
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::SessionClosed {
                session: session_id(1),
            },
        ),
        touched(true, &[10], &[20])
    );
    assert_eq!(state.sessions(), [session_id(2)]);
    assert!(state.workspace(workspace_id(10)).is_none());
    assert!(state.tab(tab_id(20)).is_none());

    let seq = state.seq();
    assert_eq!(
        apply(&mut state, HierarchyEvent::Reset),
        ApplyOutcome::Applied(Touched::everything())
    );
    assert_eq!(state.seq(), seq + 1);
    let mut empty = HierarchyState::new(STREAM);
    empty.seq = seq + 1;
    assert_eq!(state, empty);
}

#[test]
fn stale_and_duplicate_sequences_change_nothing() {
    let mut state = seeded(5);
    let rename = HierarchyEnvelope {
        stream: STREAM,
        seq: 6,
        event: HierarchyEvent::TabRenamed {
            tab: tab_id(20),
            custom_name: Some("first".into()),
        },
    };
    assert!(matches!(
        state.apply(rename.clone()),
        ApplyOutcome::Applied(_)
    ));
    let applied = state.clone();
    assert_eq!(state.apply(rename), ApplyOutcome::Stale);
    for seq in [0, 5] {
        let replay = HierarchyEnvelope {
            stream: STREAM,
            seq,
            event: HierarchyEvent::TabClosed { tab: tab_id(20) },
        };
        assert_eq!(state.apply(replay), ApplyOutcome::Stale);
    }
    assert_eq!(state, applied);
}

#[test]
fn gaps_foreign_streams_and_contradictions_require_resync() {
    let mut state = seeded(5);
    let original = state.clone();
    let gap = HierarchyEnvelope {
        stream: STREAM,
        seq: 7,
        event: HierarchyEvent::TabClosed { tab: tab_id(20) },
    };
    assert_eq!(
        state.apply(gap),
        ApplyOutcome::ResyncRequired(ResyncReason::Gap)
    );
    let foreign = StreamId::new(RuntimeId::new(8));
    // Reset carries no identity, so only the envelope can reject it.
    for event in [
        HierarchyEvent::Reset,
        HierarchyEvent::TabClosed { tab: tab_id(20) },
    ] {
        let envelope = HierarchyEnvelope {
            stream: foreign,
            seq: 6,
            event,
        };
        assert_eq!(
            state.apply(envelope),
            ApplyOutcome::ResyncRequired(ResyncReason::ForeignStream)
        );
    }
    let foreign_identity = TabInfo {
        id: TabId::in_runtime(RuntimeId::new(8), 30),
        ..tab(30)
    };
    for event in [
        HierarchyEvent::SessionCreated {
            session: session(1),
            index: 0,
        },
        HierarchyEvent::SessionCreated {
            session: session(3),
            index: 3,
        },
        HierarchyEvent::WorkspaceCreated {
            session: session_id(9),
            workspace: workspace(12),
            index: 0,
        },
        HierarchyEvent::TabOpened {
            workspace: workspace_id(10),
            tab: tab(22),
            index: 0,
        },
        HierarchyEvent::TabOpened {
            workspace: workspace_id(10),
            tab: foreign_identity,
            index: 0,
        },
        HierarchyEvent::TabMoved {
            tab: tab_id(20),
            workspace: workspace_id(10),
            index: 2,
        },
        HierarchyEvent::WorkspaceMoved {
            workspace: workspace_id(10),
            session: session_id(2),
            index: 1,
        },
        HierarchyEvent::TabRenamed {
            tab: tab_id(99),
            custom_name: None,
        },
        HierarchyEvent::WorkspaceClosed {
            workspace: workspace_id(99),
        },
    ] {
        let envelope = next(&state, event);
        assert_eq!(
            state.apply(envelope),
            ApplyOutcome::ResyncRequired(ResyncReason::Inconsistent)
        );
    }
    assert_eq!(state, original);
}

#[test]
fn projections_fed_the_same_stream_converge() {
    let mut first = seeded(3);
    let mut second = first.clone();
    let events = [
        HierarchyEvent::TabRenamed {
            tab: tab_id(21),
            custom_name: Some("build".into()),
        },
        HierarchyEvent::TabMoved {
            tab: tab_id(21),
            workspace: workspace_id(11),
            index: 1,
        },
        HierarchyEvent::WorkspaceCreated {
            session: session_id(2),
            workspace: workspace(12),
            index: 0,
        },
        HierarchyEvent::TabOpened {
            workspace: workspace_id(12),
            tab: tab(23),
            index: 0,
        },
        HierarchyEvent::WorkspaceMoved {
            workspace: workspace_id(10),
            session: session_id(2),
            index: 1,
        },
        HierarchyEvent::SessionClosed {
            session: session_id(1),
        },
    ];
    let mut summary = Touched::default();
    for event in events {
        let envelope = next(&first, event);
        let ApplyOutcome::Applied(touched) = first.apply(envelope.clone())
        else {
            panic!("first projection rejected {envelope:?}");
        };
        assert!(matches!(second.apply(envelope), ApplyOutcome::Applied(_)));
        summary.merge(touched);
    }
    assert_eq!(first, second);
    assert_eq!(first.digest(), second.digest());
    assert!(summary.names_changed());
    assert!(summary.contains_workspace(workspace_id(12)));
    assert!(summary.contains_tab(tab_id(21)));
    assert!(!summary.contains_tab(tab_id(20)));
    summary.merge(Touched::everything());
    assert!(summary.is_everything() && summary.contains_tab(tab_id(20)));
    assert!(Touched::default().is_empty());
}

#[test]
fn digest_is_stable_and_sensitive_to_every_structural_difference() {
    let state = seeded(1);
    // Fixed value, cross-checked against an independent implementation of
    // the documented encoding: the digest must not vary by process.
    assert_eq!(state.digest(), 0x5fc1_9fa0_42d3_dfe8);
    assert_eq!(seeded(9).digest(), state.digest());
    assert_eq!(HierarchyState::new(STREAM).digest(), FNV_OFFSET);

    let variant = |event| {
        let mut changed = state.clone();
        let envelope = next(&changed, event);
        assert!(matches!(changed.apply(envelope), ApplyOutcome::Applied(_)));
        changed.digest()
    };
    let digests = [
        state.digest(),
        variant(HierarchyEvent::TabRenamed {
            tab: tab_id(20),
            custom_name: Some("sh".into()),
        }),
        variant(HierarchyEvent::WorkspaceRenamed {
            workspace: workspace_id(10),
            custom_name: Some("Workspace 10".into()),
        }),
        variant(HierarchyEvent::TabMoved {
            tab: tab_id(20),
            workspace: workspace_id(10),
            index: 1,
        }),
        variant(HierarchyEvent::TabClosed { tab: tab_id(22) }),
        variant(HierarchyEvent::TabMoved {
            tab: tab_id(22),
            workspace: workspace_id(10),
            index: 2,
        }),
        variant(HierarchyEvent::WorkspaceMoved {
            workspace: workspace_id(11),
            session: session_id(2),
            index: 0,
        }),
    ];
    for (index, digest) in digests.iter().enumerate() {
        assert!(
            !digests[..index].contains(digest),
            "digest {index} collides"
        );
    }
}

#[test]
fn snapshots_reject_repeated_or_foreign_identities() {
    let repeated = HierarchyState::from_sessions(
        STREAM,
        0,
        [(session(1), vec![(workspace(10), vec![tab(20), tab(20)])])],
    );
    assert!(repeated.is_none());
    let foreign = HierarchyState::from_sessions(
        STREAM,
        0,
        [(
            SessionInfo {
                id: SessionId::in_runtime(RuntimeId::new(8), 1),
                ..session(1)
            },
            Vec::<(WorkspaceInfo, Vec<TabInfo>)>::new(),
        )],
    );
    assert!(foreign.is_none());
}
