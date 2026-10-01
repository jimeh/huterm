//! Sequenced session, workspace, and tab structure and its reducer.

use std::collections::{HashMap, HashSet};

use crate::{PaneId, RuntimeId, SessionId, TabId, TerminalId, WorkspaceId};

/// Identifies one ordered hierarchy stream.
///
/// Today a stream is one runtime incarnation within this process. A server
/// incarnation can extend it later without changing any event.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StreamId {
    runtime: RuntimeId,
}

impl StreamId {
    /// Creates the stream published by a runtime incarnation.
    #[must_use]
    pub const fn new(runtime: RuntimeId) -> Self {
        Self { runtime }
    }

    /// Returns the runtime incarnation whose structure the stream describes.
    #[must_use]
    pub const fn runtime(self) -> RuntimeId {
        self.runtime
    }
}

/// Resolves a tab label: the custom name, then a non-blank terminal title,
/// then the fallback name.
#[must_use]
pub fn resolve_tab_name<'a>(
    custom_name: Option<&'a str>,
    terminal_title: &'a str,
    fallback_name: &'a str,
) -> &'a str {
    custom_name.unwrap_or_else(|| {
        if terminal_title.trim().is_empty() {
            fallback_name
        } else {
            terminal_title
        }
    })
}

/// Identity and names of one session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionInfo {
    /// Stable session identity.
    pub id: SessionId,
    /// Custom override, if set.
    pub custom_name: Option<String>,
    /// Immutable creation-ordinal name used without an override.
    pub automatic_name: String,
}

impl SessionInfo {
    /// Returns the custom override or the automatic name.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.custom_name.as_deref().unwrap_or(&self.automatic_name)
    }
}

/// Identity and names of one workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceInfo {
    /// Stable workspace identity.
    pub id: WorkspaceId,
    /// Custom override, if set.
    pub custom_name: Option<String>,
    /// Immutable creation-ordinal name used without an override.
    pub automatic_name: String,
}

impl WorkspaceInfo {
    /// Returns the custom override or the automatic name.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.custom_name.as_deref().unwrap_or(&self.automatic_name)
    }
}

/// Identity, pane layout, and names of one tab.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabInfo {
    /// Stable tab identity.
    pub id: TabId,
    /// The tab's single pane.
    pub pane_id: PaneId,
    /// Terminal displayed by the pane.
    pub terminal_id: TerminalId,
    /// Custom override, if set.
    pub custom_name: Option<String>,
    /// Launched program name used when the terminal has no title.
    pub fallback_name: String,
}

impl TabInfo {
    /// Resolves the label through [`resolve_tab_name`].
    #[must_use]
    pub fn display_name<'a>(&'a self, terminal_title: &'a str) -> &'a str {
        resolve_tab_name(
            self.custom_name.as_deref(),
            terminal_title,
            &self.fallback_name,
        )
    }
}

/// One committed structural change.
///
/// Positions are final: `index` is where the item sits after the change, so
/// applying an event never depends on resolving an anchor.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HierarchyEvent {
    /// A session was inserted into the session order.
    SessionCreated {
        /// The new session.
        session: SessionInfo,
        /// Position in the session order.
        index: u32,
    },
    /// A session's custom name changed.
    SessionRenamed {
        /// Renamed session.
        session: SessionId,
        /// New override; `None` resumes the automatic name.
        custom_name: Option<String>,
    },
    /// A session closed, removing its workspaces and their tabs.
    SessionClosed {
        /// Closed session.
        session: SessionId,
    },
    /// A workspace was inserted into a session.
    WorkspaceCreated {
        /// Owning session.
        session: SessionId,
        /// The new workspace.
        workspace: WorkspaceInfo,
        /// Position within the session.
        index: u32,
    },
    /// A workspace's custom name changed.
    WorkspaceRenamed {
        /// Renamed workspace.
        workspace: WorkspaceId,
        /// New override; `None` resumes the automatic name.
        custom_name: Option<String>,
    },
    /// A workspace moved within or between sessions.
    WorkspaceMoved {
        /// Moved workspace.
        workspace: WorkspaceId,
        /// Destination session.
        session: SessionId,
        /// Final position within the destination session.
        index: u32,
    },
    /// A workspace closed, removing its tabs.
    WorkspaceClosed {
        /// Closed workspace.
        workspace: WorkspaceId,
    },
    /// A tab was inserted into a workspace.
    TabOpened {
        /// Owning workspace.
        workspace: WorkspaceId,
        /// The new tab.
        tab: TabInfo,
        /// Position within the workspace.
        index: u32,
    },
    /// A tab's custom name changed.
    TabRenamed {
        /// Renamed tab.
        tab: TabId,
        /// New override; `None` resumes the terminal title.
        custom_name: Option<String>,
    },
    /// A tab moved within or between workspaces.
    TabMoved {
        /// Moved tab.
        tab: TabId,
        /// Destination workspace.
        workspace: WorkspaceId,
        /// Final position within the destination workspace.
        index: u32,
    },
    /// A tab closed.
    TabClosed {
        /// Closed tab.
        tab: TabId,
    },
    /// Application shutdown removed every session.
    Reset,
}

/// A hierarchy event with its stream and contiguous sequence number.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HierarchyEnvelope {
    /// Stream that emitted the event.
    pub stream: StreamId,
    /// Sequence number, exactly one greater than the previous event's.
    pub seq: u64,
    /// The structural change.
    pub event: HierarchyEvent,
}

/// Result of applying one envelope to a [`HierarchyState`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplyOutcome {
    /// The event applied; the summary names what it changed.
    Applied(Touched),
    /// The state already reflects this sequence. Nothing changed.
    Stale,
    /// The state can no longer follow the stream and must be replaced by a
    /// fresh snapshot. Nothing changed.
    ResyncRequired(ResyncReason),
}

/// Why an envelope could not apply in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResyncReason {
    /// One or more earlier events are missing.
    Gap,
    /// The envelope belongs to a different stream.
    ForeignStream,
    /// The event contradicts the state, such as a duplicate identity, a
    /// missing target, or an out-of-range position.
    Inconsistent,
}

/// What one or more applied events changed.
///
/// Workspaces are listed when their tab membership or order changed, or when
/// they were created, moved, or closed. Tabs are listed when their name
/// changed or they opened, moved, or closed. The names flag records any
/// change to the session or workspace lists or their names.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Touched {
    everything: bool,
    names: bool,
    workspaces: HashSet<WorkspaceId>,
    tabs: HashSet<TabId>,
}

impl Touched {
    /// Returns a summary that covers every workspace, tab, and name, for
    /// recovery from a replaced snapshot.
    #[must_use]
    pub fn everything() -> Self {
        Self {
            everything: true,
            ..Self::default()
        }
    }

    /// Whether nothing changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.everything
            && !self.names
            && self.workspaces.is_empty()
            && self.tabs.is_empty()
    }

    /// Whether the summary covers everything.
    #[must_use]
    pub fn is_everything(&self) -> bool {
        self.everything
    }

    /// Whether a session or workspace list or name changed.
    #[must_use]
    pub fn names_changed(&self) -> bool {
        self.everything || self.names
    }

    /// Whether the workspace's membership, order, or placement changed.
    #[must_use]
    pub fn contains_workspace(&self, workspace: WorkspaceId) -> bool {
        self.everything || self.workspaces.contains(&workspace)
    }

    /// Whether the tab's name, membership, or placement changed.
    #[must_use]
    pub fn contains_tab(&self, tab: TabId) -> bool {
        self.everything || self.tabs.contains(&tab)
    }

    /// Adds another summary's changes to this one.
    pub fn merge(&mut self, other: Self) {
        if self.everything {
            return;
        }
        if other.everything {
            *self = other;
            return;
        }
        self.names |= other.names;
        self.workspaces.extend(other.workspaces);
        self.tabs.extend(other.tabs);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionEntry {
    info: SessionInfo,
    workspaces: Vec<WorkspaceId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorkspaceEntry {
    info: WorkspaceInfo,
    session: SessionId,
    tabs: Vec<TabId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TabEntry {
    info: TabInfo,
    workspace: WorkspaceId,
}

/// A client's copy of one stream's sessions, workspaces, and tabs, current
/// through [`Self::seq`].
///
/// Records are indexed by identity, so lookups are constant time; each parent
/// keeps its children's order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HierarchyState {
    stream: StreamId,
    seq: u64,
    session_order: Vec<SessionId>,
    sessions: HashMap<SessionId, SessionEntry>,
    workspaces: HashMap<WorkspaceId, WorkspaceEntry>,
    tabs: HashMap<TabId, TabEntry>,
}

/// 64-bit FNV-1a offset basis.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// 64-bit FNV-1a prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

impl HierarchyState {
    /// Creates an empty state at sequence zero.
    #[must_use]
    pub fn new(stream: StreamId) -> Self {
        Self {
            stream,
            seq: 0,
            session_order: Vec::new(),
            sessions: HashMap::new(),
            workspaces: HashMap::new(),
            tabs: HashMap::new(),
        }
    }

    /// Builds a snapshot from ordered sessions, their ordered workspaces, and
    /// their ordered tabs, current through `seq`.
    ///
    /// Returns `None` if an identity repeats or belongs to another stream.
    pub fn from_sessions<S, W, T>(
        stream: StreamId,
        seq: u64,
        sessions: S,
    ) -> Option<Self>
    where
        S: IntoIterator<Item = (SessionInfo, W)>,
        W: IntoIterator<Item = (WorkspaceInfo, T)>,
        T: IntoIterator<Item = TabInfo>,
    {
        let mut state = Self::new(stream);
        state.seq = seq;
        for (session, workspaces) in sessions {
            let session_id = session.id;
            state.insert_session(session, state.session_order.len())?;
            for (workspace, tabs) in workspaces {
                let workspace_id = workspace.id;
                let index = state.sessions[&session_id].workspaces.len();
                state.insert_workspace(session_id, workspace, index)?;
                for tab in tabs {
                    let index = state.workspaces[&workspace_id].tabs.len();
                    state.insert_tab(workspace_id, tab, index)?;
                }
            }
        }
        Some(state)
    }

    /// Returns the stream this state follows.
    #[must_use]
    pub fn stream(&self) -> StreamId {
        self.stream
    }

    /// Returns the sequence of the latest applied event.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Returns session identities in order.
    #[must_use]
    pub fn sessions(&self) -> &[SessionId] {
        &self.session_order
    }

    /// Looks up a session.
    #[must_use]
    pub fn session(&self, id: SessionId) -> Option<&SessionInfo> {
        self.sessions.get(&id).map(|entry| &entry.info)
    }

    /// Returns a session's workspaces in order.
    #[must_use]
    pub fn session_workspaces(&self, id: SessionId) -> Option<&[WorkspaceId]> {
        self.sessions
            .get(&id)
            .map(|entry| entry.workspaces.as_slice())
    }

    /// Looks up a workspace.
    #[must_use]
    pub fn workspace(&self, id: WorkspaceId) -> Option<&WorkspaceInfo> {
        self.workspaces.get(&id).map(|entry| &entry.info)
    }

    /// Returns the session that owns a workspace.
    #[must_use]
    pub fn workspace_session(&self, id: WorkspaceId) -> Option<SessionId> {
        self.workspaces.get(&id).map(|entry| entry.session)
    }

    /// Returns a workspace's tabs in order.
    #[must_use]
    pub fn workspace_tabs(&self, id: WorkspaceId) -> Option<&[TabId]> {
        self.workspaces.get(&id).map(|entry| entry.tabs.as_slice())
    }

    /// Looks up a tab.
    #[must_use]
    pub fn tab(&self, id: TabId) -> Option<&TabInfo> {
        self.tabs.get(&id).map(|entry| &entry.info)
    }

    /// Returns the workspace that owns a tab.
    #[must_use]
    pub fn tab_workspace(&self, id: TabId) -> Option<WorkspaceId> {
        self.tabs.get(&id).map(|entry| entry.workspace)
    }

    /// Applies the next event of this state's stream.
    ///
    /// A foreign stream, a gap, or an event that contradicts the state
    /// requires a resync; an already applied sequence is stale. Neither
    /// changes the state.
    pub fn apply(&mut self, envelope: HierarchyEnvelope) -> ApplyOutcome {
        if envelope.stream != self.stream {
            return ApplyOutcome::ResyncRequired(ResyncReason::ForeignStream);
        }
        if envelope.seq <= self.seq {
            return ApplyOutcome::Stale;
        }
        if self.seq.checked_add(1) != Some(envelope.seq) {
            return ApplyOutcome::ResyncRequired(ResyncReason::Gap);
        }
        match self.apply_event(envelope.event) {
            Some(touched) => {
                self.seq = envelope.seq;
                ApplyOutcome::Applied(touched)
            }
            None => ApplyOutcome::ResyncRequired(ResyncReason::Inconsistent),
        }
    }

    /// Returns a stable 64-bit FNV-1a hash of the structure and names.
    ///
    /// The canonical encoding visits sessions in order, each session's
    /// workspaces in order, and each workspace's tabs in order. Integers are
    /// little-endian `u64`. A string is its byte length then its UTF-8 bytes.
    /// An optional name is `0`, or `1` followed by the string.
    ///
    /// - Session: `b'S'`, ID, custom name, automatic name, workspace count.
    /// - Workspace: `b'W'`, ID, custom name, automatic name, tab count.
    /// - Tab: `b'T'`, ID, pane ID, terminal ID, custom name, fallback name.
    ///
    /// The stream and sequence are not part of the digest. The cost is
    /// proportional to the hierarchy, so do not compute it per event.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut hash = Fnv1a(FNV_OFFSET);
        for session_id in &self.session_order {
            let session = &self.sessions[session_id];
            hash.byte(b'S');
            hash.number(session.info.id.get());
            hash.optional(session.info.custom_name.as_deref());
            hash.text(&session.info.automatic_name);
            hash.count(session.workspaces.len());
            for workspace_id in &session.workspaces {
                let workspace = &self.workspaces[workspace_id];
                hash.byte(b'W');
                hash.number(workspace.info.id.get());
                hash.optional(workspace.info.custom_name.as_deref());
                hash.text(&workspace.info.automatic_name);
                hash.count(workspace.tabs.len());
                for tab_id in &workspace.tabs {
                    let tab = &self.tabs[tab_id].info;
                    hash.byte(b'T');
                    hash.number(tab.id.get());
                    hash.number(tab.pane_id.get());
                    hash.number(tab.terminal_id.get());
                    hash.optional(tab.custom_name.as_deref());
                    hash.text(&tab.fallback_name);
                }
            }
        }
        hash.0
    }

    /// Validates and applies one event, or returns `None` without changes.
    fn apply_event(&mut self, event: HierarchyEvent) -> Option<Touched> {
        let mut touched = Touched::default();
        match event {
            HierarchyEvent::SessionCreated { session, index } => {
                self.insert_session(session, position(index)?)?;
                touched.names = true;
            }
            HierarchyEvent::SessionRenamed {
                session,
                custom_name,
            } => {
                let info = &mut self.sessions.get_mut(&session)?.info;
                touched.names = rename(&mut info.custom_name, custom_name);
            }
            HierarchyEvent::SessionClosed { session } => {
                let entry = self.sessions.remove(&session)?;
                self.session_order.retain(|id| *id != session);
                for workspace in entry.workspaces {
                    self.remove_workspace(workspace, &mut touched);
                }
                touched.names = true;
            }
            HierarchyEvent::WorkspaceCreated {
                session,
                workspace,
                index,
            } => {
                let id = workspace.id;
                self.insert_workspace(session, workspace, position(index)?)?;
                touched.names = true;
                touched.workspaces.insert(id);
            }
            HierarchyEvent::WorkspaceRenamed {
                workspace,
                custom_name,
            } => {
                let info = &mut self.workspaces.get_mut(&workspace)?.info;
                touched.names = rename(&mut info.custom_name, custom_name);
            }
            HierarchyEvent::WorkspaceMoved {
                workspace,
                session,
                index,
            } => {
                self.move_workspace(workspace, session, position(index)?)?;
                touched.names = true;
                touched.workspaces.insert(workspace);
            }
            HierarchyEvent::WorkspaceClosed { workspace } => {
                let session = self.workspaces.get(&workspace)?.session;
                self.sessions
                    .get_mut(&session)?
                    .workspaces
                    .retain(|id| *id != workspace);
                self.remove_workspace(workspace, &mut touched);
                touched.names = true;
            }
            HierarchyEvent::TabOpened {
                workspace,
                tab,
                index,
            } => {
                let id = tab.id;
                self.insert_tab(workspace, tab, position(index)?)?;
                touched.workspaces.insert(workspace);
                touched.tabs.insert(id);
            }
            HierarchyEvent::TabRenamed { tab, custom_name } => {
                let info = &mut self.tabs.get_mut(&tab)?.info;
                if rename(&mut info.custom_name, custom_name) {
                    touched.tabs.insert(tab);
                }
            }
            HierarchyEvent::TabMoved {
                tab,
                workspace,
                index,
            } => {
                let source = self.move_tab(tab, workspace, position(index)?)?;
                touched.workspaces.extend([source, workspace]);
                touched.tabs.insert(tab);
            }
            HierarchyEvent::TabClosed { tab } => {
                let entry = self.tabs.remove(&tab)?;
                if let Some(workspace) =
                    self.workspaces.get_mut(&entry.workspace)
                {
                    workspace.tabs.retain(|id| *id != tab);
                }
                touched.workspaces.insert(entry.workspace);
                touched.tabs.insert(tab);
            }
            HierarchyEvent::Reset => {
                self.session_order.clear();
                self.sessions.clear();
                self.workspaces.clear();
                self.tabs.clear();
                touched = Touched::everything();
            }
        }
        Some(touched)
    }

    /// Moves a workspace to its final position in a session, or returns
    /// `None` without changes.
    fn move_workspace(
        &mut self,
        workspace: WorkspaceId,
        session: SessionId,
        index: usize,
    ) -> Option<()> {
        let source = self.workspaces.get(&workspace)?.session;
        let remaining = self.sessions.get(&session)?.workspaces.len()
            - usize::from(source == session);
        if index > remaining {
            return None;
        }
        let siblings = &mut self.sessions.get_mut(&source)?.workspaces;
        siblings.retain(|id| *id != workspace);
        self.sessions
            .get_mut(&session)?
            .workspaces
            .insert(index, workspace);
        self.workspaces.get_mut(&workspace)?.session = session;
        Some(())
    }

    /// Moves a tab to its final position in a workspace and returns its
    /// previous workspace, or returns `None` without changes.
    fn move_tab(
        &mut self,
        tab: TabId,
        workspace: WorkspaceId,
        index: usize,
    ) -> Option<WorkspaceId> {
        let source = self.tabs.get(&tab)?.workspace;
        let remaining = self.workspaces.get(&workspace)?.tabs.len()
            - usize::from(source == workspace);
        if index > remaining {
            return None;
        }
        self.workspaces
            .get_mut(&source)?
            .tabs
            .retain(|id| *id != tab);
        self.workspaces.get_mut(&workspace)?.tabs.insert(index, tab);
        self.tabs.get_mut(&tab)?.workspace = workspace;
        Some(source)
    }

    fn owns(&self, runtime: RuntimeId) -> bool {
        runtime == self.stream.runtime()
    }

    fn insert_session(
        &mut self,
        info: SessionInfo,
        index: usize,
    ) -> Option<()> {
        if !self.owns(info.id.runtime())
            || self.sessions.contains_key(&info.id)
            || index > self.session_order.len()
        {
            return None;
        }
        self.session_order.insert(index, info.id);
        self.sessions.insert(
            info.id,
            SessionEntry {
                info,
                workspaces: Vec::new(),
            },
        );
        Some(())
    }

    fn insert_workspace(
        &mut self,
        session: SessionId,
        info: WorkspaceInfo,
        index: usize,
    ) -> Option<()> {
        if !self.owns(info.id.runtime())
            || self.workspaces.contains_key(&info.id)
        {
            return None;
        }
        let siblings = &mut self.sessions.get_mut(&session)?.workspaces;
        if index > siblings.len() {
            return None;
        }
        siblings.insert(index, info.id);
        self.workspaces.insert(
            info.id,
            WorkspaceEntry {
                info,
                session,
                tabs: Vec::new(),
            },
        );
        Some(())
    }

    fn insert_tab(
        &mut self,
        workspace: WorkspaceId,
        info: TabInfo,
        index: usize,
    ) -> Option<()> {
        if !self.owns(info.id.runtime()) || self.tabs.contains_key(&info.id) {
            return None;
        }
        let siblings = &mut self.workspaces.get_mut(&workspace)?.tabs;
        if index > siblings.len() {
            return None;
        }
        siblings.insert(index, info.id);
        self.tabs.insert(info.id, TabEntry { info, workspace });
        Some(())
    }

    /// Removes a workspace record and its tabs, leaving the parent's order
    /// to the caller.
    fn remove_workspace(
        &mut self,
        workspace: WorkspaceId,
        touched: &mut Touched,
    ) {
        if let Some(entry) = self.workspaces.remove(&workspace) {
            for tab in entry.tabs {
                self.tabs.remove(&tab);
                touched.tabs.insert(tab);
            }
        }
        touched.workspaces.insert(workspace);
    }
}

/// Replaces a custom name and reports whether it changed.
fn rename(slot: &mut Option<String>, name: Option<String>) -> bool {
    if *slot == name {
        false
    } else {
        *slot = name;
        true
    }
}

fn position(index: u32) -> Option<usize> {
    usize::try_from(index).ok()
}

struct Fnv1a(u64);

impl Fnv1a {
    fn byte(&mut self, byte: u8) {
        self.0 ^= u64::from(byte);
        self.0 = self.0.wrapping_mul(FNV_PRIME);
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.byte(byte);
        }
    }

    fn number(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn count(&mut self, value: usize) {
        // A usize always fits in u64 on supported targets.
        self.number(u64::try_from(value).unwrap_or(u64::MAX));
    }

    fn text(&mut self, value: &str) {
        self.count(value.len());
        self.bytes(value.as_bytes());
    }

    fn optional(&mut self, value: Option<&str>) {
        match value {
            None => self.byte(0),
            Some(value) => {
                self.byte(1);
                self.text(value);
            }
        }
    }
}

#[cfg(test)]
mod tests;
