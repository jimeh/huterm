//! The crate's single error type.

use std::fmt;

use crate::ffi;

/// A failed libghostty-vt call, or a terminal made unusable by a host
/// callback panic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// `GHOSTTY_OUT_OF_MEMORY`: an allocation failed.
    OutOfMemory,
    /// `GHOSTTY_INVALID_VALUE`: an argument or state was invalid.
    InvalidValue,
    /// `GHOSTTY_OUT_OF_SPACE`: a caller buffer was too small.
    OutOfSpace,
    /// `GHOSTTY_NO_VALUE`: the requested value is absent.
    NoValue,
    /// `GHOSTTY_IO_ERROR`: a reader or writer callback failed.
    Io,
    /// `GHOSTTY_LIMIT_EXCEEDED`: input exceeded a configured limit.
    LimitExceeded,
    /// `GHOSTTY_REJECTED`: a safety check refused the operation.
    Rejected,
    /// A host callback panicked. The terminal answered the callback safely
    /// and refuses every later operation.
    Poisoned,
    /// A result code this binding does not know.
    Unknown(i32),
}

/// A result whose error is [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub(crate) fn from_code(code: ffi::GhosttyResult) -> Result<()> {
        Err(match code {
            ffi::GHOSTTY_SUCCESS => return Ok(()),
            ffi::GHOSTTY_OUT_OF_MEMORY => Self::OutOfMemory,
            ffi::GHOSTTY_INVALID_VALUE => Self::InvalidValue,
            ffi::GHOSTTY_OUT_OF_SPACE => Self::OutOfSpace,
            ffi::GHOSTTY_NO_VALUE => Self::NoValue,
            ffi::GHOSTTY_IO_ERROR => Self::Io,
            ffi::GHOSTTY_LIMIT_EXCEEDED => Self::LimitExceeded,
            ffi::GHOSTTY_REJECTED => Self::Rejected,
            other => Self::Unknown(other),
        })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfMemory => {
                formatter.write_str("libghostty-vt out of memory")
            }
            Self::InvalidValue => {
                formatter.write_str("libghostty-vt invalid value")
            }
            Self::OutOfSpace => {
                formatter.write_str("libghostty-vt buffer too small")
            }
            Self::NoValue => formatter.write_str("libghostty-vt value absent"),
            Self::Io => formatter.write_str("libghostty-vt I/O error"),
            Self::LimitExceeded => {
                formatter.write_str("libghostty-vt limit exceeded")
            }
            Self::Rejected => {
                formatter.write_str("libghostty-vt rejected the operation")
            }
            Self::Poisoned => formatter
                .write_str("terminal poisoned by a panicking host callback"),
            Self::Unknown(code) => {
                write!(
                    formatter,
                    "libghostty-vt returned unknown result {code}"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_result_code_maps_to_one_variant() {
        let codes = [
            (ffi::GHOSTTY_OUT_OF_MEMORY, Error::OutOfMemory),
            (ffi::GHOSTTY_INVALID_VALUE, Error::InvalidValue),
            (ffi::GHOSTTY_OUT_OF_SPACE, Error::OutOfSpace),
            (ffi::GHOSTTY_NO_VALUE, Error::NoValue),
            (ffi::GHOSTTY_IO_ERROR, Error::Io),
            (ffi::GHOSTTY_LIMIT_EXCEEDED, Error::LimitExceeded),
            (ffi::GHOSTTY_REJECTED, Error::Rejected),
            (-99, Error::Unknown(-99)),
        ];
        assert_eq!(Error::from_code(ffi::GHOSTTY_SUCCESS), Ok(()));
        for (code, error) in codes {
            assert_eq!(Error::from_code(code), Err(error));
        }
    }
}
