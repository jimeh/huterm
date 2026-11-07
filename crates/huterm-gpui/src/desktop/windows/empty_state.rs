//! The empty state (EM1): what a window draws in its pane area while the
//! workspace it shows holds no tab, such as a workspace the user selected
//! empty or one whose last tab closed while it was the session's last with
//! tabs.
//!
//! It draws the workspace icon, "No tabs in `name`", a primary New Tab
//! button with its shortcut, Switch Workspace… while the session has other
//! workspaces, and Close Workspace while the workspace is not the session's
//! only one. Switch Workspace… opens the `select_workspace` palette prompt,
//! so windows without a title row to host a workspace control are never
//! stuck. The tab bar hides as it does for any window without tabs.
//!
//! Keyboard: New Tab takes focus when the state appears; the buttons take
//! the `Panel` key context, so Tab and Shift-Tab cycle them and Enter and
//! Space press the focused one, as in the transparency panel.

use gpui::{
    AnyElement, App, Bounds, Context, FocusHandle, IntoElement, ParentElement,
    Pixels, Styled, Window, div, prelude::*, px,
};
use huterm_protocol::{CommandId, CommandInvocation, ids};
use huterm_ui::components::button::{Button, Variant};
use huterm_ui::components::empty::Empty;

use super::tab_bar::icon_element;
use super::window_menu::shortcut_key;
use super::{
    CloseTarget, CommandError, CommandOutcome, Desktop, EmptiedClose,
    WorkspaceView, terminating,
};
use crate::assets::Icon;
use crate::desktop::key_hint::KeyHint;
use crate::desktop::overlay::Swatch;
use crate::keymap::{PANEL_BUTTON_CONTEXT, Platform};

/// One of the empty state's buttons.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Control {
    NewTab,
    SwitchWorkspace,
    CloseWorkspace,
}

/// The empty state's focus handles.
pub(super) struct EmptyState {
    new_tab: FocusHandle,
    switch: FocusHandle,
    close: FocusHandle,
}

impl EmptyState {
    pub(super) fn new(cx: &mut App) -> Self {
        Self {
            new_tab: cx.focus_handle().tab_stop(true),
            switch: cx.focus_handle().tab_stop(true),
            close: cx.focus_handle().tab_stop(true),
        }
    }

    fn handle(&self, control: Control) -> &FocusHandle {
        match control {
            Control::NewTab => &self.new_tab,
            Control::SwitchWorkspace => &self.switch,
            Control::CloseWorkspace => &self.close,
        }
    }

    /// The button with keyboard focus, if any.
    fn focused(&self, window: &Window) -> Option<Control> {
        [
            Control::NewTab,
            Control::SwitchWorkspace,
            Control::CloseWorkspace,
        ]
        .into_iter()
        .find(|control| self.handle(*control).is_focused(window))
    }
}

/// What the empty state shows for the workspace a window shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct EmptyStateModel {
    pub(super) name: String,
    /// The session has other workspaces to switch to.
    pub(super) switch: bool,
    /// The workspace is not the session's only one.
    pub(super) close: bool,
}

impl EmptyStateModel {
    /// The buttons drawn, in focus order.
    pub(super) fn controls(&self) -> Vec<Control> {
        let mut controls = vec![Control::NewTab];
        if self.switch {
            controls.push(Control::SwitchWorkspace);
        }
        if self.close {
            controls.push(Control::CloseWorkspace);
        }
        controls
    }

    pub(super) fn title(&self) -> String {
        format!("No tabs in {}", self.name)
    }
}

impl WorkspaceView {
    /// The empty state this window shows, if any: its workspace belongs to
    /// its session and holds no tab, nothing installs or closes, and the
    /// window owes no emptied close. A window that is about to close, or
    /// waits for its first shell, shows none.
    pub(super) fn empty_state_model(
        &self,
        cx: &App,
    ) -> Option<EmptyStateModel> {
        if !self.tabs.is_empty()
            || self.busy
            || self.switch.is_some()
            || self.reopen.is_some()
            || self.close.current.is_some()
            || self.close.emptied != EmptiedClose::None
            || terminating(cx)
        {
            return None;
        }
        let record = self
            .record(cx)
            .filter(|record| !record.closing && record.attachment.is_some())?;
        let (session, workspace) = record.session.zip(record.workspace)?;
        let state = cx.global::<Desktop>().hierarchy.state();
        if state.workspace_session(workspace) != Some(session)
            || !self.has_nothing_to_install(cx)
        {
            return None;
        }
        let siblings = state.session_workspaces(session).unwrap_or_default();
        Some(EmptyStateModel {
            name: state.workspace(workspace)?.display_name().to_owned(),
            switch: siblings.len() > 1,
            close: siblings.len() > 1,
        })
    }

    /// Moves keyboard focus to New Tab when the empty state shows and no
    /// overlay holds focus.
    pub(super) fn focus_empty_state(&self, window: &mut Window, cx: &mut App) {
        if !self.overlay_holds_focus() && self.empty_state_model(cx).is_some() {
            self.empty_state.new_tab.clone().focus(window, cx);
        }
    }

    /// Runs a `panel_*` command on the empty state when one of its
    /// buttons holds focus; `None` leaves the command to the open panel.
    pub(super) fn run_empty_state_command(
        &mut self,
        command: CommandId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<Result<CommandOutcome, CommandError>> {
        let focused = self.empty_state.focused(window)?;
        let controls = self.empty_state_model(cx)?.controls();
        match command {
            ids::PANEL_FOCUS_NEXT | ids::PANEL_FOCUS_PREVIOUS => {
                let at = controls
                    .iter()
                    .position(|control| *control == focused)
                    .unwrap_or(0);
                let next = if command == ids::PANEL_FOCUS_NEXT {
                    (at + 1) % controls.len()
                } else {
                    (at + controls.len() - 1) % controls.len()
                };
                self.empty_state
                    .handle(controls[next])
                    .clone()
                    .focus(window, cx);
                cx.notify();
            }
            ids::PANEL_PRESS => self.press_empty_state(focused, window, cx),
            _ => {}
        }
        Some(Ok(CommandOutcome::Completed))
    }

    /// The New Tab, Switch Workspace…, and Close Workspace focus handles.
    #[cfg(test)]
    pub(super) fn empty_state_handles(
        &self,
    ) -> (&FocusHandle, &FocusHandle, &FocusHandle) {
        (
            &self.empty_state.new_tab,
            &self.empty_state.switch,
            &self.empty_state.close,
        )
    }

    /// Whether a `panel_*` command reaches the empty state.
    pub(super) fn empty_state_focused(&self, window: &Window) -> bool {
        self.empty_state.focused(window).is_some()
    }

    /// What a click on a button, or `panel_press` on it, does.
    fn press_empty_state(
        &mut self,
        control: Control,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let result = match control {
            Control::NewTab => self.new_tab(window, cx),
            Control::SwitchWorkspace => self.invoke_interactive(
                &CommandInvocation::new(ids::SELECT_WORKSPACE, Vec::new()),
                window,
                cx,
            ),
            Control::CloseWorkspace => {
                match self.close_workspace_target(None, cx) {
                    Ok(workspace) => {
                        self.request_close(
                            CloseTarget::Workspace(workspace),
                            window,
                            cx,
                        );
                        Ok(CommandOutcome::Accepted)
                    }
                    Err(error) => Err(error),
                }
            }
        };
        if let Err(error) = result {
            let title = match control {
                Control::NewTab => "Cannot open tab",
                Control::SwitchWorkspace => "Cannot switch workspace",
                Control::CloseWorkspace => "Cannot close workspace",
            };
            self.report_failure(title, error.to_string(), cx);
        }
    }

    /// The empty state's element over the pane area `bounds`, or `None`
    /// while the window shows tabs.
    pub(super) fn empty_state_element(
        &self,
        bounds: Bounds<Pixels>,
        swatch: Swatch,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let model = self.empty_state_model(cx)?;
        let keymap = &cx.global::<Desktop>().keymap;
        let hint =
            shortcut_key(keymap, &[self.key_context(window)], ids::NEW_TAB)
                .map(|key| KeyHint::parse(&key, Platform::current()));
        let button = |control: Control, label: &str, cx: &Context<'_, Self>| {
            Button::new(
                match control {
                    Control::NewTab => "empty-new-tab",
                    Control::SwitchWorkspace => "empty-switch-workspace",
                    Control::CloseWorkspace => "empty-close-workspace",
                },
                label.to_owned(),
                swatch,
            )
            .focus(self.empty_state.handle(control))
            .key_context(PANEL_BUTTON_CONTEXT)
            .on_click(Self::button_action(
                cx,
                move |view, window, cx| {
                    view.press_empty_state(control, window, cx);
                },
            ))
        };
        let mut empty = Empty::new("empty-state", model.title(), swatch)
            .media(icon_element(Icon::LayoutGrid, swatch.muted).size(px(24.0)))
            .child(
                button(Control::NewTab, "New Tab", cx)
                    .variant(Variant::Primary)
                    .when_some(hint, |button, hint| {
                        button.shortcut(hint.glyphs())
                    }),
            );
        if model.switch {
            empty = empty.child(button(
                Control::SwitchWorkspace,
                "Switch Workspace\u{2026}",
                cx,
            ));
        }
        if model.close {
            empty = empty.child(button(
                Control::CloseWorkspace,
                "Close Workspace",
                cx,
            ));
        }
        Some(
            div()
                .absolute()
                .left(bounds.origin.x)
                .top(bounds.origin.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .flex()
                .items_center()
                .justify_center()
                .child(empty)
                .into_any_element(),
        )
    }
}
