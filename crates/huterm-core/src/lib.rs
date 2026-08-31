//! Terminal runtime, emulator, PTY, and multiplexer ownership.

#![deny(missing_docs)]

mod engine;
mod input;
mod mux;
mod pty;
mod terminal;

pub use mux::{Mux, MuxError, TerminalOwner};
pub use terminal::{RuntimeClient, RuntimeError, TerminalRuntime};
