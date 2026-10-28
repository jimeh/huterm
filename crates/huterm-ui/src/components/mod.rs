//! One file per component. Each opens with its contract: purpose, anatomy,
//! keyboard and pointer behavior, states, and accessibility. Every component
//! here has an entry in `huterm-gpui`'s component gallery, which a script test
//! checks.

pub mod badge;
pub mod button;
pub mod kbd;
pub mod menu;
pub mod popover;
pub mod scroll_area;
pub mod select;
pub mod separator;
pub mod surface;
pub mod switch;
pub mod toggle_group;
pub mod tooltip;
