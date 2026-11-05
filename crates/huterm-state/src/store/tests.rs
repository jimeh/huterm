use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use huterm_protocol::{
    PaneId, PaneInfo, PaneLayout, PaneNode, SPLIT_TOTAL, SessionCapture,
    SessionId, SessionInfo, SplitAxis, SplitChild, SplitId, SplitNode, TabId,
    TabInfo, TerminalCapture, TerminalId, TerminalLaunch, WorkspaceCapture,
    WorkspaceId, WorkspaceInfo,
};

use super::*;
use crate::{FocusedPane, LockError, WindowBounds, WindowLayout, WindowMode};

/// A directory removed when the test ends.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new() -> Self {
        static UNIQUE: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "huterm-state-{}-{nanos}-{}",
            std::process::id(),
            UNIQUE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Hands out sequential identities from 1.
pub(crate) struct Ids(u64);

impl Ids {
    pub(crate) fn new() -> Self {
        Self(0)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

fn pane(ids: &mut Ids, terminals: &mut Vec<TerminalCapture>) -> PaneNode {
    let id = PaneId::new(ids.next());
    let terminal = TerminalId::new(ids.next());
    terminals.push(TerminalCapture {
        id: terminal,
        directories: vec![PathBuf::from("/tmp")],
        launch: TerminalLaunch::DefaultShell,
    });
    PaneNode::Pane(PaneInfo {
        id,
        terminal_id: terminal,
        fallback_name: String::new(),
        custom_name: None,
    })
}

fn split(ids: &mut Ids, axis: SplitAxis, nodes: Vec<PaneNode>) -> PaneNode {
    let count = u32::try_from(nodes.len()).unwrap();
    let share = SPLIT_TOTAL / count;
    PaneNode::Split(SplitNode {
        id: SplitId::new(ids.next()),
        axis,
        children: nodes
            .into_iter()
            .enumerate()
            .map(|(index, node)| SplitChild {
                weight: if index == 0 {
                    SPLIT_TOTAL - share * (count - 1)
                } else {
                    share
                },
                node,
            })
            .collect(),
    })
}

/// A layout of `panes` columns.
pub(crate) fn columns(
    ids: &mut Ids,
    terminals: &mut Vec<TerminalCapture>,
    panes: usize,
) -> PaneLayout {
    let root = if panes == 1 {
        pane(ids, terminals)
    } else {
        let nodes = (0..panes).map(|_| pane(ids, terminals)).collect();
        split(ids, SplitAxis::Columns, nodes)
    };
    PaneLayout::new(root).unwrap()
}

/// A layout whose longest root-to-pane path holds `depth` nodes: splits of
/// alternating axes, each holding a pane and the next split.
pub(crate) fn deep(
    ids: &mut Ids,
    terminals: &mut Vec<TerminalCapture>,
    depth: usize,
) -> PaneLayout {
    fn node(
        ids: &mut Ids,
        terminals: &mut Vec<TerminalCapture>,
        depth: usize,
        axis: SplitAxis,
    ) -> PaneNode {
        if depth == 1 {
            return pane(ids, terminals);
        }
        let first = pane(ids, terminals);
        let other = match axis {
            SplitAxis::Columns => SplitAxis::Rows,
            SplitAxis::Rows => SplitAxis::Columns,
        };
        let rest = node(ids, terminals, depth - 1, other);
        split(ids, axis, vec![first, rest])
    }
    PaneLayout::new(node(ids, terminals, depth, SplitAxis::Columns)).unwrap()
}

/// A capture of `sessions` sessions, each of `workspaces` workspaces of
/// `tabs` tabs of `panes` columns.
pub(crate) fn capture(
    sessions: usize,
    workspaces: usize,
    tabs: usize,
    panes: usize,
) -> RuntimeCapture {
    let mut ids = Ids::new();
    let mut terminals = Vec::new();
    let mut captured = Vec::new();
    for _ in 0..sessions {
        let session = SessionId::new(ids.next());
        let mut spaces = Vec::new();
        for _ in 0..workspaces {
            let workspace = WorkspaceId::new(ids.next());
            let tabs = (0..tabs)
                .map(|_| TabInfo {
                    id: TabId::new(ids.next()),
                    custom_name: None,
                    layout: columns(&mut ids, &mut terminals, panes),
                    zoomed: None,
                    input_sync: false,
                })
                .collect();
            spaces.push(WorkspaceCapture {
                info: WorkspaceInfo {
                    id: workspace,
                    custom_name: None,
                    automatic_name: format!("Workspace {}", workspace.get()),
                },
                tabs,
            });
        }
        captured.push(SessionCapture {
            info: SessionInfo {
                id: session,
                custom_name: None,
                automatic_name: format!("Session {}", session.get()),
            },
            workspaces: spaces,
        });
    }
    RuntimeCapture {
        socket_name: "default".into(),
        next_id: ids.0,
        session_ordinal: u64::try_from(sessions).unwrap(),
        workspace_ordinal: u64::try_from(sessions * workspaces).unwrap(),
        sessions: captured,
        terminals,
    }
}

/// A small capture with names, a split, zoom, and input sync.
pub(crate) fn rich_capture() -> RuntimeCapture {
    let mut capture = capture(2, 2, 2, 1);
    let mut ids = Ids(capture.next_id);
    let layout = columns(&mut ids, &mut capture.terminals, 3);
    let zoomed = layout.panes()[1].id;
    let mut layout = layout;
    layout.rename_pane(zoomed, Some("logs".into()));
    capture.next_id = ids.0;
    let session = &mut capture.sessions[0];
    session.info.custom_name = Some("main".into());
    session.workspaces[1].info.custom_name = Some("code".into());
    let tab = &mut session.workspaces[0].tabs[1];
    tab.custom_name = Some("editor".into());
    tab.zoomed = Some(zoomed);
    tab.input_sync = true;
    let old: Vec<TerminalId> =
        tab.layout.panes().iter().map(|p| p.terminal_id).collect();
    tab.layout = layout;
    capture
        .terminals
        .retain(|terminal| !old.contains(&terminal.id));
    capture.terminals[0].directories =
        vec![PathBuf::from("/srv/project"), PathBuf::from("/home/user")];
    capture
}

/// A layout of one window per session of `capture`.
pub(crate) fn layout_for(capture: &RuntimeCapture) -> DesktopLayout {
    DesktopLayout {
        windows: capture
            .sessions
            .iter()
            .map(|session| {
                let workspace = &session.workspaces[0];
                let tab = &workspace.tabs[0];
                WindowLayout {
                    session: session.info.id,
                    workspace: workspace.info.id,
                    active_tab: Some(tab.id),
                    tab_history: workspace.tabs.iter().map(|t| t.id).collect(),
                    focused_panes: vec![FocusedPane {
                        tab: tab.id,
                        pane: tab.layout.first_pane().id,
                    }],
                    bounds: WindowBounds {
                        x: -40.5,
                        y: 25.0,
                        width: 800.0,
                        height: 600.25,
                    },
                    mode: WindowMode::Maximized,
                    sidebar_width: 220.0,
                    quake_profile: None,
                    appearance: Some(crate::WindowAppearance {
                        opacity: Some(0.9),
                        blur: Some(12),
                        ..crate::WindowAppearance::default()
                    }),
                }
            })
            .collect(),
        active_window: Some(0),
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn open_creates_a_private_scope_and_clears_interrupted_writes() {
    let root = TempDir::new();
    let scope = root.path().join("sockets/default");
    fs::create_dir_all(&scope).unwrap();
    fs::write(scope.join("runtime.json.tmp"), b"partial").unwrap();
    drop(Store::open(root.path(), "default").unwrap());
    assert!(!scope.join("runtime.json.tmp").exists());
    assert_eq!(mode(&scope.join("lock")), 0o600);

    let empty = TempDir::new();
    let root = empty.path().join("state");
    let fresh = Store::open(&root, "other.name").unwrap();
    assert_eq!(fresh.directory(), root.join("sockets/other.name"));
    for created in [&root, &root.join("sockets"), &fresh.directory().into()] {
        assert_eq!(mode(created), 0o700, "{}", created.display());
    }
    assert!(matches!(
        Store::open(&root, "../escape"),
        Err(OpenError::InvalidSocketName)
    ));
}

#[test]
fn both_files_round_trip_with_a_kept_scope_and_rising_generations() {
    let root = TempDir::new();
    let capture = rich_capture();
    let layout = layout_for(&capture);
    let mut store = Store::open(root.path(), "default").unwrap();
    assert_eq!(
        store.load(),
        Loaded {
            runtime: RuntimeLoad::Missing,
            desktop: DesktopLoad::Missing,
        }
    );
    assert_eq!(store.save(&capture, &layout).unwrap(), 1);
    for file in [StateFile::Runtime, StateFile::Desktop] {
        assert_eq!(mode(&store.directory().join(file.name())), 0o600);
    }
    drop(store);

    let mut store = Store::open(root.path(), "default").unwrap();
    let loaded = store.load();
    let RuntimeLoad::Loaded(saved) = &loaded.runtime else {
        panic!("{loaded:?}");
    };
    assert_eq!(saved.capture, capture);
    assert_eq!(saved.generation, 1);
    assert_eq!(loaded.desktop, DesktopLoad::Loaded(layout.clone()));
    let scope_id = saved.scope_id;
    assert_eq!(store.save(&capture, &layout).unwrap(), 2);
    let RuntimeLoad::Loaded(saved) = store.load().runtime else {
        panic!("the second save did not load");
    };
    assert_eq!((saved.scope_id, saved.generation), (scope_id, 2));
}

#[test]
fn a_layout_from_another_save_or_scope_is_mismatched() {
    let root = TempDir::new();
    let capture = rich_capture();
    let layout = layout_for(&capture);
    let mut store = Store::open(root.path(), "default").unwrap();
    store.load();
    store.save(&capture, &layout).unwrap();
    // A crash between the two renames leaves the new runtime file beside
    // the previous layout.
    let error = store
        .save_with(&capture, &layout, |file| {
            if file == StateFile::Desktop {
                Err(io::Error::other("interrupted"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert!(matches!(error, SaveError::Io(_)));
    let loaded = store.load();
    assert!(matches!(
        &loaded.runtime,
        RuntimeLoad::Loaded(saved) if saved.generation == 2
    ));
    assert_eq!(loaded.desktop, DesktopLoad::Mismatched);

    // A layout left beside a discarded runtime file has no runtime to name.
    store.save(&capture, &layout).unwrap();
    store.set_aside(StateFile::Runtime).unwrap();
    assert_eq!(
        store.load(),
        Loaded {
            runtime: RuntimeLoad::Missing,
            desktop: DesktopLoad::Mismatched,
        }
    );
    // A replaced runtime file starts a new scope, so the old layout stays
    // mismatched even at the same generation.
    let mut stale = fs::read(store.directory().join("desktop.json")).unwrap();
    store.save(&capture, &layout).unwrap();
    let RuntimeLoad::Loaded(saved) = store.load().runtime else {
        panic!("the replacement did not load");
    };
    assert_eq!(saved.generation, 1);
    let text = String::from_utf8(stale.clone()).unwrap();
    stale = text
        .replace("\"generation\":3", "\"generation\":1")
        .into_bytes();
    fs::write(store.directory().join("desktop.json"), stale).unwrap();
    assert_eq!(store.load().desktop, DesktopLoad::Mismatched);
}

#[test]
fn unreadable_content_is_classified_and_newer_files_are_never_overwritten() {
    let root = TempDir::new();
    let capture = rich_capture();
    let layout = layout_for(&capture);
    let mut store = Store::open(root.path(), "default").unwrap();
    store.load();
    store.save(&capture, &layout).unwrap();
    let runtime = store.directory().join("runtime.json");
    let desktop = store.directory().join("desktop.json");
    let valid = fs::read_to_string(&runtime).unwrap();
    let corrupt = |store: &mut Store| match store.load().runtime {
        RuntimeLoad::Corrupt(reason) => reason,
        other => panic!("{other:?}"),
    };

    fs::write(&runtime, b"{not json").unwrap();
    assert_ne!(corrupt(&mut store), "");
    fs::write(&runtime, valid.replace("huterm-runtime", "huterm-other"))
        .unwrap();
    assert!(corrupt(&mut store).contains("not huterm-runtime"));
    fs::write(&runtime, valid.replace("\"version\":1", "\"version\":0"))
        .unwrap();
    assert!(corrupt(&mut store).contains("version"));
    // A tab that repeats its session's identity.
    let tab = capture.sessions[0].workspaces[0].tabs[0].id.get();
    let session = capture.sessions[0].info.id.get();
    let repeated = valid.replacen(
        &format!("\"tabs\":[{{\"id\":{tab},"),
        &format!("\"tabs\":[{{\"id\":{session},"),
        1,
    );
    assert_ne!(repeated, valid);
    fs::write(&runtime, repeated).unwrap();
    assert!(corrupt(&mut store).contains("repeats"));
    fs::write(&runtime, valid.replace("\"default\"", "\"other\"")).unwrap();
    assert!(corrupt(&mut store).contains("another socket"));
    // Unknown fields are ignored.
    fs::write(&runtime, valid.replacen('{', "{\"added\":{\"x\":[1]},", 1))
        .unwrap();
    assert!(matches!(store.load().runtime, RuntimeLoad::Loaded(_)));

    // A newer version is reported and left alone, and saves refuse it.
    let newer = valid.replace("\"version\":1", "\"version\":2");
    fs::write(&runtime, &newer).unwrap();
    assert_eq!(store.load().runtime, RuntimeLoad::Newer("version 2".into()));
    assert!(matches!(
        store.save(&capture, &layout),
        Err(SaveError::NewerVersion)
    ));
    assert_eq!(fs::read_to_string(&runtime).unwrap(), newer);
    // So is a launch this version does not know.
    fs::write(&runtime, valid.replace("default_shell", "profile")).unwrap();
    assert!(matches!(store.load().runtime, RuntimeLoad::Newer(_)));

    fs::write(&runtime, &valid).unwrap();
    let layout_text = fs::read_to_string(&desktop).unwrap();
    fs::write(&desktop, layout_text.replace("maximized", "minimized")).unwrap();
    assert!(matches!(store.load().desktop, DesktopLoad::Corrupt(_)));

    // A newer layout is left alone while saves keep writing the runtime
    // file, so the next load finds the layout from another save.
    drop(store);
    let mut store = Store::open(root.path(), "default").unwrap();
    let newer = layout_text.replace("\"version\":1", "\"version\":7");
    fs::write(&desktop, &newer).unwrap();
    assert_eq!(store.load().desktop, DesktopLoad::Newer("version 7".into()));
    let generation = store.save(&capture, &layout).unwrap();
    assert_eq!(fs::read_to_string(&desktop).unwrap(), newer);
    let loaded = store.load();
    assert!(matches!(
        &loaded.runtime,
        RuntimeLoad::Loaded(saved) if saved.generation == generation
    ));
    assert_eq!(loaded.desktop, DesktopLoad::Newer("version 7".into()));
    store.save(&capture, &layout).unwrap();
    assert_eq!(fs::read_to_string(&desktop).unwrap(), newer);
}

#[test]
fn saves_wait_until_the_runtime_file_is_known_safe_to_replace() {
    let root = TempDir::new();
    let capture = rich_capture();
    let layout = layout_for(&capture);
    let mut store = Store::open(root.path(), "default").unwrap();
    store.load();
    store.save(&capture, &layout).unwrap();
    let runtime = store.directory().join("runtime.json");
    let valid = fs::read_to_string(&runtime).unwrap();
    drop(store);
    let refused = |store: &mut Store| {
        assert!(matches!(
            store.save(&capture, &layout),
            Err(SaveError::RuntimeUnresolved)
        ));
    };

    // Before any load, a newer runtime file keeps its scope identity.
    let newer = valid.replace("\"version\":1", "\"version\":2");
    fs::write(&runtime, &newer).unwrap();
    let mut store = Store::open(root.path(), "default").unwrap();
    refused(&mut store);
    assert_eq!(fs::read_to_string(&runtime).unwrap(), newer);
    // Once found newer, it stays protected even if a later load succeeds.
    assert!(matches!(store.load().runtime, RuntimeLoad::Newer(_)));
    fs::write(&runtime, &valid).unwrap();
    assert!(matches!(store.load().runtime, RuntimeLoad::Loaded(_)));
    assert!(matches!(
        store.save(&capture, &layout),
        Err(SaveError::NewerVersion)
    ));
    assert_eq!(fs::read_to_string(&runtime).unwrap(), valid);
    drop(store);

    // A file that exists but cannot be read is never replaced.
    fs::remove_file(&runtime).unwrap();
    fs::create_dir(&runtime).unwrap();
    let mut store = Store::open(root.path(), "default").unwrap();
    assert!(matches!(store.load().runtime, RuntimeLoad::Unreadable(_)));
    refused(&mut store);
    assert!(runtime.is_dir());
    fs::remove_dir(&runtime).unwrap();

    // A corrupt file is replaced only once it has been set aside.
    fs::write(&runtime, b"{not json").unwrap();
    assert!(matches!(store.load().runtime, RuntimeLoad::Corrupt(_)));
    refused(&mut store);
    assert_eq!(fs::read(&runtime).unwrap(), b"{not json");
    store.set_aside(StateFile::Runtime).unwrap();
    assert_eq!(store.save(&capture, &layout).unwrap(), 1);
    assert!(matches!(store.load().runtime, RuntimeLoad::Loaded(_)));
}

#[test]
fn a_file_over_the_size_bound_is_rejected_before_its_body_is_read() {
    let root = TempDir::new();
    let capture = rich_capture();
    let mut store = Store::open(root.path(), "default").unwrap();
    store.load();
    store.save(&capture, &layout_for(&capture)).unwrap();
    let runtime = store.directory().join("runtime.json");
    let limit = usize::try_from(MAX_FILE_BYTES).unwrap();
    // Trailing whitespace pads a valid file to exactly the bound.
    let mut bytes = fs::read(&runtime).unwrap();
    bytes.resize(limit, b' ');
    fs::write(&runtime, &bytes).unwrap();
    assert!(matches!(store.load().runtime, RuntimeLoad::Loaded(_)));
    bytes.push(b' ');
    fs::write(&runtime, &bytes).unwrap();
    assert!(matches!(
        store.load().runtime,
        RuntimeLoad::Corrupt(reason) if reason.contains("larger than")
    ));
}

#[test]
fn a_corrupt_file_is_set_aside_replacing_an_older_copy() {
    let root = TempDir::new();
    let mut store = Store::open(root.path(), "default").unwrap();
    let directory = store.directory().to_owned();
    fs::write(directory.join("runtime.json.corrupt"), b"older").unwrap();
    fs::write(directory.join("runtime.json"), b"newer garbage").unwrap();
    store.set_aside(StateFile::Runtime).unwrap();
    assert!(!directory.join("runtime.json").exists());
    assert_eq!(
        fs::read(directory.join("runtime.json.corrupt")).unwrap(),
        b"newer garbage"
    );
    // Nothing to set aside is not an error.
    store.set_aside(StateFile::Desktop).unwrap();
    assert!(!directory.join("desktop.json.corrupt").exists());
}

#[test]
fn the_restore_attempt_marker_is_created_detected_and_removed() {
    let root = TempDir::new();
    let store = Store::open(root.path(), "default").unwrap();
    assert!(!store.restore_attempt_pending().unwrap());
    store.mark_restore_attempt().unwrap();
    assert!(store.restore_attempt_pending().unwrap());
    assert_eq!(mode(&store.directory().join("restore-attempt")), 0o600);
    store.clear_restore_attempt().unwrap();
    assert!(!store.restore_attempt_pending().unwrap());
    store.clear_restore_attempt().unwrap();
}

/// Set in the child process that holds the lock.
const HOLDER: &str = "HUTERM_STATE_TEST_LOCK_HOLDER";

#[test]
fn a_second_process_cannot_take_a_held_scope_lock() {
    if let Some(root) = std::env::var_os(HOLDER) {
        // The child: hold the lock until the parent closes stdin.
        let _store = Store::open(Path::new(&root), "default").unwrap();
        // Written past the test harness's output capture.
        let mut stdout = io::stdout().lock();
        stdout.write_all(b"LOCKED\n").unwrap();
        stdout.flush().unwrap();
        drop(stdout);
        let mut line = String::new();
        let _ = io::stdin().read_line(&mut line);
        return;
    }
    let root = TempDir::new();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "store::tests::a_second_process_cannot_take_a_held_scope_lock",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env(HOLDER, root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    assert!(
        lines.any(|line| line.is_ok_and(|line| line.ends_with("LOCKED"))),
        "the child never took the lock"
    );
    assert!(matches!(
        Store::open(root.path(), "default"),
        Err(OpenError::Lock(LockError::Held))
    ));
    // The kernel releases the lock when its holder exits.
    child.stdin.take().unwrap().write_all(b"\n").unwrap();
    assert!(child.wait().unwrap().success());
    let store = Store::open(root.path(), "default").unwrap();
    // A second open in the same process is refused too: flock locks
    // belong to open file descriptions.
    assert!(matches!(
        Store::open(root.path(), "default"),
        Err(OpenError::Lock(LockError::Held))
    ));
    drop(store);
}
