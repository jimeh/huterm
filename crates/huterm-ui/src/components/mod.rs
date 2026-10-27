//! One file per component. Each opens with its contract: purpose, anatomy,
//! keyboard and pointer behavior, states, and accessibility. Every component
//! here has an entry in `huterm-gpui`'s component gallery, which a script test
//! checks.

pub mod badge;
pub mod kbd;
pub mod scroll_area;
pub mod separator;
pub mod surface;
pub mod tooltip;
