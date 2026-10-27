//! Accessibility states GPUI has no setter for.
//!
//! GPUI's `aria_*` setters cover label, description, value, selection,
//! expansion, toggles, and numeric ranges. Disabled, read-only, invalid,
//! modal, and live-region states have none, so [`States::apply`] writes them
//! onto the element's own AccessKit node through
//! `a11y_synthetic_children`, which hands the closure that node while it is
//! being built. An element takes one such closure, so a component that
//! needs these states sets them all through one [`States`].
//!
//! GPUI reports a node only for an element with an id and a role, so apply
//! the states after `id` and `role`.

use gpui::StatefulInteractiveElement;
use gpui::accesskit::{Invalid, Live};

/// The states one element reports beyond GPUI's setters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent AccessKit state"
)]
pub struct States {
    /// The control cannot be used.
    pub disabled: bool,
    /// The control's value can be read and selected but not changed.
    pub read_only: bool,
    /// The control's value fails validation.
    pub invalid: bool,
    /// The element is a modal dialog: assistive technology confines itself
    /// to it.
    pub modal: bool,
    /// The element announces changes to its content, politely.
    pub live: bool,
}

impl States {
    /// Whether every state is off, so applying them would change nothing.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    /// Writes these states onto `element`'s accessibility node.
    #[must_use]
    pub fn apply<E: StatefulInteractiveElement>(self, element: E) -> E {
        if self.is_empty() {
            return element;
        }
        element.a11y_synthetic_children(move |builder| {
            let node = builder.parent_node();
            if self.disabled {
                node.set_disabled();
            }
            if self.read_only {
                node.set_read_only();
            }
            if self.invalid {
                node.set_invalid(Invalid::True);
            }
            if self.modal {
                node.set_modal();
            }
            if self.live {
                node.set_live(Live::Polite);
            }
        })
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{TestAppContext, div, prelude::*};

    use super::States;
    use crate::testing::{a11y_action, a11y_node, host};

    #[gpui::test]
    fn test_windows_report_an_accessibility_tree(cx: &mut TestAppContext) {
        let (_, cx) = host(cx, |_, _| {
            div()
                .id("canary")
                .role(gpui::Role::Button)
                .aria_label("Canary")
                .into_any_element()
        });
        cx.update(|window, _| {
            let node = a11y_node(window, "canary");
            assert_eq!(node["aria"]["role"], "Button");
            assert_eq!(node["aria"]["label"], "Canary");
        });
    }

    #[gpui::test]
    fn tests_can_send_accessibility_actions(cx: &mut TestAppContext) {
        let clicks = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = clicks.clone();
        let (_, cx) = host(cx, move |_, _| {
            let counted = counted.clone();
            div()
                .id("canary")
                .role(gpui::Role::Button)
                .size(gpui::px(20.0))
                .on_click(move |_, _, _| counted.set(counted.get() + 1))
                .into_any_element()
        });
        a11y_action(cx, "canary", gpui::AccessibleAction::Click);
        assert_eq!(clicks.get(), 1);
    }

    #[gpui::test]
    fn states_reach_the_element_node(cx: &mut TestAppContext) {
        let (_, cx) = host(cx, |_, _| {
            States {
                disabled: true,
                read_only: true,
                invalid: true,
                modal: true,
                live: true,
            }
            .apply(div().id("stated").role(gpui::Role::Dialog))
            .into_any_element()
        });
        cx.update(|window, _| {
            let aria = &a11y_node(window, "stated")["aria"];
            assert_eq!(aria["disabled"], true);
            assert_eq!(aria["read_only"], true);
            assert_eq!(aria["invalid"], "True");
            assert_eq!(aria["modal"], true);
            assert_eq!(aria["live"], "Polite");
        });
    }
}
