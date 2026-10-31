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
//! action stops, so no Settings command finds them. Each carries
//! [`DIALOG_BUTTON_CONTEXT`] instead, so Space's `dialog_confirm` binding
//! matches on a button and never in the dialog's field. `dialog_confirm`
//! answers with the focused button and `dialog_cancel` answers as the
//! prompt's Cancel does. While a dialog is open, the window runs no other
//! command except its own close, minimize, zoom, and fullscreen. A dialog
//! stays open while the window is inactive.
//!
//! A prompt may hold one text field, opened with
//! [`SettingsView::open_field_dialog`]. The field carries the `TextField`
//! context, as Settings' other fields do, so Left and Right edit it, and
//! [`SettingsView::run_dialog_field`] routes Enter (`text_confirm`) to the
//! prompt's confirm key and Escape (`text_cancel`) to its Cancel. The
//! context binds no Tab or Shift-Tab, so they reach the dialog's
//! `dialog_focus_*` and focus never leaves the dialog.

use std::collections::HashMap;

use gpui::{
    AnyElement, App, Context, ElementId, Entity, FocusHandle, Focusable as _,
    IntoElement, SharedString, Window,
};
use huterm_protocol::{CommandId, CommandInvocation, ids};
use huterm_ui::components::button::Button;
use huterm_ui::components::dialog::{Dismissal, Modal};
use huterm_ui::components::input::Input;

use super::super::super::overlay::Swatch;
use super::SettingsView;
use super::keybindings::{Taking, Target};
use crate::keymap::DIALOG_BUTTON_CONTEXT;

/// What the open dialog asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Prompt {
    /// Whether to delete the Quake profile.
    DeleteProfile(String),
    /// Why the Quake profile cannot be deleted.
    DeleteRefused { profile: String, message: String },
    /// The name of a new Quake profile, typed into the dialog's field.
    NewProfile,
    /// The Quake profile's new name, typed into the dialog's field.
    RenameProfile(String),
    /// Why the Quake profile was not renamed.
    RenameRefused { profile: String, message: String },
    /// The name of a new pane layout, typed into the dialog's field.
    NewLayout,
    /// The pane layout's new name, typed into the dialog's field.
    RenameLayout(String),
    /// Why the pane layout was not renamed.
    LayoutRenameRefused { layout: String, message: String },
    /// Whether to delete the pane layout.
    DeleteLayout(String),
    /// Why the pane layout cannot be deleted.
    LayoutDeleteRefused { layout: String, message: String },
    /// Why the layout cycle cannot lose the pane layout, its last member.
    CycleRefused(String),
    /// Whether a recorded shortcut, `key` for `target`, should take its
    /// key from what `taking` names.
    Reassign {
        target: Target,
        key: String,
        taking: Taking,
    },
}

impl Prompt {
    /// The action key that answers as Cancel does, which `dialog_cancel`
    /// runs too.
    fn cancel(&self) -> &'static str {
        match self {
            Self::DeleteProfile(_) | Self::DeleteRefused { .. } => "qdelno",
            Self::NewProfile => "qcancel",
            Self::RenameProfile(_) | Self::RenameRefused { .. } => "qrenno",
            Self::NewLayout => "lcancel",
            Self::RenameLayout(_) | Self::LayoutRenameRefused { .. } => {
                "lrenno"
            }
            Self::DeleteLayout(_) | Self::LayoutDeleteRefused { .. } => {
                "ldelno"
            }
            Self::CycleRefused(_) => "lcycno",
            Self::Reassign { .. } => "kreno",
        }
    }

    /// The action key that Enter in the prompt's field runs, as its
    /// primary button does.
    fn confirm(&self) -> Option<&'static str> {
        match self {
            Self::NewProfile => Some("qcreate"),
            Self::RenameProfile(_) => Some("qrename"),
            Self::NewLayout => Some("lcreate"),
            Self::RenameLayout(_) => Some("lrename"),
            Self::DeleteProfile(_)
            | Self::DeleteRefused { .. }
            | Self::RenameRefused { .. }
            | Self::LayoutRenameRefused { .. }
            | Self::DeleteLayout(_)
            | Self::LayoutDeleteRefused { .. }
            | Self::CycleRefused(_)
            | Self::Reassign { .. } => None,
        }
    }

    /// What smoke state calls it.
    fn name(&self) -> &'static str {
        match self {
            Self::DeleteProfile(_) => "delete",
            Self::DeleteRefused { .. } => "delete-refused",
            Self::NewProfile => "new-profile",
            Self::RenameProfile(_) => "rename",
            Self::RenameRefused { .. } => "rename-refused",
            Self::NewLayout => "new-layout",
            Self::RenameLayout(_) => "rename-layout",
            Self::LayoutRenameRefused { .. } => "rename-layout-refused",
            Self::DeleteLayout(_) => "delete-layout",
            Self::LayoutDeleteRefused { .. } => "delete-layout-refused",
            Self::CycleRefused(_) => "cycle-refused",
            Self::Reassign { .. } => "reassign-key",
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
    /// The open prompt's text field, if it has one.
    field: Option<Entity<Input>>,
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

    /// The open prompt's text field.
    pub(super) fn field(&self) -> Option<&Entity<Input>> {
        self.field.as_ref().filter(|_| self.prompt().is_some())
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
        self.dialog_from = self
            .actions
            .iter()
            .find(|(_, handle)| *handle == return_to)
            .map(|(key, _)| key.clone());
        self.dialog.prompt = Some(prompt);
        self.dialog.field = None;
        self.dialog.modal.open(return_to);
    }

    /// Opens `prompt` with `field` in its body, as [`Self::open_dialog`]
    /// does; the prompt's renderer draws the field and gives it focus.
    pub(super) fn open_field_dialog(
        &mut self,
        prompt: Prompt,
        field: Entity<Input>,
        return_to: &FocusHandle,
    ) {
        self.open_dialog(prompt, return_to);
        self.dialog.field = Some(field);
    }

    /// Closes the dialog and returns focus to the target given to
    /// [`Self::open_dialog`].
    pub(super) fn close_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.dialog.prompt = None;
        self.dialog.field = None;
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
            .key_context(DIALOG_BUTTON_CONTEXT)
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

    /// Takes a text-editing command while the open dialog's field has
    /// focus: Enter runs the prompt's confirm key, Escape answers as Cancel
    /// does, and every other command edits the field. Returns whether it took the command; any command
    /// while a dialog is open is taken, since nothing it covers may edit.
    pub(super) fn run_dialog_field(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(prompt) = self.dialog.prompt().cloned() else {
            return false;
        };
        let Some(field) = self.focused_dialog_field(window, cx) else {
            return true;
        };
        match invocation.id {
            ids::TEXT_CONFIRM => {
                if let Some(key) = prompt.confirm() {
                    self.act(key, window, cx);
                }
            }
            ids::TEXT_CANCEL => {
                self.dismiss_dialog(Dismissal::Escape, window, cx);
            }
            _ => {
                let _ = field.update(cx, |field, cx| {
                    crate::ui::input::run(field, invocation, cx)
                });
            }
        }
        cx.notify();
        true
    }

    /// The open dialog's field, when it has focus.
    pub(super) fn focused_dialog_field(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<Entity<Input>> {
        self.dialog
            .field()
            .filter(|field| field.read(cx).focus_handle(cx).is_focused(window))
            .cloned()
    }

    /// Answers the dialog as its Cancel does, for `dialog_cancel` or an
    /// owner change. An outside press leaves it open, keeping a typed name,
    /// as a macOS sheet does; an alert never reports one. The window does
    /// not route its deactivation here.
    pub(super) fn dismiss_dialog(
        &mut self,
        reason: Dismissal,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if reason == Dismissal::OutsidePress {
            return;
        }
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
            Prompt::NewProfile | Prompt::RenameProfile(_) => {
                self.render_name_dialog(&prompt, swatch, window, cx)
            }
            Prompt::RenameRefused { profile, message } => {
                Some(self.render_rename_refusal(&profile, message, swatch, cx))
            }
            Prompt::NewLayout | Prompt::RenameLayout(_) => {
                self.render_layout_name_dialog(&prompt, swatch, window, cx)
            }
            Prompt::LayoutRenameRefused { .. }
            | Prompt::DeleteLayout(_)
            | Prompt::LayoutDeleteRefused { .. }
            | Prompt::CycleRefused(_) => {
                self.render_layout_alert(&prompt, swatch, window, cx)
            }
            Prompt::Reassign {
                target,
                key,
                taking,
            } => Some(self.render_reassign(
                &target,
                &key,
                &taking.from,
                swatch,
                cx,
            )),
        }
    }

    /// The section the window shows, for tests that check nothing moved
    /// it behind a dialog.
    #[cfg(all(test, feature = "test-support"))]
    pub(in crate::desktop::windows) fn shown_section(
        &self,
    ) -> huterm_config::settings::SectionId {
        self.section
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

    /// The open dialog's field's caret and selection, as byte offsets.
    pub(in crate::desktop::windows) fn dialog_caret(
        &self,
        cx: &App,
    ) -> Option<(usize, std::ops::Range<usize>)> {
        let field = self.dialog.field()?.read(cx);
        Some((field.cursor(), field.selection()))
    }

    /// The number of writes that finished, whatever their outcome.
    pub(in crate::desktop::windows) fn writes_finished(&self) -> u64 {
        self.epoch
    }

    /// The open dialog panel's painted bounds.
    pub(in crate::desktop::windows) fn dialog_panel(
        &self,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        self.dialog.modal.panel_bounds()
    }
}
