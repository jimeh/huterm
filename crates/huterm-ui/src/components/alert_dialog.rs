//! Alert dialog: a modal question that interrupts and needs an answer,
//! such as a delete confirmation, or a refusal with one button that
//! acknowledges it, as `AppKit`'s alert sheets and libadwaita's
//! `AdwAlertDialog` are.
//!
//! Anatomy: the [`Dialog`] shell, with the alert's Cancel button and then
//! its actions in the footer, the most consequential action last. The
//! owner gives a destructive action [`Variant::Destructive`] and a refusal
//! only a Cancel button labelled to acknowledge it, such as OK.
//!
//! Focus: on opening, focus moves to Cancel, the least destructive answer,
//! or to the first enabled action when there is no Cancel or it is
//! disabled. An owner whose question expects the action, such as a close
//! confirmation that Return confirms, focuses that action instead with
//! [`AlertDialog::initial_focus`]. Tab and Shift-Tab cycle through Cancel
//! and the enabled actions, and closing returns focus as a [`Dialog`]'s
//! does.
//!
//! Keyboard: as a [`Dialog`]'s. The owner routes its cancel command
//! (Escape) to the same answer as Cancel.
//!
//! An owner with its own header, such as one with a severity mark beside
//! the title, makes the alert [`AlertDialog::bare`], as a [`Dialog`]; the
//! footer still holds Cancel and the actions.
//!
//! Dismissal: as a [`Dialog`]'s, except that a press on the scrim outside
//! the panel is stopped and never reported, so only an answer, Escape, or the owner
//! closes the alert.
//!
//! Accessibility: a modal `AlertDialog` named by its title and described by
//! its description.
//!
//! Reference: Radix's Alert Dialog and WAI-ARIA's alert dialog pattern.
//!
//! [`Variant::Destructive`]: crate::components::button::Variant::Destructive

use gpui::{
    AnyElement, App, Corners, ElementId, FocusHandle, IntoElement,
    ParentElement, Pixels, RenderOnce, SharedString, StyleRefinement, Styled,
    Window,
};

use crate::components::button::Button;
use crate::components::dialog::{Dialog, Modal};
use crate::tokens::Tokens;

/// An alert dialog's panel over its scrim, drawn while its [`Modal`] is
/// open.
#[derive(IntoElement)]
pub struct AlertDialog {
    dialog: Dialog,
    cancel: Option<(Button, FocusHandle)>,
    actions: Vec<(Button, FocusHandle)>,
}

impl std::fmt::Debug for AlertDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlertDialog")
            .field("dialog", &self.dialog)
            .finish_non_exhaustive()
    }
}

impl AlertDialog {
    /// An alert titled `title`, open while `modal` is.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        modal: &Modal,
        tokens: Tokens,
    ) -> Self {
        Self {
            dialog: Dialog::new(id, title, modal, tokens).alert(),
            cancel: None,
            actions: Vec::new(),
        }
    }

    /// A sentence under the title, such as what the action cannot undo,
    /// which also describes the alert to assistive technology.
    #[must_use]
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.dialog = self.dialog.description(text);
        self
    }

    /// The button that declines, focused through `focus`; it takes focus on
    /// opening and comes first in the footer.
    #[must_use]
    pub fn cancel(mut self, button: Button, focus: &FocusHandle) -> Self {
        self.cancel = Some((button, focus.clone()));
        self
    }

    /// An action, focused through `focus`, after Cancel and the actions
    /// already added.
    #[must_use]
    pub fn action(mut self, button: Button, focus: &FocusHandle) -> Self {
        self.actions.push((button, focus.clone()));
        self
    }

    /// The button that takes focus on opening instead of Cancel; skipped,
    /// as Cancel is, while disabled.
    #[must_use]
    pub fn initial_focus(mut self, focus: &FocusHandle) -> Self {
        self.dialog = self.dialog.initial_focus(focus);
        self
    }

    /// The owner draws the header and body, as [`Dialog::bare`] describes;
    /// the footer still holds Cancel and the actions.
    #[must_use]
    pub fn bare(mut self) -> Self {
        self.dialog = self.dialog.bare();
        self
    }

    /// Rounds the scrim's corners, as [`Dialog::scrim_corners`] describes.
    #[must_use]
    pub fn scrim_corners(mut self, corners: Corners<Pixels>) -> Self {
        self.dialog = self.dialog.scrim_corners(corners);
        self
    }
}

impl Styled for AlertDialog {
    fn style(&mut self) -> &mut StyleRefinement {
        self.dialog.style()
    }
}

impl ParentElement for AlertDialog {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.dialog.extend(elements);
    }
}

impl RenderOnce for AlertDialog {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        // Cancel comes first, so the dialog focuses it on opening.
        self.cancel
            .into_iter()
            .chain(self.actions)
            .fold(self.dialog, |dialog, (button, focus)| {
                dialog.button(button, &focus)
            })
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};

    use super::super::dialog::tests::{
        Kind, Probe, beneath, click, focused, owner, press_tab,
    };
    use crate::components::dialog::Dismissal;
    use crate::testing::a11y_node;

    fn alert(cx: &mut TestAppContext) -> (Probe, &mut VisualTestContext) {
        owner(cx, Kind::Alert, &[])
    }

    #[gpui::test]
    fn an_alert_opens_with_focus_on_cancel(cx: &mut TestAppContext) {
        // The host adds the action before Cancel; Cancel still takes focus.
        let (probe, cx) = alert(cx);
        probe.open(cx);
        assert_eq!(focused(&probe, cx), Some("cancel"));
    }

    #[gpui::test]
    fn an_alert_opens_on_the_action_its_owner_chose(cx: &mut TestAppContext) {
        let (probe, cx) = owner(cx, Kind::AlertOnAction, &[]);
        probe.open(cx);
        assert_eq!(focused(&probe, cx), Some("confirm"));
    }

    #[gpui::test]
    fn tab_cycles_between_cancel_and_the_action(cx: &mut TestAppContext) {
        let (probe, cx) = alert(cx);
        probe.open(cx);
        press_tab(cx, false);
        assert_eq!(focused(&probe, cx), Some("confirm"));
        press_tab(cx, false);
        assert_eq!(focused(&probe, cx), Some("cancel"));
        press_tab(cx, true);
        assert_eq!(focused(&probe, cx), Some("confirm"));
    }

    #[gpui::test]
    fn an_outside_press_never_dismisses_an_alert(cx: &mut TestAppContext) {
        // The host closes on every reason it receives.
        let (probe, cx) = alert(cx);
        probe.open(cx);
        click(cx, beneath());
        assert!(probe.is_open(cx));
        assert_eq!(probe.log(), Vec::<String>::new());
        assert_eq!(focused(&probe, cx), Some("cancel"));
    }

    #[gpui::test]
    fn escape_cancels_an_alert_and_returns_focus(cx: &mut TestAppContext) {
        let (probe, cx) = alert(cx);
        probe.open(cx);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(probe.log(), [format!("{:?}", Dismissal::Escape)]);
        assert!(!probe.is_open(cx));
        assert_eq!(focused(&probe, cx), Some("trigger"));
    }

    #[gpui::test]
    fn an_alert_reports_its_role_name_and_description(cx: &mut TestAppContext) {
        let (probe, cx) = alert(cx);
        probe.open(cx);
        cx.update(|window, _| {
            let aria = &a11y_node(window, "dialog")["aria"];
            assert_eq!(aria["role"], "AlertDialog");
            assert_eq!(aria["label"], "Delete profile?");
            assert_eq!(aria["description"], "This cannot be undone.");
            assert_eq!(aria["modal"], true);
        });
    }
}
