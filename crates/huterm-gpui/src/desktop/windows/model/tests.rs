use super::*;
use gpui::{Bounds, point, px, size};

fn window(id: u64) -> WindowId {
    WindowId::from(id)
}

fn layout(x: f32) -> WindowLayout {
    WindowLayout {
        bounds: WindowBounds::Windowed(Bounds {
            origin: point(px(x), px(10.0)),
            size: size(px(800.0), px(600.0)),
        }),
        sidebar_width: px(180.0),
    }
}

fn entry(tab: u64) -> TabEntry {
    TabEntry {
        id: TabId::new(tab),
        terminal: TerminalId::new(tab),
        title: format!("tab {tab}"),
    }
}

fn quake(profile: &str, visible: bool) -> QuakeRecord {
    QuakeRecord {
        profile: profile.to_owned(),
        visible,
    }
}

fn ids(record: &WindowRecord) -> Vec<TabId> {
    record.tabs.iter().map(|entry| entry.id).collect()
}

/// An ordinary window 1 attached with tabs 1..=count, activated in order.
fn attached(count: u64) -> WindowModel {
    let mut model = WindowModel::default();
    model.open(window(1), None, layout(5.0));
    model.attach(
        window(1),
        Some(AttachmentId::new(1)),
        SessionId::new(1),
        WorkspaceId::new(1),
    );
    for tab in 1..=count {
        model.open_tab(window(1), entry(tab));
    }
    model
}

#[test]
fn open_attach_and_detach_track_attachment_and_session() {
    let mut model = WindowModel::default();
    model.open(window(1), None, layout(5.0));
    let record = model.record(window(1)).unwrap();
    assert_eq!(
        (record.attachment, record.session, record.workspace),
        (None, None, None)
    );

    model.attach(
        window(1),
        Some(AttachmentId::new(7)),
        SessionId::new(2),
        WorkspaceId::new(3),
    );
    // A later spawn into the same workspace creates no attachment and keeps
    // the recorded one.
    model.attach(window(1), None, SessionId::new(2), WorkspaceId::new(3));
    let record = model.record(window(1)).unwrap();
    assert_eq!(record.attachment, Some(AttachmentId::new(7)));
    assert_eq!(record.session, Some(SessionId::new(2)));
    assert_eq!(record.workspace, Some(WorkspaceId::new(3)));

    model.detach_attachment(window(1));
    let record = model.record(window(1)).unwrap();
    assert_eq!((record.attachment, record.session), (None, None));
    assert_eq!(record.workspace, Some(WorkspaceId::new(3)));
}

#[test]
fn opening_tabs_appends_and_activates_them() {
    let model = attached(3);
    let record = model.record(window(1)).unwrap();
    assert_eq!(ids(record), [TabId::new(1), TabId::new(2), TabId::new(3)]);
    assert_eq!(record.active, Some(TabId::new(3)));
    assert_eq!(
        record.history,
        [TabId::new(3), TabId::new(2), TabId::new(1)]
    );
}

#[test]
fn selection_records_activation_and_ignores_foreign_tabs() {
    let mut model = attached(3);
    assert!(model.select(window(1), TabId::new(1)));
    assert!(!model.select(window(1), TabId::new(9)));
    assert!(!model.select(window(2), TabId::new(1)));
    let record = model.record(window(1)).unwrap();
    assert_eq!(record.active, Some(TabId::new(1)));
    assert_eq!(
        record.history,
        [TabId::new(1), TabId::new(3), TabId::new(2)]
    );
}

#[test]
fn activation_history_tracks_selection_and_prunes_closed_tabs() {
    let (first, second, third) = (TabId::new(1), TabId::new(2), TabId::new(3));
    let mut history = Vec::new();

    record_tab_activation(&mut history, first);
    record_tab_activation(&mut history, second);
    record_tab_activation(&mut history, third);
    record_tab_activation(&mut history, first);
    assert_eq!(history, [first, third, second]);

    prune_tab_history(&mut history, third);
    assert_eq!(history, [first, second]);
}

#[test]
fn recent_tab_toggles_between_two_tabs_and_is_empty_alone() {
    let mut model = attached(2);
    assert_eq!(model.recent_tab(window(1)), Some(TabId::new(1)));
    model.select(window(1), TabId::new(1));
    assert_eq!(model.recent_tab(window(1)), Some(TabId::new(2)));
    model.select(window(1), TabId::new(2));
    let record = model.record(window(1)).unwrap();
    assert_eq!(record.active, Some(TabId::new(2)));
    assert_eq!(record.history, [TabId::new(2), TabId::new(1)]);

    assert_eq!(attached(1).recent_tab(window(1)), None);
    assert_eq!(model.recent_tab(window(9)), None);
}

#[test]
fn applying_canonical_order_retains_view_state_and_rejects_stale_reply() {
    let a = TabId::new(1);
    let b = TabId::new(2);
    let c = TabId::new(3);
    let active = b;
    let mut views = vec![
        (a, "selection-a", 123),
        (b, "selection-b", 456),
        (c, "selection-c", 789),
    ];
    assert!(apply_tab_order(&mut views, &[c, a, b], |view| view.0));
    assert_eq!(
        views,
        [
            (c, "selection-c", 789),
            (a, "selection-a", 123),
            (b, "selection-b", 456)
        ]
    );
    assert_eq!(views.iter().find(|view| view.0 == active).unwrap().2, 456);
    let before = views.clone();
    assert!(!apply_tab_order(&mut views, &[a, a, c], |view| view.0));
    assert!(!apply_tab_order(
        &mut views,
        &[a, b, TabId::new(4)],
        |view| { view.0 }
    ));
    assert_eq!(views, before);
}

#[test]
fn model_reorder_keeps_entries_and_rejects_mismatched_orders() {
    let mut model = attached(3);
    let (a, b, c) = (TabId::new(1), TabId::new(2), TabId::new(3));
    assert!(model.apply_order(window(1), &[c, a, b]));
    let record = model.record(window(1)).unwrap();
    assert_eq!(ids(record), [c, a, b]);
    assert_eq!(record.tabs[0], entry(3));
    assert_eq!(record.active, Some(c));

    assert!(!model.apply_order(window(1), &[c, a]));
    assert!(!model.apply_order(window(9), &[c, a, b]));
    assert_eq!(ids(model.record(window(1)).unwrap()), [c, a, b]);
}

#[test]
fn closing_one_tab_moves_the_active_tab_like_the_tab_bar() {
    let (a, b, c) = (TabId::new(1), TabId::new(2), TabId::new(3));
    // Closing the active middle tab selects the tab that takes its place.
    let mut model = attached(3);
    model.select(window(1), b);
    assert_eq!(model.close_tabs(window(1), &[b]), Some(c));
    let record = model.record(window(1)).unwrap();
    assert_eq!(ids(record), [a, c]);
    assert_eq!(record.history, [c, a]);

    // Closing the active last tab selects the new last tab.
    assert_eq!(model.close_tabs(window(1), &[c]), Some(a));
    // Closing an inactive tab keeps the active one; a repeat is a no-op.
    let mut model = attached(3);
    assert_eq!(model.close_tabs(window(1), &[a]), Some(c));
    assert_eq!(model.close_tabs(window(1), &[a]), Some(c));
    assert_eq!(ids(model.record(window(1)).unwrap()), [b, c]);
}

#[test]
fn closing_several_tabs_matches_close_other_tabs_and_close_tabs_after() {
    let tab = TabId::new;
    // Close Other Tabs from inactive tab 2 in a window showing tab 4.
    let mut model = attached(4);
    assert_eq!(
        model.close_tabs(window(1), &[tab(1), tab(3), tab(4)]),
        Some(tab(2))
    );
    let record = model.record(window(1)).unwrap();
    assert_eq!(ids(record), [tab(2)]);
    assert_eq!(record.history, [tab(2)]);

    // Close Tabs After tab 1 while tab 3 is active.
    let mut model = attached(4);
    model.select(window(1), tab(3));
    assert_eq!(
        model.close_tabs(window(1), &[tab(2), tab(3), tab(4)]),
        Some(tab(1))
    );
    let record = model.record(window(1)).unwrap();
    assert_eq!(ids(record), [tab(1)]);
    assert_eq!(record.history, [tab(1)]);

    // Closing every tab leaves no active tab.
    let mut model = attached(2);
    assert_eq!(model.close_tabs(window(1), &[tab(1), tab(2)]), None);
    assert!(model.record(window(1)).unwrap().history.is_empty());
}

#[test]
fn published_titles_change_only_when_the_string_differs() {
    let mut model = attached(2);
    assert!(!model.set_tab_title(window(1), TabId::new(1), "tab 1"));
    assert!(model.set_tab_title(window(1), TabId::new(1), "vim · exited"));
    assert!(!model.set_tab_title(window(1), TabId::new(9), "missing"));
    assert!(!model.set_tab_title(window(9), TabId::new(1), "missing"));
    assert_eq!(
        model.record(window(1)).unwrap().tabs[0].title,
        "vim · exited"
    );
}

#[test]
fn title_scopes_cover_one_window_or_every_open_window() {
    let mut model = attached(2);
    model.open(window(2), None, layout(50.0));
    model.open_tab(window(2), entry(3));
    model.open(window(3), None, layout(90.0));
    model.open_tab(window(3), entry(4));
    model.begin_close(window(3));

    let titles = |scope| {
        model
            .tab_titles(scope)
            .into_iter()
            .map(|entry| entry.terminal)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        titles(TitleScope::Window(window(1))),
        [TerminalId::new(1), TerminalId::new(2)]
    );
    // A closing window still reads its own titles.
    assert_eq!(titles(TitleScope::Window(window(3))), [TerminalId::new(4)]);
    assert_eq!(
        titles(TitleScope::Open),
        [TerminalId::new(1), TerminalId::new(2), TerminalId::new(3)]
    );
    assert!(titles(TitleScope::Window(window(9))).is_empty());
}

#[test]
fn quake_state_follows_summon_hide_tabs_detach_and_close() {
    let mut model = WindowModel::default();
    assert_eq!(model.quake_state("logs"), None);

    model.open(window(2), Some(quake("logs", true)), layout(5.0));
    assert_eq!(model.quake_window("logs"), Some(window(2)));
    assert_eq!(model.quake_state("logs"), Some((true, 0)));

    model.open_tab(window(2), entry(1));
    model.open_tab(window(2), entry(2));
    model.set_quake_visible(window(2), false);
    assert_eq!(model.quake_state("logs"), Some((false, 2)));

    model.close_tabs(window(2), &[TabId::new(2)]);
    model.set_quake_visible(window(2), true);
    assert_eq!(model.quake_state("logs"), Some((true, 1)));
    assert_eq!(model.quake_state("default"), None);

    // Detaching ends the association; a late visibility publication for the
    // now-ordinary window changes nothing.
    model.detach_quake(window(2));
    assert_eq!(model.quake_window("logs"), None);
    assert_eq!(model.quake_state("logs"), None);
    model.set_quake_visible(window(2), false);
    assert!(model.record(window(2)).unwrap().quake.is_none());

    model.open(window(3), Some(quake("scratch", true)), layout(5.0));
    model.begin_close(window(3));
    assert_eq!(model.quake_window("scratch"), None);
    model.remove(window(3));
    assert!(model.record(window(3)).is_none());
}

#[test]
fn palette_tab_order_lists_recent_tabs_first_and_the_active_tab_last() {
    let (a, b, c) = (TabId::new(1), TabId::new(2), TabId::new(3));
    let mut model = attached(3);
    model.select(window(1), a);
    model.select(window(1), b);
    assert_eq!(model.palette_tab_order(window(1)), [a, c, b]);

    // Tabs missing from history follow the recent ones, in display order.
    model.record_mut(window(1)).unwrap().history = vec![b, c];
    assert_eq!(model.palette_tab_order(window(1)), [c, a, b]);
    assert!(model.palette_tab_order(window(9)).is_empty());
}

#[test]
fn restore_capture_takes_open_attached_windows_in_opening_order() {
    let mut model = attached(2);
    model.set_layout_bounds(window(1), layout(40.0).bounds);
    model.set_sidebar_width(window(1), px(220.0));
    // Not attached yet: nothing to restore.
    model.open(window(2), None, layout(5.0));
    model.open(window(3), Some(quake("logs", false)), layout(70.0));
    model.attach(
        window(3),
        Some(AttachmentId::new(3)),
        SessionId::new(3),
        WorkspaceId::new(3),
    );
    model.open(window(4), None, layout(90.0));
    model.attach(
        window(4),
        Some(AttachmentId::new(4)),
        SessionId::new(4),
        WorkspaceId::new(4),
    );
    model.begin_close(window(4));

    assert_eq!(
        model.restore_windows(TabPosition::Left),
        [
            WindowRestore {
                attachment: AttachmentId::new(1),
                workspace: Some(WorkspaceId::new(1)),
                active: Some(TabId::new(2)),
                bounds: layout(40.0).bounds,
                tab_position: TabPosition::Left,
                sidebar_width: px(220.0),
                quake_profile: None,
            },
            WindowRestore {
                attachment: AttachmentId::new(3),
                workspace: Some(WorkspaceId::new(3)),
                active: None,
                bounds: layout(70.0).bounds,
                tab_position: TabPosition::Left,
                sidebar_width: px(180.0),
                quake_profile: Some("logs".to_owned()),
            },
        ]
    );
}

#[test]
fn every_reader_answers_from_the_model_alone() {
    // No GPUI context exists here: each reader takes only the model.
    let mut model = attached(2);
    model.open(window(2), Some(quake("logs", false)), layout(5.0));
    model.open_tab(window(2), entry(3));

    assert!(model.record(window(1)).is_some());
    assert_eq!(model.quake_window("logs"), Some(window(2)));
    assert_eq!(
        model.quake_windows().collect::<Vec<_>>(),
        [("logs", window(2))]
    );
    assert_eq!(model.quake_state("logs"), Some((false, 1)));
    assert_eq!(
        model.palette_tab_order(window(1)),
        [TabId::new(1), TabId::new(2)]
    );
    assert_eq!(model.recent_tab(window(1)), Some(TabId::new(1)));
    assert_eq!(model.tab_titles(TitleScope::Open).len(), 3);
    assert_eq!(model.restore_windows(TabPosition::Top).len(), 1);
}
