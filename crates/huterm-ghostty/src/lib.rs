//! Safe bindings to Ghostty's `libghostty-vt` C API.
//!
//! This crate owns everything that changes with the Ghostty pin: the native
//! build, the generated FFI declarations, the safe wrapper, and workarounds
//! for API gaps such as the mouse-mode and color-override probes. Huterm's
//! terminal policy lives in `huterm-core`.
//!
//! All `unsafe` code lives in `native` (library calls), `callbacks` (C
//! trampolines), and the generated `ffi::bindings`; test-only allocator
//! hooks live in `test_alloc`. Everything else is safe code.
//!
//! Handles are neither `Send` nor `Sync`: each terminal is created and used
//! on its runtime owner thread. Host callbacks receive plain values and
//! never the terminal, because `libghostty-vt` forbids re-entrant writes.
//! Their effects queue in callback order until
//! [`Terminal::take_effects`]. A panicking callback is contained: the
//! library receives a safe answer and the terminal returns
//! [`Error::Poisoned`] from then on.

mod callbacks;
mod error;
mod ffi;
mod mouse;
mod native;
mod overrides;
mod render;
mod terminal;
mod types;

#[cfg(test)]
mod abi_tests;
#[cfg(test)]
mod contract_tests;
#[cfg(test)]
mod key_tests;
#[cfg(test)]
mod test_alloc;
#[cfg(test)]
mod test_layout;

pub use callbacks::Host;
pub use error::{Error, Result};
pub use mouse::{MouseProbe, ProbedFormat, ProbedTracking};
pub use native::NativeBytes;
pub use overrides::ColorOverrides;
pub use render::{RenderCells, RenderRow, RenderState, Rows};
pub use terminal::{GridRef, Options, Terminal};
pub use types::{
    Cell, CellContent, CellWidth, ClipboardContent, ClipboardLocation,
    ClipboardWrite, ClipboardWriteResult, ColorScheme, Cursor, CursorStyle,
    DeviceAttributes, Dirty, Effect, Fill, Mode, Optimize, Palette, Point,
    PointSpace, RenderColors, Rgb, Row, Screen, Scroll, Scrollbar, Style,
    StyleColor,
};

/// The optimization mode of the linked library.
///
/// # Errors
///
/// Fails if the library rejects the query.
pub fn optimize() -> Result<Optimize> {
    Ok(
        match native::build_info::<ffi::keys::build_info::Optimize>()? {
            ffi::GHOSTTY_OPTIMIZE_DEBUG => Optimize::Debug,
            ffi::GHOSTTY_OPTIMIZE_RELEASE_SAFE => Optimize::ReleaseSafe,
            ffi::GHOSTTY_OPTIMIZE_RELEASE_SMALL => Optimize::ReleaseSmall,
            ffi::GHOSTTY_OPTIMIZE_RELEASE_FAST => Optimize::ReleaseFast,
            other => Optimize::Unknown(other),
        },
    )
}
