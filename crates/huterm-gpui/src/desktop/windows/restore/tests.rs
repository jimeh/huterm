use super::*;
use huterm_protocol::{
    PaneInfo, PaneLayout, PaneNode, SessionCapture, SessionInfo, TabInfo,
    TerminalLaunch, WorkspaceCapture, WorkspaceInfo,
};

/// A temporary state root, removed when the test ends.
struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        static UNIQUE: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "huterm-restore-{}-{}",
            std::process::id(),
            UNIQUE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn scope(&self) -> PathBuf {
        self.0.join("sockets/default")
    }

    fn file(&self, name: &str) -> PathBuf {
        self.scope().join(name)
    }

    fn prepare(&self, enabled: bool) -> Prepared {
        prepare(Ok(self.0.clone()), "default", enabled)
    }

    /// Saves one session with one tab, and a layout for it.
    fn save(&self) -> (RuntimeCapture, DesktopLayout) {
        let capture = capture();
        let layout = layout(&capture);
        let mut store = Store::open(&self.0, "default").unwrap();
        store.load();
        store.save(&capture, &layout).unwrap();
        (capture, layout)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn capture() -> RuntimeCapture {
    let pane = PaneInfo {
        id: PaneId::new(4),
        terminal_id: TerminalId::new(5),
        fallback_name: String::new(),
        custom_name: Some("logs".into()),
    };
    RuntimeCapture {
        socket_name: "default".into(),
        next_id: 5,
        session_ordinal: 1,
        workspace_ordinal: 1,
        sessions: vec![SessionCapture {
            info: SessionInfo {
                id: SessionId::new(1),
                custom_name: Some("main".into()),
                automatic_name: "Session 1".into(),
            },
            workspaces: vec![WorkspaceCapture {
                info: WorkspaceInfo {
                    id: WorkspaceId::new(2),
                    custom_name: None,
                    automatic_name: "Workspace 1".into(),
                },
                tabs: vec![TabInfo {
                    id: TabId::new(3),
                    custom_name: None,
                    layout: PaneLayout::new(PaneNode::Pane(pane)).unwrap(),
                    zoomed: None,
                    input_sync: false,
                }],
            }],
        }],
        terminals: vec![TerminalCapture {
            id: TerminalId::new(5),
            directories: vec![PathBuf::from("/")],
            launch: TerminalLaunch::DefaultShell,
        }],
    }
}

fn layout(capture: &RuntimeCapture) -> DesktopLayout {
    let session = &capture.sessions[0];
    DesktopLayout {
        windows: vec![huterm_state::WindowLayout {
            session: session.info.id,
            workspace: session.workspaces[0].info.id,
            active_tab: Some(TabId::new(3)),
            tab_history: vec![TabId::new(3)],
            focused_panes: Vec::new(),
            bounds: huterm_state::WindowBounds {
                x: 10.0,
                y: 20.0,
                width: 640.0,
                height: 480.0,
            },
            mode: WindowMode::Windowed,
            sidebar_width: 180.0,
            display: None,
            quake_profile: None,
            appearance: None,
        }],
        active_window: Some(0),
    }
}

fn messages(prepared: &Prepared) -> Vec<(Severity, Lifetime, &str)> {
    prepared
        .notices
        .iter()
        .map(|notice| {
            (notice.severity, notice.lifetime, notice.message.as_str())
        })
        .collect()
}

#[test]
fn a_saved_scope_restores_with_its_layout_and_writes_nothing_until_settled() {
    let root = Root::new();
    let (capture, layout) = root.save();
    let prepared = root.prepare(true);
    assert!(prepared.notices.is_empty(), "{:?}", messages(&prepared));
    assert_eq!(prepared.persistence.state(), State::Bootstrapping);
    let saved = prepared.saved.as_ref().unwrap();
    assert_eq!(saved.capture, capture);
    assert_eq!(saved.layout.as_ref(), Some(&layout));
}

#[test]
fn nothing_saved_starts_running_and_an_empty_capture_restores_nothing() {
    let root = Root::new();
    let prepared = root.prepare(true);
    assert!(prepared.saved.is_none());
    assert_eq!(prepared.notices, []);
    assert_eq!(prepared.persistence.state(), State::Running);
    drop(prepared);

    // The last window closed: the files hold no session.
    let mut store = Store::open(&root.0, "default").unwrap();
    store.load();
    let mut empty = capture();
    empty.sessions.clear();
    empty.terminals.clear();
    store.save(&empty, &DesktopLayout::default()).unwrap();
    drop(store);
    let prepared = root.prepare(true);
    assert!(prepared.saved.is_none());
    assert_eq!(prepared.notices, []);
    assert_eq!(prepared.persistence.state(), State::Running);
}

#[test]
fn a_layout_from_another_save_is_ignored_with_a_notice() {
    let root = Root::new();
    root.save();
    let layout_bytes = std::fs::read(root.file("desktop.json")).unwrap();
    // A later save whose layout write never landed.
    root.save();
    std::fs::write(root.file("desktop.json"), layout_bytes).unwrap();
    let prepared = root.prepare(true);
    let saved = prepared.saved.as_ref().unwrap();
    assert!(saved.layout.is_none());
    assert_eq!(
        messages(&prepared),
        [(
            Severity::Info,
            Lifetime::Persistent,
            "Your sessions reopened in new windows because their saved window layout couldn't be used."
        )]
    );
    assert_eq!(prepared.persistence.state(), State::Bootstrapping);
}

#[test]
fn a_corrupt_runtime_file_is_set_aside_and_saving_starts_fresh() {
    let root = Root::new();
    root.save();
    std::fs::write(root.file("runtime.json"), b"{broken").unwrap();
    let prepared = root.prepare(true);
    assert!(prepared.saved.is_none());
    assert_eq!(prepared.persistence.state(), State::Running);
    assert_eq!(
        std::fs::read(root.file("runtime.json.corrupt")).unwrap(),
        b"{broken"
    );
    assert!(!root.file("runtime.json").exists());
    let [(severity, lifetime, message)] = messages(&prepared)[..] else {
        panic!("{:?}", messages(&prepared));
    };
    assert_eq!(
        (severity, lifetime),
        (Severity::Warning, Lifetime::Persistent)
    );
    assert!(message.starts_with("Couldn't restore your previous windows: "));
    assert!(
        message.ends_with("The saved state was kept as runtime.json.corrupt.")
    );
}

#[test]
fn a_newer_runtime_file_stays_and_keeps_saving_off() {
    let root = Root::new();
    root.save();
    let newer = std::fs::read_to_string(root.file("runtime.json"))
        .unwrap()
        .replace("\"version\":1", "\"version\":2");
    std::fs::write(root.file("runtime.json"), &newer).unwrap();
    let prepared = root.prepare(true);
    assert!(prepared.saved.is_none());
    assert_eq!(prepared.persistence.state(), State::Off);
    assert!(
        !prepared.persistence.set_enabled(true),
        "the setting cannot turn it on"
    );
    assert_eq!(
        messages(&prepared),
        [(Severity::Warning, Lifetime::Persistent, NEWER)]
    );
    assert_eq!(
        std::fs::read_to_string(root.file("runtime.json")).unwrap(),
        newer
    );
}

#[test]
fn a_leftover_restore_marker_sets_both_files_aside() {
    let root = Root::new();
    root.save();
    let store = Store::open(&root.0, "default").unwrap();
    store.mark_restore_attempt().unwrap();
    drop(store);
    let prepared = root.prepare(true);
    assert!(prepared.saved.is_none());
    assert_eq!(prepared.persistence.state(), State::Running);
    for name in ["runtime.json", "desktop.json", "restore-attempt"] {
        assert!(!root.file(name).exists(), "{name}");
    }
    assert!(root.file("runtime.json.corrupt").exists());
    assert!(root.file("desktop.json.corrupt").exists());
    assert_eq!(
        messages(&prepared),
        [(
            Severity::Warning,
            Lifetime::Persistent,
            "Couldn't restore your previous windows: the last restore did not finish. The saved state was kept as runtime.json.corrupt."
        )]
    );
}

#[test]
fn a_held_lock_runs_isolated_without_touching_the_files() {
    let root = Root::new();
    root.save();
    let before = std::fs::read(root.file("runtime.json")).unwrap();
    let holder = Store::open(&root.0, "default").unwrap();
    let prepared = root.prepare(true);
    assert!(prepared.saved.is_none());
    assert_eq!(prepared.persistence.state(), State::Off);
    assert!(!prepared.persistence.set_enabled(true));
    assert_eq!(
        messages(&prepared),
        [(Severity::Info, Lifetime::Persistent, LOCK_HELD)]
    );
    assert_eq!(std::fs::read(root.file("runtime.json")).unwrap(), before);
    drop(holder);
}

#[test]
fn an_unusable_state_root_reports_and_keeps_saving_off() {
    let prepared = prepare(
        Err(huterm_state::PathError::RelativeOverride),
        "default",
        true,
    );
    assert_eq!(prepared.persistence.state(), State::Off);
    let [(severity, _, message)] = messages(&prepared)[..] else {
        panic!("{:?}", messages(&prepared));
    };
    assert_eq!(severity, Severity::Warning);
    assert!(message.contains("HUTERM_STATE_DIR is not an absolute path"));
}

#[test]
fn the_setting_off_deletes_saved_state_and_can_turn_saving_back_on() {
    let root = Root::new();
    root.save();
    let prepared = root.prepare(false);
    assert!(prepared.saved.is_none());
    assert_eq!(prepared.notices, []);
    assert_eq!(prepared.persistence.state(), State::Off);
    assert!(!root.file("runtime.json").exists());
    assert!(!root.file("desktop.json").exists());
    assert!(prepared.persistence.set_enabled(true));
    assert_eq!(prepared.persistence.state(), State::Running);
}

#[test]
fn a_restored_terminal_starts_in_its_first_usable_directory() {
    let root = Root::new();
    let base = TerminalCommand {
        program: "/bin/sh".into(),
        arguments: Vec::new(),
        working_directory: PathBuf::from("/"),
        environment: Vec::new(),
        grid_size: GridSize::clamped(40, 8),
        cell_size: CellSize {
            width: 8,
            height: 16,
        },
        presentation: huterm_protocol::TerminalPresentation::default(),
        scrollback: huterm_protocol::ScrollbackLimit::DEFAULT,
        shell_integration: huterm_protocol::ShellIntegration::OFF,
        scrollback_compression: false,
    };
    let gone = root.0.join("gone");
    let record = |directories: Vec<PathBuf>| TerminalCapture {
        id: TerminalId::new(1),
        directories,
        launch: TerminalLaunch::DefaultShell,
    };
    let (command, fallback) =
        restored_command(&base, &record(vec![root.0.clone(), gone.clone()]));
    assert_eq!(command.working_directory, root.0);
    assert_eq!(fallback, None);
    let (command, fallback) =
        restored_command(&base, &record(vec![gone.clone(), root.0.clone()]));
    assert_eq!(command.working_directory, root.0);
    assert_eq!(fallback, Some(Fallback::Earlier));
    let (command, fallback) = restored_command(&base, &record(vec![gone]));
    assert_eq!(command.working_directory, PathBuf::from("/"));
    assert_eq!(fallback, Some(Fallback::Default));
    let (_, fallback) = restored_command(&base, &record(Vec::new()));
    assert_eq!(fallback, Some(Fallback::Default));
}

#[test]
fn fallback_notices_count_panes_and_name_where_they_started() {
    let one = fallback_message(0, 1);
    assert!(one.starts_with("1 pane started in "), "{one}");
    assert!(
        one.ends_with(" because its folder no longer exists."),
        "{one}"
    );
    let three = fallback_message(0, 3);
    assert!(three.starts_with("3 panes started in "), "{three}");
    assert!(three.ends_with(" because their folders no longer exist."));
    if cfg!(target_os = "macos") {
        assert_eq!(
            three,
            "3 panes started in your home folder because their folders no longer exist."
        );
    }
    assert_eq!(
        fallback_message(1, 0),
        "1 pane started in an earlier folder because its last folder no longer exists."
    );
    assert_eq!(
        fallback_message(2, 0),
        "2 panes started in an earlier folder because their last folders no longer exist."
    );
    assert_eq!(
        fallback_message(2, 1),
        "3 panes started in a different folder because their last folders no longer exist."
    );
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    }
}

#[test]
fn saved_bounds_move_onto_a_current_display() {
    let primary = rect(0.0, 25.0, 1440.0, 875.0);
    let side = rect(1440.0, 0.0, 1920.0, 1080.0);
    let displays = [primary, side];
    // Inside a display: unchanged.
    let inside = rect(100.0, 100.0, 800.0, 600.0);
    assert_eq!(place_on_displays(inside, &displays, Some(primary)), inside);
    // Over an edge: moved fully onto the display it overlaps most.
    assert_eq!(
        place_on_displays(
            rect(3000.0, -50.0, 800.0, 600.0),
            &displays,
            Some(primary)
        ),
        rect(2560.0, 0.0, 800.0, 600.0)
    );
    // Larger than its display: shrunk to fit.
    assert_eq!(
        place_on_displays(
            rect(10.0, 30.0, 2000.0, 1000.0),
            &displays,
            Some(primary)
        ),
        rect(0.0, 25.0, 1440.0, 875.0)
    );
    // On no current display: centred on the primary one.
    assert_eq!(
        place_on_displays(
            rect(-5000.0, 0.0, 800.0, 600.0),
            &displays,
            Some(primary)
        ),
        rect(320.0, 162.5, 800.0, 600.0)
    );
}

const BUILT_IN: &str = "0a1b2c3d-4e5f-4061-8273-94a5b6c7d8e9";
const SIDE: &str = "f9e8d7c6-b5a4-4321-9fed-cba987654321";

/// A built-in primary display under a menu bar, and a larger one beside
/// it, each from its own origin as macOS reports them.
fn screens() -> [Screen; 2] {
    [
        Screen {
            id: gpui::DisplayId::new(1),
            uuid: Some(BUILT_IN.into()),
            visible: rect(0.0, 25.0, 1440.0, 875.0),
            primary: true,
        },
        Screen {
            id: gpui::DisplayId::new(2),
            uuid: Some(SIDE.into()),
            visible: rect(0.0, 0.0, 1920.0, 1080.0),
            primary: false,
        },
    ]
}

#[test]
fn a_window_returns_to_its_saved_display_at_the_same_position() {
    let screens = screens();
    let side = Some(gpui::DisplayId::new(2));
    let saved = rect(1000.0, 400.0, 800.0, 600.0);
    assert_eq!(
        place_restored(saved, Some(SIDE), &screens, false),
        (side, saved)
    );
    assert_eq!(
        place_restored(saved, Some(&SIDE.to_uppercase()), &screens, false),
        (side, saved)
    );
    // Over that display's edge: moved onto its visible area.
    assert_eq!(
        place_restored(
            rect(1500.0, 700.0, 800.0, 600.0),
            Some(SIDE),
            &screens,
            false
        ),
        (side, rect(1120.0, 480.0, 800.0, 600.0))
    );
}

#[test]
fn a_window_whose_saved_display_is_missing_opens_on_the_primary_display() {
    let screens = screens();
    let gone = "11111111-2222-4333-8444-555555555555";
    // Within the primary display's reach: clamped onto it.
    let saved = rect(1000.0, 400.0, 800.0, 600.0);
    let on_primary = (None, rect(640.0, 300.0, 800.0, 600.0));
    assert_eq!(
        place_restored(saved, Some(gone), &screens, false),
        on_primary
    );
    assert_eq!(place_restored(saved, None, &screens, false), on_primary);
    // Beyond it: centred on it.
    assert_eq!(
        place_restored(
            rect(5000.0, 0.0, 800.0, 600.0),
            Some(gone),
            &screens,
            false
        ),
        (None, rect(320.0, 162.5, 800.0, 600.0))
    );
    // Where every display shares one space, any display it meets keeps it.
    let mut global = screens;
    global[1].visible = rect(1440.0, 0.0, 1920.0, 1080.0);
    assert_eq!(
        place_restored(rect(2000.0, 100.0, 800.0, 600.0), None, &global, true),
        (None, rect(2000.0, 100.0, 800.0, 600.0))
    );
}
