//! One file per component. Each opens with its contract: purpose, anatomy,
//! keyboard and pointer behavior, states, and accessibility. Every component
//! here has an entry in `huterm-gpui`'s component gallery, which a script test
//! checks.

pub mod alert;
pub mod alert_dialog;
pub mod badge;
pub mod button;
pub mod card;
pub mod collapsible;
pub mod combobox;
pub mod command;
pub mod dialog;
pub mod empty;
pub mod field;
pub mod input;
pub mod input_group;
pub mod item;
pub mod kbd;
pub mod key_recorder;
pub mod menu;
pub mod number_field;
pub mod popover;
pub mod radio_group;
pub mod scroll_area;
pub mod select;
pub mod separator;
pub mod slider;
pub mod surface;
pub mod switch;
pub mod tabs;
pub mod toggle_group;
pub mod tooltip;
