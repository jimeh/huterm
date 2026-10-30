use super::*;
use crate::{PaneInfo, PaneNode, SPLIT_TOTAL, SplitChild, SplitId, SplitNode};

const HALF: u32 = SPLIT_TOTAL / 2;

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

fn pane_id(value: u64) -> PaneId {
    PaneId::in_runtime(RUNTIME, value)
}

fn pane(value: u64) -> PaneInfo {
    PaneInfo {
        id: pane_id(value),
        terminal_id: TerminalId::new(value + 100),
        fallback_name: "sh".into(),
        custom_name: None,
    }
}

/// A one-pane tab whose pane is `value + 100` and terminal `value + 200`.
fn tab(value: u64) -> TabInfo {
    TabInfo {
        id: tab_id(value),
        custom_name: None,
        layout: PaneLayout::single(pane(value + 100)),
        zoomed: None,
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
    touched_layouts(names, workspaces, tabs, &[])
}

fn touched_layouts(
    names: bool,
    workspaces: &[u64],
    tabs: &[u64],
    layouts: &[u64],
) -> ApplyOutcome {
    ApplyOutcome::Applied(Touched {
        everything: false,
        names,
        workspaces: workspaces.iter().copied().map(workspace_id).collect(),
        tabs: tabs.iter().copied().map(tab_id).collect(),
        layouts: layouts.iter().copied().map(tab_id).collect(),
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
            touched_layouts(false, &[10], &[value], &[value])
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
    // A pane rename touches its tab but not its layout, so no client
    // recomputes geometry; an unchanged name touches nothing.
    let renamed = HierarchyEvent::PaneRenamed {
        pane: pane_id(122),
        custom_name: Some("tail".into()),
    };
    assert_eq!(
        apply(&mut state, renamed.clone()),
        touched(false, &[], &[22])
    );
    assert_eq!(
        state
            .tab(tab_id(22))
            .unwrap()
            .layout
            .first_pane()
            .custom_name
            .as_deref(),
        Some("tail")
    );
    assert_eq!(apply(&mut state, renamed), touched(false, &[], &[]));
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
        HierarchyEvent::PaneRenamed {
            pane: pane_id(99),
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

/// Splits tab 20 into panes `panes` along `axis`, the first with `weight`.
fn split_tab(
    axis: SplitAxis,
    panes: [u64; 2],
    weight: u32,
    zoomed: Option<PaneId>,
) -> HierarchyEvent {
    HierarchyEvent::PaneLayoutChanged {
        tab: tab_id(20),
        layout: two_panes(axis, panes, weight),
        zoomed,
    }
}

fn two_panes(axis: SplitAxis, panes: [u64; 2], weight: u32) -> PaneLayout {
    PaneLayout::new(PaneNode::Split(SplitNode {
        id: SplitId::in_runtime(RUNTIME, 900),
        axis,
        children: vec![
            SplitChild {
                weight,
                node: PaneNode::Pane(pane(panes[0])),
            },
            SplitChild {
                weight: SPLIT_TOTAL - weight,
                node: PaneNode::Pane(pane(panes[1])),
            },
        ],
    }))
    .unwrap()
}

#[test]
fn layout_and_zoom_events_replace_a_tab_layout_and_its_pane_indexes() {
    let mut state = seeded(1);
    assert_eq!(state.pane_tab(pane_id(120)), Some(tab_id(20)));
    assert_eq!(state.terminal_tab(TerminalId::new(220)), Some(tab_id(20)));
    assert_eq!(
        apply(
            &mut state,
            split_tab(SplitAxis::Columns, [120, 130], HALF, None)
        ),
        touched_layouts(false, &[10], &[20], &[20])
    );
    assert_eq!(state.tab(tab_id(20)).unwrap().layout.pane_count(), 2);
    assert_eq!(state.pane_tab(pane_id(130)), Some(tab_id(20)));
    assert_eq!(state.terminal_tab(TerminalId::new(230)), Some(tab_id(20)));
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::PaneZoomChanged {
                tab: tab_id(20),
                zoomed: Some(pane_id(130)),
            },
        ),
        touched_layouts(false, &[10], &[20], &[20])
    );
    assert_eq!(state.tab(tab_id(20)).unwrap().zoomed, Some(pane_id(130)));
    // Setting the current zoom again changes nothing.
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::PaneZoomChanged {
                tab: tab_id(20),
                zoomed: Some(pane_id(130)),
            },
        ),
        touched(false, &[], &[])
    );
    // A close back to one pane clears the zoom and drops the old pane's
    // indexes.
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::PaneLayoutChanged {
                tab: tab_id(20),
                layout: PaneLayout::single(pane(130)),
                zoomed: None,
            },
        ),
        touched_layouts(false, &[10], &[20], &[20])
    );
    assert_eq!(state.pane_tab(pane_id(120)), None);
    assert_eq!(state.terminal_tab(TerminalId::new(220)), None);
    assert_eq!(state.tab(tab_id(20)).unwrap().fallback_name(), "sh");
    assert_eq!(
        apply(&mut state, HierarchyEvent::TabClosed { tab: tab_id(20) }),
        touched(false, &[10], &[20])
    );
    assert_eq!(state.pane_tab(pane_id(130)), None);
    assert_eq!(
        apply(
            &mut state,
            HierarchyEvent::WorkspaceClosed {
                workspace: workspace_id(11)
            }
        ),
        touched(true, &[11], &[22])
    );
    assert_eq!(state.pane_tab(pane_id(122)), None);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one table of inconsistent layout events, each applied alike"
)]
fn layouts_that_reuse_another_tabs_panes_or_name_foreign_zoom_are_inconsistent()
{
    let inconsistent = ApplyOutcome::ResyncRequired(ResyncReason::Inconsistent);
    // Pane 121 belongs to tab 21.
    let reused_pane = split_tab(SplitAxis::Columns, [120, 121], HALF, None);
    let reused_terminal = HierarchyEvent::PaneLayoutChanged {
        tab: tab_id(20),
        layout: PaneLayout::new(PaneNode::Split(SplitNode {
            id: SplitId::in_runtime(RUNTIME, 900),
            axis: SplitAxis::Columns,
            children: vec![
                SplitChild {
                    weight: HALF,
                    node: PaneNode::Pane(pane(120)),
                },
                SplitChild {
                    weight: HALF,
                    node: PaneNode::Pane(PaneInfo {
                        id: pane_id(130),
                        ..pane(121)
                    }),
                },
            ],
        }))
        .unwrap(),
        zoomed: None,
    };
    // Pane 121's identity with a terminal no tab shows.
    let reused_pane_only = HierarchyEvent::PaneLayoutChanged {
        tab: tab_id(20),
        layout: PaneLayout::single(PaneInfo {
            id: pane_id(121),
            ..pane(199)
        }),
        zoomed: None,
    };
    let foreign_pane = HierarchyEvent::PaneLayoutChanged {
        tab: tab_id(20),
        layout: PaneLayout::single(PaneInfo {
            id: PaneId::in_runtime(RuntimeId::new(8), 120),
            ..pane(120)
        }),
        zoomed: None,
    };
    let foreign_split = HierarchyEvent::PaneLayoutChanged {
        tab: tab_id(20),
        layout: PaneLayout::new(PaneNode::Split(SplitNode {
            id: SplitId::in_runtime(RuntimeId::new(8), 900),
            axis: SplitAxis::Columns,
            children: vec![
                SplitChild {
                    weight: HALF,
                    node: PaneNode::Pane(pane(120)),
                },
                SplitChild {
                    weight: HALF,
                    node: PaneNode::Pane(pane(130)),
                },
            ],
        }))
        .unwrap(),
        zoomed: None,
    };
    for event in [
        reused_pane,
        reused_pane_only,
        foreign_split,
        reused_terminal,
        foreign_pane,
        split_tab(SplitAxis::Columns, [120, 130], HALF, Some(pane_id(140))),
        HierarchyEvent::PaneLayoutChanged {
            tab: tab_id(20),
            layout: PaneLayout::single(pane(120)),
            zoomed: Some(pane_id(120)),
        },
        HierarchyEvent::PaneZoomChanged {
            tab: tab_id(20),
            zoomed: Some(pane_id(120)),
        },
        HierarchyEvent::PaneZoomChanged {
            tab: tab_id(99),
            zoomed: None,
        },
        HierarchyEvent::PaneLayoutChanged {
            tab: tab_id(99),
            layout: PaneLayout::single(pane(199)),
            zoomed: None,
        },
        HierarchyEvent::TabOpened {
            workspace: workspace_id(10),
            tab: TabInfo {
                layout: PaneLayout::single(pane(121)),
                ..tab(30)
            },
            index: 0,
        },
    ] {
        let mut state = seeded(1);
        let before = state.clone();
        assert_eq!(apply(&mut state, event.clone()), inconsistent, "{event:?}");
        assert_eq!(state, before);
    }
    let reused = HierarchyState::from_sessions(
        STREAM,
        0,
        [(
            session(1),
            vec![(
                workspace(10),
                vec![
                    tab(20),
                    TabInfo {
                        layout: PaneLayout::single(pane(120)),
                        ..tab(21)
                    },
                ],
            )],
        )],
    );
    assert!(reused.is_none());
}

#[test]
fn layout_events_follow_the_stale_and_gap_rules() {
    let mut state = seeded(4);
    let event = split_tab(SplitAxis::Columns, [120, 130], HALF, None);
    let replayed = HierarchyEnvelope {
        stream: STREAM,
        seq: 4,
        event: event.clone(),
    };
    assert_eq!(state.apply(replayed), ApplyOutcome::Stale);
    let gap = HierarchyEnvelope {
        stream: STREAM,
        seq: 6,
        event,
    };
    assert_eq!(
        state.apply(gap),
        ApplyOutcome::ResyncRequired(ResyncReason::Gap)
    );
    assert_eq!(state, seeded(4));
}

#[test]
fn digest_is_stable_and_sensitive_to_every_structural_difference() {
    let state = seeded(1);
    // Fixed value, cross-checked against an independent implementation of
    // the documented encoding: the digest must not vary by process.
    assert_eq!(state.digest(), 0x43e8_caa9_4013_6e70);
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
        variant(HierarchyEvent::PaneRenamed {
            pane: pane_id(120),
            custom_name: Some("sh".into()),
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
        variant(split_tab(SplitAxis::Columns, [120, 130], 400_000, None)),
        variant(split_tab(SplitAxis::Columns, [120, 130], 600_000, None)),
        variant(split_tab(SplitAxis::Rows, [120, 130], 400_000, None)),
        variant(split_tab(SplitAxis::Columns, [130, 120], 400_000, None)),
        variant(split_tab(
            SplitAxis::Columns,
            [120, 130],
            400_000,
            Some(pane_id(130)),
        )),
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

fn layout_change(value: u64, layout: PaneLayout) -> TabChange {
    TabChange::Layout {
        tab: tab_id(value),
        layout,
        zoomed: None,
    }
}

fn rearranged(changes: Vec<TabChange>) -> HierarchyEvent {
    HierarchyEvent::PanesRearranged { changes }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "walk one pane through every kind of rearrangement"
)]
fn rearrangements_move_panes_between_tabs_atomically() {
    let mut state = seeded(1);
    apply(
        &mut state,
        split_tab(SplitAxis::Columns, [120, 130], HALF, None),
    );
    // Pane 130 moves from tab 20 into tab 21.
    assert_eq!(
        apply(
            &mut state,
            rearranged(vec![
                layout_change(20, PaneLayout::single(pane(120))),
                layout_change(21, two_panes(SplitAxis::Rows, [121, 130], HALF)),
            ]),
        ),
        touched_layouts(false, &[10], &[20, 21], &[20, 21])
    );
    assert_eq!(state.pane_tab(pane_id(130)), Some(tab_id(21)));
    assert_eq!(state.terminal_tab(TerminalId::new(230)), Some(tab_id(21)));
    assert_eq!(state.pane_tab(pane_id(120)), Some(tab_id(20)));
    // It leaves for a new tab 23 at the front of workspace 11.
    assert_eq!(
        apply(
            &mut state,
            rearranged(vec![
                TabChange::Opened {
                    workspace: workspace_id(11),
                    tab: TabInfo {
                        layout: PaneLayout::single(pane(130)),
                        ..tab(23)
                    },
                    index: 0,
                },
                layout_change(21, PaneLayout::single(pane(121))),
            ]),
        ),
        touched_layouts(false, &[10, 11], &[21, 23], &[21, 23])
    );
    assert_eq!(
        state.workspace_tabs(workspace_id(11)).unwrap(),
        [tab_id(23), tab_id(22)]
    );
    assert_eq!(state.pane_tab(pane_id(130)), Some(tab_id(23)));
    // The lone-pane tab moves to workspace 10 between tabs 20 and 21.
    assert_eq!(
        apply(
            &mut state,
            rearranged(vec![TabChange::Placed {
                tab: tab_id(23),
                workspace: workspace_id(10),
                index: 1,
            }]),
        ),
        touched_layouts(false, &[10, 11], &[23], &[23])
    );
    assert_eq!(
        state.workspace_tabs(workspace_id(10)).unwrap(),
        [tab_id(20), tab_id(23), tab_id(21)]
    );
    assert_eq!(
        state.workspace_tabs(workspace_id(11)).unwrap(),
        [tab_id(22)]
    );
    assert_eq!(state.tab_workspace(tab_id(23)), Some(workspace_id(10)));
    // Tab 23 drops into tab 20 and is removed, its pane kept.
    assert_eq!(
        apply(
            &mut state,
            rearranged(vec![
                TabChange::Removed { tab: tab_id(23) },
                layout_change(20, two_panes(SplitAxis::Rows, [120, 130], HALF)),
            ]),
        ),
        touched_layouts(false, &[10], &[20, 23], &[20, 23])
    );
    assert!(state.tab(tab_id(23)).is_none());
    assert_eq!(
        state.workspace_tabs(workspace_id(10)).unwrap(),
        [tab_id(20), tab_id(21)]
    );
    assert_eq!(state.pane_tab(pane_id(130)), Some(tab_id(20)));
    // A removed tab's identity can open again later, as a restore does.
    assert_eq!(
        apply(
            &mut state,
            rearranged(vec![
                TabChange::Opened {
                    workspace: workspace_id(10),
                    tab: TabInfo {
                        layout: PaneLayout::single(pane(130)),
                        ..tab(23)
                    },
                    index: 2,
                },
                layout_change(20, PaneLayout::single(pane(120))),
            ]),
        ),
        touched_layouts(false, &[10], &[20, 23], &[20, 23])
    );
    assert_eq!(
        state.workspace_tabs(workspace_id(10)).unwrap(),
        [tab_id(20), tab_id(21), tab_id(23)]
    );
    // The indexes match a snapshot of the same structure.
    let fresh = HierarchyState::from_sessions(
        STREAM,
        state.seq(),
        [
            (
                session(1),
                vec![
                    (
                        workspace(10),
                        vec![
                            tab(20),
                            tab(21),
                            TabInfo {
                                layout: PaneLayout::single(pane(130)),
                                ..tab(23)
                            },
                        ],
                    ),
                    (workspace(11), vec![tab(22)]),
                ],
            ),
            (session(2), vec![]),
        ],
    )
    .unwrap();
    assert_eq!(state, fresh);
}

#[test]
fn rearrangements_that_break_conservation_or_structure_change_nothing() {
    let inconsistent = ApplyOutcome::ResyncRequired(ResyncReason::Inconsistent);
    let lone = |value| PaneLayout::single(pane(value));
    for changes in [
        // Nothing to apply.
        vec![],
        // An unknown tab.
        vec![layout_change(99, lone(199))],
        // A tab with two layout changes.
        vec![layout_change(20, lone(120)), layout_change(20, lone(120))],
        // A tab that changes layout and is removed; the panes balance, so
        // only the one-role rule refuses it.
        vec![
            layout_change(21, lone(121)),
            TabChange::Removed { tab: tab_id(21) },
            layout_change(20, two_panes(SplitAxis::Rows, [120, 121], HALF)),
        ],
        // Opening a tab that exists, with its own panes.
        vec![TabChange::Opened {
            workspace: workspace_id(10),
            tab: tab(21),
            index: 0,
        }],
        // Opening into an unknown workspace.
        vec![
            TabChange::Removed { tab: tab_id(22) },
            TabChange::Opened {
                workspace: workspace_id(99),
                tab: TabInfo {
                    layout: lone(122),
                    ..tab(23)
                },
                index: 0,
            },
        ],
        // Opening a tab of another stream.
        vec![
            TabChange::Removed { tab: tab_id(22) },
            TabChange::Opened {
                workspace: workspace_id(10),
                tab: TabInfo {
                    id: TabId::in_runtime(RuntimeId::new(8), 23),
                    layout: lone(122),
                    ..tab(23)
                },
                index: 0,
            },
        ],
        // A placement past the end.
        vec![TabChange::Placed {
            tab: tab_id(22),
            workspace: workspace_id(10),
            index: 3,
        }],
        // A pane taken from an untouched tab.
        vec![layout_change(
            20,
            two_panes(SplitAxis::Rows, [120, 121], HALF),
        )],
        // A pane paired with another terminal.
        vec![layout_change(
            20,
            PaneLayout::single(PaneInfo {
                terminal_id: TerminalId::new(999),
                ..pane(120)
            }),
        )],
        // A tab removed without its panes going anywhere.
        vec![TabChange::Removed { tab: tab_id(21) }],
        // A pane that appears from nowhere.
        vec![layout_change(
            20,
            two_panes(SplitAxis::Rows, [120, 130], HALF),
        )],
        // A zoom on a lone pane.
        vec![TabChange::Layout {
            tab: tab_id(20),
            layout: lone(120),
            zoomed: Some(pane_id(120)),
        }],
    ] {
        let mut state = seeded(1);
        let before = state.clone();
        let event = rearranged(changes);
        assert_eq!(apply(&mut state, event.clone()), inconsistent, "{event:?}");
        assert_eq!(state, before);
    }
}

#[test]
fn rearrangements_follow_the_stale_and_gap_rules() {
    let mut state = seeded(4);
    let event = rearranged(vec![
        TabChange::Removed { tab: tab_id(21) },
        layout_change(20, two_panes(SplitAxis::Rows, [120, 121], HALF)),
    ]);
    let replayed = HierarchyEnvelope {
        stream: STREAM,
        seq: 4,
        event: event.clone(),
    };
    assert_eq!(state.apply(replayed), ApplyOutcome::Stale);
    let gap = HierarchyEnvelope {
        stream: STREAM,
        seq: 6,
        event,
    };
    assert_eq!(
        state.apply(gap),
        ApplyOutcome::ResyncRequired(ResyncReason::Gap)
    );
    assert_eq!(state, seeded(4));
}

#[test]
fn placements_apply_in_order_with_each_index_final_at_its_step() {
    let mut state = seeded(1);
    // Workspace 10 holds [20, 21] and workspace 11 holds [22]. Tab 22 goes
    // to the front of 10, then tab 20 moves to 11, where 22 has left; tab
    // 21 then trades panes with 22 in the same event.
    assert_eq!(
        apply(
            &mut state,
            rearranged(vec![
                TabChange::Placed {
                    tab: tab_id(22),
                    workspace: workspace_id(10),
                    index: 0,
                },
                TabChange::Placed {
                    tab: tab_id(20),
                    workspace: workspace_id(11),
                    index: 0,
                },
                layout_change(21, PaneLayout::single(pane(122))),
                layout_change(22, PaneLayout::single(pane(121))),
            ]),
        ),
        touched_layouts(false, &[10, 11], &[20, 21, 22], &[20, 21, 22])
    );
    assert_eq!(
        state.workspace_tabs(workspace_id(10)).unwrap(),
        [tab_id(22), tab_id(21)]
    );
    assert_eq!(
        state.workspace_tabs(workspace_id(11)).unwrap(),
        [tab_id(20)]
    );
    assert_eq!(state.pane_tab(pane_id(121)), Some(tab_id(22)));
    assert_eq!(state.pane_tab(pane_id(122)), Some(tab_id(21)));
    // An index valid only before an earlier step's removal is refused.
    let before = state.clone();
    assert_eq!(
        apply(
            &mut state,
            rearranged(vec![
                TabChange::Placed {
                    tab: tab_id(20),
                    workspace: workspace_id(10),
                    index: 0,
                },
                TabChange::Placed {
                    tab: tab_id(21),
                    workspace: workspace_id(11),
                    index: 1,
                },
            ]),
        ),
        ApplyOutcome::ResyncRequired(ResyncReason::Inconsistent)
    );
    assert_eq!(state, before);
}
