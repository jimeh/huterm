use super::*;
use crate::store::tests::{
    Ids, capture, columns, deep, layout_for, rich_capture,
};

const SCOPE: ScopeId = ScopeId([7; 16]);

/// Encodes without checking any bound, as a damaged or foreign writer
/// might.
fn unchecked(capture: &RuntimeCapture) -> Vec<u8> {
    serde_json::to_vec(&RuntimeFile {
        format: RUNTIME_FORMAT.into(),
        version: VERSION,
        scope_id: SCOPE.to_string(),
        generation: 1,
        socket_name: capture.socket_name.clone(),
        next_id: capture.next_id,
        session_ordinal: capture.session_ordinal,
        workspace_ordinal: 0,
        sessions: Bounded(
            capture.sessions.iter().map(session_record).collect(),
        ),
        terminals: Bounded(
            capture
                .terminals
                .iter()
                .map(|terminal| terminal_record(terminal).unwrap())
                .collect(),
        ),
    })
    .unwrap()
}

/// Asserts the writer accepts `within` and refuses `over`, and the reader
/// accepts the one and rejects the other, each naming `reason`.
fn assert_bound(within: &RuntimeCapture, over: &RuntimeCapture, reason: &str) {
    let bytes = encode_runtime(within, SCOPE, 1).unwrap();
    match decode_runtime(&bytes) {
        Decoded::Ok(file) => assert_eq!(&file.content, within),
        other => panic!("the bound itself was rejected: {other:?}"),
    }
    let refused = encode_runtime(over, SCOPE, 1).unwrap_err();
    assert!(refused.reason().contains(reason), "writer: {refused}");
    match decode_runtime(&unchecked(over)) {
        Decoded::Corrupt(error) => {
            assert!(error.contains(reason), "reader: {error}");
        }
        other => panic!("the reader accepted {other:?}"),
    }
}

/// Moves the last session's first workspace into a new tabbed layout of
/// `panes` columns, so one capture reaches an exact pane count.
fn with_tab_panes(mut capture: RuntimeCapture, panes: usize) -> RuntimeCapture {
    let mut ids = Ids::new();
    (0..capture.next_id).for_each(|_| {
        ids.next();
    });
    let tab = &mut capture.sessions[0].workspaces[0].tabs[0];
    let old: Vec<TerminalId> =
        tab.layout.panes().iter().map(|p| p.terminal_id).collect();
    capture
        .terminals
        .retain(|terminal| !old.contains(&terminal.id));
    let mut terminals = Vec::new();
    tab.layout = columns(&mut ids, &mut terminals, panes);
    capture.terminals.extend(terminals);
    capture.next_id = ids.next();
    capture
}

#[test]
fn a_capture_round_trips_through_its_file() {
    let capture = rich_capture();
    let bytes = encode_runtime(&capture, SCOPE, 9).unwrap();
    let Decoded::Ok(file) = decode_runtime(&bytes) else {
        panic!("the file did not decode");
    };
    assert_eq!(file.content, capture);
    assert_eq!((file.scope_id, file.generation), (SCOPE, 9));
    let layout = layout_for(&capture);
    let bytes = encode_desktop(&layout, SCOPE, 9).unwrap();
    let Decoded::Ok(file) = decode_desktop(&bytes) else {
        panic!("the layout did not decode");
    };
    assert_eq!(file.content, layout);
    assert_eq!(ScopeId::parse(&SCOPE.to_string()), Some(SCOPE));
    assert_eq!(SCOPE.to_string(), "07".repeat(16));
}

#[test]
fn workspace_counters_round_trip_per_session_and_older_files_load() {
    let mut capture = rich_capture();
    capture.sessions[0].workspace_ordinal = 7;
    capture.sessions[1].workspace_ordinal = 3;
    let bytes = encode_runtime(&capture, SCOPE, 1).unwrap();
    let Decoded::Ok(file) = decode_runtime(&bytes) else {
        panic!("the file did not decode");
    };
    assert_eq!(file.content, capture);
    // Builds that require the runtime-wide field still find it.
    let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["workspace_ordinal"], 7);

    // A file from before per-session counters has only the runtime-wide
    // one, which loads as each session's unknown (zero) counter.
    for session in json["sessions"].as_array_mut().unwrap() {
        session.as_object_mut().unwrap().remove("workspace_ordinal");
    }
    let older = serde_json::to_vec(&json).unwrap();
    let Decoded::Ok(file) = decode_runtime(&older) else {
        panic!("an older file did not load");
    };
    assert!(
        file.content
            .sessions
            .iter()
            .all(|session| session.workspace_ordinal == 0)
    );
}

const DISPLAY: &str = "1e2f3a4b-5c6d-7e8f-9a0b-1c2d3e4f5a6b";

#[test]
fn a_window_display_round_trips_and_is_left_out_when_unknown() {
    let mut layout = layout_for(&rich_capture());
    assert_eq!(layout.windows.len(), 2);
    layout.windows[0].display = Some(DISPLAY.into());
    let bytes = encode_desktop(&layout, SCOPE, 1).unwrap();
    let Decoded::Ok(file) = decode_desktop(&bytes) else {
        panic!("the layout did not decode");
    };
    assert_eq!(file.content, layout);
    // The second window names no display, so it writes no field, as a file
    // from before the field has none.
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(text.matches("\"display\"").count(), 1, "{text}");
}

#[test]
fn a_windows_sidebar_state_round_trips_and_older_files_load() {
    let mut layout = layout_for(&rich_capture());
    layout.windows[0].workspace_panel_visible = Some(false);
    layout.windows[0].workspace_panel_width = 230.0;
    let bytes = encode_desktop(&layout, SCOPE, 1).unwrap();
    let Decoded::Ok(file) = decode_desktop(&bytes) else {
        panic!("the layout did not decode");
    };
    assert_eq!(file.content, layout);
    // The second window keeps the defaults, so it writes neither field, as
    // a file from before them has none, and such a file loads them as
    // defaults.
    let text = String::from_utf8(bytes).unwrap();
    for field in ["\"workspace_panel_visible\"", "\"workspace_panel_width\""] {
        assert_eq!(text.matches(field).count(), 1, "{text}");
    }
    let plain = layout_for(&rich_capture());
    let text =
        String::from_utf8(encode_desktop(&plain, SCOPE, 1).unwrap()).unwrap();
    assert!(!text.contains("workspace_panel"), "{text}");
    let Decoded::Ok(file) = decode_desktop(text.as_bytes()) else {
        panic!("a file without the fields did not load");
    };
    assert!(file.content.windows.iter().all(|window| {
        window.workspace_panel_visible.is_none()
            && window.workspace_panel_width == 0.0
    }));
    // A width that is not a finite, non-negative number is refused.
    for width in [-1.0, f32::NAN, f32::INFINITY] {
        let mut broken = layout.clone();
        broken.windows[0].workspace_panel_width = width;
        let refused = encode_desktop(&broken, SCOPE, 1).unwrap_err();
        assert!(refused.reason().contains("geometry"), "{refused}");
    }
}

/// Encodes a desktop layout without checking any rule, as a damaged or
/// foreign writer might.
fn unchecked_desktop(layout: &DesktopLayout) -> Vec<u8> {
    serde_json::to_vec(&DesktopFile {
        format: DESKTOP_FORMAT.into(),
        version: VERSION,
        scope_id: SCOPE.to_string(),
        generation: 1,
        active_window: layout.active_window,
        windows: Bounded(layout.windows.iter().map(window_record).collect()),
    })
    .unwrap()
}

/// The rich layout, with its first window remembering where it was in the
/// second window's session.
fn remembering() -> DesktopLayout {
    let mut layout = layout_for(&rich_capture());
    let other = layout.windows[1].clone();
    layout.windows[0].remembered = vec![SessionNavigation {
        session: other.session,
        workspace: other.workspace,
        active_tab: other.active_tab,
        tab_history: other.tab_history,
        focused_panes: other.focused_panes,
    }];
    layout
}

#[test]
fn remembered_sessions_round_trip_and_files_without_them_load() {
    let layout = remembering();
    let bytes = encode_desktop(&layout, SCOPE, 1).unwrap();
    let Decoded::Ok(file) = decode_desktop(&bytes) else {
        panic!("the layout did not decode");
    };
    assert_eq!(file.content, layout);
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(text.matches("\"remembered\"").count(), 1, "{text}");

    // A window without memory writes no field, as a file from before the
    // field has none, and such a file loads with no memory.
    let plain = layout_for(&rich_capture());
    let bytes = encode_desktop(&plain, SCOPE, 1).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains("remembered"), "{text}");
    let Decoded::Ok(file) = decode_desktop(text.as_bytes()) else {
        panic!("a file without memory did not load");
    };
    assert!(file.content.windows.iter().all(|w| w.remembered.is_empty()));
}

#[test]
fn remembered_workspaces_of_one_session_round_trip_in_order() {
    let mut layout = remembering();
    let first = layout.windows[0].remembered[0].clone();
    let second = SessionNavigation {
        workspace: WorkspaceId::new(first.workspace.get() + 1000),
        active_tab: None,
        tab_history: Vec::new(),
        focused_panes: Vec::new(),
        ..first.clone()
    };
    // One entry per workspace, so the session appears twice, most recent
    // first, as an older build's validation already accepts.
    layout.windows[0].remembered = vec![second, first];
    let bytes = encode_desktop(&layout, SCOPE, 1).unwrap();
    let Decoded::Ok(file) = decode_desktop(&bytes) else {
        panic!("the layout did not decode");
    };
    assert_eq!(file.content, layout);
    let remembered = &file.content.windows[0].remembered;
    assert_eq!(remembered[0].session, remembered[1].session);
    assert_ne!(remembered[0].workspace, remembered[1].workspace);
}

#[test]
fn remembered_sessions_breaking_a_window_rule_are_refused() {
    let valid = remembering();
    let mut breaks: Vec<(DesktopLayout, &str)> = Vec::new();
    let mut zero = valid.clone();
    zero.windows[0].remembered[0].workspace = WorkspaceId::new(0);
    breaks.push((zero, "identity zero"));
    let mut zero_focus = valid.clone();
    zero_focus.windows[0].remembered[0].focused_panes[0].pane = PaneId::new(0);
    breaks.push((zero_focus, "identity zero"));
    let mut tabs = valid.clone();
    tabs.windows[0].remembered[0].tab_history =
        vec![TabId::new(1); MAX_TABS + 1];
    breaks.push((tabs, "more than 4096"));
    let mut sessions = valid.clone();
    sessions.windows[0].remembered =
        vec![valid.windows[0].remembered[0].clone(); MAX_SESSIONS + 1];
    breaks.push((sessions, "more than 256"));
    for (layout, reason) in breaks {
        let refused = encode_desktop(&layout, SCOPE, 1).unwrap_err();
        assert!(refused.reason().contains(reason), "writer: {refused}");
        match decode_desktop(&unchecked_desktop(&layout)) {
            Decoded::Corrupt(error) => {
                assert!(error.contains(reason), "reader: {error}");
            }
            other => panic!("the reader accepted {other:?}"),
        }
    }
    let mut at_bound = valid.clone();
    at_bound.windows[0].remembered =
        vec![valid.windows[0].remembered[0].clone(); MAX_SESSIONS];
    let bytes = encode_desktop(&at_bound, SCOPE, 1).unwrap();
    assert!(matches!(decode_desktop(&bytes), Decoded::Ok(_)));

    // An entry missing a required field is malformed, as a window would be.
    let text =
        String::from_utf8(encode_desktop(&valid, SCOPE, 1).unwrap()).unwrap();
    let workspace = valid.windows[0].remembered[0].workspace.get();
    let broken = text.replacen(
        &format!(
            "\"remembered\":[{{\"session\":{},\"workspace\":{workspace},",
            valid.windows[0].remembered[0].session.get()
        ),
        &format!(
            "\"remembered\":[{{\"session\":{},",
            valid.windows[0].remembered[0].session.get()
        ),
        1,
    );
    assert_ne!(broken, text);
    assert!(matches!(
        decode_desktop(broken.as_bytes()),
        Decoded::Corrupt(reason) if reason.contains("workspace")
    ));
}

#[test]
fn the_writer_drops_directories_a_restore_cannot_use() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let mut capture = rich_capture();
    capture.terminals[0].directories = vec![
        PathBuf::from(OsStr::from_bytes(b"/not\xffutf8")),
        PathBuf::from("relative"),
        PathBuf::from("/kept"),
    ];
    let bytes = encode_runtime(&capture, SCOPE, 1).unwrap();
    let Decoded::Ok(file) = decode_runtime(&bytes) else {
        panic!("the file did not decode");
    };
    assert_eq!(
        file.content.terminals[0].directories,
        [PathBuf::from("/kept")]
    );
}

#[test]
fn sessions_stop_at_their_bound() {
    assert_bound(
        &capture(MAX_SESSIONS, 1, 1, 1),
        &capture(MAX_SESSIONS + 1, 1, 1, 1),
        "more than 256",
    );
}

#[test]
fn workspaces_stop_at_their_bound_across_sessions() {
    let half = MAX_WORKSPACES / 2;
    let within = capture(2, half, 1, 1);
    // One more workspace in the second session.
    let mut over = capture(2, half + 1, 1, 1);
    let removed = over.sessions[0].workspaces.pop().unwrap();
    let gone: Vec<TerminalId> = removed
        .tabs
        .iter()
        .flat_map(|tab| tab.layout.panes())
        .map(|pane| pane.terminal_id)
        .collect();
    over.terminals
        .retain(|terminal| !gone.contains(&terminal.id));
    assert_bound(&within, &over, "more than 1024 workspaces");
}

#[test]
fn tabs_stop_at_their_bound_across_workspaces() {
    assert_bound(
        &capture(1, 4, MAX_TABS / 4, 1),
        &capture(1, 5, MAX_TABS / 4, 1),
        "more than 4096 tabs",
    );
}

#[test]
fn panes_stop_at_their_bound_across_tabs() {
    let within = capture(1, 4, MAX_TABS / 4, MAX_PANES / MAX_TABS);
    let over = with_tab_panes(within.clone(), MAX_PANES / MAX_TABS + 1);
    // The reader stops at the terminal list, one record per pane.
    assert_bound(&within, &over, "more than 16384");
}

#[test]
fn layouts_stop_at_their_depth_bound() {
    let deepen = |depth: usize| {
        let mut capture = capture(1, 1, 1, 1);
        let mut ids = Ids::new();
        (0..capture.next_id).for_each(|_| {
            ids.next();
        });
        let mut terminals = Vec::new();
        let layout = deep(&mut ids, &mut terminals, depth);
        assert_eq!(layout.depth(), depth);
        capture.sessions[0].workspaces[0].tabs[0].layout = layout;
        capture.terminals = terminals;
        capture.next_id = ids.next();
        capture
    };
    assert_bound(
        &deepen(MAX_LAYOUT_DEPTH),
        &deepen(MAX_LAYOUT_DEPTH + 1),
        "deeper than 32",
    );
    // Far deeper nesting fails in the parser, before any layout is built.
    let pane = r#"{"pane":1,"terminal":2}"#;
    let nested = format!(
        "{}{pane}{}",
        r#"{"split":3,"axis":"rows","weights":[1],"children":["#.repeat(200),
        "]}".repeat(200)
    );
    let error = serde_json::from_str::<NodeRecord>(&nested).unwrap_err();
    assert!(error.to_string().contains("recursion limit"), "{error}");
}

#[test]
fn saved_paths_stop_at_their_bound() {
    let with_path = |bytes: usize| {
        let mut capture = capture(1, 1, 1, 1);
        capture.terminals[0].directories = vec![
            PathBuf::from(format!("/{}", "a".repeat(bytes - 1))),
            PathBuf::from("/kept"),
        ];
        capture
    };
    let within = with_path(MAX_PATH_BYTES);
    let bytes = encode_runtime(&within, SCOPE, 1).unwrap();
    match decode_runtime(&bytes) {
        Decoded::Ok(file) => assert_eq!(file.content, within),
        other => panic!("the bound itself was rejected: {other:?}"),
    }
    // The writer drops an over-long candidate, which is optional, and
    // saves the rest.
    let over = with_path(MAX_PATH_BYTES + 1);
    let bytes = encode_runtime(&over, SCOPE, 1).unwrap();
    let Decoded::Ok(file) = decode_runtime(&bytes) else {
        panic!("the file did not decode");
    };
    assert_eq!(
        file.content.terminals[0].directories,
        [PathBuf::from("/kept")]
    );
    // The reader rejects one in a file it did not write.
    match decode_runtime(&unchecked(&over)) {
        Decoded::Corrupt(error) => {
            assert!(error.contains("longer than 4096 bytes"), "{error}");
        }
        other => panic!("the reader accepted {other:?}"),
    }
}

#[test]
fn files_stop_at_their_size_bound() {
    let named = |bytes: usize| {
        let mut capture = capture(1, 1, 1, 1);
        let plain = encode_runtime(&capture, SCOPE, 1).unwrap().len();
        let name = "x".repeat(bytes - plain + "Session 1".len());
        capture.sessions[0].info.automatic_name = name;
        capture
    };
    let limit = usize::try_from(MAX_FILE_BYTES).unwrap();
    let within = named(limit);
    assert_eq!(encode_runtime(&within, SCOPE, 1).unwrap().len(), limit);
    let refused = encode_runtime(&named(limit + 1), SCOPE, 1).unwrap_err();
    assert!(refused.reason().contains("exceed"), "{refused}");
}

#[test]
fn a_desktop_layout_breaking_its_rules_is_refused_by_writer_and_reader() {
    let capture = rich_capture();
    let valid = layout_for(&capture);
    let mut breaks: Vec<(DesktopLayout, &str)> = Vec::new();
    let mut missing = valid.clone();
    missing.active_window = Some(missing.windows.len());
    breaks.push((missing, "active window"));
    let mut zero = valid.clone();
    zero.windows[0].session = SessionId::new(0);
    breaks.push((zero, "identity zero"));
    let mut empty = valid.clone();
    empty.windows[0].bounds.width = 0.0;
    breaks.push((empty, "geometry"));
    let mut nan = valid.clone();
    nan.windows[0].bounds.x = f32::NAN;
    breaks.push((nan, "geometry"));
    let mut blank = valid.clone();
    blank.windows[0].quake_profile = Some(" ".into());
    breaks.push((blank, "blank"));
    let mut display = valid.clone();
    display.windows[0].display = Some(format!("{DISPLAY}0"));
    breaks.push((display, "display is not a UUID"));
    let mut many = valid.clone();
    many.windows = vec![valid.windows[0].clone(); MAX_WINDOWS + 1];
    breaks.push((many, "more than 256 windows"));
    for (layout, reason) in breaks {
        let refused = encode_desktop(&layout, SCOPE, 1).unwrap_err();
        assert!(refused.reason().contains(reason), "{refused}");
    }
    let mut at_bound = valid.clone();
    at_bound.windows = vec![valid.windows[0].clone(); MAX_WINDOWS];
    let bytes = encode_desktop(&at_bound, SCOPE, 1).unwrap();
    assert!(matches!(decode_desktop(&bytes), Decoded::Ok(_)));

    // The reader applies the same rules to a file it did not write.
    let bytes = encode_desktop(&valid, SCOPE, 1).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let broken = text.replace("\"active_window\":0", "\"active_window\":5");
    assert!(matches!(
        decode_desktop(broken.as_bytes()),
        Decoded::Corrupt(reason) if reason.contains("active window")
    ));
    let mut named = valid.clone();
    named.windows[0].display = Some(DISPLAY.into());
    let text =
        String::from_utf8(encode_desktop(&named, SCOPE, 1).unwrap()).unwrap();
    for invalid in ["display-1", "1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b", ""] {
        let broken = text.replace(DISPLAY, invalid);
        assert!(
            matches!(
                decode_desktop(broken.as_bytes()),
                Decoded::Corrupt(ref reason) if reason.contains("display is not a UUID")
            ),
            "{invalid:?}"
        );
    }
}
