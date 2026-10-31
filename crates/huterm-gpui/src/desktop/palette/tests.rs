use super::*;
use crate::config::KeybindingEntry;
use crate::keymap::{Platform, compile};
use huterm_core::Mux;
use huterm_protocol::{CellSize, GridSize, RuntimeId, TerminalCommand};

fn target() -> PaletteTarget {
    PaletteTarget {
        session: None,
        workspace: None,
        tab: None,
        terminal: None,
        terminal_view: None,
        contexts: vec![
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ],
    }
}

/// Palette rows from a fresh projection of `mux`.
fn hierarchy(mux: &mut Mux, tab_order: &[TabId]) -> PaletteHierarchy {
    let (state, _) = mux.subscribe_hierarchy();
    PaletteHierarchy::from_projection(&state, |_| None, tab_order)
}

fn terminal_command() -> TerminalCommand {
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

#[test]
fn palette_commands_are_hidden_from_a_terminal_context() {
    let target = target();
    let listed: Vec<_> = catalog()
        .iter()
        .filter(|spec| context_matches(spec.context, &target.contexts))
        .map(|spec| spec.id)
        .collect();
    assert!(listed.contains(&ids::RENAME_TAB));
    assert!(listed.contains(&ids::OPEN_COMMAND_PALETTE));
    assert!(!listed.contains(&ids::PALETTE_CONFIRM));
    assert!(!listed.contains(&ids::TEXT_COPY));
    let palette_context = [
        KeyContext::parse("Workspace palette").unwrap(),
        KeyContext::parse("Palette").unwrap(),
    ];
    assert!(context_matches(Some("Palette"), &palette_context));
}

/// The palette forwards every editing command to its field; the field
/// verbs it leaves to its own bindings.
#[test]
fn forwarded_text_commands_cover_the_catalog() {
    let catalog_text_commands = catalog()
        .iter()
        .filter(|spec| spec.id.as_str().starts_with("text_"))
        .map(|spec| spec.id)
        .filter(|id| !crate::ui::input::is_field_verb(*id))
        .collect::<Vec<_>>();
    assert_eq!(FORWARDED_TEXT_COMMANDS, catalog_text_commands);
}

#[test]
fn title_highlights_use_utf8_ranges_and_merge_adjacent_matches() {
    assert_eq!(
        title_highlight_ranges("Toggle Fullscreen", &[0, 7, 11]),
        [0..1, 7..8, 11..12]
    );
    let adjacent = title_highlight_ranges("Toggle Fullscreen", &[7, 8, 9, 10]);
    assert_eq!(adjacent.len(), 1);
    assert_eq!(adjacent[0], 7..11);
}

#[test]
fn top_placement_keeps_its_inset_with_a_clamped_panel() {
    assert!(
        (panel_top(PalettePlacement::Top, 300.0, 200.0) - PANEL_TOP_INSET)
            .abs()
            < f32::EPSILON
    );
}

#[test]
fn center_placement_uses_the_clamped_panel_height() {
    assert!(
        (panel_top(PalettePlacement::Center, 300.0, 200.0) - 50.0).abs()
            < f32::EPSILON
    );
}

#[test]
fn ten_tab_picker_caps_match_select_tab_positions_exactly() {
    let bare = KeybindingEntry {
        key: "cmd-shift-o".into(),
        command: "select_tab".into(),
        args: None,
        when: None,
        description: None,
        disabled: None,
    };
    let (_, keymap) =
        compile(Platform::MacOs, &[bare]).unwrap().install_parts();
    let spec = huterm_protocol::lookup(ids::SELECT_TAB.as_str()).unwrap();
    let contexts = target().contexts;
    let caps = (1..=10)
        .map(|position| {
            let index = select_tab_shortcut_index(position, 10)?;
            shortcut_keys_for(
                &keymap,
                &contexts,
                spec,
                Some(&[CommandArgument::new(
                    "index",
                    CommandValue::Integer(index),
                )]),
            )
            .into_iter()
            .next()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        caps,
        [
            Some("cmd-1".into()),
            Some("cmd-2".into()),
            Some("cmd-3".into()),
            Some("cmd-4".into()),
            Some("cmd-5".into()),
            Some("cmd-6".into()),
            Some("cmd-7".into()),
            Some("cmd-8".into()),
            None,
            Some("cmd-9".into()),
        ]
    );
    assert!(caps.iter().flatten().all(|key| key != "cmd-shift-o"));
}

#[test]
fn tabs_follow_activation_order_with_the_active_tab_last() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let first = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
    let second = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
    let third = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
    let hierarchy = hierarchy(&mut mux, &[third.id, first.id, second.id]);
    let order: Vec<_> = hierarchy
        .tabs
        .iter()
        .map(|row| (row.value.clone(), row.position))
        .collect();
    assert_eq!(
        order,
        [
            (CommandValue::Tab(third.id), Some(3)),
            (CommandValue::Tab(first.id), Some(1)),
            (CommandValue::Tab(second.id), Some(2)),
        ]
    );
    mux.close_session(session).unwrap();
}

#[test]
fn window_scoped_commands_list_only_this_windows_tabs() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let other = mux.create_workspace(session, None).unwrap();
    let mine = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
    let theirs = mux.open_tab(other, &terminal_command()).unwrap().tab;
    let hierarchy = hierarchy(&mut mux, &[]);
    let target = PaletteTarget {
        session: Some(session),
        workspace: Some(workspace),
        tab: Some(mine.id),
        ..target()
    };
    let window_only = DomainView {
        hierarchy: &hierarchy,
        profiles: &[],
        layouts: &[],
        target: &target,
        window_only: true,
        pane_exclusion: PaneExclusion::OwnPane,
    };
    assert_eq!(
        window_only.values(ArgumentKind::Tab),
        vec![CommandValue::Tab(mine.id)]
    );
    let runtime = DomainView {
        window_only: false,
        ..window_only
    };
    assert_eq!(runtime.values(ArgumentKind::Tab).len(), 2);
    assert!(
        runtime
            .label(ArgumentKind::Tab, &CommandValue::Tab(theirs.id))
            .is_some()
    );
    mux.close_session(session).unwrap();
}

#[test]
fn identity_replacement_and_duplicate_labels_keep_scoped_ids() {
    let mut mux = Mux::default();
    let first_session = mux.create_session(Some("duplicate")).unwrap();
    let second_session = mux.create_session(Some("duplicate")).unwrap();
    let first_workspace = mux
        .create_workspace(first_session, Some("duplicate"))
        .unwrap();
    let second_workspace = mux
        .create_workspace(second_session, Some("duplicate"))
        .unwrap();
    let hierarchy = hierarchy(&mut mux, &[]);
    assert_eq!(hierarchy.sessions[0].label, hierarchy.sessions[1].label);
    assert_ne!(hierarchy.sessions[0].value, hierarchy.sessions[1].value);
    assert!(
        hierarchy
            .workspaces
            .iter()
            .find(|row| row.value == CommandValue::Workspace(first_workspace))
            .is_some_and(|row| row.custom_name)
    );

    let rename =
        huterm_protocol::lookup(ids::RENAME_WORKSPACE.as_str()).unwrap();
    let target = PaletteTarget {
        session: Some(first_session),
        workspace: Some(first_workspace),
        ..target()
    };
    let domain = DomainView {
        hierarchy: &hierarchy,
        profiles: &[],
        layouts: &[],
        target: &target,
        window_only: false,
        pane_exclusion: PaneExclusion::OwnPane,
    };
    let mut editor = SlotEditor::new(rename, &[], &domain, false).unwrap();
    editor.set_text("chosen");
    editor.next_slot(None).unwrap();
    assert_eq!(editor.active().spec.name, "workspace");
    let commit = editor
        .commit(Some(CommandValue::Workspace(second_workspace)))
        .unwrap();
    let Commit::Run(invocation) = commit else {
        panic!("picking the workspace runs the rename");
    };
    assert_eq!(invocation.workspace("workspace"), Some(second_workspace));
    assert_eq!(invocation.text("name"), Some("chosen"));
}

#[test]
fn rename_targets_prefill_from_the_projected_session_and_workspace() {
    let mut mux = Mux::default();
    let session = mux.create_session(Some("named session")).unwrap();
    let workspace = mux
        .create_workspace(session, Some("named workspace"))
        .unwrap();
    let (state, _) = mux.subscribe_hierarchy();
    let hierarchy = PaletteHierarchy::from_projection(&state, |_| None, &[]);
    // The window knows its workspace; the projection supplies its session.
    let target = PaletteTarget {
        session: state.workspace_session(workspace),
        workspace: Some(workspace),
        ..target()
    };
    let domain = DomainView {
        hierarchy: &hierarchy,
        profiles: &[],
        layouts: &[],
        target: &target,
        window_only: false,
        pane_exclusion: PaneExclusion::OwnPane,
    };
    for (command, kind, name) in [
        (ids::RENAME_SESSION, ArgumentKind::Session, "named session"),
        (
            ids::RENAME_WORKSPACE,
            ArgumentKind::Workspace,
            "named workspace",
        ),
    ] {
        let spec = huterm_protocol::lookup(command.as_str()).unwrap();
        let editor = SlotEditor::new(spec, &[], &domain, false).unwrap();
        let slot = editor
            .slots()
            .iter()
            .find(|slot| slot.spec.kind == kind)
            .unwrap();
        assert_eq!(slot.state, SlotState::Prefilled);
        assert_eq!(
            current_custom_name(&editor, &domain).as_deref(),
            Some(name)
        );
    }
}

#[test]
fn palette_invocation_renames_exact_tab_and_stale_target_changes_nothing() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let first = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
    let second = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
    mux.rename_tab(first.id, Some("duplicate")).unwrap();
    mux.rename_tab(second.id, Some("duplicate")).unwrap();
    let hierarchy = hierarchy(&mut mux, &[]);

    let rename = huterm_protocol::lookup(ids::RENAME_TAB.as_str()).unwrap();
    let target = PaletteTarget {
        session: Some(session),
        workspace: Some(workspace),
        tab: Some(second.id),
        ..target()
    };
    let domain = DomainView {
        hierarchy: &hierarchy,
        profiles: &[],
        layouts: &[],
        target: &target,
        window_only: false,
        pane_exclusion: PaneExclusion::OwnPane,
    };
    let mut editor = SlotEditor::new(rename, &[], &domain, false).unwrap();
    editor.set_text("chosen");
    let Commit::Run(invocation) = editor.commit(None).unwrap() else {
        panic!("one Enter runs the rename with the prefilled tab");
    };
    assert_eq!(
        huterm_core::execute(&mut mux, &invocation),
        Ok(huterm_protocol::CommandOutcome::Completed)
    );
    assert_eq!(mux.tab(first.id).unwrap().custom_name(), Some("duplicate"));
    assert_eq!(mux.tab(second.id).unwrap().custom_name(), Some("chosen"));

    mux.close_tab(workspace, second.id).unwrap();
    assert_eq!(
        huterm_core::execute(&mut mux, &invocation),
        Err(CommandError::StaleTarget)
    );
    assert_eq!(mux.tab(first.id).unwrap().custom_name(), Some("duplicate"));
    mux.close_session(session).unwrap();
}

#[test]
fn quake_profiles_form_a_strict_domain_with_a_default() {
    let profiles = profile_rows(vec![
        QuakeProfileRow {
            name: "default".into(),
            detail: "top".into(),
        },
        QuakeProfileRow {
            name: "logs".into(),
            detail: "right".into(),
        },
    ]);
    let target = target();
    let hierarchy = PaletteHierarchy::default();
    let domain = DomainView {
        hierarchy: &hierarchy,
        profiles: &profiles,
        layouts: &[],
        target: &target,
        window_only: false,
        pane_exclusion: PaneExclusion::OwnPane,
    };
    let toggle = huterm_protocol::lookup(ids::TOGGLE_QUAKE.as_str()).unwrap();
    assert!(slots::runs_without_prompt(toggle, &domain));
    let editor = SlotEditor::new(toggle, &[], &domain, false).unwrap();
    assert_eq!(editor.slots()[0].state, SlotState::Prefilled);
    assert_eq!(
        editor.invocation().unwrap().text("profile"),
        Some("default")
    );
    let mut picking = SlotEditor::new(toggle, &[], &domain, false).unwrap();
    picking.edit(0);
    let Commit::Run(invocation) = picking
        .commit(Some(CommandValue::Text("logs".into())))
        .unwrap()
    else {
        panic!("picking a profile runs the command");
    };
    assert_eq!(invocation.text("profile"), Some("logs"));
}

/// Session 1 holds workspaces 10 and 11; workspace 10 holds tabs 1 and
/// 2, which carry no custom names.
fn projected() -> HierarchyState {
    use huterm_protocol::{
        HierarchyEvent, PaneId, PaneInfo, PaneLayout, SessionInfo, StreamId,
        TabInfo, WorkspaceInfo,
    };
    let runtime = RuntimeId::new(9);
    let mut state = HierarchyState::new(StreamId::new(runtime));
    let session = SessionId::in_runtime(runtime, 1);
    let mut events = vec![HierarchyEvent::SessionCreated {
        session: SessionInfo {
            id: session,
            custom_name: None,
            automatic_name: "Session 1".into(),
        },
        index: 0,
    }];
    for (index, value) in [10, 11].into_iter().enumerate() {
        events.push(HierarchyEvent::WorkspaceCreated {
            session,
            workspace: WorkspaceInfo {
                id: WorkspaceId::in_runtime(runtime, value),
                custom_name: None,
                automatic_name: format!("Workspace {value}"),
            },
            index: u32::try_from(index).unwrap(),
        });
    }
    for (index, value) in [1, 2].into_iter().enumerate() {
        events.push(HierarchyEvent::TabOpened {
            workspace: WorkspaceId::in_runtime(runtime, 10),
            tab: TabInfo {
                id: TabId::in_runtime(runtime, value),
                custom_name: None,
                layout: PaneLayout::single(PaneInfo {
                    id: PaneId::in_runtime(runtime, value),
                    terminal_id: TerminalId::new(value),
                    fallback_name: "sh".into(),
                    custom_name: None,
                }),
                zoomed: None,
                input_sync: false,
            },
            index: u32::try_from(index).unwrap(),
        });
    }
    for event in events {
        apply(&mut state, event);
    }
    state
}

fn apply(state: &mut HierarchyState, event: huterm_protocol::HierarchyEvent) {
    let envelope = huterm_protocol::HierarchyEnvelope {
        stream: state.stream(),
        seq: state.seq() + 1,
        event,
    };
    assert!(matches!(
        state.apply(envelope),
        huterm_protocol::ApplyOutcome::Applied(_)
    ));
}

fn tab_rows(
    hierarchy: &PaletteHierarchy,
) -> Vec<(CommandValue, String, Option<i64>, String)> {
    hierarchy
        .tabs
        .iter()
        .map(|row| {
            (
                row.value.clone(),
                row.label.clone(),
                row.position,
                row.detail.clone(),
            )
        })
        .collect()
}

#[test]
fn tab_rows_prefer_custom_names_then_published_titles_then_fallbacks() {
    let mut state = projected();
    let runtime = RuntimeId::new(9);
    let (first, second) =
        (TabId::in_runtime(runtime, 1), TabId::in_runtime(runtime, 2));
    let published = |tab: TabId| (tab == first).then_some("vim");
    let rows = PaletteHierarchy::from_projection(&state, published, &[]);
    let labels: Vec<_> = tab_rows(&rows).into_iter().map(|row| row.1).collect();
    assert_eq!(labels, ["vim", "sh"]);
    apply(
        &mut state,
        huterm_protocol::HierarchyEvent::TabRenamed {
            tab: second,
            custom_name: Some("logs".into()),
        },
    );
    let rows = PaletteHierarchy::from_projection(&state, published, &[]);
    let labels: Vec<_> = tab_rows(&rows).into_iter().map(|row| row.1).collect();
    assert_eq!(labels, ["vim", "logs"]);
}

#[test]
fn a_move_rebuilds_order_positions_and_parents() {
    let mut state = projected();
    let runtime = RuntimeId::new(9);
    let first = CommandValue::Tab(TabId::in_runtime(runtime, 1));
    let second = CommandValue::Tab(TabId::in_runtime(runtime, 2));
    let before = PaletteHierarchy::from_projection(&state, |_| None, &[]);
    assert_eq!(
        tab_rows(&before),
        [
            (
                first.clone(),
                "sh".into(),
                Some(1),
                "Session 1 › Workspace 10".into()
            ),
            (
                second.clone(),
                "sh".into(),
                Some(2),
                "Session 1 › Workspace 10".into()
            ),
        ]
    );
    apply(
        &mut state,
        huterm_protocol::HierarchyEvent::TabMoved {
            tab: TabId::in_runtime(runtime, 1),
            workspace: WorkspaceId::in_runtime(runtime, 11),
            index: 0,
        },
    );
    let after = PaletteHierarchy::from_projection(&state, |_| None, &[]);
    assert_ne!(before, after, "a reorder with no rename still rebuilds");
    assert_eq!(
        tab_rows(&after),
        [
            (
                second.clone(),
                "sh".into(),
                Some(1),
                "Session 1 › Workspace 10".into()
            ),
            (
                first.clone(),
                "sh".into(),
                Some(1),
                "Session 1 › Workspace 11".into()
            ),
        ]
    );
}

#[test]
fn automatic_name_text_follows_its_target_and_edited_text_stays() {
    // Another window's rename makes a nonempty prefill stale.
    assert_eq!(
        refreshed_name(NameText::Automatic, "old", Some("new")).as_deref(),
        Some("new")
    );
    assert_eq!(
        refreshed_name(NameText::Automatic, "old", None).as_deref(),
        Some("")
    );
    assert_eq!(
        refreshed_name(NameText::Automatic, "same", Some("same")),
        None
    );
    // A deliberately blanked name survives, so Enter still clears.
    assert_eq!(refreshed_name(NameText::Edited, "", Some("old")), None);
    // Navigating back to a blanked, uncommitted name slot keeps it
    // edited instead of seeding it again; an untouched slot still seeds.
    assert_eq!(
        entered_name_mode(NameText::Edited, false, ""),
        NameText::Edited
    );
    assert_eq!(
        entered_name_mode(NameText::Automatic, false, ""),
        NameText::Automatic
    );
    assert_eq!(
        entered_name_mode(NameText::Automatic, true, ""),
        NameText::Edited
    );
    assert_eq!(
        entered_name_mode(NameText::Automatic, false, "typed"),
        NameText::Edited
    );
}

#[test]
fn a_rebuild_moves_window_scoped_pickers_and_defaults_to_the_current_scope() {
    let runtime = RuntimeId::new(9);
    let session = SessionId::in_runtime(runtime, 1);
    let rows = PaletteHierarchy::from_projection(&projected(), |_| None, &[]);
    // The palette opened while the window showed workspace 11, whose
    // tabs have since gone; the window now shows workspace 10.
    let mut target = PaletteTarget {
        session: Some(session),
        workspace: Some(WorkspaceId::in_runtime(runtime, 11)),
        tab: Some(TabId::in_runtime(runtime, 7)),
        ..target()
    };
    let current = PaletteScope {
        session: Some(session),
        workspace: Some(WorkspaceId::in_runtime(runtime, 10)),
        tab: Some(TabId::in_runtime(runtime, 2)),
        terminal: Some(TerminalId::new(2)),
        terminal_view: None,
    };
    assert!(target.rescope(current.clone()));
    assert!(!target.rescope(current), "an unchanged scope is no change");
    // Terminal-scope commands follow the active tab's terminal too.
    assert_eq!(target.terminal, Some(TerminalId::new(2)));
    let domain = DomainView {
        hierarchy: &rows,
        profiles: &[],
        layouts: &[],
        target: &target,
        window_only: true,
        pane_exclusion: PaneExclusion::OwnPane,
    };
    assert_eq!(
        domain.values(ArgumentKind::Tab),
        [1, 2]
            .map(|value| CommandValue::Tab(TabId::in_runtime(runtime, value)))
    );
    assert_eq!(
        domain.default(ArgumentKind::Tab),
        Some(CommandValue::Tab(TabId::in_runtime(runtime, 2)))
    );
}

#[test]
fn owned_history_ranks_recent_commands_first() {
    let mut recent = RecentCommands::default();
    recent.record(ids::RENAME_TAB);
    let frequency = CommandFrequency::default();
    let history = OwnedHistory::capture(&HistoryView {
        recent: &recent,
        frequency: &frequency,
    });
    assert_eq!(history.recency(ids::RENAME_TAB), Some(0));
    assert_eq!(history.recency(ids::NEW_TAB), None);
    let runtime = RuntimeId::new(3);
    let _ = TabId::in_runtime(runtime, 1);
    let mut engine = CommandSearch::new();
    let ranked = engine.rank("", catalog().iter(), &history);
    assert_eq!(ranked[0].spec.id, ids::RENAME_TAB);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one hierarchy drives the pane picker of both move commands"
)]
fn pane_and_side_pickers_build_the_explicit_move_invocation() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let tab = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
    let focused = tab.first_pane().clone();
    let other = mux
        .split_pane(
            workspace,
            tab.id,
            focused.id,
            huterm_protocol::SplitDirection::Right,
            &terminal_command(),
        )
        .unwrap()
        .pane;
    let third = mux
        .split_pane(
            workspace,
            tab.id,
            other.id,
            huterm_protocol::SplitDirection::Down,
            &terminal_command(),
        )
        .unwrap()
        .pane;
    let second = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
    let (state, _) = mux.subscribe_hierarchy();
    let published =
        |terminal: TerminalId| (terminal == other.terminal_id).then_some("vim");
    let hierarchy = PaletteHierarchy::from_projection_with(
        &state,
        |_| None,
        published,
        &[],
    );
    let target = PaletteTarget {
        session: Some(session),
        workspace: Some(workspace),
        tab: Some(tab.id),
        terminal: Some(focused.terminal_id),
        ..target()
    };
    let domain = DomainView {
        hierarchy: &hierarchy,
        profiles: &[],
        layouts: &[],
        target: &target,
        window_only: true,
        pane_exclusion: PaneExclusion::OwnPane,
    };
    // The pane being moved is never offered; another pane is labelled by
    // its published title, else its program name.
    assert_eq!(
        domain.values(ArgumentKind::Pane),
        vec![
            CommandValue::Pane(other.id),
            CommandValue::Pane(third.id),
            CommandValue::Pane(second.first_pane().id),
        ]
    );
    // A tab move leaves out the moving tab's own panes instead, so moving
    // an inactive tab offers the focused pane.
    let moving = |tab: TabId| DomainView {
        pane_exclusion: PaneExclusion::OwnTab(tab),
        ..domain
    };
    assert_eq!(
        moving(second.id).values(ArgumentKind::Pane),
        [focused.id, other.id, third.id].map(CommandValue::Pane)
    );
    assert_eq!(
        moving(tab.id).values(ArgumentKind::Pane),
        vec![CommandValue::Pane(second.first_pane().id)]
    );
    assert_eq!(
        domain.label(ArgumentKind::Pane, &CommandValue::Pane(third.id)),
        Some("sh".to_owned())
    );
    assert_eq!(
        domain.label(ArgumentKind::Pane, &CommandValue::Pane(other.id)),
        Some("vim".to_owned())
    );
    assert_eq!(
        domain.values(ArgumentKind::Side),
        ["left", "right", "up", "down"]
            .map(|side| CommandValue::Text(side.to_owned()))
    );
    let spec =
        huterm_protocol::lookup(ids::MOVE_PANE_TO_PANE.as_str()).unwrap();
    let mut editor = SlotEditor::new(spec, &[], &domain, false).unwrap();
    assert_eq!(editor.active().spec.name, "pane");
    assert!(matches!(
        editor.commit(Some(CommandValue::Pane(other.id))),
        Ok(Commit::Next)
    ));
    assert_eq!(editor.active().spec.name, "side");
    let Ok(Commit::Run(invocation)) =
        editor.commit(Some(CommandValue::Text("down".to_owned())))
    else {
        panic!("picking the side runs the move");
    };
    let explicit = CommandInvocation::new(
        ids::MOVE_PANE_TO_PANE,
        vec![
            CommandArgument::new("pane", CommandValue::Pane(other.id)),
            CommandArgument::new("side", CommandValue::Text("down".into())),
        ],
    );
    assert_eq!(invocation, explicit);
    assert!(huterm_protocol::validate(&invocation).is_ok());
    mux.close_session(session).unwrap();
}

#[test]
fn rename_pane_defaults_to_the_focused_pane_and_prefills_its_custom_name() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let tab = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
    let focused = tab.first_pane().clone();
    let other = mux
        .split_pane(
            workspace,
            tab.id,
            focused.id,
            huterm_protocol::SplitDirection::Right,
            &terminal_command(),
        )
        .unwrap()
        .pane;
    mux.rename_pane(focused.id, Some("logs")).unwrap();
    let (state, _) = mux.subscribe_hierarchy();
    // A custom name wins over the title a window published.
    let hierarchy = PaletteHierarchy::from_projection_with(
        &state,
        |_| None,
        |_| Some("vim"),
        &[],
    );
    let target = PaletteTarget {
        session: Some(session),
        workspace: Some(workspace),
        tab: Some(tab.id),
        terminal: Some(focused.terminal_id),
        ..target()
    };
    let spec = huterm_protocol::lookup(ids::RENAME_PANE.as_str()).unwrap();
    let domain = DomainView {
        hierarchy: &hierarchy,
        profiles: &[],
        layouts: &[],
        target: &target,
        window_only: false,
        pane_exclusion: PaneExclusion::Nothing,
    };
    assert_eq!(
        domain.values(ArgumentKind::Pane),
        [focused.id, other.id].map(CommandValue::Pane),
        "a rename offers the focused pane too"
    );
    assert_eq!(
        domain.label(ArgumentKind::Pane, &CommandValue::Pane(focused.id)),
        Some("logs".to_owned())
    );
    assert_eq!(
        domain.label(ArgumentKind::Pane, &CommandValue::Pane(other.id)),
        Some("vim".to_owned())
    );
    let editor = SlotEditor::new(spec, &[], &domain, false).unwrap();
    assert_eq!(editor.active().spec.name, "name");
    assert_eq!(
        current_custom_name(&editor, &domain).as_deref(),
        Some("logs")
    );
    // The move picker still leaves the focused pane out.
    let moving = DomainView {
        pane_exclusion: PaneExclusion::OwnPane,
        ..domain
    };
    assert_eq!(
        moving.values(ArgumentKind::Pane),
        vec![CommandValue::Pane(other.id)]
    );
    mux.close_session(session).unwrap();
}

#[test]
fn the_layout_picker_lists_built_ins_by_title_then_user_layouts_by_name() {
    let hierarchy = PaletteHierarchy::default();
    let target = target();
    let config = huterm_config::PanesConfig {
        layouts: [
            (
                "pair".to_owned(),
                "rows(columns(pane, pane) 70%, columns(*))".to_owned(),
            ),
            ("dev".to_owned(), "columns(pane 55%, rows(*))".to_owned()),
        ]
        .into(),
        ..huterm_config::PanesConfig::default()
    };
    let layouts = layout_rows(&config.layouts());
    let domain = DomainView {
        hierarchy: &hierarchy,
        profiles: &[],
        layouts: &layouts,
        target: &target,
        window_only: true,
        pane_exclusion: PaneExclusion::OwnPane,
    };
    let rows: Vec<(&str, &str)> = domain
        .rows(ArgumentKind::Layout)
        .into_iter()
        .map(|row| (row.label.as_str(), row.detail.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("Equal Columns", "columns(*)"),
            ("Equal Rows", "rows(*)"),
            ("Main Left", "columns(pane 60%, rows(*))"),
            ("Main Top", "rows(pane 60%, columns(*))"),
            ("Main Centered", "columns(rows(*), pane 50%, rows(*))"),
            ("Tiled", "grid(*)"),
            ("dev", "columns(pane 55%, rows(*))"),
            ("pair", "rows(columns(pane, pane) 70%, columns(*))"),
        ]
    );
    // Picking a row gives the invocation a binding would carry.
    let spec = huterm_protocol::lookup(ids::ARRANGE_PANES.as_str()).unwrap();
    let mut editor = SlotEditor::new(spec, &[], &domain, false).unwrap();
    assert_eq!(editor.active().spec.name, "layout");
    let Ok(Commit::Run(invocation)) =
        editor.commit(Some(CommandValue::Text("dev".to_owned())))
    else {
        panic!("picking the layout runs the command");
    };
    assert_eq!(
        invocation,
        CommandInvocation::new(
            ids::ARRANGE_PANES,
            vec![CommandArgument::new(
                "layout",
                CommandValue::Text("dev".into())
            )],
        )
    );
}

/// The palette in a window: its lists, their highlight, and its events.
#[cfg(feature = "test-support")]
mod lists {
    use std::cell::RefCell;

    use gpui::{
        Modifiers, Pixels, Point, TestAppContext, VisualTestContext, point,
    };

    use super::*;
    use crate::config::Theme;
    use crate::desktop::overlay::overlay_colors;

    type Events = Rc<RefCell<Vec<PaletteEvent>>>;

    fn runtime() -> RuntimeId {
        RuntimeId::new(9)
    }

    fn tab(value: u64) -> CommandValue {
        CommandValue::Tab(TabId::in_runtime(runtime(), value))
    }

    /// The scope of a window showing workspace 10 of [`projected`], with
    /// tab 2 active.
    fn scope() -> PaletteScope {
        PaletteScope {
            session: Some(SessionId::in_runtime(runtime(), 1)),
            workspace: Some(WorkspaceId::in_runtime(runtime(), 10)),
            tab: Some(TabId::in_runtime(runtime(), 2)),
            terminal: None,
            terminal_view: None,
        }
    }

    /// A palette over [`projected`] in [`scope`], opened on `request`'s
    /// slots or else on command search, and the events it emits.
    fn open(
        cx: &mut TestAppContext,
        request: Option<CommandInvocation>,
    ) -> (Entity<CommandPalette>, &mut VisualTestContext, Events) {
        let (palette, cx) = cx.add_window_view(move |window, cx| {
            let scope = scope();
            let target = PaletteTarget {
                session: scope.session,
                workspace: scope.workspace,
                tab: scope.tab,
                ..target()
            };
            let (_, keymap) =
                compile(Platform::MacOs, &[]).unwrap().install_parts();
            CommandPalette::open(
                PaletteOpen {
                    target,
                    keymap,
                    availability: HashMap::new(),
                    titles: HashMap::new(),
                    colors: overlay_colors(&Theme::huterm_dark()),
                    placement: PalettePlacement::Top,
                    history: OwnedHistory::default(),
                    profiles: Vec::new(),
                    layouts: Vec::new(),
                    request: request.as_ref(),
                    retained_query: None,
                    hierarchy: PaletteHierarchy::from_projection(
                        &projected(),
                        |_| None,
                        &[],
                    ),
                    hierarchy_seq: 0,
                    clock: FrameClock::new(window),
                },
                cx,
            )
        });
        let events = Events::default();
        let log = Rc::clone(&events);
        cx.update(|_, cx| {
            cx.subscribe(&palette, move |_, event: &PaletteEvent, _| {
                log.borrow_mut().push(event.clone());
            })
            .detach();
        });
        cx.run_until_parked();
        (palette, cx, events)
    }

    fn run(
        palette: &Entity<CommandPalette>,
        cx: &mut VisualTestContext,
        id: CommandId,
    ) {
        palette.update(cx, |palette, cx| {
            palette
                .run(&CommandInvocation::new(id, Vec::new()), cx)
                .unwrap();
        });
        cx.run_until_parked();
    }

    /// Writes `text` into the field, as typing there does.
    fn type_query(
        palette: &Entity<CommandPalette>,
        cx: &mut VisualTestContext,
        text: &str,
    ) {
        let input = palette.read_with(cx, |palette, _| palette.input.clone());
        input.update(cx, |input, cx| input.set_text(text, cx));
        cx.run_until_parked();
    }

    /// The highlighted picker value, or the highlighted result's command.
    fn highlighted(
        palette: &Entity<CommandPalette>,
        cx: &mut VisualTestContext,
    ) -> Option<String> {
        palette.read_with(cx, |palette, cx| match &palette.stage {
            Stage::Search(_) => palette
                .selected_match(cx)
                .map(|matched| matched.spec.id.as_str().to_owned()),
            Stage::Slots(_) => {
                palette.picker_value(cx).map(|value| display_value(&value))
            }
        })
    }

    /// The ids of the list's rows.
    fn rows(
        palette: &Entity<CommandPalette>,
        cx: &mut VisualTestContext,
    ) -> Vec<String> {
        palette.read_with(cx, |palette, cx| {
            palette.list.as_ref().map_or_else(Vec::new, |list| {
                list.read(cx)
                    .options()
                    .iter()
                    .map(|option| option.id.to_string())
                    .collect()
            })
        })
    }

    fn diagnostic(
        palette: &Entity<CommandPalette>,
        cx: &mut VisualTestContext,
    ) -> Option<String> {
        palette.read_with(cx, |palette, _| palette.diagnostic.clone())
    }

    /// Replaces the identity rows with `state`'s, as a projection update
    /// does.
    fn rebuild(
        palette: &Entity<CommandPalette>,
        cx: &mut VisualTestContext,
        state: &HierarchyState,
    ) {
        palette.update(cx, |palette, cx| {
            let hierarchy =
                PaletteHierarchy::from_projection(state, |_| None, &[]);
            palette.set_hierarchy(hierarchy, scope(), state.seq(), cx);
        });
        cx.run_until_parked();
    }

    /// The center of the list's row at `row`, as last laid out.
    fn row_center(
        palette: &Entity<CommandPalette>,
        cx: &mut VisualTestContext,
        row: u8,
    ) -> Point<Pixels> {
        let (_, bounds) = palette.read_with(cx, |palette, cx| {
            palette.list.as_ref().unwrap().read(cx).scroll_position()
        });
        let y = LIST_PADDING + ROW_HEIGHT * (f32::from(row) + 0.5);
        point(bounds.center().x, bounds.top() + px(y))
    }

    /// Clicks the list's row at `row`.
    fn click_row(
        palette: &Entity<CommandPalette>,
        cx: &mut VisualTestContext,
        row: u8,
    ) {
        let center = row_center(palette, cx, row);
        cx.simulate_click(center, Modifiers::none());
        cx.run_until_parked();
    }

    fn executed(events: &Events) -> Vec<CommandInvocation> {
        events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                PaletteEvent::Execute(invocation) => Some(invocation.clone()),
                PaletteEvent::Cancel { .. } => None,
            })
            .collect()
    }

    #[gpui::test]
    fn each_query_highlights_its_first_result_and_a_click_runs_a_row(
        cx: &mut TestAppContext,
    ) {
        let (palette, cx, events) = open(cx, None);
        type_query(&palette, cx, "scroll");
        let results = rows(&palette, cx);
        assert!(results.len() > 2, "{results:?}");
        assert_eq!(highlighted(&palette, cx), results.first().cloned());
        run(&palette, cx, ids::PALETTE_SELECT_PREVIOUS);
        assert_eq!(highlighted(&palette, cx), results.first().cloned());
        run(&palette, cx, ids::PALETTE_SELECT_NEXT);
        assert_eq!(highlighted(&palette, cx), results.get(1).cloned());
        // Another query starts again at its own first result.
        type_query(&palette, cx, "scroll to");
        let narrower = rows(&palette, cx);
        assert_ne!(narrower, results);
        assert_eq!(highlighted(&palette, cx), narrower.first().cloned());
        click_row(&palette, cx, 1);
        assert_eq!(
            executed(&events)
                .iter()
                .map(|invocation| invocation.id.as_str().to_owned())
                .collect::<Vec<_>>(),
            narrower.get(1).cloned().into_iter().collect::<Vec<_>>()
        );
    }

    #[gpui::test]
    fn hovering_a_result_leaves_enter_on_the_highlighted_one(
        cx: &mut TestAppContext,
    ) {
        let (palette, cx, events) = open(cx, None);
        type_query(&palette, cx, "scroll");
        let results = rows(&palette, cx);
        assert!(results.len() > 2, "{results:?}");
        let third = row_center(&palette, cx, 2);
        cx.simulate_mouse_move(third, None, Modifiers::none());
        cx.run_until_parked();
        let hovered = palette.read_with(cx, CommandPalette::hovered_row);
        assert_eq!(hovered, Some(2), "the pointer reached the third row");
        assert_eq!(highlighted(&palette, cx), results.first().cloned());
        run(&palette, cx, ids::PALETTE_CONFIRM);
        assert_eq!(
            executed(&events)
                .iter()
                .map(|invocation| invocation.id.as_str().to_owned())
                .collect::<Vec<_>>(),
            results.first().cloned().into_iter().collect::<Vec<_>>()
        );
    }

    #[gpui::test]
    fn a_picker_keeps_its_highlight_through_rebuilds_and_never_moves_it(
        cx: &mut TestAppContext,
    ) {
        let request = CommandInvocation::new(ids::SELECT_TAB, Vec::new());
        let (palette, cx, events) = open(cx, Some(request));
        assert_eq!(rows(&palette, cx).len(), 2);
        // A required slot has no value, so the first row starts highlighted.
        assert_eq!(highlighted(&palette, cx), Some(display_value(&tab(1))));
        run(&palette, cx, ids::PALETTE_SELECT_NEXT);
        assert_eq!(highlighted(&palette, cx), Some(display_value(&tab(2))));
        // A reorder keeps the highlight on its tab at its new row.
        let mut state = projected();
        apply(
            &mut state,
            huterm_protocol::HierarchyEvent::TabMoved {
                tab: TabId::in_runtime(runtime(), 2),
                workspace: WorkspaceId::in_runtime(runtime(), 10),
                index: 0,
            },
        );
        rebuild(&palette, cx, &state);
        assert_eq!(
            rows(&palette, cx).first(),
            Some(&value_id(&tab(2)).to_string())
        );
        assert_eq!(highlighted(&palette, cx), Some(display_value(&tab(2))));
        // Closing the highlighted tab leaves nothing highlighted, through a
        // later rebuild that lists it again too, and Enter commits nothing.
        let mut closed = projected();
        apply(
            &mut closed,
            huterm_protocol::HierarchyEvent::TabClosed {
                tab: TabId::in_runtime(runtime(), 2),
            },
        );
        rebuild(&palette, cx, &closed);
        assert_eq!(highlighted(&palette, cx), None);
        rebuild(&palette, cx, &projected());
        assert_eq!(highlighted(&palette, cx), None);
        run(&palette, cx, ids::PALETTE_CONFIRM);
        assert_eq!(executed(&events), []);
        assert_eq!(diagnostic(&palette, cx), None);
        // Up highlights the last row again.
        run(&palette, cx, ids::PALETTE_SELECT_PREVIOUS);
        assert_eq!(highlighted(&palette, cx), Some(display_value(&tab(2))));
        // Filtering starts again at the first row, even from a lost
        // highlight.
        rebuild(&palette, cx, &closed);
        assert_eq!(highlighted(&palette, cx), None);
        type_query(&palette, cx, "s");
        assert_eq!(highlighted(&palette, cx), Some(display_value(&tab(1))));
        // A filter that matches nothing says so on Enter, and a rebuild
        // that brings a match highlights it.
        type_query(&palette, cx, "zz");
        assert_eq!(rows(&palette, cx), Vec::<String>::new());
        run(&palette, cx, ids::PALETTE_CONFIRM);
        assert_eq!(diagnostic(&palette, cx), Some("No matching tab".into()));
        let mut renamed = projected();
        apply(
            &mut renamed,
            huterm_protocol::HierarchyEvent::TabRenamed {
                tab: TabId::in_runtime(runtime(), 2),
                custom_name: Some("zz".into()),
            },
        );
        rebuild(&palette, cx, &renamed);
        assert_eq!(highlighted(&palette, cx), Some(display_value(&tab(2))));
        type_query(&palette, cx, "");
        click_row(&palette, cx, 0);
        assert_eq!(
            executed(&events),
            [CommandInvocation::new(
                ids::SELECT_TAB,
                vec![CommandArgument::new("tab", tab(1))],
            )]
        );
    }

    /// The list's scroll offset from its top.
    fn scroll_offset(
        palette: &Entity<CommandPalette>,
        cx: &mut VisualTestContext,
    ) -> Pixels {
        palette.read_with(cx, |palette, cx| {
            palette.list.as_ref().unwrap().read(cx).scroll_position().0
        })
    }

    /// Scrolls the list down with the wheel over its first row.
    fn wheel_down(
        palette: &Entity<CommandPalette>,
        cx: &mut VisualTestContext,
    ) {
        // The wheel scrolls what the pointer is over, as of a frame drawn
        // after the pointer arrived.
        let center = row_center(palette, cx, 0);
        cx.simulate_mouse_move(center, None, Modifiers::none());
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.simulate_event(gpui::ScrollEvent {
            position: center,
            delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-150.0))),
            modifiers: Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
            ..Default::default()
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_new_query_scrolls_back_to_its_first_result(cx: &mut TestAppContext) {
        let (palette, cx, _) = open(cx, None);
        let first = rows(&palette, cx).first().cloned().unwrap();
        // A prefix of the first result's title that keeps it first, on the
        // same row, among more results than show at once.
        let title = huterm_protocol::lookup(&first).unwrap().title;
        let query = (1..=title.len())
            .filter_map(|end| title.get(..end))
            .find(|prefix| {
                type_query(&palette, cx, prefix);
                let rows = rows(&palette, cx);
                rows.first() == Some(&first) && rows.len() > 6
            })
            .expect("a prefix that keeps the first result first")
            .to_owned();
        type_query(&palette, cx, "");
        wheel_down(&palette, cx);
        assert!(
            scroll_offset(&palette, cx) > px(0.0),
            "the wheel scrolled nothing"
        );
        type_query(&palette, cx, &query);
        assert_eq!(rows(&palette, cx).first(), Some(&first));
        assert_eq!(scroll_offset(&palette, cx), px(0.0));
    }

    fn close_tab(value: u64) -> HierarchyState {
        let mut state = projected();
        apply(
            &mut state,
            huterm_protocol::HierarchyEvent::TabClosed {
                tab: TabId::in_runtime(runtime(), value),
            },
        );
        state
    }

    #[gpui::test]
    fn a_rebuild_loses_only_a_highlighted_row_that_it_removes(
        cx: &mut TestAppContext,
    ) {
        let request = CommandInvocation::new(ids::SELECT_TAB, Vec::new());
        let (palette, cx, events) = open(cx, Some(request));
        run(&palette, cx, ids::PALETTE_SELECT_NEXT);
        assert_eq!(highlighted(&palette, cx), Some(display_value(&tab(2))));
        // Removing the row above keeps the highlight on its tab, now first.
        rebuild(&palette, cx, &close_tab(1));
        assert_eq!(highlighted(&palette, cx), Some(display_value(&tab(2))));
        rebuild(&palette, cx, &projected());
        assert_eq!(highlighted(&palette, cx), Some(display_value(&tab(2))));
        // Removing the highlighted last row never moves the highlight to
        // the row that stays.
        rebuild(&palette, cx, &close_tab(2));
        assert_eq!(highlighted(&palette, cx), None);
        run(&palette, cx, ids::PALETTE_CONFIRM);
        assert_eq!(executed(&events), []);
    }

    #[gpui::test]
    fn a_rebuild_keeps_a_scrolled_picker_where_the_user_left_it(
        cx: &mut TestAppContext,
    ) {
        use huterm_protocol::{PaneId, PaneInfo, PaneLayout, TabInfo};
        let request = CommandInvocation::new(ids::SELECT_TAB, Vec::new());
        let (palette, cx, _) = open(cx, Some(request));
        let mut many = projected();
        for value in 3..=12_u64 {
            apply(
                &mut many,
                huterm_protocol::HierarchyEvent::TabOpened {
                    workspace: WorkspaceId::in_runtime(runtime(), 10),
                    tab: TabInfo {
                        id: TabId::in_runtime(runtime(), value),
                        custom_name: None,
                        layout: PaneLayout::single(PaneInfo {
                            id: PaneId::in_runtime(runtime(), value),
                            terminal_id: TerminalId::new(value),
                            fallback_name: "sh".into(),
                            custom_name: None,
                        }),
                        zoomed: None,
                        input_sync: false,
                    },
                    index: u32::try_from(value - 1).unwrap(),
                },
            );
        }
        rebuild(&palette, cx, &many);
        assert_eq!(rows(&palette, cx).len(), 12);
        let highlight = highlighted(&palette, cx);
        wheel_down(&palette, cx);
        let scrolled = scroll_offset(&palette, cx);
        assert!(scrolled > px(0.0), "the wheel scrolled nothing");
        // Another tab's new title rebuilds the rows in place.
        apply(
            &mut many,
            huterm_protocol::HierarchyEvent::TabRenamed {
                tab: TabId::in_runtime(runtime(), 7),
                custom_name: Some("logs".into()),
            },
        );
        rebuild(&palette, cx, &many);
        assert_eq!(highlighted(&palette, cx), highlight);
        assert_eq!(scroll_offset(&palette, cx), scrolled);
    }

    #[gpui::test]
    fn a_reopened_slot_highlights_its_value(cx: &mut TestAppContext) {
        // Rename's tab slot holds the active tab, the second row.
        let request = CommandInvocation::new(ids::RENAME_TAB, Vec::new());
        let (palette, cx, _) = open(cx, Some(request));
        assert_eq!(rows(&palette, cx), Vec::<String>::new());
        run(&palette, cx, ids::PALETTE_NEXT_SLOT);
        assert_eq!(rows(&palette, cx).len(), 2);
        assert_eq!(highlighted(&palette, cx), Some(display_value(&tab(2))));
    }
}
