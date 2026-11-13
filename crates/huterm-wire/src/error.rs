//! The error half of a request's result.
//!
//! A [`WireError`] carries a stable numeric code, whether the same request
//! may safely be sent again, the hierarchy sequence (and digest) a
//! structural request committed at before failing, and a diagnostic message
//! clients never parse. Codes are stable numbers; a minor version may add codes, which a
//! server withholds from older peers like any newer variant.

use huterm_protocol::CommandError;

use crate::codec::{DecodeError, EncodeError, Reader, Writer};
use crate::handshake::truncated;
use crate::response::{Committed, decode_committed, encode_committed};

/// The longest diagnostic message a [`WireError`] carries; longer messages
/// are cut at a character boundary.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 1024;

/// A stable error code.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ErrorCode {
    /// A target no longer exists, or an identity is not the connection's.
    StaleTarget,
    /// A target belongs to another runtime scope.
    ForeignScope,
    /// The viewer lacks the capability the request needs.
    NotPermitted,
    /// The viewer was revoked.
    Revoked,
    /// The terminal's runtime stopped.
    Stopped,
    /// The server is busy; nothing was committed.
    Busy,
    /// The connection did not negotiate what the request needs.
    Unsupported,
    /// An argument is missing, unknown, repeated, mistyped, or out of
    /// range.
    InvalidArgument,
    /// A limit would be exceeded.
    LimitExceeded,
    /// A close assessment changed since its ticket was prepared.
    StaleClose,
    /// No catalog command has the identifier.
    UnknownCommand,
    /// The connection ended before the request's outcome was known.
    OutcomeUnknown,
    /// The connection ended; the request did not run.
    Disconnected,
    /// An undo token's history slot was evicted or replaced.
    StaleUndo,
    /// A name is blank or too long.
    InvalidName,
    /// The close needs the user's confirmation.
    ConfirmationRequired,
    /// The executor refused the command in its current state.
    Unavailable,
    /// The command needs a client window or view.
    ClientRequired,
    /// The runtime failed while executing the request.
    Internal,
    /// The client cancelled the request before it started; nothing was
    /// committed.
    Cancelled,
}

impl ErrorCode {
    /// Every code, in number order.
    pub const ALL: [Self; 20] = [
        Self::StaleTarget,
        Self::ForeignScope,
        Self::NotPermitted,
        Self::Revoked,
        Self::Stopped,
        Self::Busy,
        Self::Unsupported,
        Self::InvalidArgument,
        Self::LimitExceeded,
        Self::StaleClose,
        Self::UnknownCommand,
        Self::OutcomeUnknown,
        Self::Disconnected,
        Self::StaleUndo,
        Self::InvalidName,
        Self::ConfirmationRequired,
        Self::Unavailable,
        Self::ClientRequired,
        Self::Internal,
        Self::Cancelled,
    ];

    /// The code's stable number.
    #[must_use]
    pub const fn number(self) -> u64 {
        match self {
            Self::StaleTarget => 1,
            Self::ForeignScope => 2,
            Self::NotPermitted => 3,
            Self::Revoked => 4,
            Self::Stopped => 5,
            Self::Busy => 6,
            Self::Unsupported => 7,
            Self::InvalidArgument => 8,
            Self::LimitExceeded => 9,
            Self::StaleClose => 10,
            Self::UnknownCommand => 11,
            Self::OutcomeUnknown => 12,
            Self::Disconnected => 13,
            Self::StaleUndo => 14,
            Self::InvalidName => 15,
            Self::ConfirmationRequired => 16,
            Self::Unavailable => 17,
            Self::ClientRequired => 18,
            Self::Internal => 19,
            Self::Cancelled => 20,
        }
    }

    /// The code with this number, if any.
    #[must_use]
    pub fn from_number(number: u64) -> Option<Self> {
        Self::ALL.into_iter().find(|code| code.number() == number)
    }
}

/// Whether a failed request may be sent again unchanged.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Retry {
    /// Nothing was committed, so the same request may be sent again.
    Safe,
    /// The request must not be repeated.
    Never,
}

/// A request's failure as it crosses the wire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireError {
    /// The stable code.
    pub code: ErrorCode,
    /// Whether the request may be sent again.
    pub retry: Retry,
    /// Where a structural request committed before it failed, as a
    /// failed close cleanup does.
    pub committed: Option<Committed>,
    /// A diagnostic message, never parsed.
    pub message: String,
}

impl WireError {
    /// An error that committed nothing; only [`ErrorCode::Busy`] and
    /// [`ErrorCode::Cancelled`] are safe to retry.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            retry: if matches!(code, ErrorCode::Busy | ErrorCode::Cancelled) {
                Retry::Safe
            } else {
                Retry::Never
            },
            committed: None,
            message: message.into(),
        }
    }
}

impl From<&CommandError> for WireError {
    fn from(error: &CommandError) -> Self {
        let code = match error {
            CommandError::UnknownCommand(_) => ErrorCode::UnknownCommand,
            CommandError::UnknownArgument { .. }
            | CommandError::DuplicateArgument { .. }
            | CommandError::MissingArgument { .. }
            | CommandError::ConflictingArguments { .. }
            | CommandError::ArgumentType { .. }
            | CommandError::ArgumentRange { .. } => ErrorCode::InvalidArgument,
            CommandError::Unavailable(_) => ErrorCode::Unavailable,
            CommandError::StaleTarget => ErrorCode::StaleTarget,
            CommandError::ClientRequired => ErrorCode::ClientRequired,
            CommandError::Busy => ErrorCode::Busy,
            CommandError::Runtime(_) => ErrorCode::Internal,
        };
        Self::new(code, error.to_string())
    }
}

/// Encodes a [`WireError`] record.
///
/// # Errors
/// Never fails today; the signature matches the other encoders.
pub fn encode_error(
    w: &mut Writer,
    error: &WireError,
) -> Result<(), EncodeError> {
    let WireError {
        code,
        retry,
        committed,
        message,
    } = error;
    w.record(|w| {
        w.tag(code.number());
        w.tag(match retry {
            Retry::Safe => 0,
            Retry::Never => 1,
        });
        encode_committed(w, *committed)?;
        w.str(
            truncated(message, MAX_ERROR_MESSAGE_BYTES),
            MAX_ERROR_MESSAGE_BYTES,
            "error message",
        )
    })
}

/// Decodes a [`WireError`] record.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_error(r: &mut Reader<'_>) -> Result<WireError, DecodeError> {
    r.record(|r| {
        let number = r.tag()?;
        let code =
            ErrorCode::from_number(number).ok_or(DecodeError::UnknownTag {
                field: "error code",
                tag: number,
            })?;
        let retry = match r.tag()? {
            0 => Retry::Safe,
            1 => Retry::Never,
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "retry",
                    tag,
                });
            }
        };
        Ok(WireError {
            code,
            retry,
            committed: decode_committed(r)?,
            message: r.string(MAX_ERROR_MESSAGE_BYTES, "error message")?,
        })
    })
}

#[cfg(test)]
mod tests;
