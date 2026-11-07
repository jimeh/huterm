//! The title bar's session control and the session menu it opens.
//!
//! The control is a `huterm_ui` [`MenuTrigger`] that names the window's
//! session. Where it goes and how wide it is resolve once per frame into a
//! [`SessionControlPlacement`], which rendering, the tab strip's trailing
//! slots, title-row hit testing, pane-drop exclusions, menu anchoring, and
//! smoke state share. A click or `open_session_menu` opens the session
//! menu, a [`MenuSpec`] like the window menu's, so it honours
//! `macos_native_menus`. Huterm's popover reads the projection, so names,
//! counts, and window tags update while it is open; "exited" tags arrive
//! from a worker read of terminal status started when the menu opens. A
//! native menu is a snapshot, so it opens once that read answers.

use gpui::{Hsla, TextRun};
use huterm_ui::components::menu_trigger::{self, MenuTrigger};
use huterm_ui::typography;

use super::session_menu::{
    SessionControlState, SessionMenuInput, SessionPick, parse_pick, resolve,
    session_menu, session_rows,
};
use super::window_menu::{
    SESSION_CONTROL_MAX, SessionControlHost, SessionControlPlacement,
    session_control_host, shortcut_key,
};
use super::*;
use huterm_config::SessionButtonPlacement;

/// Which of the control's hosts a control is drawn in.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum SessionHostSlot {
    /// The title row, tab bar end, or new-tab row.
    #[default]
    Title,
    /// The workspace panel's first row or the bar's leading slot.
    Sidebar,
}

/// The session name the control last measured, and its width at the
/// control's text size.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct SessionLabel {
    pub(super) text: String,
    pub(super) width: Pixels,
}

/// Where the window's top-right controls start: the menu button's slot
/// and the session control's, when the title strip holds them.
pub(super) fn title_strip_corner(
    menu: MenuButtonPlacement,
    session: SessionControlPlacement,
) -> Pixels {
    let menu = if menu == MenuButtonPlacement::TitleStrip {
        CONTROL_SLOT
    } else {
        px(0.0)
    };
    let session = if session.host == SessionControlHost::TitleStrip {
        session.slot()
    } else {
        px(0.0)
    };
    menu + session
}

impl WorkspaceView {
    /// Whether the session menu is open, Huterm's or native, or a native
    /// one waits for its exited read.
    pub(super) fn session_menu_open(&self) -> bool {
        self.menu
            .as_ref()
            .is_some_and(|menu| menu.kind == MenuKind::Session)
            || self.native_menu_kind() == Some(&MenuKind::Session)
            || self.native_session_pending.is_some()
    }

    /// What the control shows, or `None` while the projection does not
    /// hold the window's session.
    pub(super) fn session_control_state(
        &self,
        cx: &App,
    ) -> Option<SessionControlState> {
        SessionControlState::derive(
            cx.global::<Desktop>().hierarchy.state(),
            self.attached_session(cx),
            self.session_menu_open(),
        )
    }

    /// Measures the session's name at the control's text size, so layout
    /// outside render reserves the control's slot. Render calls it before
    /// laying out, as it measures tab titles.
    pub(super) fn measure_session_label(&mut self, window: &Window, cx: &App) {
        let Some(state) = self.session_control_state(cx) else {
            self.session_label = SessionLabel::default();
            return;
        };
        let size = typography::ui(menu_trigger::TEXT_SIZE)
            .to_pixels(window.rem_size());
        let mut font = window.text_style().font();
        font.family = Typography::of(cx).family;
        let run = TextRun {
            len: state.name.len(),
            font,
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let width = window
            .text_system()
            .layout_line(&state.name, size, &[run], None)
            .width;
        self.session_label = SessionLabel {
            text: state.name,
            width,
        };
    }

    /// The control's title-bar host for this window, with the bar shown or
    /// not. `sidebar` placement uses it only while the window shows no
    /// workspace panel or bar (F1).
    fn session_host(
        &self,
        layout: &ChromeLayout,
        bar_shown: bool,
    ) -> SessionControlHost {
        let title = match self.config.window.session_button_placement {
            SessionButtonPlacement::TitleBar | SessionButtonPlacement::Both => {
                true
            }
            SessionButtonPlacement::Sidebar => {
                layout.workspace_chrome.edge().is_none()
            }
        };
        session_control_host(
            self.config.window.session_button && title,
            self.title_row() > px(0.0),
            self.corner_tab_position(),
            bar_shown,
        )
    }

    /// Whether the workspace panel or bar hosts the control: `sidebar` or
    /// `both` placement while one shows.
    pub(super) fn session_in_sidebar(&self, layout: &ChromeLayout) -> bool {
        self.config.window.session_button
            && self.config.window.session_button_placement
                != SessionButtonPlacement::TitleBar
            && layout.workspace_chrome.edge().is_some()
    }

    /// The control in the workspace panel's first row or the bar's leading
    /// slot, or `None` while the window shows neither or the placement
    /// keeps it in the title bar.
    pub(super) fn sidebar_session_element(
        &self,
        layout: &ChromeLayout,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        if !self.session_in_sidebar(layout) {
            return None;
        }
        let edge = layout.workspace_chrome.edge()?;
        let band = layout.workspace?;
        let width = if edge.vertical() {
            band.size.width - CONTROL_INSET * 4.0
        } else {
            (band.size.width * 0.25).min(SESSION_CONTROL_MAX)
        };
        let placement = SessionControlPlacement::resolve(
            SessionControlHost::TitleStrip,
            width,
            self.session_label.width,
            true,
        );
        let control = self.session_trigger(
            placement,
            SessionHostSlot::Sidebar,
            colors,
            cx,
        )?;
        Some(if edge.vertical() {
            div()
                .flex()
                .flex_none()
                .items_center()
                .h(CONTROL_SLOT)
                .px(CONTROL_INSET * 2.0)
                .child(control)
                .into_any_element()
        } else {
            control.into_any_element()
        })
    }

    /// The control in its host as the tab bar's geometry sees it: the bar
    /// reserves its slot whether or not an overlay bar is revealed, as it
    /// does the menu button's.
    pub(super) fn bar_session_placement(
        &self,
        layout: &ChromeLayout,
    ) -> SessionControlPlacement {
        self.resolve_session_placement(layout, self.session_host(layout, true))
    }

    /// The control as this frame draws it.
    pub(super) fn session_control_placement(
        &self,
        layout: &ChromeLayout,
    ) -> SessionControlPlacement {
        let presentation = self.presentation();
        let bar_shown = presentation == Presentation::Reserved
            || presentation == Presentation::Overlay
                && self.reveal.progress > 0.0;
        self.resolve_session_placement(
            layout,
            self.session_host(layout, bar_shown),
        )
    }

    /// Sizes the control for `host` from the room the host leaves it.
    fn resolve_session_placement(
        &self,
        layout: &ChromeLayout,
        host: SessionControlHost,
    ) -> SessionControlPlacement {
        // The menu button keeps the rightmost slot of the same host.
        let menu = if self.config.window.menu_button {
            CONTROL_SLOT
        } else {
            px(0.0)
        };
        let tabs = self.layout_tabs();
        let (room, whole) = match host {
            SessionControlHost::Hidden | SessionControlHost::SplitRow => {
                (px(0.0), false)
            }
            // The strip less `+` and the menu button.
            SessionControlHost::BarEnd => (
                layout.strip_bounds(tabs).size.width - CONTROL_SLOT - menu,
                false,
            ),
            // A right workspace panel's top strip, beside the menu button.
            SessionControlHost::TitleStrip if self.panel_corners(layout).1 => (
                layout.workspace.map_or(px(0.0), |band| band.size.width)
                    - menu
                    - CONTROL_INSET,
                true,
            ),
            // A right column's top strip, beside the menu button: the
            // control truncates to the column.
            SessionControlHost::TitleStrip
                if tabs.position == TabPosition::Right && self.bar_drawn() =>
            {
                (layout.tabs.size.width - menu - CONTROL_INSET, true)
            }
            // The title row less what starts and ends it.
            SessionControlHost::TitleStrip => {
                let (left, width) = if tabs.position.vertical() {
                    (layout.terminal.origin.x, layout.terminal.size.width)
                } else {
                    (layout.content.origin.x, layout.content.size.width)
                };
                let leading = if layout.frame.controls {
                    layout.frame.buttons.leading.width()
                } else {
                    TRAFFIC_LIGHT_INSET
                };
                let lead =
                    (layout.content.origin.x + leading - left).max(px(0.0));
                (
                    width - lead - layout.frame.buttons.trailing.width() - menu,
                    false,
                )
            }
        };
        SessionControlPlacement::resolve(
            host,
            room,
            self.session_label.width,
            whole,
        )
    }

    /// The session control at `placement`, or `None` while it is hidden or
    /// the window shows no session. The caller positions it. It records
    /// its painted bounds for the menu's anchor and never holds focus.
    pub(super) fn session_control_element(
        &self,
        placement: SessionControlPlacement,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> Option<MenuTrigger> {
        self.session_trigger(placement, SessionHostSlot::Title, colors, cx)
    }

    /// The session control for one of its hosts, which records its own
    /// painted bounds and names itself when it opens the menu.
    fn session_trigger(
        &self,
        placement: SessionControlPlacement,
        slot: SessionHostSlot,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> Option<MenuTrigger> {
        if placement.host == SessionControlHost::Hidden {
            return None;
        }
        let state = self.session_control_state(cx)?;
        let bounds_cell = match slot {
            SessionHostSlot::Title => {
                self.session_control_drawn.set(true);
                Rc::clone(&self.session_button_bounds)
            }
            SessionHostSlot::Sidebar => {
                self.session_sidebar_drawn.set(true);
                Rc::clone(&self.session_sidebar_bounds)
            }
        };
        let swatch = tokens(&self.config.theme);
        let accessible = state.accessible_name();
        let tooltip = placement.truncated.then(|| state.name.clone().into());
        let open = state.open && self.session_menu_from == slot;
        let trigger = MenuTrigger::new(
            match slot {
                SessionHostSlot::Title => "session-control",
                SessionHostSlot::Sidebar => "session-control-sidebar",
            },
            accessible,
            state.name,
            Icon::Layers.asset_path(),
            Icon::ChevronDown.asset_path(),
            swatch,
            ButtonColors {
                text: colors.foreground,
                ..colors.control()
            },
        )
        .quiet(colors.inactive)
        .open(open)
        .open_fill(colors.active)
        .collapsed(placement.collapsed)
        .label_width(placement.label_width)
        .tooltip(tooltip)
        .occlude()
        .stop_presses()
        .debug_selector(move || match slot {
            SessionHostSlot::Title => "session-control".to_owned(),
            SessionHostSlot::Sidebar => "session-control-sidebar".to_owned(),
        })
        .on_click(Self::button_action(cx, move |view, window, cx| {
            view.session_menu_from = slot;
            view.toggle_session_menu(window, cx);
            cx.stop_propagation();
        }))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), _, _| bounds_cell.set(Some(bounds)),
            )
            .absolute()
            .inset_0(),
        );
        let trigger = if state.automatic {
            trigger.muted(colors.inactive)
        } else {
            trigger
        };
        // The title row's controls are 26-point squares.
        let trigger = if placement.collapsed {
            trigger.size(CONTROL_SIZE)
        } else {
            trigger.h(CONTROL_SIZE)
        };
        // The popover's trigger is whichever control its open menu came
        // from, so a press on it toggles rather than counting as outside.
        Some(if open {
            trigger.child(self.popover.trigger_probe())
        } else {
            trigger
        })
    }

    /// The session menu's anchor: the painted control, or the window
    /// menu's fallback anchor while the control is hidden.
    pub(super) fn session_menu_anchor(
        &self,
        layout: &ChromeLayout,
    ) -> Bounds<Pixels> {
        self.session_anchor_cell()
            .get()
            .unwrap_or_else(|| self.menu_anchor(layout))
    }

    /// The session menu's anchor as the next frame places it.
    #[cfg(test)]
    pub(super) fn session_menu_anchor_for_tests(
        &self,
    ) -> Option<Bounds<Pixels>> {
        self.session_anchor_cell().get()
    }

    /// The bounds cell of the host that opens the session menu: the one
    /// clicked, else the first host drawn, title bar first.
    pub(super) fn session_anchor_cell(
        &self,
    ) -> &Rc<Cell<Option<Bounds<Pixels>>>> {
        let sidebar = self.session_sidebar_drawn.get();
        let use_sidebar = match self.session_menu_from {
            SessionHostSlot::Sidebar => sidebar,
            SessionHostSlot::Title => {
                sidebar && !self.session_control_drawn.get()
            }
        };
        if use_sidebar {
            &self.session_sidebar_bounds
        } else {
            &self.session_button_bounds
        }
    }

    /// Whether `open_session_menu` can run.
    pub(super) fn session_menu_availability(
        &self,
        cx: &App,
    ) -> Result<(), CommandError> {
        self.check_available(false)?;
        if self.session_control_state(cx).is_none() {
            return Err(CommandError::Unavailable(
                "window shows no session".to_owned(),
            ));
        }
        Ok(())
    }

    /// Opens the session menu at the control, or focuses it when it is
    /// already open; another open menu is replaced. `from_keyboard`
    /// selects the first row, as `open_session_menu` does; the control
    /// opens it with no selection.
    pub(super) fn open_session_menu(
        &mut self,
        from_keyboard: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.session_menu_availability(cx)?;
        if self.native_menu.is_some() || self.native_session_pending.is_some() {
            return Ok(CommandOutcome::Completed);
        }
        if let Some(menu) = &self.menu
            && menu.kind == MenuKind::Session
        {
            menu.view.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
            return Ok(CommandOutcome::Completed);
        }
        if self.palette.is_some() {
            return Err(CommandError::Unavailable(
                "command palette is open".to_owned(),
            ));
        }
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        if let Some(terminal) = self.active_view(cx) {
            terminal.update(cx, TerminalView::clear_composition);
        }
        self.read_exited_sessions(window, cx);
        if self.uses_native_menus(cx) {
            // A native menu cannot take tags later; it opens once the read
            // answers, and the control shows it open meanwhile.
            self.native_session_pending = Some(from_keyboard);
            cx.notify();
            return Ok(CommandOutcome::Completed);
        }
        self.present_session_menu(from_keyboard, window, cx);
        Ok(CommandOutcome::Completed)
    }

    /// Shows the session menu at the control: natively when this window
    /// uses native menus and the presenter answers, else Huterm's popover.
    fn present_session_menu(
        &mut self,
        from_keyboard: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let contexts = window.context_stack();
        let spec = self.session_menu_spec(cx);
        let anchor = MenuAnchor::Below(
            self.session_menu_anchor(&self.chrome_layout(window)),
        );
        if self.open_native_menu(MenuKind::Session, &spec, anchor, window, cx) {
            return;
        }
        self.mount_menu(
            spec.to_listbox(Platform::current()),
            MenuKind::Session,
            None,
            contexts,
            from_keyboard,
            window,
            cx,
        );
    }

    /// Presents the native session menu that waited for the exited read,
    /// unless the request went away (`cancel_pending_session_menu`) or the
    /// window can no longer take it.
    fn present_pending_session_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(from_keyboard) = self.native_session_pending.take() else {
            return;
        };
        cx.notify();
        if self.menu.is_some()
            || self.native_menu.is_some()
            || self.palette.is_some()
            || self.session_menu_availability(cx).is_err()
        {
            return;
        }
        self.present_session_menu(from_keyboard, window, cx);
    }

    /// Whether a native session menu waiting for its exited read takes the
    /// `menu_*` commands, while no other menu is mounted.
    pub(super) fn pending_session_menu_takes_keys(&self) -> bool {
        self.menu.is_none() && self.native_session_pending.is_some()
    }

    /// Drops a native session menu still waiting for its exited read, and
    /// supersedes that read, so its answer presents nothing: a dismissal,
    /// Escape, window deactivation, the palette, or another menu replaced
    /// it.
    /// Returns whether one was waiting.
    pub(super) fn cancel_pending_session_menu(
        &mut self,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if self.native_session_pending.take().is_none() {
            return false;
        }
        self.exited_read = self.exited_read.wrapping_add(1);
        cx.notify();
        true
    }

    /// The control: closes an open session menu, otherwise opens it with
    /// no selection.
    fn toggle_session_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.cancel_pending_session_menu(cx) {
            return;
        }
        if self
            .menu
            .as_ref()
            .is_some_and(|menu| menu.kind == MenuKind::Session)
        {
            self.close_menu(MenuFocusReturn::Terminal, window, cx);
            return;
        }
        if let Err(error) = self.open_session_menu(false, window, cx) {
            self.report_failure("Open Session Menu", error.to_string(), cx);
        }
    }

    /// The session menu's rows from the projection, the window model, and
    /// the last exited read.
    pub(super) fn session_menu_spec(&self, cx: &App) -> MenuSpec {
        let desktop = cx.global::<Desktop>();
        let mut contexts = self
            .menu
            .as_ref()
            .map(|menu| menu.contexts.clone())
            .unwrap_or_default();
        contexts.push(
            KeyContext::parse(huterm_ui::components::menu::KEY_CONTEXT)
                .unwrap_or_default(),
        );
        session_menu(&SessionMenuInput {
            rows: session_rows(
                desktop.hierarchy.state(),
                &desktop.windows,
                self.window,
                &self.exited_sessions,
            ),
            current: self.attached_session(cx),
            alternate: shortcut_key(
                &desktop.keymap,
                &contexts,
                ids::MENU_CONFIRM_ALTERNATE,
            ),
            workspaces: self.config.workspaces.show
                != huterm_config::WorkspaceShow::Never,
        })
    }

    /// Reads which sessions' shells have all exited on a worker, from each
    /// terminal's published status. When it answers, the tags go into the
    /// open popover, or a native menu that waited for them opens. A later
    /// read supersedes an earlier one.
    fn read_exited_sessions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.exited_read = self.exited_read.wrapping_add(1);
        let generation = self.exited_read;
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let task = cx.background_executor().spawn(async move {
            #[cfg(all(test, feature = "test-support"))]
            runtime
                .worker_hooks
                .enter(worker_hooks::Stage::ExitedRead)
                .await;
            runtime.lock().exited_sessions()
        });
        cx.spawn_in(window, async move |view, cx| {
            let exited = task.await;
            let _ = view.update_in(cx, |view, window, cx| {
                if view.exited_read != generation {
                    return;
                }
                let exited: HashSet<SessionId> = exited.into_iter().collect();
                if view.exited_sessions != exited {
                    view.exited_sessions = exited;
                    view.refresh_session_menu(cx);
                }
                view.present_pending_session_menu(window, cx);
            });
        })
        .detach();
    }

    /// Rebuilds an open session menu's rows from the projection; the menu
    /// keeps its selection on the same row, or on the row that took the
    /// place of one that went away.
    pub(super) fn refresh_session_menu(&mut self, cx: &mut Context<'_, Self>) {
        let Some(menu) = self
            .menu
            .as_ref()
            .filter(|menu| menu.kind == MenuKind::Session)
            .map(|menu| menu.view.clone())
        else {
            return;
        };
        let model = self.session_menu_spec(cx).to_listbox(Platform::current());
        menu.update(cx, |menu, cx| menu.set_model_keeping_place(model, cx));
    }

    /// Runs a session menu pick: switch to a session, open it in a new
    /// window, or one of the menu's actions, as the palette would.
    pub(super) fn run_session_pick(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let state = cx.global::<Desktop>().hierarchy.state();
        let session = |raw: u64| {
            resolve(state, raw).ok_or_else(|| {
                CommandError::Unavailable("Session no longer exists".to_owned())
            })
        };
        let invocation = match parse_pick(id) {
            Some(SessionPick::Select(raw)) => session(raw).map(|session| {
                CommandInvocation::new(
                    ids::SELECT_SESSION,
                    vec![CommandArgument::new(
                        "session",
                        CommandValue::Session(session),
                    )],
                )
            }),
            Some(SessionPick::OpenInNewWindow(raw)) => {
                session(raw).map(|session| {
                    CommandInvocation::new(
                        ids::OPEN_SESSION_IN_NEW_WINDOW,
                        vec![CommandArgument::new(
                            "session",
                            CommandValue::Session(session),
                        )],
                    )
                })
            }
            Some(SessionPick::Command(command)) => {
                Ok(CommandInvocation::new(command, Vec::new()))
            }
            None => Err(CommandError::Unavailable(format!(
                "unknown session menu item `{id}`"
            ))),
        };
        // Picking the session the window shows changes nothing.
        if let Ok(invocation) = &invocation
            && invocation.id == ids::SELECT_SESSION
            && invocation.session("session") == self.attached_session(cx)
        {
            return;
        }
        let result = invocation.and_then(|invocation| {
            let result = self.invoke_interactive(&invocation, window, cx);
            if result.is_ok() {
                self.recent.record(invocation.id);
                cx.global_mut::<Desktop>().frequency.record(invocation.id);
            }
            result
        });
        if let Err(error) = result {
            self.report_failure("Command failed", error.to_string(), cx);
        }
        cx.notify();
    }

    /// Smoke output for the session control: its host, painted bounds,
    /// whether it is collapsed, truncated, and open, and its name.
    pub(super) fn session_control_smoke_state(
        &self,
        prefix: &str,
        window: &Window,
        cx: &App,
    ) -> String {
        let placement =
            self.session_control_placement(&self.chrome_layout(window));
        let rect = self
            .session_button_bounds
            .get()
            .filter(|_| placement.host != SessionControlHost::Hidden)
            .map_or_else(
                || "none".to_owned(),
                |bounds| {
                    format!(
                        "{},{},{},{}",
                        f32::from(bounds.origin.x),
                        f32::from(bounds.origin.y),
                        f32::from(bounds.size.width),
                        f32::from(bounds.size.height)
                    )
                },
            );
        let state = self.session_control_state(cx);
        format!(
            "{prefix}session_host={:?} {prefix}session_rect={rect} {prefix}session_collapsed={} {prefix}session_truncated={} {prefix}session_open={} {prefix}session_name={:?} {prefix}session_automatic={}",
            placement.host,
            placement.collapsed,
            placement.truncated,
            self.session_menu_open(),
            state.as_ref().map_or("", |state| state.name.as_str()),
            state.as_ref().is_some_and(|state| state.automatic),
        )
    }
}
