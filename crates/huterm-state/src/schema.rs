//! Version 1 of `runtime.json` and `desktop.json`, and their conversions
//! to and from the protocol capture and the desktop layout.
//!
//! Readers ignore unknown fields, so an additive optional field needs no
//! version bump. A change in meaning bumps [`VERSION`] and adds a migration
//! from each older version, run in memory at load; version 1 has nothing to
//! migrate. Every list stops at its bound while it is parsed, and the file
//! size cap, checked before reading, bounds the rest.

use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use huterm_protocol::{
    MAX_DIRECTORY_CANDIDATES, MAX_SPLIT_CHILDREN, PaneId, PaneInfo, PaneLayout,
    PaneNode, RuntimeCapture, SessionCapture, SessionId, SessionInfo,
    SplitAxis, SplitChild, SplitId, SplitNode, TabId, TabInfo, TerminalCapture,
    TerminalId, TerminalLaunch, WorkspaceCapture, WorkspaceId, WorkspaceInfo,
};
use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};

use crate::bounds::{
    MAX_FILE_BYTES, MAX_LAYOUT_DEPTH, MAX_PANES, MAX_PATH_BYTES, MAX_SESSIONS,
    MAX_TABS, MAX_WINDOWS, MAX_WORKSPACES,
};
use crate::desktop::{
    DesktopLayout, FocusedPane, SessionNavigation, WindowAppearance,
    WindowBounds, WindowLayout, WindowMode,
};
use crate::{InvalidState, valid_socket_name};

/// The schema version this build reads and writes.
pub(crate) const VERSION: u64 = 1;
const RUNTIME_FORMAT: &str = "huterm-runtime";
const DESKTOP_FORMAT: &str = "huterm-desktop-layout";
const DEFAULT_SHELL: &str = "default_shell";

/// A saved scope's stable identity: 16 random bytes, kept across restores
/// and replaced with the runtime file.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ScopeId([u8; 16]);

impl ScopeId {
    /// Reads 16 bytes from `/dev/urandom`.
    pub(crate) fn generate() -> std::io::Result<Self> {
        use std::io::Read;
        let mut bytes = [0; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(Self(bytes))
    }

    /// The identity's bytes.
    #[must_use]
    pub fn bytes(&self) -> [u8; 16] {
        self.0
    }

    fn parse(text: &str) -> Option<Self> {
        if text.len() != 32 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let mut bytes = [0; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte =
                u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
        }
        Some(Self(bytes))
    }
}

impl fmt::Display for ScopeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

/// The result of reading one file's bytes.
#[derive(Debug)]
pub(crate) enum Decoded<T> {
    Ok(T),
    /// Unreadable, invalid, or over a bound, with the reason.
    Corrupt(String),
    /// Written by a newer version or naming a newer feature.
    Newer(String),
}

/// A decoded file's content with its identity.
#[derive(Debug)]
pub(crate) struct Stamped<T> {
    pub(crate) content: T,
    pub(crate) scope_id: ScopeId,
    pub(crate) generation: u64,
}

/// A list that refuses more than `N` items while it is parsed.
#[derive(Debug)]
struct Bounded<T, const N: usize>(Vec<T>);

impl<T, const N: usize> Default for Bounded<T, N> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T, const N: usize> Bounded<T, N> {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<T: Serialize, const N: usize> Serialize for Bounded<T, N> {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de>
    for Bounded<T, N>
{
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        struct List<T, const N: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for List<T, N> {
            type Value = Bounded<T, N>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a list of at most {N} items")
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    if items.len() == N {
                        return Err(de::Error::custom(format_args!(
                            "a list holds more than {N} items"
                        )));
                    }
                    items.push(item);
                }
                Ok(Bounded(items))
            }
        }
        deserializer.deserialize_seq(List(PhantomData))
    }
}

/// The fields every file starts with, read before the rest so a newer
/// version is recognized without parsing its body as this version.
#[derive(Deserialize)]
struct Header {
    format: Option<String>,
    version: Option<u64>,
}

#[derive(Deserialize, Serialize)]
struct RuntimeFile {
    format: String,
    version: u64,
    scope_id: String,
    generation: u64,
    socket_name: String,
    next_id: u64,
    session_ordinal: u64,
    /// The runtime-wide workspace ordinal from before workspaces numbered
    /// per session. Builds that require it still read this file, so it is
    /// written as the highest session's counter; this build ignores it.
    #[serde(default)]
    workspace_ordinal: u64,
    sessions: Bounded<SessionRecord, MAX_SESSIONS>,
    terminals: Bounded<TerminalRecord, MAX_PANES>,
}

#[derive(Deserialize, Serialize)]
struct SessionRecord {
    id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    custom_name: Option<String>,
    automatic_name: String,
    /// Missing from files saved before workspaces numbered per session,
    /// which a restore resumes from the session's workspaces instead.
    #[serde(default)]
    workspace_ordinal: u64,
    workspaces: Bounded<WorkspaceRecord, MAX_WORKSPACES>,
}

#[derive(Deserialize, Serialize)]
struct WorkspaceRecord {
    id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    custom_name: Option<String>,
    automatic_name: String,
    tabs: Bounded<TabRecord, MAX_TABS>,
    /// Older files hold no pin; their workspaces load unpinned.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    directory: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct TabRecord {
    id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    custom_name: Option<String>,
    layout: NodeRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    zoomed: Option<u64>,
    #[serde(default)]
    input_sync: bool,
}

/// A layout node: a pane (`pane`, `terminal`, optional `custom_name`) or a
/// split (`split`, `axis`, `weights`, `children`). Flat, so each layout
/// level nests the JSON two levels deep.
#[derive(Debug, Default, Deserialize, Serialize)]
struct NodeRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pane: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    custom_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    split: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    axis: Option<AxisRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    weights: Option<Bounded<u32, MAX_SPLIT_CHILDREN>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    children: Option<Bounded<NodeRecord, MAX_SPLIT_CHILDREN>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum AxisRecord {
    Columns,
    Rows,
}

#[derive(Deserialize, Serialize)]
struct TerminalRecord {
    id: u64,
    directories: Bounded<String, MAX_DIRECTORY_CANDIDATES>,
    launch: String,
}

#[derive(Deserialize, Serialize)]
struct DesktopFile {
    format: String,
    version: u64,
    scope_id: String,
    generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_window: Option<usize>,
    windows: Bounded<WindowRecord, MAX_WINDOWS>,
}

#[derive(Deserialize, Serialize)]
struct WindowRecord {
    session: u64,
    workspace: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_tab: Option<u64>,
    #[serde(default)]
    tab_history: Bounded<u64, MAX_TABS>,
    #[serde(default)]
    focused_panes: Bounded<FocusRecord, MAX_TABS>,
    bounds: BoundsRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display: Option<String>,
    mode: ModeRecord,
    sidebar_width: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_panel_visible: Option<bool>,
    #[serde(default, skip_serializing_if = "is_zero")]
    workspace_panel_width: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quake_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    appearance: Option<AppearanceRecord>,
    #[serde(default, skip_serializing_if = "Bounded::is_empty")]
    remembered: Bounded<NavigationRecord, MAX_SESSIONS>,
}

/// One remembered session's navigation, with a window's own field names.
#[derive(Deserialize, Serialize)]
struct NavigationRecord {
    session: u64,
    workspace: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_tab: Option<u64>,
    #[serde(default)]
    tab_history: Bounded<u64, MAX_TABS>,
    #[serde(default)]
    focused_panes: Bounded<FocusRecord, MAX_TABS>,
}

#[derive(Deserialize, Serialize)]
struct FocusRecord {
    tab: u64,
    pane: u64,
}

#[derive(Deserialize, Serialize)]
struct BoundsRecord {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ModeRecord {
    Windowed,
    Maximized,
    Fullscreen,
}

#[derive(Deserialize, Serialize)]
struct AppearanceRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opacity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    background_opacity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    foreground_opacity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blur: Option<u32>,
}

/// Checks every bound and rule a runtime file meets, for the writer and
/// the reader alike: the socket grammar, the counts, layout depth, saved
/// paths, and the capture's own validation.
pub(crate) fn check_runtime(
    capture: &RuntimeCapture,
) -> Result<(), InvalidState> {
    if !valid_socket_name(&capture.socket_name) {
        return Err(InvalidState::new("the socket name is invalid"));
    }
    let workspaces = capture.sessions.iter().map(|s| s.workspaces.len()).sum();
    let tabs = capture.tabs().count();
    let panes = capture.tabs().map(|tab| tab.layout.pane_count()).sum();
    for (count, bound, kind) in [
        (capture.sessions.len(), MAX_SESSIONS, "sessions"),
        (workspaces, MAX_WORKSPACES, "workspaces"),
        (tabs, MAX_TABS, "tabs"),
        (panes, MAX_PANES, "panes"),
        (capture.terminals.len(), MAX_PANES, "terminals"),
    ] {
        if count > bound {
            return Err(InvalidState::new(format!("more than {bound} {kind}")));
        }
    }
    if capture
        .tabs()
        .any(|tab| tab.layout.depth() > MAX_LAYOUT_DEPTH)
    {
        return Err(InvalidState::new(format!(
            "a layout is deeper than {MAX_LAYOUT_DEPTH}"
        )));
    }
    let workspace_directories = capture
        .sessions
        .iter()
        .flat_map(|session| &session.workspaces)
        .filter_map(|workspace| workspace.directory.as_ref());
    for directory in capture
        .terminals
        .iter()
        .flat_map(|t| &t.directories)
        .chain(workspace_directories)
    {
        let Some(text) = directory.to_str().filter(|_| directory.is_absolute())
        else {
            return Err(InvalidState::new("a saved directory is not absolute"));
        };
        if text.len() > MAX_PATH_BYTES {
            return Err(InvalidState::new(format!(
                "a saved directory is longer than {MAX_PATH_BYTES} bytes"
            )));
        }
    }
    capture
        .validate()
        .map_err(|error| InvalidState::new(error.to_string()))
}

/// Encodes a runtime capture, refusing one over a bound. Directory
/// candidates are optional, so ones JSON cannot hold (not UTF-8), a restore
/// cannot use (not absolute), or over the path bound are left out; a
/// restore falls back to the next one. The reader rejects all three.
pub(crate) fn encode_runtime(
    capture: &RuntimeCapture,
    scope_id: ScopeId,
    generation: u64,
) -> Result<Vec<u8>, InvalidState> {
    let mut capture = capture.clone();
    let savable = |path: &PathBuf| {
        path.is_absolute()
            && path
                .to_str()
                .is_some_and(|text| text.len() <= MAX_PATH_BYTES)
    };
    for terminal in &mut capture.terminals {
        terminal.directories.retain(savable);
    }
    for workspace in capture
        .sessions
        .iter_mut()
        .flat_map(|session| &mut session.workspaces)
    {
        workspace.directory = workspace.directory.take().filter(savable);
    }
    check_runtime(&capture)?;
    let file = RuntimeFile {
        format: RUNTIME_FORMAT.into(),
        version: VERSION,
        scope_id: scope_id.to_string(),
        generation,
        socket_name: capture.socket_name.clone(),
        next_id: capture.next_id,
        session_ordinal: capture.session_ordinal,
        workspace_ordinal: capture
            .sessions
            .iter()
            .map(|session| session.workspace_ordinal)
            .max()
            .unwrap_or(0),
        sessions: Bounded(
            capture.sessions.iter().map(session_record).collect(),
        ),
        terminals: Bounded(
            capture
                .terminals
                .iter()
                .map(terminal_record)
                .collect::<Result<_, _>>()?,
        ),
    };
    bounded_bytes(serde_json::to_vec(&file))
}

/// Encodes a desktop layout, refusing one over a bound.
pub(crate) fn encode_desktop(
    layout: &DesktopLayout,
    scope_id: ScopeId,
    generation: u64,
) -> Result<Vec<u8>, InvalidState> {
    layout.validate()?;
    let file = DesktopFile {
        format: DESKTOP_FORMAT.into(),
        version: VERSION,
        scope_id: scope_id.to_string(),
        generation,
        active_window: layout.active_window,
        windows: Bounded(layout.windows.iter().map(window_record).collect()),
    };
    bounded_bytes(serde_json::to_vec(&file))
}

fn bounded_bytes(
    bytes: serde_json::Result<Vec<u8>>,
) -> Result<Vec<u8>, InvalidState> {
    let bytes = bytes.map_err(|error| InvalidState::new(error.to_string()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(InvalidState::new(format!(
            "the file would exceed {MAX_FILE_BYTES} bytes"
        )));
    }
    Ok(bytes)
}

/// Decodes a runtime file's bytes.
pub(crate) fn decode_runtime(bytes: &[u8]) -> Decoded<Stamped<RuntimeCapture>> {
    if let Some(early) = header(bytes, RUNTIME_FORMAT) {
        return early;
    }
    let file: RuntimeFile = match serde_json::from_slice(bytes) {
        Ok(file) => file,
        Err(error) => return Decoded::Corrupt(error.to_string()),
    };
    let Some(scope_id) = ScopeId::parse(&file.scope_id) else {
        return Decoded::Corrupt("the scope identity is invalid".into());
    };
    let capture = match runtime_capture(file.sessions, file.terminals) {
        Ok((sessions, terminals)) => RuntimeCapture {
            socket_name: file.socket_name,
            next_id: file.next_id,
            session_ordinal: file.session_ordinal,
            sessions,
            terminals,
        },
        Err(Unreadable::Invalid(error)) => return Decoded::Corrupt(error.0),
        Err(Unreadable::Newer(reason)) => return Decoded::Newer(reason),
    };
    if let Err(error) = check_runtime(&capture) {
        return Decoded::Corrupt(error.0);
    }
    Decoded::Ok(Stamped {
        content: capture,
        scope_id,
        generation: file.generation,
    })
}

/// Decodes a desktop file's bytes.
pub(crate) fn decode_desktop(bytes: &[u8]) -> Decoded<Stamped<DesktopLayout>> {
    if let Some(early) = header(bytes, DESKTOP_FORMAT) {
        return early;
    }
    let file: DesktopFile = match serde_json::from_slice(bytes) {
        Ok(file) => file,
        Err(error) => return Decoded::Corrupt(error.to_string()),
    };
    let Some(scope_id) = ScopeId::parse(&file.scope_id) else {
        return Decoded::Corrupt("the scope identity is invalid".into());
    };
    let layout = DesktopLayout {
        windows: file.windows.0.into_iter().map(window_layout).collect(),
        active_window: file.active_window,
    };
    if let Err(error) = layout.validate() {
        return Decoded::Corrupt(error.0);
    }
    Decoded::Ok(Stamped {
        content: layout,
        scope_id,
        generation: file.generation,
    })
}

/// Classifies a file by its header: `None` when it is this version of
/// `format`, so the body can be read.
fn header<T>(bytes: &[u8], format: &str) -> Option<Decoded<T>> {
    let header: Header = match serde_json::from_slice(bytes) {
        Ok(header) => header,
        Err(error) => return Some(Decoded::Corrupt(error.to_string())),
    };
    if header.format.as_deref() != Some(format) {
        return Some(Decoded::Corrupt(format!("the file is not {format}")));
    }
    match header.version {
        Some(VERSION) => None,
        Some(version) if version > VERSION => {
            Some(Decoded::Newer(format!("version {version}")))
        }
        _ => Some(Decoded::Corrupt("the version is missing or invalid".into())),
    }
}

enum Unreadable {
    Invalid(InvalidState),
    Newer(String),
}

impl From<InvalidState> for Unreadable {
    fn from(error: InvalidState) -> Self {
        Self::Invalid(error)
    }
}

type Captured = (Vec<SessionCapture>, Vec<TerminalCapture>);

fn runtime_capture(
    sessions: Bounded<SessionRecord, MAX_SESSIONS>,
    terminals: Bounded<TerminalRecord, MAX_PANES>,
) -> Result<Captured, Unreadable> {
    let sessions = sessions
        .0
        .into_iter()
        .map(|session| {
            Ok(SessionCapture {
                info: SessionInfo {
                    id: SessionId::new(session.id),
                    custom_name: session.custom_name,
                    automatic_name: session.automatic_name,
                },
                workspace_ordinal: session.workspace_ordinal,
                workspaces: session
                    .workspaces
                    .0
                    .into_iter()
                    .map(workspace_capture)
                    .collect::<Result<_, InvalidState>>()?,
            })
        })
        .collect::<Result<_, InvalidState>>()?;
    let terminals = terminals
        .0
        .into_iter()
        .map(|terminal| {
            if terminal.launch != DEFAULT_SHELL {
                return Err(Unreadable::Newer(format!(
                    "launch {:?}",
                    terminal.launch
                )));
            }
            Ok(TerminalCapture {
                id: TerminalId::new(terminal.id),
                directories: terminal
                    .directories
                    .0
                    .into_iter()
                    .map(PathBuf::from)
                    .collect(),
                launch: TerminalLaunch::DefaultShell,
            })
        })
        .collect::<Result<_, Unreadable>>()?;
    Ok((sessions, terminals))
}

fn workspace_capture(
    workspace: WorkspaceRecord,
) -> Result<WorkspaceCapture, InvalidState> {
    Ok(WorkspaceCapture {
        info: WorkspaceInfo {
            id: WorkspaceId::new(workspace.id),
            custom_name: workspace.custom_name,
            automatic_name: workspace.automatic_name,
            pinned: workspace.pinned,
        },
        directory: workspace.directory.map(PathBuf::from),
        tabs: workspace
            .tabs
            .0
            .into_iter()
            .map(|tab| {
                let root = layout_node(tab.layout, 1)?;
                let layout = PaneLayout::new(root)
                    .map_err(|error| InvalidState::new(error.to_string()))?;
                Ok(TabInfo {
                    id: TabId::new(tab.id),
                    custom_name: tab.custom_name,
                    layout,
                    zoomed: tab.zoomed.map(PaneId::new),
                    input_sync: tab.input_sync,
                })
            })
            .collect::<Result<_, _>>()?,
    })
}

/// Builds a layout node at `depth`, refusing one past the depth bound
/// before descending further.
fn layout_node(
    node: NodeRecord,
    depth: usize,
) -> Result<PaneNode, InvalidState> {
    if depth > MAX_LAYOUT_DEPTH {
        return Err(InvalidState::new(format!(
            "a layout is deeper than {MAX_LAYOUT_DEPTH}"
        )));
    }
    match node {
        NodeRecord {
            pane: Some(pane),
            terminal: Some(terminal),
            custom_name,
            split: None,
            axis: None,
            weights: None,
            children: None,
        } => Ok(PaneNode::Pane(PaneInfo {
            id: PaneId::new(pane),
            terminal_id: TerminalId::new(terminal),
            // A restore names panes after the command it launches.
            fallback_name: String::new(),
            custom_name,
        })),
        NodeRecord {
            pane: None,
            terminal: None,
            custom_name: None,
            split: Some(split),
            axis: Some(axis),
            weights: Some(weights),
            children: Some(children),
        } if weights.0.len() == children.0.len() => {
            Ok(PaneNode::Split(SplitNode {
                id: SplitId::new(split),
                axis: match axis {
                    AxisRecord::Columns => SplitAxis::Columns,
                    AxisRecord::Rows => SplitAxis::Rows,
                },
                children: weights
                    .0
                    .into_iter()
                    .zip(children.0)
                    .map(|(weight, child)| {
                        Ok(SplitChild {
                            weight,
                            node: layout_node(child, depth + 1)?,
                        })
                    })
                    .collect::<Result<_, InvalidState>>()?,
            }))
        }
        _ => Err(InvalidState::new("a layout node is neither pane nor split")),
    }
}

fn session_record(session: &SessionCapture) -> SessionRecord {
    SessionRecord {
        id: session.info.id.get(),
        custom_name: session.info.custom_name.clone(),
        automatic_name: session.info.automatic_name.clone(),
        workspace_ordinal: session.workspace_ordinal,
        workspaces: Bounded(
            session
                .workspaces
                .iter()
                .map(|workspace| WorkspaceRecord {
                    id: workspace.info.id.get(),
                    custom_name: workspace.info.custom_name.clone(),
                    automatic_name: workspace.info.automatic_name.clone(),
                    pinned: workspace.info.pinned,
                    directory: workspace
                        .directory
                        .as_deref()
                        .and_then(Path::to_str)
                        .map(str::to_owned),
                    tabs: Bounded(
                        workspace
                            .tabs
                            .iter()
                            .map(|tab| TabRecord {
                                id: tab.id.get(),
                                custom_name: tab.custom_name.clone(),
                                layout: node_record(tab.layout.root()),
                                zoomed: tab.zoomed.map(PaneId::get),
                                input_sync: tab.input_sync,
                            })
                            .collect(),
                    ),
                })
                .collect(),
        ),
    }
}

fn node_record(node: &PaneNode) -> NodeRecord {
    match node {
        PaneNode::Pane(pane) => NodeRecord {
            pane: Some(pane.id.get()),
            terminal: Some(pane.terminal_id.get()),
            custom_name: pane.custom_name.clone(),
            ..NodeRecord::default()
        },
        PaneNode::Split(split) => NodeRecord {
            split: Some(split.id.get()),
            axis: Some(match split.axis {
                SplitAxis::Columns => AxisRecord::Columns,
                SplitAxis::Rows => AxisRecord::Rows,
            }),
            weights: Some(Bounded(
                split.children.iter().map(|child| child.weight).collect(),
            )),
            children: Some(Bounded(
                split
                    .children
                    .iter()
                    .map(|child| node_record(&child.node))
                    .collect(),
            )),
            ..NodeRecord::default()
        },
    }
}

fn terminal_record(
    terminal: &TerminalCapture,
) -> Result<TerminalRecord, InvalidState> {
    let launch = match terminal.launch {
        TerminalLaunch::DefaultShell => DEFAULT_SHELL,
        _ => return Err(InvalidState::new("a launch has no saved form")),
    };
    Ok(TerminalRecord {
        id: terminal.id.get(),
        directories: Bounded(
            terminal
                .directories
                .iter()
                .filter_map(|path| Path::to_str(path).map(str::to_owned))
                .collect(),
        ),
        launch: launch.into(),
    })
}

fn window_record(window: &WindowLayout) -> WindowRecord {
    WindowRecord {
        session: window.session.get(),
        workspace: window.workspace.get(),
        active_tab: window.active_tab.map(TabId::get),
        tab_history: tab_ids(&window.tab_history),
        focused_panes: focus_records(&window.focused_panes),
        bounds: BoundsRecord {
            x: window.bounds.x,
            y: window.bounds.y,
            width: window.bounds.width,
            height: window.bounds.height,
        },
        display: window.display.clone(),
        mode: match window.mode {
            WindowMode::Windowed => ModeRecord::Windowed,
            WindowMode::Maximized => ModeRecord::Maximized,
            WindowMode::Fullscreen => ModeRecord::Fullscreen,
        },
        sidebar_width: window.sidebar_width,
        workspace_panel_visible: window.workspace_panel_visible,
        workspace_panel_width: window.workspace_panel_width,
        quake_profile: window.quake_profile.clone(),
        appearance: window.appearance.map(|appearance| AppearanceRecord {
            opacity: appearance.opacity,
            background_opacity: appearance.background_opacity,
            foreground_opacity: appearance.foreground_opacity,
            blur: appearance.blur,
        }),
        remembered: Bounded(
            window
                .remembered
                .iter()
                .map(|memory| NavigationRecord {
                    session: memory.session.get(),
                    workspace: memory.workspace.get(),
                    active_tab: memory.active_tab.map(TabId::get),
                    tab_history: tab_ids(&memory.tab_history),
                    focused_panes: focus_records(&memory.focused_panes),
                })
                .collect(),
        ),
    }
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

fn tab_ids(tabs: &[TabId]) -> Bounded<u64, MAX_TABS> {
    Bounded(tabs.iter().map(|tab| tab.get()).collect())
}

fn focus_records(focused: &[FocusedPane]) -> Bounded<FocusRecord, MAX_TABS> {
    Bounded(
        focused
            .iter()
            .map(|focus| FocusRecord {
                tab: focus.tab.get(),
                pane: focus.pane.get(),
            })
            .collect(),
    )
}

fn window_layout(window: WindowRecord) -> WindowLayout {
    WindowLayout {
        session: SessionId::new(window.session),
        workspace: WorkspaceId::new(window.workspace),
        active_tab: window.active_tab.map(TabId::new),
        tab_history: tab_history(window.tab_history),
        focused_panes: focused_panes(window.focused_panes),
        bounds: WindowBounds {
            x: window.bounds.x,
            y: window.bounds.y,
            width: window.bounds.width,
            height: window.bounds.height,
        },
        display: window.display,
        mode: match window.mode {
            ModeRecord::Windowed => WindowMode::Windowed,
            ModeRecord::Maximized => WindowMode::Maximized,
            ModeRecord::Fullscreen => WindowMode::Fullscreen,
        },
        sidebar_width: window.sidebar_width,
        workspace_panel_visible: window.workspace_panel_visible,
        workspace_panel_width: window.workspace_panel_width,
        quake_profile: window.quake_profile,
        appearance: window.appearance.map(|appearance| WindowAppearance {
            opacity: appearance.opacity,
            background_opacity: appearance.background_opacity,
            foreground_opacity: appearance.foreground_opacity,
            blur: appearance.blur,
        }),
        remembered: window
            .remembered
            .0
            .into_iter()
            .map(|memory| SessionNavigation {
                session: SessionId::new(memory.session),
                workspace: WorkspaceId::new(memory.workspace),
                active_tab: memory.active_tab.map(TabId::new),
                tab_history: tab_history(memory.tab_history),
                focused_panes: focused_panes(memory.focused_panes),
            })
            .collect(),
    }
}

fn tab_history(tabs: Bounded<u64, MAX_TABS>) -> Vec<TabId> {
    tabs.0.into_iter().map(TabId::new).collect()
}

fn focused_panes(focused: Bounded<FocusRecord, MAX_TABS>) -> Vec<FocusedPane> {
    focused
        .0
        .into_iter()
        .map(|focus| FocusedPane {
            tab: TabId::new(focus.tab),
            pane: PaneId::new(focus.pane),
        })
        .collect()
}

#[cfg(test)]
mod tests;
