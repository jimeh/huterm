//! Catalog invocations and their outcomes.
//!
//! A command identifier travels as its string, and decoding resolves it
//! through the catalog. An identifier the catalog does not know decodes to
//! [`DecodedInvocation::Unknown`], which the server answers with
//! `UnknownCommand` carrying the text, so an older server reports a newer
//! client's command instead of dropping the connection.

use huterm_protocol::{
    CommandArgument, CommandInvocation, CommandOutcome, CommandValue, lookup,
};

use crate::codec::{DecodeError, EncodeError, IdScope, Reader, Writer};

/// The longest command identifier or argument name.
pub const MAX_COMMAND_NAME_BYTES: usize = 64;

/// The most arguments one invocation carries.
pub const MAX_COMMAND_ARGUMENTS: usize = 32;

/// The longest text argument.
pub const MAX_ARGUMENT_TEXT_BYTES: usize = 4096;

/// An invocation as a server decodes it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodedInvocation {
    /// A catalog command and its arguments, not yet validated.
    Known(CommandInvocation),
    /// An identifier the catalog does not know, with the arguments sent.
    Unknown {
        /// The identifier's text.
        id: String,
        /// The arguments, not validated.
        args: Vec<CommandArgument>,
    },
}

fn encode_value(
    w: &mut Writer,
    scope: IdScope,
    value: &CommandValue,
) -> Result<(), EncodeError> {
    match value {
        CommandValue::Bool(value) => {
            w.tag(0);
            w.record(|w| {
                w.bool(*value);
                Ok(())
            })
        }
        CommandValue::Integer(value) => {
            w.tag(1);
            w.record(|w| {
                w.i64(*value);
                Ok(())
            })
        }
        CommandValue::Text(value) => {
            w.tag(2);
            w.record(|w| w.str(value, MAX_ARGUMENT_TEXT_BYTES, "argument text"))
        }
        CommandValue::Tab(id) => {
            w.tag(3);
            w.record(|w| w.id(scope, *id))
        }
        CommandValue::Workspace(id) => {
            w.tag(4);
            w.record(|w| w.id(scope, *id))
        }
        CommandValue::Session(id) => {
            w.tag(5);
            w.record(|w| w.id(scope, *id))
        }
        CommandValue::Pane(id) => {
            w.tag(6);
            w.record(|w| w.id(scope, *id))
        }
    }
}

fn decode_value(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<CommandValue, DecodeError> {
    let tag = r.tag()?;
    r.record(|r| {
        Ok(match tag {
            0 => CommandValue::Bool(r.bool()?),
            1 => CommandValue::Integer(r.i64()?),
            2 => CommandValue::Text(
                r.string(MAX_ARGUMENT_TEXT_BYTES, "argument text")?,
            ),
            3 => CommandValue::Tab(r.id(scope)?),
            4 => CommandValue::Workspace(r.id(scope)?),
            5 => CommandValue::Session(r.id(scope)?),
            6 => CommandValue::Pane(r.id(scope)?),
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "command value",
                    tag,
                });
            }
        })
    })
}

/// Encodes a catalog invocation.
///
/// # Errors
/// Refuses an identity of another scope, or an identifier, name, text, or
/// argument list past its limit.
pub fn encode_invocation(
    w: &mut Writer,
    scope: IdScope,
    invocation: &CommandInvocation,
) -> Result<(), EncodeError> {
    let CommandInvocation { id, args } = invocation;
    w.record(|w| {
        w.str(id.as_str(), MAX_COMMAND_NAME_BYTES, "command ID")?;
        w.seq(
            args.iter(),
            MAX_COMMAND_ARGUMENTS,
            "command arguments",
            |w, arg| {
                let CommandArgument { name, value } = arg;
                w.record(|w| {
                    w.str(name, MAX_COMMAND_NAME_BYTES, "argument name")?;
                    encode_value(w, scope, value)
                })
            },
        )
    })
}

/// Decodes an invocation, resolving its identifier through the catalog.
///
/// # Errors
/// Returns the first decode failure. An unknown identifier is not an error.
pub fn decode_invocation(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<DecodedInvocation, DecodeError> {
    r.record(|r| {
        let id = r.str(MAX_COMMAND_NAME_BYTES, "command ID")?;
        let args = r.seq(MAX_COMMAND_ARGUMENTS, "command arguments", |r| {
            r.record(|r| {
                Ok(CommandArgument {
                    name: r.string(MAX_COMMAND_NAME_BYTES, "argument name")?,
                    value: decode_value(r, scope)?,
                })
            })
        })?;
        Ok(match lookup(id) {
            Some(spec) => {
                DecodedInvocation::Known(CommandInvocation::new(spec.id, args))
            }
            None => DecodedInvocation::Unknown {
                id: id.to_owned(),
                args,
            },
        })
    })
}

/// Encodes a command's outcome.
pub fn encode_outcome(w: &mut Writer, outcome: CommandOutcome) {
    w.tag(match outcome {
        CommandOutcome::Completed => 0,
        CommandOutcome::Accepted => 1,
    });
}

/// Decodes a command's outcome.
///
/// # Errors
/// Returns [`DecodeError::UnknownTag`] for an outcome this version does not
/// define.
pub fn decode_outcome(
    r: &mut Reader<'_>,
) -> Result<CommandOutcome, DecodeError> {
    match r.tag()? {
        0 => Ok(CommandOutcome::Completed),
        1 => Ok(CommandOutcome::Accepted),
        tag => Err(DecodeError::UnknownTag {
            field: "command outcome",
            tag,
        }),
    }
}

#[cfg(test)]
mod tests;
