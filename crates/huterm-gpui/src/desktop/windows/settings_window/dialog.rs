//! The Settings window's dialog: at most one `huterm-ui` dialog or alert
//! dialog at a time, drawn over the whole window, with the `dialog_*`
//! commands that reach it under `confirming` and its buttons' focus.
//!
//! A section opens a [`Prompt`] with [`SettingsView::open_dialog`] and draws
//! it from [`SettingsView::render_dialog`]. Its footer buttons come from
//! [`SettingsView::dialog_button`] and answer with action keys, which
//! `act` runs as it runs the window's other buttons. They carry no
//! `SettingsControl` context, which would win over `confirming` for Enter,
//! Space, and the arrows, and their focus handles stay out of the window's
//! action stops, so no Settings command finds them. `dialog_confirm`
//! answers with the focused button and `dialog_cancel` answers as the
//! prompt's Cancel does. While a dialog is open, the window runs no other
//! command except its own close, minimize, zoom, and fullscreen. A dialog
//! stays open while the window is inactive.

use std::collections::HashMap;

use gpui::{
    AnyElement, Context, ElementId, FocusHandle, IntoElement, SharedString,
    Window,
};
use huterm_protocol::{CommandId, ids};
use huterm_ui::components::button::Button;
use huterm_ui::components::dialog::{Dismissal, Modal};

use super::super::super::overlay::Swatch;
use super::SettingsView;

/// What the open dialog asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Prompt {
    /// Whether to delete the Quake profile.
    DeleteProfile(String),
    /// Why the Quake profile cannot be deleted.
    DeleteRefused { profile: String, message: String },
}

impl Prompt {
    /// The action key that answers as Cancel does, which `dialog_cancel`
    /// runs too.
    fn cancel(&self) -> &'static str {
        match self {
            Self::DeleteProfile(_) | Self::DeleteRefused { .. } => "qdelno",
        }
    }

    /// What smoke state calls it.
    fn name(&self) -> &'static str {
        match self {
            Self::DeleteProfile(_) => "delete",
            Self::DeleteRefused { .. } => "delete-refused",
        }
    }
}

/// The window's dialog slot.
#[derive(Default)]
pub(super) struct SettingsDialog {
    modal: Modal,
    prompt: Option<Prompt>,
    /// Footer buttons' focus handles, by the action key each answers with.
    buttons: HashMap<&'static str, FocusHandle>,
}

impl SettingsDialog {
    /// The open dialog's prompt.
    pub(super) fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref().filter(|_| self.modal.is_open())
    }

    /// The modal the open dialog draws with.
    pub(super) fn modal(&self) -> &Modal {
        &self.modal
    }

    /// The action key of the footer button that has focus.
    fn focused(&self, window: &Window) -> Option<&'static str> {
        self.buttons
            .iter()
            .find(|(_, handle)| handle.is_focused(window))
            .map(|(key, _)| *key)
    }
}

impl SettingsView {
    /// Opens `prompt` over the window, closing any open menu first. Focus
    /// moves to the dialog on its first frame and returns to `return_to`
    /// when it closes. An open dialog shows `prompt` in its place, and
    /// focus moves to its initial control again.
    pub(super) fn open_dialog(
        &mut self,
        prompt: Prompt,
        return_to: &FocusHandle,
    ) {
        let _ = self.drop_menu();
        self.dialog.prompt = Some(prompt);
        self.dialog.modal.open(return_to);
    }

    /// Closes the dialog and returns focus to the target given to
    /// [`Self::open_dialog`].
    pub(super) fn close_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.dialog.prompt = None;
        if self.dialog.modal.close(window, cx) {
            cx.notify();
        }
    }

    /// A dialog footer button reading `label` that answers with the action
    /// `key` on a pointer click, or on `dialog_confirm` while it has focus,
    /// with the handle the dialog focuses it through.
    pub(super) fn dialog_button(
        &mut self,
        id: &'static str,
        label: impl Into<SharedString>,
        key: &'static str,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> (Button, FocusHandle) {
        let focus = self
            .dialog
            .buttons
            .entry(key)
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let act = Self::listen(cx, move |view, (), window, cx| {
            view.act(key, window, cx);
        });
        let button = Button::new(ElementId::Name(id.into()), label, swatch)
            .on_click(move |window, cx| act((), window, cx));
        (button, focus)
    }

    /// Takes a command while a dialog is open: the `dialog_*` commands
    /// reach it, and every other command except the window's own close,
    /// minimize, zoom, and fullscreen does nothing. Returns whether it took
    /// the command.
    pub(super) fn run_dialog(
        &mut self,
        id: CommandId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if self.dialog.prompt().is_none() {
            return false;
        }
        match id {
            ids::DIALOG_FOCUS_NEXT => self.dialog.modal.focus_next(window, cx),
            ids::DIALOG_FOCUS_PREVIOUS => {
                self.dialog.modal.focus_previous(window, cx);
            }
            ids::DIALOG_CONFIRM => {
                if let Some(key) = self.dialog.focused(window) {
                    self.act(key, window, cx);
                }
            }
            ids::DIALOG_CANCEL => {
                self.dismiss_dialog(Dismissal::Escape, window, cx);
            }
            ids::CLOSE_WINDOW
            | ids::CLOSE_TAB
            | ids::CLOSE_PANE
            | ids::MINIMIZE
            | ids::ZOOM
            | ids::TOGGLE_FULLSCREEN
            | ids::TOGGLE_NATIVE_FULLSCREEN => return false,
            _ => {}
        }
        cx.notify();
        true
    }

    /// Answers the dialog as its Cancel does, for `dialog_cancel`, an owner
    /// change, or an outside press, which an alert never reports. The
    /// window does not route its deactivation here.
    pub(super) fn dismiss_dialog(
        &mut self,
        _reason: Dismissal,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(key) = self.dialog.prompt().map(Prompt::cancel) {
            self.act(key, window, cx);
        }
    }

    /// The dialog's capture-phase press and wheel guard, which the window
    /// mounts on every frame, open or not.
    pub(super) fn dialog_guard(
        &self,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement + use<> {
        let weak = cx.entity().downgrade();
        self.dialog.modal.guard(move |reason, window, cx| {
            let _ = weak.update(cx, |view, cx| {
                view.dismiss_dialog(reason, window, cx);
            });
        })
    }

    /// The open dialog, drawn as the window's last child.
    pub(super) fn render_dialog(
        &mut self,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let prompt = self.dialog.prompt()?.clone();
        match prompt {
            Prompt::DeleteProfile(_) | Prompt::DeleteRefused { .. } => {
                self.render_delete_alert(&prompt, swatch, window, cx)
            }
        }
    }

    /// The open dialog's smoke name, or `none`.
    pub(in crate::desktop::windows) fn dialog_name(&self) -> &'static str {
        self.dialog.prompt().map_or("none", Prompt::name)
    }

    /// The action key of the dialog button that has focus.
    pub(super) fn focused_dialog_button(
        &self,
        window: &Window,
    ) -> Option<&'static str> {
        self.dialog.prompt()?;
        self.dialog.focused(window)
    }
}

/// What desktop tests drive and observe in the dialog.
#[cfg(all(test, feature = "test-support"))]
impl SettingsView {
    /// Opens the delete alert as Delete profile does, without asking for
    /// a frame, so the window still shows the frame before it.
    pub(in crate::desktop::windows) fn open_delete_alert_unseen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let _ = self.act_quake("qdel", "", window, cx);
    }

    /// The open dialog panel's painted bounds.
    pub(in crate::desktop::windows) fn dialog_panel(
        &self,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        self.dialog.modal.panel_bounds()
    }
}
