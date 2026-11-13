//! Encoding of a terminal's status, which a viewer's lane carries whole
//! whenever its revision changes.
//!
//! Every text is bounded by [`MAX_STATUS_TEXT_BYTES`], which core applies
//! when it publishes status, so a status at every limit fits its message
//! cap. Times travel as seconds and nanoseconds from the Unix epoch, signed
//! seconds first, so a time before the epoch survives.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use huterm_protocol::{
    COMMAND_RECORD_CAPACITY, CommandRecord, CommandState, ExitStatus,
    MAX_STATUS_TEXT_BYTES, TERMINAL_FAILURE_CAPACITY, TerminalDirectory,
    TerminalFailure, TerminalLifecycle, TerminalMetadata, TerminalProgress,
    TerminalStatus,
};

use crate::codec::{DecodeError, EncodeError, Reader, Writer};

const NANOS_PER_SECOND: u32 = 1_000_000_000;

fn write_text(
    w: &mut Writer,
    text: &str,
    field: &'static str,
) -> Result<(), EncodeError> {
    w.str(text, MAX_STATUS_TEXT_BYTES, field)
}

fn read_text(
    r: &mut Reader<'_>,
    field: &'static str,
) -> Result<String, DecodeError> {
    r.string(MAX_STATUS_TEXT_BYTES, field)
}

fn write_directory(
    w: &mut Writer,
    directory: Option<&TerminalDirectory>,
) -> Result<(), EncodeError> {
    // The type's fields are private; its accessors cover every one.
    w.option(directory, |w, directory| {
        w.record(|w| {
            w.optional_str(
                directory.host(),
                MAX_STATUS_TEXT_BYTES,
                "directory host",
            )?;
            write_text(w, directory.path(), "directory path")?;
            w.bool(directory.is_local());
            Ok(())
        })
    })
}

fn read_directory(
    r: &mut Reader<'_>,
) -> Result<Option<TerminalDirectory>, DecodeError> {
    r.option("directory", |r| {
        r.record(|r| {
            Ok(TerminalDirectory::new(
                r.optional_string(MAX_STATUS_TEXT_BYTES, "directory host")?,
                read_text(r, "directory path")?,
                r.bool()?,
            ))
        })
    })
}

fn write_metadata(
    w: &mut Writer,
    metadata: &TerminalMetadata,
) -> Result<(), EncodeError> {
    w.record(|w| {
        write_directory(w, metadata.directory())?;
        w.optional_str(
            metadata.foreground_process(),
            MAX_STATUS_TEXT_BYTES,
            "foreground process",
        )?;
        w.optional_str(
            metadata.foreground_title(),
            MAX_STATUS_TEXT_BYTES,
            "foreground title",
        )?;
        write_directory(w, metadata.shell_directory())
    })
}

fn read_metadata(r: &mut Reader<'_>) -> Result<TerminalMetadata, DecodeError> {
    r.record(|r| {
        let directory = read_directory(r)?;
        let process =
            r.optional_string(MAX_STATUS_TEXT_BYTES, "foreground process")?;
        let title =
            r.optional_string(MAX_STATUS_TEXT_BYTES, "foreground title")?;
        let shell = read_directory(r)?;
        Ok(TerminalMetadata::new(directory, process)
            .with_foreground_title(title)
            .with_shell_directory(shell))
    })
}

fn write_time(w: &mut Writer, time: SystemTime) -> Result<(), EncodeError> {
    let out_of_range = || EncodeError::LimitExceeded { field: "time" };
    let (seconds, nanos) = match time.duration_since(UNIX_EPOCH) {
        Ok(after) => (
            i64::try_from(after.as_secs()).map_err(|_| out_of_range())?,
            after.subsec_nanos(),
        ),
        Err(before) => {
            let before = before.duration();
            let seconds =
                i64::try_from(before.as_secs()).map_err(|_| out_of_range())?;
            match before.subsec_nanos() {
                0 => (-seconds, 0),
                nanos => (-seconds - 1, NANOS_PER_SECOND - nanos),
            }
        }
    };
    w.i64(seconds);
    w.u32(nanos);
    Ok(())
}

fn read_time(r: &mut Reader<'_>) -> Result<SystemTime, DecodeError> {
    let seconds = r.i64()?;
    let nanos = r.u32()?;
    if nanos >= NANOS_PER_SECOND {
        return Err(DecodeError::Invalid("time"));
    }
    let base = if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(seconds.unsigned_abs()))
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(seconds.unsigned_abs()))
    };
    base.and_then(|base| base.checked_add(Duration::from_nanos(nanos.into())))
        .ok_or(DecodeError::Invalid("time"))
}

fn write_duration(w: &mut Writer, duration: Duration) {
    w.u64(duration.as_secs());
    w.u32(duration.subsec_nanos());
}

fn read_duration(r: &mut Reader<'_>) -> Result<Duration, DecodeError> {
    let seconds = r.u64()?;
    let nanos = r.u32()?;
    if nanos >= NANOS_PER_SECOND {
        return Err(DecodeError::Invalid("duration"));
    }
    Ok(Duration::new(seconds, nanos))
}

fn write_command(
    w: &mut Writer,
    record: &CommandRecord,
) -> Result<(), EncodeError> {
    let CommandRecord {
        sequence,
        text,
        state,
        started_at,
        finished_at,
        duration,
    } = record;
    w.record(|w| {
        w.u64(*sequence);
        w.optional_str(text.as_deref(), MAX_STATUS_TEXT_BYTES, "command text")?;
        match state {
            CommandState::Running => w.tag(0),
            CommandState::Finished { exit_code } => {
                w.tag(1);
                w.option(*exit_code, |w, code| {
                    w.i64(code.into());
                    Ok(())
                })?;
            }
            CommandState::Abandoned => w.tag(2),
        }
        write_time(w, *started_at)?;
        w.option(*finished_at, write_time)?;
        w.option(*duration, |w, duration| {
            write_duration(w, duration);
            Ok(())
        })
    })
}

fn read_command(r: &mut Reader<'_>) -> Result<CommandRecord, DecodeError> {
    r.record(|r| {
        Ok(CommandRecord {
            sequence: r.u64()?,
            text: r.optional_string(MAX_STATUS_TEXT_BYTES, "command text")?,
            state: match r.tag()? {
                0 => CommandState::Running,
                1 => CommandState::Finished {
                    exit_code: r.option("exit code", |r| {
                        i32::try_from(r.i64()?)
                            .map_err(|_| DecodeError::OutOfRange)
                    })?,
                },
                2 => CommandState::Abandoned,
                tag => {
                    return Err(DecodeError::UnknownTag {
                        field: "command state",
                        tag,
                    });
                }
            },
            started_at: read_time(r)?,
            finished_at: r.option("finished at", read_time)?,
            duration: r.option("duration", read_duration)?,
        })
    })
}

fn write_percent(w: &mut Writer, percent: u8) {
    w.fixed(&[percent]);
}

fn read_percent(r: &mut Reader<'_>) -> Result<u8, DecodeError> {
    let [percent] = r.fixed()?;
    if percent > 100 {
        return Err(DecodeError::Invalid("progress percentage"));
    }
    Ok(percent)
}

fn write_progress(w: &mut Writer, progress: Option<TerminalProgress>) {
    match progress {
        None => w.tag(0),
        Some(TerminalProgress::Determinate(percent)) => {
            w.tag(1);
            write_percent(w, percent);
        }
        Some(TerminalProgress::Indeterminate) => w.tag(2),
        Some(TerminalProgress::Paused(percent)) => {
            w.tag(3);
            // An option tag never fails to encode.
            let _ = w.option(percent, |w, percent| {
                write_percent(w, percent);
                Ok(())
            });
        }
        Some(TerminalProgress::Error(percent)) => {
            w.tag(4);
            let _ = w.option(percent, |w, percent| {
                write_percent(w, percent);
                Ok(())
            });
        }
    }
}

fn read_progress(
    r: &mut Reader<'_>,
) -> Result<Option<TerminalProgress>, DecodeError> {
    Ok(match r.tag()? {
        0 => None,
        1 => Some(TerminalProgress::Determinate(read_percent(r)?)),
        2 => Some(TerminalProgress::Indeterminate),
        3 => Some(TerminalProgress::Paused(
            r.option("progress", read_percent)?,
        )),
        4 => Some(TerminalProgress::Error(r.option("progress", read_percent)?)),
        tag => {
            return Err(DecodeError::UnknownTag {
                field: "progress",
                tag,
            });
        }
    })
}

/// Encodes a terminal status's record.
///
/// # Errors
/// Refuses a text past [`MAX_STATUS_TEXT_BYTES`], more failures or command
/// records than their logs hold, or a time beyond `i64` seconds, with
/// [`EncodeError::LimitExceeded`].
pub fn encode_status(
    w: &mut Writer,
    status: &TerminalStatus,
) -> Result<(), EncodeError> {
    let TerminalStatus {
        revision,
        title,
        metadata,
        metadata_revision,
        lifecycle,
        bells,
        failures,
        commands,
        progress,
    } = status;
    w.record(|w| {
        w.u64(*revision);
        write_text(w, title, "title")?;
        write_metadata(w, metadata)?;
        w.u64(*metadata_revision);
        match lifecycle {
            TerminalLifecycle::Running => w.tag(0),
            TerminalLifecycle::Exited(ExitStatus { code, success }) => {
                w.tag(1);
                w.record(|w| {
                    w.option(*code, |w, code| {
                        w.u32(code);
                        Ok(())
                    })?;
                    w.bool(*success);
                    Ok(())
                })?;
            }
        }
        w.u64(*bells);
        w.seq(
            failures.iter(),
            TERMINAL_FAILURE_CAPACITY,
            "failures",
            |w, failure| {
                let TerminalFailure { sequence, message } = failure;
                w.record(|w| {
                    w.u64(*sequence);
                    write_text(w, message, "failure message")
                })
            },
        )?;
        w.seq(
            commands.iter(),
            COMMAND_RECORD_CAPACITY,
            "command records",
            write_command,
        )?;
        write_progress(w, *progress);
        Ok(())
    })
}

/// Decodes a terminal status's record. Failures and command records must
/// be in increasing sequence order, as the protocol's lookups assume.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_status(
    r: &mut Reader<'_>,
) -> Result<TerminalStatus, DecodeError> {
    r.record(|r| {
        let revision = r.u64()?;
        let title = read_text(r, "title")?;
        let metadata = read_metadata(r)?;
        let metadata_revision = r.u64()?;
        let lifecycle = match r.tag()? {
            0 => TerminalLifecycle::Running,
            1 => TerminalLifecycle::Exited(r.record(|r| {
                Ok(ExitStatus {
                    code: r.option("exit code", Reader::u32)?,
                    success: r.bool()?,
                })
            })?),
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "lifecycle",
                    tag,
                });
            }
        };
        let bells = r.u64()?;
        let failures = r.seq(TERMINAL_FAILURE_CAPACITY, "failures", |r| {
            r.record(|r| {
                Ok(TerminalFailure {
                    sequence: r.u64()?,
                    message: read_text(r, "failure message")?,
                })
            })
        })?;
        let commands: Vec<CommandRecord> =
            r.seq(COMMAND_RECORD_CAPACITY, "command records", read_command)?;
        let increasing = |sequences: &[u64]| {
            sequences.windows(2).all(|pair| pair[0] < pair[1])
        };
        if !increasing(&failures.iter().map(|f| f.sequence).collect::<Vec<_>>())
            || !increasing(
                &commands.iter().map(|c| c.sequence).collect::<Vec<_>>(),
            )
        {
            return Err(DecodeError::Invalid("status sequence order"));
        }
        let progress = read_progress(r)?;
        Ok(TerminalStatus {
            revision,
            title,
            metadata,
            metadata_revision,
            lifecycle,
            bells,
            failures,
            commands: Arc::from(commands),
            progress,
        })
    })
}

#[cfg(test)]
mod tests;
