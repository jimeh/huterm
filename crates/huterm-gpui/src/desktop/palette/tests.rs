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

#[test]
fn forwarded_text_commands_cover_the_catalog() {
    let catalog_text_commands = catalog()
        .iter()
        .filter(|spec| spec.id.as_str().starts_with("text_"))
        .map(|spec| spec.id)
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
        target: &target,
        window_only: true,
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
        target: &target,
        window_only: false,
    };
    let mut editor = SlotEditor::new(rename, &[], &domain, false);
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
        target: &target,
        window_only: false,
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
        let editor = SlotEditor::new(spec, &[], &domain, false);
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
        target: &target,
        window_only: false,
    };
    let mut editor = SlotEditor::new(rename, &[], &domain, false);
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
        target: &target,
        window_only: false,
    };
    let toggle = huterm_protocol::lookup(ids::TOGGLE_QUAKE.as_str()).unwrap();
    assert!(slots::runs_without_prompt(toggle, &domain));
    let editor = SlotEditor::new(toggle, &[], &domain, false);
    assert_eq!(editor.slots()[0].state, SlotState::Prefilled);
    assert_eq!(
        editor.invocation().unwrap().text("profile"),
        Some("default")
    );
    let mut picking = SlotEditor::new(toggle, &[], &domain, false);
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
        HierarchyEvent, PaneId, SessionInfo, StreamId, TabInfo, WorkspaceInfo,
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
                pane_id: PaneId::new(value),
                terminal_id: TerminalId::new(value),
                custom_name: None,
                fallback_name: "sh".into(),
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
fn a_move_rebuilds_order_positions_and_parents_and_keeps_the_highlight() {
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
    // The highlighted identity stays highlighted at its new row.
    let highlight =
        rebuilt_highlight(&Highlight::Initial, Some(first.clone()), |value| {
            after.tabs.iter().any(|row| &row.value == value)
        });
    assert_eq!(highlight, Highlight::Value(first.clone()));
    let position = |value: &CommandValue| {
        after.tabs.iter().position(|row| &row.value == value)
    };
    assert_eq!(highlighted_row(&highlight, position, None, 2), Some(1));
}

#[test]
fn a_rebuild_scrolls_only_when_the_highlighted_row_moves() {
    // An unrelated title change keeps the user's scroll position.
    assert_eq!(rebuild_scroll(Some(4), Some(4)), None);
    assert_eq!(rebuild_scroll(None, None), None);
    // A reorder that moves the highlighted row follows it.
    assert_eq!(rebuild_scroll(Some(4), Some(1)), Some(1));
    assert_eq!(rebuild_scroll(None, Some(0)), Some(0));
    // A lost highlight scrolls nowhere.
    assert_eq!(rebuild_scroll(Some(2), None), None);
}

#[test]
fn removing_the_highlighted_value_leaves_no_highlight_until_moved_or_filtered()
{
    let gone = CommandValue::Tab(TabId::in_runtime(RuntimeId::new(9), 7));
    let highlight =
        rebuilt_highlight(&Highlight::Value(gone.clone()), Some(gone), |_| {
            false
        });
    assert_eq!(highlight, Highlight::Lost);
    let position = |_: &CommandValue| Some(0);
    assert_eq!(highlighted_row(&highlight, position, None, 3), None);
    assert!(enter_blocked(&highlight, ArgumentKind::Tab));
    assert!(!enter_blocked(&highlight, ArgumentKind::Text));
    assert!(!enter_blocked(&Highlight::Initial, ArgumentKind::Tab));
    // A lost highlight stays lost through a later rebuild.
    assert_eq!(
        rebuilt_highlight(&highlight, None, |_| true),
        Highlight::Lost
    );
    // Down selects the first row and Up the last.
    assert_eq!(moved_row(None, 1, 3), Some(0));
    assert_eq!(moved_row(None, -1, 3), Some(2));
    assert_eq!(moved_row(None, 1, 0), None);
    assert_eq!(moved_row(Some(1), 8, 3), Some(2));
    // A stale click on a row a rebuild removed commits nothing.
    let removed = CommandValue::Tab(TabId::in_runtime(RuntimeId::new(9), 5));
    let clicked = clicked_highlight(removed.clone(), false);
    assert_eq!(clicked, Highlight::Lost);
    assert!(enter_blocked(&clicked, ArgumentKind::Tab));
    assert_eq!(
        clicked_highlight(removed.clone(), true),
        Highlight::Value(removed.clone())
    );
    // An explicit value that is no longer listed highlights nothing,
    // never the preselected or first row.
    let preselected =
        CommandValue::Tab(TabId::in_runtime(RuntimeId::new(9), 2));
    let only_preselected =
        |value: &CommandValue| (value == &preselected).then_some(1);
    assert_eq!(
        highlighted_row(
            &Highlight::Value(removed),
            only_preselected,
            Some(&preselected),
            3
        ),
        None
    );
    // Changing the filter restores the initial selection: the slot's
    // value when listed, else the first row.
    let slot = CommandValue::Tab(TabId::in_runtime(RuntimeId::new(9), 2));
    let listed = |value: &CommandValue| (value == &slot).then_some(1);
    assert_eq!(
        highlighted_row(&Highlight::Initial, listed, Some(&slot), 3),
        Some(1)
    );
    assert_eq!(
        highlighted_row(&Highlight::Initial, |_| None, None, 3),
        Some(0)
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
        target: &target,
        window_only: true,
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
