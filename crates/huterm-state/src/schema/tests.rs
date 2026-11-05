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
        workspace_ordinal: capture.workspace_ordinal,
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
}
