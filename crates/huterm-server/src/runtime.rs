//! Running a connection's [`Work`](crate::Work) against the shared `Mux`.
//!
//! [`execute`] runs one job on the calling thread, which must be a worker:
//! spawns, process-table scans, and terminal teardown block. Each
//! structural operation reads the hierarchy sequence, and the digest when
//! the job asks for one, under the same lock as the change, so the
//! `Committed` it reports is exact. A failure that committed before failing,
//! such as a close whose cleanup failed after it removed the tab, reports
//! where it committed; core advances the sequence exactly when it commits.
//! No job holds the lock while it waits on anything but core.
//!
//! [`error_code`] is the one mapping from core's errors onto wire codes.

use huterm_core::{
    CloseAssessment, JobState, Mux, MuxError, RuntimeError, SelectionTarget,
};
use huterm_protocol::{CommandError, DigestProfile};
use huterm_wire::command::DecodedInvocation;
use huterm_wire::error::{ErrorCode, WireError};
use huterm_wire::request::Request;
use huterm_wire::response::{
    BusyTerminal, CloseAssessment as Summary, Committed,
    MAX_CLOSE_SUMMARY_PROCESSES, MAX_CLOSE_SUMMARY_TERMINALS,
    MAX_PROCESS_COMMAND_BYTES, MAX_PROCESS_COMMAND_LINE_BYTES, OpenedPane,
    OpenedTab, ProcessSummary, Response, Selection, Success, TerminalJobs,
};

use crate::convert;
use crate::feed::Subscribed;
use crate::server::Server;
use crate::work::{
    Completion, Job, Operation, ReplaceJob, Replaced, RequestJob,
};

/// Runs `job` against `server`'s runtime and returns its completion.
#[must_use]
pub fn execute(server: &Server, job: Job) -> Completion {
    match job {
        Job::Request(RequestJob { operation, digest }) => {
            operation_completion(server, operation, digest)
        }
        Job::Replace(job) => replace(server, job),
        Job::Detach(attachments) => {
            let mut mux = server.lock();
            for attachment in attachments {
                // Another connection may have ended it already.
                let _ = mux.detach_session(attachment);
            }
            Completion::Cleaned
        }
    }
}

/// The wire code for a core error. Every target that no longer resolves,
/// or that belongs to another runtime, is `StaleTarget`, as a client
/// refuses an identity of another scope locally and as core's command
/// executor maps it, so in-process and IPC callers see one code.
#[must_use]
pub fn error_code(error: &MuxError) -> ErrorCode {
    match error {
        MuxError::UnknownAttachment(_)
        | MuxError::TerminalNotInAttachment { .. }
        | MuxError::ForeignRuntime(_)
        | MuxError::UnknownSession(_)
        | MuxError::UnknownWorkspace(_)
        | MuxError::UnknownTab(_)
        | MuxError::UnknownPane(_)
        | MuxError::UnknownSplit(_) => ErrorCode::StaleTarget,
        MuxError::StaleClose => ErrorCode::StaleClose,
        MuxError::ConfirmationRequired => ErrorCode::ConfirmationRequired,
        MuxError::ViewNotEmpty(_)
        | MuxError::PinnedWorkspace(_)
        | MuxError::LastWorkspace(_)
        | MuxError::PaneTooSmall(_) => ErrorCode::Unavailable,
        MuxError::EmptyClose | MuxError::InvalidLayout(_) => {
            ErrorCode::InvalidArgument
        }
        // Only a layout restore reports these: the entry no longer
        // applies, and the server dropped it.
        MuxError::LayoutChanged | MuxError::InvalidRestore => {
            ErrorCode::StaleUndo
        }
        MuxError::IdExhausted | MuxError::LimitExceeded => {
            ErrorCode::LimitExceeded
        }
        MuxError::InvalidName => ErrorCode::InvalidName,
        MuxError::Runtime(error) => runtime_code(error),
        MuxError::RestoreNotFresh
        | MuxError::RestoreForeignSocket
        | MuxError::InvalidCapture(_)
        | MuxError::RestoreMismatch
        | MuxError::RestoreCancelled => ErrorCode::Internal,
    }
}

/// The wire code for a terminal runtime error.
#[must_use]
pub fn runtime_code(error: &RuntimeError) -> ErrorCode {
    match error {
        RuntimeError::Stopped => ErrorCode::Stopped,
        RuntimeError::Busy => ErrorCode::Busy,
        RuntimeError::NotPermitted => ErrorCode::NotPermitted,
        RuntimeError::Revoked => ErrorCode::Revoked,
        RuntimeError::ViewerLimit => ErrorCode::LimitExceeded,
        RuntimeError::Engine(_)
        | RuntimeError::Pty(_)
        | RuntimeError::Spawn(_)
        | RuntimeError::Thread(_)
        | RuntimeError::ShutdownTimedOut
        | RuntimeError::ThreadPanic
        | RuntimeError::Invariant(_)
        | RuntimeError::TimedOut => ErrorCode::Internal,
    }
}

/// A core error as it crosses the wire, committing nothing.
#[must_use]
pub fn wire_error(error: &MuxError) -> WireError {
    WireError::new(error_code(error), error.to_string())
}

/// Where the runtime stands under the caller's lock: its sequence and, for
/// a connection that holds a hierarchy subscription, its digest.
fn committed(mux: &Mux, digest: Option<DigestProfile>) -> Committed {
    Committed {
        seq: mux.hierarchy_seq(),
        digest: digest.map(|profile| mux.hierarchy_digest(profile)),
    }
}

/// A structural outcome with where it committed. A failure carries its
/// sequence only when the operation advanced it before failing.
fn finish<E>(
    mux: &Mux,
    before: u64,
    digest: Option<DigestProfile>,
    result: Result<Response, E>,
    error: impl FnOnce(E) -> WireError,
) -> Result<Success, WireError> {
    match result {
        Ok(response) => Ok(Success {
            response,
            committed: Some(committed(mux, digest)),
        }),
        Err(failure) => {
            let mut wire = error(failure);
            if mux.hierarchy_seq() != before {
                wire.committed = Some(committed(mux, digest));
            }
            Err(wire)
        }
    }
}

fn operation_completion(
    server: &Server,
    operation: Operation,
    digest: Option<DigestProfile>,
) -> Completion {
    match operation {
        Operation::Plain(request) => plain(server, request, digest),
        Operation::PrepareClose { ticket, request } => {
            let (prepared, committed) = {
                let mux = server.lock();
                (mux.prepare_close(request), committed(&mux, digest))
            };
            // Process tables are read off the lock.
            let prepared = prepared.map(|ticket| ticket.check_jobs());
            Completion::Prepared(match prepared {
                Ok(assessment) => Ok((
                    Success {
                        response: Response::CloseAssessment(summary(
                            ticket,
                            &assessment,
                        )),
                        committed: Some(committed),
                    },
                    assessment,
                )),
                Err(error) => Err(wire_error(&error)),
            })
        }
        Operation::CommitClose {
            consent,
            confirmed,
            attachments,
        } => {
            let current = consent.assessment().recheck();
            let mut mux = server.lock();
            let before = mux.hierarchy_seq();
            let result =
                mux.commit_close(consent.assessment(), &current, confirmed);
            let confirm = matches!(result, Err(MuxError::ConfirmationRequired));
            let ended = ended(&mux, attachments);
            let outcome = finish(
                &mux,
                before,
                digest,
                result.map(|()| Response::Unit),
                |error| wire_error(&error),
            );
            drop(mux);
            Completion::Committed {
                outcome,
                consent: confirm.then_some(consent),
                ended,
            }
        }
        Operation::CloseSession {
            session,
            attachments,
        } => {
            let mut mux = server.lock();
            let before = mux.hierarchy_seq();
            let result = mux.close_session(session);
            let ended = ended(&mux, attachments);
            let outcome = finish(
                &mux,
                before,
                digest,
                result.map(|()| Response::Unit),
                |error| wire_error(&error),
            );
            Completion::Ended { outcome, ended }
        }
        Operation::AttachSession {
            session,
            attachments,
        } => {
            let mut mux = server.lock();
            let before = mux.hierarchy_seq();
            let result = mux.attach_session(session);
            let ended = ended(&mux, attachments);
            let outcome = attached(&mux, before, digest, result);
            Completion::Attached { outcome, ended }
        }
        Operation::Restore { record, direction } => {
            let mut mux = server.lock();
            let before = mux.hierarchy_seq();
            let result = mux.restore_tabs(record.change(), direction);
            let outcome = finish(
                &mux,
                before,
                digest,
                result.map(|_| Response::Undo { token: None }),
                |error| wire_error(&error),
            );
            drop(mux);
            Completion::Restored { outcome, record }
        }
    }
}

/// An attachment's outcome with where it committed.
fn attached(
    mux: &Mux,
    before: u64,
    digest: Option<DigestProfile>,
    result: Result<huterm_protocol::AttachmentId, MuxError>,
) -> Result<(Success, huterm_protocol::AttachmentId), WireError> {
    let attachment = result.map_err(|error| wire_error(&error))?;
    finish(
        mux,
        before,
        digest,
        Ok::<_, MuxError>(Response::Attachment(attachment)),
        |error| wire_error(&error),
    )
    .map(|success| (success, attachment))
}

/// The attachments of `attachments` that no longer exist.
fn ended(
    mux: &Mux,
    attachments: Vec<huterm_protocol::AttachmentId>,
) -> Vec<huterm_protocol::AttachmentId> {
    attachments
        .into_iter()
        .filter(|attachment| mux.attachment_session(*attachment).is_err())
        .collect()
}

fn selection(target: SelectionTarget) -> Response {
    let SelectionTarget {
        session,
        workspace,
        tab,
        pane,
    } = target;
    Response::Selection(Selection {
        session,
        workspace,
        tab,
        pane,
    })
}

fn opened_tab(
    workspace: huterm_protocol::WorkspaceId,
    opened: &huterm_core::OpenedTab,
) -> Response {
    let pane = opened.tab.first_pane();
    Response::OpenedTab(OpenedTab {
        tab: opened.tab.id,
        workspace,
        pane: pane.id,
        terminal: pane.terminal_id,
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per request mirrors the request inventory"
)]
fn plain(
    server: &Server,
    request: Request,
    digest: Option<DigestProfile>,
) -> Completion {
    let mut mux = server.lock();
    let mux = &mut *mux;
    let before = mux.hierarchy_seq();
    let unit = |result: Result<(), MuxError>| result.map(|()| Response::Unit);
    let result = match request {
        Request::SelectSession { session } => {
            mux.select_session(session).map(selection)
        }
        Request::SelectWorkspace { workspace } => {
            mux.select_workspace(workspace).map(selection)
        }
        Request::SelectTab { tab } => mux.select_tab(tab).map(selection),
        Request::SelectPane { pane } => mux.select_pane(pane).map(selection),
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
        Request::CloseWorkspace { workspace } => {
            unit(mux.close_workspace(workspace))
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
        Request::SetWorkspacePinned { workspace, pinned } => {
            unit(mux.set_workspace_pinned(workspace, pinned))
        }
        Request::ToggleInputSync { workspace, tab } => {
            // Read and toggled under one lock, as the desktop does, so two
            // quick toggles cancel out.
            let toggled = mux
                .tab(tab)
                .map(|tab| !tab.input_sync())
                .ok_or(MuxError::UnknownTab(tab));
            unit(toggled.and_then(|input_sync| {
                mux.set_input_sync(workspace, tab, input_sync)
            }))
        }
        Request::CloseEmptiedWorkspace { session, workspace } => mux
            .close_emptied_workspace(session, workspace)
            .map(|closed| Response::EmptiedWorkspace { closed }),
        Request::ResizeSplit {
            workspace,
            tab,
            split,
            children,
            weights,
        } => unit(mux.resize_split(workspace, tab, split, &children, &weights)),
        Request::SetPaneZoom {
            workspace,
            tab,
            zoomed,
        } => unit(mux.set_pane_zoom(workspace, tab, zoomed)),
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
        } => unit(mux.move_workspace(source, workspace, destination, before)),
        Request::MoveWorkspaceToNewSession { source, workspace } => mux
            .move_workspace_to_new_session(source, workspace)
            .map(Response::Session),
        Request::SwapPanes {
            workspace,
            tab,
            a,
            b,
            ..
        } => {
            let result = mux.swap_panes(workspace, tab, a, b);
            return rearranged(mux, before, digest, result);
        }
        Request::MovePane { from, to, .. } => {
            let result = mux
                .move_pane(convert::pane_ref(from), convert::destination(to));
            return rearranged(mux, before, digest, result);
        }
        Request::MoveTabToPane {
            workspace, tab, to, ..
        } => {
            let result =
                mux.move_tab_to_pane(workspace, tab, convert::beside(to));
            return rearranged(mux, before, digest, result);
        }
        Request::ArrangePanes {
            workspace,
            tab,
            expected,
            arrangement,
            ..
        } => {
            let result =
                mux.arrange_panes(workspace, tab, &expected, &arrangement);
            return rearranged(mux, before, digest, result);
        }
        Request::AttachSession { session } => {
            let result = mux.attach_session(session);
            return Completion::Attached {
                outcome: attached(mux, before, digest, result),
                ended: Vec::new(),
            };
        }
        Request::DetachSession { attachment } => {
            unit(mux.detach_session(attachment))
        }
        Request::RetargetAttachment {
            attachment,
            session,
        } => unit(mux.retarget_attachment(attachment, session)),
        Request::ExecuteCommand {
            invocation: DecodedInvocation::Known(invocation),
        } => {
            let result = huterm_core::execute(mux, &invocation);
            return Completion::Answered(finish(
                mux,
                before,
                digest,
                result.map(Response::Command),
                |error: CommandError| WireError::from(&error),
            ));
        }
        // The connection answers everything else without a job.
        _ => {
            return Completion::Answered(Err(WireError::new(
                ErrorCode::Unsupported,
                "this server does not serve that request",
            )));
        }
    };
    Completion::Answered(finish(mux, before, digest, result, |error| {
        wire_error(&error)
    }))
}

fn rearranged(
    mux: &Mux,
    before: u64,
    digest: Option<DigestProfile>,
    result: Result<huterm_protocol::LayoutChange, MuxError>,
) -> Completion {
    let (response, change) = match result {
        Ok(change) => (Ok(Response::Undo { token: None }), Some(change)),
        Err(error) => (Err(error), None),
    };
    Completion::Rearranged(
        finish(mux, before, digest, response, |error| wire_error(&error))
            .map(|success| (success, change.unwrap_or_default())),
    )
}

fn replace(server: &Server, job: ReplaceJob) -> Completion {
    let ReplaceJob { encoding, kind } = job;
    let Subscribed {
        seq,
        digest,
        snapshot,
        feed,
    } = {
        let mut mux = server.lock();
        server.hierarchy().subscribe(&mut mux, encoding)
    };
    // The snapshot encodes off the lock.
    Completion::Replaced(
        snapshot
            .encode(kind, encoding)
            .map(|message| Replaced {
                seq,
                digest,
                message,
                feed,
            })
            .map_err(|error| {
                WireError::new(ErrorCode::Internal, error.to_string())
            }),
    )
}

/// The bounded job summary of a close assessment under `ticket`: at most
/// [`MAX_CLOSE_SUMMARY_TERMINALS`] busy terminals with at most
/// [`MAX_CLOSE_SUMMARY_PROCESSES`] processes among them, each command cut
/// to its limit, with counts of what it left out.
pub(crate) fn summary(ticket: u32, assessment: &CloseAssessment) -> Summary {
    let mut busy = Vec::new();
    let mut omitted_terminals = 0_u32;
    let mut omitted_processes = 0_u32;
    let mut listed = 0;
    for (terminal, state) in assessment.terminal_jobs() {
        let processes = match state {
            JobState::Idle => continue,
            JobState::Unknown => None,
            JobState::Running(processes) => Some(processes),
        };
        if busy.len() == MAX_CLOSE_SUMMARY_TERMINALS {
            omitted_terminals = omitted_terminals.saturating_add(1);
            omitted_processes = omitted_processes
                .saturating_add(count(processes.map_or(0, Vec::len)));
            continue;
        }
        let jobs = match processes {
            None => TerminalJobs::Unknown,
            Some(processes) => {
                let room = MAX_CLOSE_SUMMARY_PROCESSES - listed;
                let shown: Vec<ProcessSummary> = processes
                    .iter()
                    .take(room)
                    .map(|process| ProcessSummary {
                        pid: process.pid,
                        foreground: process.foreground,
                        command: cut(
                            &process.command,
                            MAX_PROCESS_COMMAND_BYTES,
                        ),
                        command_line: process.command_line.as_deref().map(
                            |line| cut(line, MAX_PROCESS_COMMAND_LINE_BYTES),
                        ),
                    })
                    .collect();
                listed += shown.len();
                omitted_processes = omitted_processes
                    .saturating_add(count(processes.len() - shown.len()));
                TerminalJobs::Running(shown)
            }
        };
        busy.push(BusyTerminal { terminal, jobs });
    }
    Summary {
        ticket,
        effect: convert::close_effect(assessment.effect()),
        needs_confirmation: assessment.needs_confirmation(),
        busy,
        omitted_terminals,
        omitted_processes,
    }
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// `text` cut to at most `limit` bytes at a character boundary.
fn cut(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_keeps_whole_characters_at_the_limit() {
        assert_eq!(cut("abc", 3), "abc");
        assert_eq!(cut("abcd", 3), "abc");
        // "é" takes two bytes, so a cut through it keeps the byte before.
        assert_eq!(cut("aé", 2), "a");
        assert_eq!(cut("aé", 3), "aé");
    }

    #[test]
    fn core_errors_map_onto_one_wire_code_each() {
        let tab = huterm_protocol::TabId::in_runtime(
            huterm_protocol::RuntimeId::allocate().unwrap(),
            1,
        );
        for (error, code) in [
            (MuxError::UnknownTab(tab), ErrorCode::StaleTarget),
            (
                MuxError::ForeignRuntime(tab.runtime()),
                ErrorCode::StaleTarget,
            ),
            (MuxError::StaleClose, ErrorCode::StaleClose),
            (
                MuxError::ConfirmationRequired,
                ErrorCode::ConfirmationRequired,
            ),
            (MuxError::InvalidName, ErrorCode::InvalidName),
            (MuxError::LimitExceeded, ErrorCode::LimitExceeded),
            (MuxError::LayoutChanged, ErrorCode::StaleUndo),
            (MuxError::EmptyClose, ErrorCode::InvalidArgument),
            (MuxError::Runtime(RuntimeError::Stopped), ErrorCode::Stopped),
            (
                MuxError::Runtime(RuntimeError::Spawn("no".into())),
                ErrorCode::Internal,
            ),
        ] {
            let wire = wire_error(&error);
            assert_eq!(wire.code, code, "{error}");
            assert_eq!(wire.committed, None);
        }
    }
}
