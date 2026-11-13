//! Contract tests: each scenario runs on a fresh `Mux` in process and over
//! IPC, and both paths must give the same results and hierarchy digests.
//!
//! A scenario drives a [`Script`] with the wire's `Request`s, the
//! vocabulary both paths share. [`Direct`] calls `Mux` itself, as embedded
//! mode does, maps core errors through [`wire_error`], the documented
//! mapping, and keeps close tickets and layout history with the server's
//! semantics over core's own values. [`Ipc`] runs the native client against
//! a [`crate::Listener`] on a socket in a private temporary directory, so
//! every scenario crosses the real socket transport, peer check included.
//! Every step records its
//! result and then the hierarchy's sequence and digest: from
//! `Mux::hierarchy_digest` in process and from the client's projection over
//! IPC. Results compare identities by number, opaque tokens and tickets by
//! first appearance, and errors by code, retry, and committed position.
//! Each test then pins which steps failed, so a scenario cannot pass by
//! failing the same way on both paths.
//!
//! The harness sits in `huterm-server`'s unit tests rather than a `tests/`
//! target because the server's test support, such as the quiet command,
//! is compiled for `cfg(test)`, which an integration target of the same
//! crate does not get.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use huterm_client::native::{Client, ClientHandle};
use huterm_client::{ClientConfig, Resolution};
use huterm_core::{self as core, JobState, Mux, MuxError};
use huterm_protocol::{
    Arrangement, DigestProfile, LayoutChange, MAX_NAME_BYTES, PaneLayout,
    RuntimeId, SPLIT_TOTAL, SessionId, SplitAxis, SplitDirection, TabId,
    TerminalCommand, WorkspaceId,
};
use huterm_wire::error::{ErrorCode, Retry, WireError};
use huterm_wire::handshake::ClientKind;
use huterm_wire::request::{
    Beside, CloseEffect, CloseRequest, MoveDestination, PaneRef, Request,
    RequestKind, RestoreDirection,
};
use huterm_wire::response::{
    BusyTerminal, CloseAssessment, Committed, OpenedPane, OpenedTab,
    ProcessSummary, Response, Selection, Success, TerminalJobs,
};

use crate::runtime::wire_error;
use crate::test_support::{SocketServer, quiet_command, server};
use crate::{Server, ServerConfig};

/// How long the IPC path waits for something that should happen promptly.
const PROMPT: Duration = Duration::from_secs(10);

/// What a caller sees of one runtime, in process or over IPC.
trait Path {
    /// Runs `request`. Once it returns, the path's view of the hierarchy
    /// holds everything the request committed.
    fn request(&mut self, request: Request) -> Result<Success, WireError>;

    /// The hierarchy's sequence and digest as the caller sees them.
    fn hierarchy(&mut self) -> (u64, u64);

    /// A tab's layout as the caller sees it.
    fn layout(&mut self, tab: TabId) -> PaneLayout;

    /// Ends the path and shuts its `Mux` down.
    fn shutdown(self: Box<Self>);
}

/// Runs `scenario` on both paths and compares their transcripts step by
/// step, returning the in-process transcript.
fn contract(scenario: fn(&mut Script)) -> Vec<Step> {
    let direct = Script::run("in process", Box::new(Direct::new()), scenario);
    let ipc = Script::run("over IPC", Box::new(Ipc::new()), scenario);
    for (index, (direct, ipc)) in direct.iter().zip(&ipc).enumerate() {
        assert_eq!(
            direct, ipc,
            "step {index} differs in process (left) and over IPC (right)"
        );
    }
    assert_eq!(direct.len(), ipc.len(), "the paths ran different steps");
    direct
}

/// The steps of a transcript that failed, with their codes.
fn failures(steps: &[Step]) -> Vec<(RequestKind, ErrorCode)> {
    steps
        .iter()
        .filter_map(|step| match &step.outcome {
            Outcome::Failed { code, .. } => Some((step.request, *code)),
            Outcome::Succeeded { .. } => None,
        })
        .collect()
}

/// One step of a transcript, normalized so both paths can compare.
#[derive(Debug, Eq, PartialEq)]
struct Step {
    request: RequestKind,
    outcome: Outcome,
    /// The hierarchy's sequence after the step.
    seq: u64,
    /// The hierarchy's digest after the step.
    digest: u64,
}

#[derive(Debug, Eq, PartialEq)]
enum Outcome {
    Succeeded {
        /// The response's `Debug` form with every runtime scope and
        /// process ID blanked and opaque handles numbered by appearance.
        response: String,
        committed: Option<Committed>,
    },
    Failed {
        code: ErrorCode,
        retry: Retry,
        committed: Option<Committed>,
    },
}

/// A scenario's view of one path, recording a transcript.
struct Script {
    label: &'static str,
    path: Box<dyn Path>,
    steps: Vec<Step>,
    tokens: HashMap<u64, u64>,
    tickets: HashMap<u32, u32>,
}

impl Script {
    fn run(
        label: &'static str,
        path: Box<dyn Path>,
        scenario: fn(&mut Self),
    ) -> Vec<Step> {
        let mut script = Self {
            label,
            path,
            steps: Vec::new(),
            tokens: HashMap::new(),
            tickets: HashMap::new(),
        };
        scenario(&mut script);
        script.path.shutdown();
        script.steps
    }

    /// Runs and records one request whose outcome only the comparison
    /// judges.
    fn step(&mut self, request: Request) {
        let _ = self.record(request);
    }

    /// Runs and records one request.
    fn record(&mut self, request: Request) -> Result<Success, WireError> {
        let kind = request.kind();
        let outcome = self.path.request(request);
        let (seq, digest) = self.path.hierarchy();
        let normalized = self.normalize(&outcome);
        self.steps.push(Step {
            request: kind,
            outcome: normalized,
            seq,
            digest,
        });
        outcome
    }

    /// Runs a request the scenario needs to succeed.
    fn ok(&mut self, request: Request) -> Response {
        let kind = request.kind();
        match self.record(request) {
            Ok(success) => success.response,
            Err(error) => panic!("{}: {kind:?} failed: {error:?}", self.label),
        }
    }

    fn layout(&mut self, tab: TabId) -> PaneLayout {
        self.path.layout(tab)
    }

    fn session(&mut self, name: Option<&str>) -> SessionId {
        let name = name.map(str::to_owned);
        match self.ok(Request::CreateSession { name }) {
            Response::Session(session) => session,
            other => panic!("{}: a session, not {other:?}", self.label),
        }
    }

    fn workspace(
        &mut self,
        session: SessionId,
        name: Option<&str>,
    ) -> WorkspaceId {
        let name = name.map(str::to_owned);
        match self.ok(Request::CreateWorkspace { session, name }) {
            Response::Workspace(workspace) => workspace,
            other => panic!("{}: a workspace, not {other:?}", self.label),
        }
    }

    fn tab(&mut self, workspace: WorkspaceId) -> OpenedTab {
        match self.ok(Request::OpenTab {
            workspace,
            command: quiet(),
        }) {
            Response::OpenedTab(opened) => opened,
            other => panic!("{}: a tab, not {other:?}", self.label),
        }
    }

    /// A session with one workspace holding one tab.
    fn fixture(&mut self) -> (SessionId, WorkspaceId, OpenedTab) {
        let session = self.session(None);
        let workspace = self.workspace(session, None);
        let tab = self.tab(workspace);
        (session, workspace, tab)
    }

    fn split(
        &mut self,
        workspace: WorkspaceId,
        tab: &OpenedTab,
        pane: huterm_protocol::PaneId,
        direction: SplitDirection,
    ) -> OpenedPane {
        match self.ok(Request::SplitPane {
            workspace,
            tab: tab.tab,
            pane,
            direction,
            command: quiet(),
        }) {
            Response::OpenedPane(opened) => opened,
            other => panic!("{}: a pane, not {other:?}", self.label),
        }
    }

    /// Runs a rearrangement or restore that files an entry, returning its
    /// token.
    fn token(&mut self, request: Request) -> u64 {
        match self.ok(request) {
            Response::Undo { token: Some(token) } => token,
            other => panic!("{}: an undo token, not {other:?}", self.label),
        }
    }

    fn ticket(&mut self, request: CloseRequest) -> u32 {
        match self.ok(Request::PrepareClose { request }) {
            Response::CloseAssessment(assessment) => assessment.ticket,
            other => panic!("{}: a close ticket, not {other:?}", self.label),
        }
    }

    fn normalize(&mut self, outcome: &Result<Success, WireError>) -> Outcome {
        match outcome {
            Ok(success) => {
                let response = match success.response.clone() {
                    Response::Undo { token } => Response::Undo {
                        token: token.map(|token| {
                            let next = self.tokens.len() as u64 + 1;
                            *self.tokens.entry(token).or_insert(next)
                        }),
                    },
                    Response::CloseAssessment(mut assessment) => {
                        let next =
                            u32::try_from(self.tickets.len()).unwrap() + 1;
                        assessment.ticket = *self
                            .tickets
                            .entry(assessment.ticket)
                            .or_insert(next);
                        Response::CloseAssessment(assessment)
                    }
                    other => other,
                };
                Outcome::Succeeded {
                    response: blank(
                        "pid: ",
                        &blank("RuntimeId(", &format!("{response:?}")),
                    ),
                    committed: success.committed,
                }
            }
            Err(error) => Outcome::Failed {
                code: error.code,
                retry: error.retry,
                committed: error.committed,
            },
        }
    }
}

/// `text` with the digits after every `marker` replaced by `_`, so
/// identities compare by number and process IDs not at all.
fn blank(marker: &str, text: &str) -> String {
    let mut blanked = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(marker) {
        let (head, tail) = rest.split_at(at + marker.len());
        blanked.push_str(head);
        blanked.push('_');
        rest = tail.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    blanked.push_str(rest);
    blanked
}

fn quiet() -> Box<TerminalCommand> {
    Box::new(quiet_command())
}

/// The in-process path: `Mux` called directly.
struct Direct {
    mux: Mux,
    tickets: BTreeMap<u32, core::CloseAssessment>,
    last_ticket: u32,
    history: History,
}

impl Direct {
    fn new() -> Self {
        Self {
            mux: Mux::default(),
            tickets: BTreeMap::new(),
            last_ticket: 0,
            history: History::default(),
        }
    }

    fn committed(&self) -> Committed {
        Committed {
            seq: self.mux.hierarchy_seq(),
            digest: Some(self.mux.hierarchy_digest(DigestProfile::Base)),
        }
    }

    /// A result as the wire states it: a success carries where it
    /// committed, and a failure carries it only when the sequence moved.
    fn finish(
        &self,
        before: u64,
        result: Result<Response, MuxError>,
    ) -> Result<Success, WireError> {
        match result {
            Ok(response) => Ok(Success {
                response,
                committed: Some(self.committed()),
            }),
            Err(error) => {
                let mut wire = wire_error(&error);
                if self.mux.hierarchy_seq() != before {
                    wire.committed = Some(self.committed());
                }
                Err(wire)
            }
        }
    }

    fn rearranged(
        &mut self,
        slot: u32,
        before: u64,
        result: Result<LayoutChange, MuxError>,
    ) -> Result<Success, WireError> {
        let (result, change) = match result {
            Ok(change) => (Ok(Response::Undo { token: None }), change),
            Err(error) => (Err(error), LayoutChange::default()),
        };
        let mut success = self.finish(before, result)?;
        let seq = self.mux.hierarchy_seq();
        success.response = Response::Undo {
            token: self.history.file(slot, change, seq),
        };
        Ok(success)
    }

    fn restore(
        &mut self,
        token: u64,
        direction: RestoreDirection,
    ) -> Result<Success, WireError> {
        let Some((slot, change)) = self.history.take(token, direction) else {
            return Err(WireError::new(ErrorCode::StaleUndo, "no entry"));
        };
        let before = self.mux.hierarchy_seq();
        let result = self
            .mux
            .restore_tabs(&change, core_direction(direction))
            .map(|_| Response::Undo { token: None });
        // A failed restore drops its entry.
        let mut success = self.finish(before, result)?;
        let seq = self.mux.hierarchy_seq();
        success.response = Response::Undo {
            token: Some(self.history.refile(slot, direction, change, seq)),
        };
        Ok(success)
    }

    fn prepare_close(
        &mut self,
        request: CloseRequest,
    ) -> Result<Success, WireError> {
        let ticket = self
            .mux
            .prepare_close(core_close(request))
            .map_err(|error| wire_error(&error))?;
        let committed = self.committed();
        let assessment = ticket.check_jobs();
        self.last_ticket += 1;
        let summary = CloseAssessment {
            ticket: self.last_ticket,
            effect: wire_effect(assessment.effect()),
            needs_confirmation: assessment.needs_confirmation(),
            busy: busy(&assessment),
            omitted_terminals: 0,
            omitted_processes: 0,
        };
        self.tickets.insert(self.last_ticket, assessment);
        Ok(Success {
            response: Response::CloseAssessment(summary),
            committed: Some(committed),
        })
    }

    fn commit_close(
        &mut self,
        ticket: u32,
        confirmed: bool,
    ) -> Result<Success, WireError> {
        let Some(consent) = self.tickets.remove(&ticket) else {
            return Err(WireError::new(ErrorCode::StaleTarget, "no ticket"));
        };
        let current = consent.recheck();
        let before = self.mux.hierarchy_seq();
        let result = self.mux.commit_close(&consent, &current, confirmed);
        // Only a commit that needs confirmation keeps its ticket.
        if matches!(result, Err(MuxError::ConfirmationRequired)) {
            self.tickets.insert(ticket, consent);
        }
        self.finish(before, result.map(|()| Response::Unit))
    }
}

impl Path for Direct {
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per covered request mirrors the request inventory"
    )]
    fn request(&mut self, request: Request) -> Result<Success, WireError> {
        let before = self.mux.hierarchy_seq();
        let unit =
            |result: Result<(), MuxError>| result.map(|()| Response::Unit);
        let mux = &mut self.mux;
        let result = match request {
            Request::SelectTab { tab } => mux.select_tab(tab).map(|target| {
                Response::Selection(Selection {
                    session: target.session,
                    workspace: target.workspace,
                    tab: target.tab,
                    pane: target.pane,
                })
            }),
            Request::CreateSession { name } => {
                mux.create_session(name.as_deref()).map(Response::Session)
            }
            Request::CreateWorkspace { session, name } => mux
                .create_workspace(session, name.as_deref())
                .map(Response::Workspace),
            Request::CreateWorkspaceWithTab {
                session,
                name,
                command,
            } => mux
                .create_workspace_with_tab(session, name.as_deref(), &command)
                .map(|(workspace, opened)| opened_tab(workspace, &opened)),
            Request::OpenTab { workspace, command } => mux
                .open_tab(workspace, &command)
                .map(|opened| opened_tab(workspace, &opened)),
            Request::SplitPane {
                workspace,
                tab,
                pane,
                direction,
                command,
            } => mux
                .split_pane(workspace, tab, pane, direction, &command)
                .map(|opened| {
                    Response::OpenedPane(OpenedPane {
                        tab: opened.tab,
                        pane: opened.pane.id,
                        terminal: opened.pane.terminal_id,
                    })
                }),
            Request::ClosePane {
                workspace,
                tab,
                pane,
            } => unit(mux.close_pane(workspace, tab, pane)),
            Request::CloseTab { workspace, tab } => {
                unit(mux.close_tab(workspace, tab))
            }
            Request::RenameSession { session, name } => {
                unit(mux.rename_session(session, name.as_deref()))
            }
            Request::RenameWorkspace { workspace, name } => {
                unit(mux.rename_workspace(workspace, name.as_deref()))
            }
            Request::RenameTab { tab, name } => {
                unit(mux.rename_tab(tab, name.as_deref()))
            }
            Request::RenamePane { pane, name } => {
                unit(mux.rename_pane(pane, name.as_deref()))
            }
            Request::ReorderTab {
                workspace,
                tab,
                before,
            } => unit(mux.reorder_tab(workspace, tab, before)),
            Request::MoveTab {
                source,
                tab,
                destination,
                before,
            } => unit(mux.move_tab(source, tab, destination, before)),
            Request::MoveSession { session, before } => {
                unit(mux.move_session(session, before))
            }
            Request::MoveWorkspace {
                source,
                workspace,
                destination,
                before,
            } => {
                unit(mux.move_workspace(source, workspace, destination, before))
            }
            Request::MoveWorkspaceToNewSession { source, workspace } => mux
                .move_workspace_to_new_session(source, workspace)
                .map(Response::Session),
            Request::SwapPanes {
                slot,
                workspace,
                tab,
                a,
                b,
            } => {
                let result = mux.swap_panes(workspace, tab, a, b);
                return self.rearranged(slot, before, result);
            }
            Request::MovePane { slot, from, to } => {
                let result =
                    mux.move_pane(core_pane(from), core_destination(to));
                return self.rearranged(slot, before, result);
            }
            Request::ArrangePanes {
                slot,
                workspace,
                tab,
                expected,
                arrangement,
            } => {
                let result =
                    mux.arrange_panes(workspace, tab, &expected, &arrangement);
                return self.rearranged(slot, before, result);
            }
            Request::RestoreLayout { token, direction } => {
                return self.restore(token, direction);
            }
            Request::PrepareClose { request } => {
                return self.prepare_close(request);
            }
            Request::CommitClose { ticket, confirmed } => {
                return self.commit_close(ticket, confirmed);
            }
            Request::ReleaseCloseTicket { ticket } => {
                return match self.tickets.remove(&ticket) {
                    Some(_) => Ok(Success {
                        response: Response::Unit,
                        committed: None,
                    }),
                    None => {
                        Err(WireError::new(ErrorCode::StaleTarget, "no ticket"))
                    }
                };
            }
            other => {
                panic!("the contract harness does not cover {:?}", other.kind())
            }
        };
        self.finish(before, result)
    }

    fn hierarchy(&mut self) -> (u64, u64) {
        (
            self.mux.hierarchy_seq(),
            self.mux.hierarchy_digest(DigestProfile::Base),
        )
    }

    fn layout(&mut self, tab: TabId) -> PaneLayout {
        self.mux.tab(tab).expect("the tab exists").layout().clone()
    }

    fn shutdown(mut self: Box<Self>) {
        self.mux.shutdown().expect("the Mux shuts down");
    }
}

fn opened_tab(workspace: WorkspaceId, opened: &core::OpenedTab) -> Response {
    let pane = opened.tab.first_pane();
    Response::OpenedTab(OpenedTab {
        tab: opened.tab.id,
        workspace,
        pane: pane.id,
        terminal: pane.terminal_id,
    })
}

/// The busy terminals of an assessment, unabridged: the scenarios' quiet
/// shells stay far below the summary's limits.
fn busy(assessment: &core::CloseAssessment) -> Vec<BusyTerminal> {
    assessment
        .terminal_jobs()
        .filter_map(|(terminal, state)| {
            let jobs = match state {
                JobState::Idle => return None,
                JobState::Unknown => TerminalJobs::Unknown,
                JobState::Running(processes) => TerminalJobs::Running(
                    processes
                        .iter()
                        .map(|process| ProcessSummary {
                            pid: process.pid,
                            foreground: process.foreground,
                            command: process.command.clone(),
                            command_line: process.command_line.clone(),
                        })
                        .collect(),
                ),
            };
            Some(BusyTerminal { terminal, jobs })
        })
        .collect()
}

/// The server's layout history semantics over core's `LayoutChange`: each
/// slot keeps one entry each way and the newest sequence it filed at, and
/// each token names one entry in one direction. The scenarios use fewer
/// slots than the server keeps, so nothing is evicted.
#[derive(Default)]
struct History {
    slots: HashMap<u32, Slot>,
    tokens: HashMap<u64, (u32, RestoreDirection)>,
    last_token: u64,
}

#[derive(Default)]
struct Slot {
    undo: Option<(u64, LayoutChange)>,
    redo: Option<(u64, LayoutChange)>,
    newest: u64,
}

impl History {
    /// Replaces `slot`'s entry in `direction`, returning the new entry's
    /// token.
    fn put(
        &mut self,
        slot: u32,
        direction: RestoreDirection,
        change: Option<LayoutChange>,
    ) -> Option<u64> {
        let entry = change.map(|change| {
            self.last_token += 1;
            self.tokens.insert(self.last_token, (slot, direction));
            (self.last_token, change)
        });
        let token = entry.as_ref().map(|(token, _)| *token);
        let filed = self.slots.entry(slot).or_default();
        let old = match direction {
            RestoreDirection::Undo => std::mem::replace(&mut filed.undo, entry),
            RestoreDirection::Redo => std::mem::replace(&mut filed.redo, entry),
        };
        if let Some((old, _)) = old {
            self.tokens.remove(&old);
        }
        token
    }

    /// Files a rearrangement as the undo entry and clears redo, unless it
    /// changed nothing or a newer entry covers it.
    fn file(
        &mut self,
        slot: u32,
        change: LayoutChange,
        seq: u64,
    ) -> Option<u64> {
        let filed = self.slots.entry(slot).or_default();
        if change.is_empty() || seq < filed.newest {
            return None;
        }
        filed.newest = seq;
        let token = self.put(slot, RestoreDirection::Undo, Some(change));
        self.put(slot, RestoreDirection::Redo, None);
        token
    }

    /// Takes the entry `token` names, when it is its slot's current entry
    /// in `direction`.
    fn take(
        &mut self,
        token: u64,
        direction: RestoreDirection,
    ) -> Option<(u32, LayoutChange)> {
        let (slot, filed) = *self.tokens.get(&token)?;
        if filed != direction {
            return None;
        }
        self.tokens.remove(&token);
        let entry = self.slots.get_mut(&slot)?;
        let (_, change) = match direction {
            RestoreDirection::Undo => entry.undo.take(),
            RestoreDirection::Redo => entry.redo.take(),
        }?;
        Some((slot, change))
    }

    /// Files a change a restore applied the other way.
    fn refile(
        &mut self,
        slot: u32,
        direction: RestoreDirection,
        change: LayoutChange,
        seq: u64,
    ) -> u64 {
        let filed = self.slots.entry(slot).or_default();
        filed.newest = filed.newest.max(seq);
        let other = match direction {
            RestoreDirection::Undo => RestoreDirection::Redo,
            RestoreDirection::Redo => RestoreDirection::Undo,
        };
        self.put(slot, other, Some(change)).expect("a filed entry")
    }
}

/// The harness's own conversions between the wire's mirrors and core's
/// types, so a slip in the server's conversions shows up as a difference.
fn core_direction(direction: RestoreDirection) -> core::RestoreDirection {
    match direction {
        RestoreDirection::Undo => core::RestoreDirection::Undo,
        RestoreDirection::Redo => core::RestoreDirection::Redo,
    }
}

fn core_pane(pane: PaneRef) -> core::PaneRef {
    core::PaneRef {
        workspace: pane.workspace,
        tab: pane.tab,
        pane: pane.pane,
    }
}

fn core_destination(destination: MoveDestination) -> core::MoveDestination {
    match destination {
        MoveDestination::Beside(Beside { target, side }) => {
            core::MoveDestination::Beside(core::Beside {
                target: core_pane(target),
                side,
            })
        }
        MoveDestination::NewTab { workspace, before } => {
            core::MoveDestination::NewTab { workspace, before }
        }
    }
}

fn core_close(request: CloseRequest) -> core::CloseRequest {
    match request {
        CloseRequest::Tab { workspace, tab } => {
            core::CloseRequest::Tab { workspace, tab }
        }
        CloseRequest::Pane {
            workspace,
            tab,
            pane,
        } => core::CloseRequest::Pane {
            workspace,
            tab,
            pane,
        },
        CloseRequest::Workspace(workspace) => {
            core::CloseRequest::Workspace(workspace)
        }
        other => panic!("the contract harness does not cover {other:?}"),
    }
}

fn wire_effect(effect: &core::CloseEffect) -> CloseEffect {
    match effect.clone() {
        core::CloseEffect::Tab { workspace, tab } => {
            CloseEffect::Tab { workspace, tab }
        }
        core::CloseEffect::Pane {
            workspace,
            tab,
            pane,
        } => CloseEffect::Pane {
            workspace,
            tab,
            pane,
        },
        core::CloseEffect::Workspace(workspace) => {
            CloseEffect::Workspace(workspace)
        }
        other => panic!("the contract harness does not cover {other:?}"),
    }
}

/// The IPC path: the native client connected to a listener on a bound
/// socket, against a fresh `Mux`.
struct Ipc {
    server: Arc<Server>,
    listening: SocketServer,
    client: Client,
    handle: ClientHandle,
}

impl Ipc {
    fn new() -> Self {
        let server = Arc::new(server(ServerConfig::default()));
        let listening = SocketServer::start(Arc::clone(&server));
        let client = Client::connect(
            ClientConfig::local(ClientKind::Desktop, "contract", "test"),
            listening.socket(),
        )
        .unwrap();
        let handle = client.handle();
        block_on(handle.connected()).expect("the handshake completes");
        block_on(handle.request(Request::SubscribeHierarchy))
            .expect("the client subscribes");
        Self {
            server,
            listening,
            client,
            handle,
        }
    }

    fn snapshot(&self) -> huterm_wire::hierarchy::HierarchySnapshot {
        block_on(self.handle.hierarchy()).expect("a subscribed projection")
    }
}

impl Path for Ipc {
    fn request(&mut self, request: Request) -> Result<Success, WireError> {
        let outcome = block_on(self.handle.request(request));
        let committed = match &outcome {
            Ok(success) => success.committed,
            Err(error) => error.committed,
        };
        if let Some(committed) = committed {
            assert_eq!(
                block_on(self.handle.wait_for(committed.seq)),
                Resolution::Ready,
                "the projection reaches the committed sequence"
            );
        }
        outcome
    }

    fn hierarchy(&mut self) -> (u64, u64) {
        let snapshot = self.snapshot();
        (snapshot.state.seq(), snapshot.digest)
    }

    fn layout(&mut self, tab: TabId) -> PaneLayout {
        self.snapshot()
            .state
            .tab(tab)
            .expect("the projection holds the tab")
            .layout
            .clone()
    }

    fn shutdown(self: Box<Self>) {
        let Self {
            server,
            listening,
            client,
            handle: _,
        } = *self;
        client
            .shutdown()
            .expect("the client's threads exit cleanly");
        let (done, exited) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(listening.shutdown().is_ok());
        });
        assert!(
            exited
                .recv_timeout(PROMPT)
                .expect("the server's threads exit"),
            "a server thread panicked"
        );
        server.lock().shutdown().expect("the Mux shuts down");
    }
}

/// Runs `future` to completion, failing the test after [`PROMPT`].
fn block_on<T>(future: impl Future<Output = T>) -> T {
    futures_lite::future::block_on(futures_lite::future::or(
        async { Some(future.await) },
        async {
            async_io::Timer::after(PROMPT).await;
            None
        },
    ))
    .unwrap_or_else(|| panic!("nothing resolved within {PROMPT:?}"))
}

/// The layout's panes stacked in rows of nearly equal weight.
fn rows(layout: &PaneLayout) -> Arrangement {
    let panes: Vec<_> = layout.panes().iter().map(|pane| pane.id).collect();
    let count = u32::try_from(panes.len()).unwrap();
    let share = SPLIT_TOTAL / count;
    let children = panes
        .iter()
        .enumerate()
        .map(|(index, pane)| {
            let weight = if index + 1 == panes.len() {
                SPLIT_TOTAL - share * (count - 1)
            } else {
                share
            };
            (weight, Arrangement::Pane(*pane))
        })
        .collect();
    Arrangement::Split {
        axis: SplitAxis::Rows,
        children,
    }
}

#[test]
fn creating_sessions_workspaces_and_tabs() {
    let steps = contract(|s| {
        let session = s.session(None);
        let named = s.session(Some("work"));
        let workspace = s.workspace(session, None);
        s.workspace(named, Some("build"));
        let tab = s.tab(workspace);
        s.tab(workspace);
        s.step(Request::CreateWorkspaceWithTab {
            session: named,
            name: Some("dev".into()),
            command: quiet(),
        });
        s.step(Request::SelectTab { tab: tab.tab });
        s.step(Request::CreateSession {
            name: Some("x".repeat(MAX_NAME_BYTES + 1)),
        });
    });
    assert_eq!(
        failures(&steps),
        [(RequestKind::CreateSession, ErrorCode::InvalidName)]
    );
}

#[test]
fn renaming_every_kind() {
    let steps = contract(|s| {
        let (session, workspace, tab) = s.fixture();
        let names = [
            Some("renamed".to_owned()),
            Some(String::new()),
            Some("   ".to_owned()),
            Some("n".repeat(MAX_NAME_BYTES)),
            Some("n".repeat(MAX_NAME_BYTES + 1)),
            None,
        ];
        for name in names {
            s.step(Request::RenameSession {
                session,
                name: name.clone(),
            });
            s.step(Request::RenameWorkspace {
                workspace,
                name: name.clone(),
            });
            s.step(Request::RenameTab {
                tab: tab.tab,
                name: name.clone(),
            });
            s.step(Request::RenamePane {
                pane: tab.pane,
                name,
            });
        }
    });
    // The blank, the whitespace, and the over-long names fail for each of
    // the four kinds.
    let kinds = [
        RequestKind::RenameSession,
        RequestKind::RenameWorkspace,
        RequestKind::RenameTab,
        RequestKind::RenamePane,
    ];
    let expected: Vec<_> = (0..3)
        .flat_map(|_| kinds.map(|kind| (kind, ErrorCode::InvalidName)))
        .collect();
    assert_eq!(failures(&steps), expected);
}

#[test]
fn moving_tabs_workspaces_sessions_and_panes() {
    let steps = contract(|s| {
        let (first, home, tab) = s.fixture();
        let second = s.session(None);
        let away = s.workspace(second, None);
        let spare = s.workspace(second, None);
        s.tab(away);
        s.step(Request::MoveTab {
            source: home,
            tab: tab.tab,
            destination: away,
            before: None,
        });
        // The tab left `home`, so the expected source no longer holds.
        s.step(Request::MoveTab {
            source: home,
            tab: tab.tab,
            destination: home,
            before: None,
        });
        s.step(Request::MoveWorkspace {
            source: second,
            workspace: spare,
            destination: first,
            before: Some(home),
        });
        s.step(Request::MoveSession {
            session: second,
            before: Some(first),
        });
        s.step(Request::MoveWorkspaceToNewSession {
            source: first,
            workspace: spare,
        });
        let right = s.split(away, &tab, tab.pane, SplitDirection::Right);
        s.step(Request::MovePane {
            slot: 1,
            from: PaneRef {
                workspace: away,
                tab: tab.tab,
                pane: right.pane,
            },
            to: MoveDestination::NewTab {
                workspace: home,
                before: None,
            },
        });
        // The pane moved, so its old tab no longer holds it.
        s.step(Request::MovePane {
            slot: 1,
            from: PaneRef {
                workspace: away,
                tab: tab.tab,
                pane: right.pane,
            },
            to: MoveDestination::Beside(Beside {
                target: PaneRef {
                    workspace: away,
                    tab: tab.tab,
                    pane: tab.pane,
                },
                side: SplitDirection::Down,
            }),
        });
    });
    assert_eq!(
        failures(&steps),
        [
            (RequestKind::MoveTab, ErrorCode::StaleTarget),
            (RequestKind::MovePane, ErrorCode::StaleTarget),
        ]
    );
}

#[test]
fn reordering_tabs() {
    let steps = contract(|s| {
        let (session, workspace, first) = s.fixture();
        let second = s.tab(workspace);
        let third = s.tab(workspace);
        s.step(Request::ReorderTab {
            workspace,
            tab: third.tab,
            before: Some(first.tab),
        });
        s.step(Request::ReorderTab {
            workspace,
            tab: first.tab,
            before: None,
        });
        s.step(Request::ReorderTab {
            workspace,
            tab: second.tab,
            before: Some(second.tab),
        });
        let elsewhere = s.workspace(session, None);
        s.step(Request::ReorderTab {
            workspace: elsewhere,
            tab: first.tab,
            before: None,
        });
    });
    assert_eq!(
        failures(&steps),
        [(RequestKind::ReorderTab, ErrorCode::StaleTarget)]
    );
}

#[test]
fn splitting_panes() {
    let steps = contract(|s| {
        let (_, workspace, tab) = s.fixture();
        let right = s.split(workspace, &tab, tab.pane, SplitDirection::Right);
        s.split(workspace, &tab, right.pane, SplitDirection::Down);
        s.split(workspace, &tab, tab.pane, SplitDirection::Up);
        let other = s.tab(workspace);
        // A pane of another tab.
        s.step(Request::SplitPane {
            workspace,
            tab: other.tab,
            pane: right.pane,
            direction: SplitDirection::Left,
            command: quiet(),
        });
        s.step(Request::ClosePane {
            workspace,
            tab: tab.tab,
            pane: right.pane,
        });
    });
    assert_eq!(
        failures(&steps),
        [(RequestKind::SplitPane, ErrorCode::StaleTarget)]
    );
}

#[test]
fn arranging_panes() {
    let steps = contract(|s| {
        let (_, workspace, tab) = s.fixture();
        let right = s.split(workspace, &tab, tab.pane, SplitDirection::Right);
        s.split(workspace, &tab, right.pane, SplitDirection::Down);
        let nested = s.layout(tab.tab);
        let arrangement = rows(&nested);
        s.step(Request::ArrangePanes {
            slot: 1,
            workspace,
            tab: tab.tab,
            expected: nested.clone(),
            arrangement: arrangement.clone(),
        });
        // The same arrangement again changes nothing and files no entry.
        let flat = s.layout(tab.tab);
        s.step(Request::ArrangePanes {
            slot: 1,
            workspace,
            tab: tab.tab,
            expected: flat.clone(),
            arrangement,
        });
        // The tab no longer has the structure the old layout recorded.
        s.step(Request::ArrangePanes {
            slot: 1,
            workspace,
            tab: tab.tab,
            expected: nested,
            arrangement: flat.balanced(),
        });
        // An arrangement that drops a pane.
        s.step(Request::ArrangePanes {
            slot: 1,
            workspace,
            tab: tab.tab,
            expected: flat,
            arrangement: Arrangement::Pane(tab.pane),
        });
    });
    assert_eq!(
        failures(&steps),
        [
            (RequestKind::ArrangePanes, ErrorCode::InvalidArgument),
            (RequestKind::ArrangePanes, ErrorCode::InvalidArgument),
        ]
    );
}

#[test]
fn undo_and_redo_through_tokens() {
    let steps = contract(|s| {
        let (_, workspace, tab) = s.fixture();
        let right = s.split(workspace, &tab, tab.pane, SplitDirection::Right);
        let swapped = s.token(Request::SwapPanes {
            slot: 1,
            workspace,
            tab: tab.tab,
            a: tab.pane,
            b: right.pane,
        });
        let undo = |token| Request::RestoreLayout {
            token,
            direction: RestoreDirection::Undo,
        };
        let redo = |token| Request::RestoreLayout {
            token,
            direction: RestoreDirection::Redo,
        };
        let undone = s.token(undo(swapped));
        // A restore consumes its token, and a token names one direction.
        s.step(undo(swapped));
        s.step(undo(undone));
        let redone = s.token(redo(undone));
        // A newer rearrangement in the slot replaces its entries.
        let moved = s.token(Request::MovePane {
            slot: 1,
            from: PaneRef {
                workspace,
                tab: tab.tab,
                pane: right.pane,
            },
            to: MoveDestination::NewTab {
                workspace,
                before: None,
            },
        });
        s.step(undo(redone));
        s.token(undo(moved));
        // A restore that no longer applies drops its entry.
        let stale = s.token(Request::SwapPanes {
            slot: 2,
            workspace,
            tab: tab.tab,
            a: tab.pane,
            b: right.pane,
        });
        s.ok(Request::ClosePane {
            workspace,
            tab: tab.tab,
            pane: right.pane,
        });
        s.step(undo(stale));
        s.step(undo(stale));
    });
    assert_eq!(
        failures(&steps),
        [
            (RequestKind::RestoreLayout, ErrorCode::StaleUndo),
            (RequestKind::RestoreLayout, ErrorCode::StaleUndo),
            (RequestKind::RestoreLayout, ErrorCode::StaleUndo),
            (RequestKind::RestoreLayout, ErrorCode::StaleUndo),
            (RequestKind::RestoreLayout, ErrorCode::StaleUndo),
        ]
    );
}

#[test]
fn closing_with_tickets() {
    let steps = contract(|s| {
        let (_, workspace, first) = s.fixture();
        let second = s.tab(workspace);
        let ticket = s.ticket(CloseRequest::Tab {
            workspace,
            tab: first.tab,
        });
        s.step(Request::CommitClose {
            ticket,
            confirmed: true,
        });
        // A commit consumes its ticket.
        s.step(Request::CommitClose {
            ticket,
            confirmed: true,
        });
        let stale = s.ticket(CloseRequest::Tab {
            workspace,
            tab: second.tab,
        });
        s.split(workspace, &second, second.pane, SplitDirection::Right);
        s.step(Request::CommitClose {
            ticket: stale,
            confirmed: true,
        });
        let released = s.ticket(CloseRequest::Pane {
            workspace,
            tab: second.tab,
            pane: second.pane,
        });
        s.step(Request::ReleaseCloseTicket { ticket: released });
        s.step(Request::CommitClose {
            ticket: released,
            confirmed: true,
        });
        s.step(Request::PrepareClose {
            request: CloseRequest::Tab {
                workspace,
                tab: first.tab,
            },
        });
    });
    assert_eq!(
        failures(&steps),
        [
            (RequestKind::CommitClose, ErrorCode::StaleTarget),
            (RequestKind::CommitClose, ErrorCode::StaleClose),
            (RequestKind::CommitClose, ErrorCode::StaleTarget),
            (RequestKind::PrepareClose, ErrorCode::StaleTarget),
        ]
    );
}

#[test]
fn stale_targets() {
    let steps = contract(|s| {
        let (session, workspace, closed) = s.fixture();
        let open = s.tab(workspace);
        s.ok(Request::CloseTab {
            workspace,
            tab: closed.tab,
        });
        s.step(Request::RenameTab {
            tab: closed.tab,
            name: Some("gone".into()),
        });
        s.step(Request::SelectTab { tab: closed.tab });
        s.step(Request::SplitPane {
            workspace,
            tab: closed.tab,
            pane: closed.pane,
            direction: SplitDirection::Right,
            command: quiet(),
        });
        s.step(Request::CloseTab {
            workspace,
            tab: closed.tab,
        });
        // The same numbers in a runtime neither path knows.
        let foreign = RuntimeId::allocate().unwrap();
        s.step(Request::RenameSession {
            session: SessionId::in_runtime(foreign, session.get()),
            name: Some("foreign".into()),
        });
        s.step(Request::RenameTab {
            tab: TabId::in_runtime(foreign, open.tab.get()),
            name: Some("foreign".into()),
        });
        s.step(Request::OpenTab {
            workspace: WorkspaceId::in_runtime(foreign, workspace.get()),
            command: quiet(),
        });
        s.step(Request::MoveTab {
            source: workspace,
            tab: open.tab,
            destination: WorkspaceId::in_runtime(foreign, workspace.get()),
            before: None,
        });
    });
    assert_eq!(
        failures(&steps),
        [
            (RequestKind::RenameTab, ErrorCode::StaleTarget),
            (RequestKind::SelectTab, ErrorCode::StaleTarget),
            (RequestKind::SplitPane, ErrorCode::StaleTarget),
            (RequestKind::CloseTab, ErrorCode::StaleTarget),
            (RequestKind::RenameSession, ErrorCode::StaleTarget),
            (RequestKind::RenameTab, ErrorCode::StaleTarget),
            (RequestKind::OpenTab, ErrorCode::StaleTarget),
            (RequestKind::MoveTab, ErrorCode::StaleTarget),
        ]
    );
}
