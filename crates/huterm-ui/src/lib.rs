//! Generic GPUI components for Huterm's desktop client.
//!
//! The crate knows nothing about Huterm: no catalog commands, configuration,
//! or terminal state. Components expose methods and take their colors as
//! [`tokens::Tokens`]; `huterm-gpui` maps its catalog commands onto those
//! methods and builds the tokens from the terminal theme.
//! [`docs/plans/ui-components.md`] records each component's contract.
//!
//! Primitives sit at the crate root: [`tokens`], [`focus`] visibility, the
//! [`a11y`] state helper, the [`animation`] schedule, and the [`scrollbar`]
//! geometry with its [`list_scrollbar`] overlay. One file per component lives
//! under [`components`], and each has an entry in `huterm-gpui`'s component
//! gallery.
//!
//! [`docs/plans/ui-components.md`]: https://github.com/jimeh/huterm/blob/main/docs/plans/ui-components.md

#![cfg(any(target_os = "macos", target_os = "linux"))]
#![deny(missing_docs)]

pub mod a11y;
pub mod animation;
pub mod click;
pub mod components;
pub mod focus;
pub mod list_scrollbar;
pub mod listbox;
pub mod scrollbar;
pub mod tokens;

#[cfg(all(test, feature = "test-support"))]
mod testing;
