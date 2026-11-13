//! Results: the success half, [`Response`], and the sequence a structural
//! request committed at.
//!
//! Every minor version 0 result fits one frame at the smallest `max_frame`,
//! including a close assessment, whose job summary is bounded by
//! [`MAX_CLOSE_SUMMARY_TERMINALS`] and [`MAX_CLOSE_SUMMARY_PROCESSES`] with
//! counts of what it left out.

use huterm_protocol::{
    AttachmentId, CommandOutcome, PaneId, SessionId, TabId, TerminalId,
    WorkspaceId,
};

use crate::codec::{DecodeError, EncodeError, IdScope, Reader, Writer};
use crate::command::{decode_outcome, encode_outcome};
use crate::error::{WireError, decode_error, encode_error};
use crate::request::{
    CloseEffect, RequestId, decode_close_effect, encode_close_effect,
    read_request_id, write_request_id,
};

/// The most busy terminals a close assessment lists.
pub const MAX_CLOSE_SUMMARY_TERMINALS: usize = 32;

/// The most processes a close assessment lists, across its terminals.
pub const MAX_CLOSE_SUMMARY_PROCESSES: usize = 32;

/// The longest display command of a listed process.
pub const MAX_PROCESS_COMMAND_BYTES: usize = 256;

/// The longest command line of a listed process: core keeps 200
/// characters, which take at most 800 bytes.
pub const MAX_PROCESS_COMMAND_LINE_BYTES: usize = 1024;

/// The hierarchy sequence a structural request committed at, with the
/// digest of the state there when the connection holds a hierarchy
/// subscription.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Committed {
    /// The sequence, read under the same lock as the change.
    pub seq: u64,
    /// The digest at `seq` under the connection's negotiated profile.
    pub digest: Option<u64>,
}

/// Encodes an optional [`Committed`].
///
/// # Errors
/// Never fails today; the signature matches the other encoders.
pub fn encode_committed(
    w: &mut Writer,
    committed: Option<Committed>,
) -> Result<(), EncodeError> {
    w.option(committed, |w, committed| {
        let Committed { seq, digest } = committed;
        w.record(|w| {
            w.u64(seq);
            w.option(digest, |w, digest| {
                w.fixed(&digest.to_le_bytes());
                Ok(())
            })
        })
    })
}

/// Decodes an optional [`Committed`].
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_committed(
    r: &mut Reader<'_>,
) -> Result<Option<Committed>, DecodeError> {
    r.option("committed", |r| {
        r.record(|r| {
            Ok(Committed {
                seq: r.u64()?,
                digest: r.option("committed digest", |r| {
                    Ok(u64::from_le_bytes(r.fixed()?))
                })?,
            })
        })
    })
}

/// A tab a request opened, with its workspace, first pane, and terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenedTab {
    /// The tab.
    pub tab: TabId,
    /// Its workspace.
    pub workspace: WorkspaceId,
    /// Its only pane.
    pub pane: PaneId,
    /// The pane's terminal.
    pub terminal: TerminalId,
}

/// A pane a split opened, with its tab and terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenedPane {
    /// The tab.
    pub tab: TabId,
    /// The pane.
    pub pane: PaneId,
    /// The pane's terminal.
    pub terminal: TerminalId,
}

/// A validated selection with its ancestry, as core's `SelectionTarget`
/// names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selection {
    /// Owning session.
    pub session: SessionId,
    /// Selected workspace, if any.
    pub workspace: Option<WorkspaceId>,
    /// Selected tab, if any.
    pub tab: Option<TabId>,
    /// Selected pane, if any.
    pub pane: Option<PaneId>,
}

/// One process a close would end.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSummary {
    /// Operating system process ID.
    pub pid: u32,
    /// Whether it holds the terminal's foreground.
    pub foreground: bool,
    /// The display command, at most [`MAX_PROCESS_COMMAND_BYTES`].
    pub command: String,
    /// The command line when the system shares it, at most
    /// [`MAX_PROCESS_COMMAND_LINE_BYTES`].
    pub command_line: Option<String>,
}

/// What a busy terminal is running.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalJobs {
    /// The process table could not be read.
    Unknown,
    /// Its processes, as many as the assessment's process limit left
    /// room for.
    Running(Vec<ProcessSummary>),
}

/// A terminal whose close needs consent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BusyTerminal {
    /// The terminal.
    pub terminal: TerminalId,
    /// What it runs.
    pub jobs: TerminalJobs,
}

/// A prepared close: the ticket the connection holds for it, its effect,
/// and a bounded summary of the jobs that need consent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloseAssessment {
    /// The ticket a commit or release names.
    pub ticket: u32,
    /// The effect the close resolved to.
    pub effect: CloseEffect,
    /// Whether committing needs the user's confirmation.
    pub needs_confirmation: bool,
    /// Busy terminals, at most [`MAX_CLOSE_SUMMARY_TERMINALS`] with at most
    /// [`MAX_CLOSE_SUMMARY_PROCESSES`] processes among them.
    pub busy: Vec<BusyTerminal>,
    /// Busy terminals left out.
    pub omitted_terminals: u32,
    /// Processes left out, the omitted terminals' included.
    pub omitted_processes: u32,
}

/// The success half of a result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Response {
    /// Done; nothing to report.
    Unit,
    /// A created session.
    Session(SessionId),
    /// A created workspace.
    Workspace(WorkspaceId),
    /// A created attachment.
    Attachment(AttachmentId),
    /// An opened tab.
    OpenedTab(OpenedTab),
    /// An opened pane.
    OpenedPane(OpenedPane),
    /// A selection's ancestry.
    Selection(Selection),
    /// A prepared close.
    CloseAssessment(CloseAssessment),
    /// A rearrangement's undo token, or `None` when it filed nothing.
    Undo {
        /// The token.
        token: Option<u64>,
    },
    /// A catalog command's outcome.
    Command(CommandOutcome),
    /// A hierarchy subscription or resync: the snapshot's sequence and
    /// digest, which the hierarchy lane's snapshot carries too.
    Hierarchy {
        /// The snapshot's sequence.
        seq: u64,
        /// Its digest under the negotiated profile.
        digest: u64,
    },
    /// Whether an emptied workspace closed.
    EmptiedWorkspace {
        /// Whether it closed.
        closed: bool,
    },
}

/// A successful result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Success {
    /// What the request produced.
    pub response: Response,
    /// Where a structural request committed.
    pub committed: Option<Committed>,
}

/// The result of one request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestResult {
    /// The request it answers.
    pub request: RequestId,
    /// Its success or failure.
    pub outcome: Result<Success, WireError>,
}

fn encode_process(
    w: &mut Writer,
    process: &ProcessSummary,
) -> Result<(), EncodeError> {
    let ProcessSummary {
        pid,
        foreground,
        command,
        command_line,
    } = process;
    w.record(|w| {
        w.u32(*pid);
        w.bool(*foreground);
        w.str(command, MAX_PROCESS_COMMAND_BYTES, "process command")?;
        w.optional_str(
            command_line.as_deref(),
            MAX_PROCESS_COMMAND_LINE_BYTES,
            "process command line",
        )
    })
}

fn decode_process(r: &mut Reader<'_>) -> Result<ProcessSummary, DecodeError> {
    r.record(|r| {
        Ok(ProcessSummary {
            pid: r.u32()?,
            foreground: r.bool()?,
            command: r.string(MAX_PROCESS_COMMAND_BYTES, "process command")?,
            command_line: r.optional_string(
                MAX_PROCESS_COMMAND_LINE_BYTES,
                "process command line",
            )?,
        })
    })
}

fn encode_assessment(
    w: &mut Writer,
    scope: IdScope,
    assessment: &CloseAssessment,
) -> Result<(), EncodeError> {
    let CloseAssessment {
        ticket,
        effect,
        needs_confirmation,
        busy,
        omitted_terminals,
        omitted_processes,
    } = assessment;
    let processes: usize = busy
        .iter()
        .map(|terminal| match &terminal.jobs {
            TerminalJobs::Unknown => 0,
            TerminalJobs::Running(processes) => processes.len(),
        })
        .sum();
    if processes > MAX_CLOSE_SUMMARY_PROCESSES {
        return Err(EncodeError::LimitExceeded {
            field: "close processes",
        });
    }
    w.u32(*ticket);
    encode_close_effect(w, scope, effect)?;
    w.bool(*needs_confirmation);
    w.seq(
        busy.iter(),
        MAX_CLOSE_SUMMARY_TERMINALS,
        "busy terminals",
        |w, busy| {
            let BusyTerminal { terminal, jobs } = busy;
            w.record(|w| {
                w.id(scope, *terminal)?;
                match jobs {
                    TerminalJobs::Unknown => {
                        w.tag(0);
                        w.record(|_| Ok(()))
                    }
                    TerminalJobs::Running(processes) => {
                        w.tag(1);
                        w.record(|w| {
                            w.seq(
                                processes.iter(),
                                MAX_CLOSE_SUMMARY_PROCESSES,
                                "close processes",
                                encode_process,
                            )
                        })
                    }
                }
            })
        },
    )?;
    w.u32(*omitted_terminals);
    w.u32(*omitted_processes);
    Ok(())
}

fn decode_assessment(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<CloseAssessment, DecodeError> {
    let ticket = r.u32()?;
    let effect = decode_close_effect(r, scope)?;
    let needs_confirmation = r.bool()?;
    let mut processes = 0;
    let busy = r.seq(MAX_CLOSE_SUMMARY_TERMINALS, "busy terminals", |r| {
        r.record(|r| {
            let terminal = r.id(scope)?;
            let tag = r.tag()?;
            let jobs = r.record(|r| match tag {
                0 => Ok(TerminalJobs::Unknown),
                1 => {
                    let listed = r.seq(
                        MAX_CLOSE_SUMMARY_PROCESSES - processes,
                        "close processes",
                        decode_process,
                    )?;
                    processes += listed.len();
                    Ok(TerminalJobs::Running(listed))
                }
                tag => Err(DecodeError::UnknownTag {
                    field: "terminal jobs",
                    tag,
                }),
            })?;
            Ok(BusyTerminal { terminal, jobs })
        })
    })?;
    Ok(CloseAssessment {
        ticket,
        effect,
        needs_confirmation,
        busy,
        omitted_terminals: r.u32()?,
        omitted_processes: r.u32()?,
    })
}

/// Encodes a response: its tag, then its variant's record.
///
/// # Errors
/// Refuses an identity of another scope, or a close assessment past its
/// limits.
pub fn encode_response(
    w: &mut Writer,
    scope: IdScope,
    response: &Response,
) -> Result<(), EncodeError> {
    match response {
        Response::Unit => {
            w.tag(0);
            w.record(|_| Ok(()))
        }
        Response::Session(session) => {
            w.tag(1);
            w.record(|w| w.id(scope, *session))
        }
        Response::Workspace(workspace) => {
            w.tag(2);
            w.record(|w| w.id(scope, *workspace))
        }
        Response::Attachment(attachment) => {
            w.tag(3);
            w.record(|w| w.id(scope, *attachment))
        }
        Response::OpenedTab(OpenedTab {
            tab,
            workspace,
            pane,
            terminal,
        }) => {
            w.tag(4);
            w.record(|w| {
                w.id(scope, *tab)?;
                w.id(scope, *workspace)?;
                w.id(scope, *pane)?;
                w.id(scope, *terminal)
            })
        }
        Response::OpenedPane(OpenedPane {
            tab,
            pane,
            terminal,
        }) => {
            w.tag(5);
            w.record(|w| {
                w.id(scope, *tab)?;
                w.id(scope, *pane)?;
                w.id(scope, *terminal)
            })
        }
        Response::Selection(Selection {
            session,
            workspace,
            tab,
            pane,
        }) => {
            w.tag(6);
            w.record(|w| {
                w.id(scope, *session)?;
                w.option(*workspace, |w, id| w.id(scope, id))?;
                w.option(*tab, |w, id| w.id(scope, id))?;
                w.option(*pane, |w, id| w.id(scope, id))
            })
        }
        Response::CloseAssessment(assessment) => {
            w.tag(7);
            w.record(|w| encode_assessment(w, scope, assessment))
        }
        Response::Undo { token } => {
            w.tag(8);
            w.record(|w| {
                w.option(*token, |w, token| {
                    w.u64(token);
                    Ok(())
                })
            })
        }
        Response::Command(outcome) => {
            w.tag(9);
            w.record(|w| {
                encode_outcome(w, *outcome);
                Ok(())
            })
        }
        Response::Hierarchy { seq, digest } => {
            w.tag(10);
            w.record(|w| {
                w.u64(*seq);
                w.fixed(&digest.to_le_bytes());
                Ok(())
            })
        }
        Response::EmptiedWorkspace { closed } => {
            w.tag(11);
            w.record(|w| {
                w.bool(*closed);
                Ok(())
            })
        }
    }
}

/// Decodes a response.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_response(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<Response, DecodeError> {
    let tag = r.tag()?;
    r.record(|r| {
        Ok(match tag {
            0 => Response::Unit,
            1 => Response::Session(r.id(scope)?),
            2 => Response::Workspace(r.id(scope)?),
            3 => Response::Attachment(r.id(scope)?),
            4 => Response::OpenedTab(OpenedTab {
                tab: r.id(scope)?,
                workspace: r.id(scope)?,
                pane: r.id(scope)?,
                terminal: r.id(scope)?,
            }),
            5 => Response::OpenedPane(OpenedPane {
                tab: r.id(scope)?,
                pane: r.id(scope)?,
                terminal: r.id(scope)?,
            }),
            6 => Response::Selection(Selection {
                session: r.id(scope)?,
                workspace: r.option("workspace", |r| r.id(scope))?,
                tab: r.option("tab", |r| r.id(scope))?,
                pane: r.option("pane", |r| r.id(scope))?,
            }),
            7 => Response::CloseAssessment(decode_assessment(r, scope)?),
            8 => Response::Undo {
                token: r.option("undo token", Reader::u64)?,
            },
            9 => Response::Command(decode_outcome(r)?),
            10 => Response::Hierarchy {
                seq: r.u64()?,
                digest: u64::from_le_bytes(r.fixed()?),
            },
            11 => Response::EmptiedWorkspace { closed: r.bool()? },
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "response",
                    tag,
                });
            }
        })
    })
}

/// Encodes a result record: the request it answers, then a success's
/// response and commit, or an error.
///
/// # Errors
/// Refuses what [`encode_response`] refuses.
pub fn encode_result(
    w: &mut Writer,
    scope: IdScope,
    result: &RequestResult,
) -> Result<(), EncodeError> {
    let RequestResult { request, outcome } = result;
    w.record(|w| {
        write_request_id(w, *request);
        match outcome {
            Ok(Success {
                response,
                committed,
            }) => {
                w.tag(0);
                w.record(|w| {
                    encode_response(w, scope, response)?;
                    encode_committed(w, *committed)
                })
            }
            Err(error) => {
                w.tag(1);
                encode_error(w, error)
            }
        }
    })
}

/// Decodes a result record.
///
/// # Errors
/// Returns [`DecodeError::Invalid`] for a zero request identity, or the
/// first decode failure.
pub fn decode_result(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<RequestResult, DecodeError> {
    r.record(|r| {
        let request = read_request_id(r)?;
        let outcome = match r.tag()? {
            0 => Ok(r.record(|r| {
                Ok(Success {
                    response: decode_response(r, scope)?,
                    committed: decode_committed(r)?,
                })
            })?),
            1 => Err(decode_error(r)?),
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "result",
                    tag,
                });
            }
        };
        Ok(RequestResult { request, outcome })
    })
}

#[cfg(test)]
mod tests;
