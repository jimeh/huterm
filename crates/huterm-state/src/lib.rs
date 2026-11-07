//! Saved session state for one socket scope.
//!
//! A scope directory, `<state root>/sockets/<socket name>/`, holds two JSON
//! files. `runtime.json` records the runtime structure as a
//! [`RuntimeCapture`](huterm_protocol::RuntimeCapture), and `desktop.json`
//! records the desktop client's [`DesktopLayout`]. Only the process that
//! holds the scope's lock reads or writes them. Each save writes both files
//! with the same scope identity and the next generation, so a layout from
//! another save is detected and ignored.
//!
//! Core and protocol stay serialization-free: this crate owns the schema
//! and its conversions, and applies the same validation core applies, plus
//! the size bounds every saved file must meet.

#![deny(missing_docs)]

mod atomic;
pub mod bounds;
mod desktop;
mod error;
mod lock;
mod paths;
mod schema;
mod store;

pub use desktop::{
    DesktopLayout, FocusedPane, SessionNavigation, WindowAppearance,
    WindowBounds, WindowLayout, WindowMode,
};
pub use error::{InvalidState, OpenError, SaveError};
pub use lock::{LockError, ScopeLock};
pub use paths::{PathError, state_root, valid_socket_name};
pub use store::{
    DesktopLoad, Loaded, RuntimeLoad, SavedRuntime, ScopeId, StateFile, Store,
};
