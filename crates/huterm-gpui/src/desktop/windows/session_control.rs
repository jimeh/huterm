//! The session switcher and the session list it opens.
//!
//! The switcher names the window's session: beside the sidebar button while
//! the sidebar is hidden and Settings show it there, and as the sidebar's
//! session row. Either opens the session list, a GPUI popover even with
//! native menus, as the workspace switcher's is: a "SESSIONS" header with
//! New Session, then one two-line row per session in runtime order. A row
//! press shows its session; a drag reorders the sessions; a right press,
//! the "⋯" button, Shift-F10, or the menu key opens the row's menu. The
//! list reads the projection, so names, counts, and window tags update
//! while it is open; "exited" tags arrive from a worker read of terminal
//! status started when it opens.

use gpui::{Hsla, TextRun};
use huterm_ui::components::menu_trigger::{self, MenuTrigger};
use huterm_ui::components::scroll_area::ScrollArea;
use huterm_ui::typography;

use super::drag::{DropTarget, Payload, RowKey, RowList};
use super::session_menu::{
    SessionControlState, SessionPick, SessionRow, parse_pick, resolve,
    row_menu, session_rows,
};
use super::sidebar_rows::{
    self, ListRow, ROW_GAP, ROW_HEIGHT, ROW_MARGIN, RowBadge,
};
use super::workspace_panel::{HEADER_HEIGHT, RowAction, SWITCHER_LABEL_MAX};
use super::*;

/// The session list's width, and the height its rows scroll beyond.
const LIST_WIDTH: Pixels = px(260.0);
const LIST_ROWS_MAX: Pixels = px(300.0);

/// Where the session list opened from, for its anchor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SessionListAnchor {
    /// The session switcher beside the sidebar button.
    Corner,
    /// The session row of the docked sidebar or the sheet.
    Sidebar,
    /// The menu button's place, from `open_session_menu` while the window
    /// draws neither.
    Fallback,
}

/// A name a switcher last measured, and its width at the switcher's text
/// size.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct SessionLabel {
    pub(super) text: String,
    pub(super) width: Pixels,
}

/// `text`'s width at a switcher's text size.
pub(super) fn switcher_label(
    text: String,
    window: &Window,
    cx: &App,
) -> SessionLabel {
    let size =
        typography::ui(menu_trigger::TEXT_SIZE).to_pixels(window.rem_size());
    let mut font = window.text_style().font();
    font.family = Typography::of(cx).family;
    let run = TextRun {
        len: text.len(),
        font,
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let width = window
        .text_system()
        .layout_line(&text, size, &[run], None)
        .width;
    SessionLabel { text, width }
}

impl WorkspaceView {
    /// Whether the session list is open.
    pub(super) fn session_list_open(&self) -> bool {
        self.session_list.is_some()
    }

    /// What the switcher shows, or `None` while the projection does not
    /// hold the window's session.
    pub(super) fn session_control_state(
        &self,
        cx: &App,
    ) -> Option<SessionControlState> {
        SessionControlState::derive(
            cx.global::<Desktop>().hierarchy.state(),
            self.attached_session(cx),
            self.session_list_open(),
        )
    }

    /// Measures the session's name at the switcher's text size, so layout
    /// outside render reserves the switcher's width. Render calls it
    /// before laying out, as it measures tab titles.
    pub(super) fn measure_session_label(&mut self, window: &Window, cx: &App) {
        self.session_label = self
            .session_control_state(cx)
            .map_or_else(SessionLabel::default, |state| {
                switcher_label(state.name, window, cx)
            });
    }

    /// The corner session switcher's drawn width with its name.
    pub(super) fn session_switcher_width(&self) -> Pixels {
        menu_trigger::width_for_label(
            self.session_label.width.ceil().min(SWITCHER_LABEL_MAX),
        )
    }

    /// The session switcher beside the sidebar button: the layers icon, the
    /// session's name, dim when automatic, and a chevron, `width` wide, or
    /// the icon alone. It records its painted bounds for the list's anchor
    /// and never holds focus.
    pub(super) fn session_switcher_element(
        &self,
        width: Pixels,
        icon_only: bool,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let state = self.session_control_state(cx)?;
        let open = self.session_list == Some(SessionListAnchor::Corner);
        let bounds_cell = Rc::clone(&self.session_corner_bounds);
        self.session_corner_drawn.set(true);
        let label_width = (width - menu_trigger::width_for_label(px(0.0)))
            .max(px(0.0))
            .min(self.session_label.width.ceil());
        let truncated =
            icon_only || self.session_label.width.ceil() > label_width;
        let trigger = MenuTrigger::new(
            "session-switcher-corner",
            state.accessible_name(),
            state.name.clone(),
            Icon::Layers.asset_path(),
            Icon::ChevronDown.asset_path(),
            tokens(&self.config.theme),
            ButtonColors {
                text: colors.foreground,
                ..colors.control()
            },
        )
        .quiet(colors.inactive)
        .open(open)
        .open_fill(colors.active)
        .collapsed(icon_only)
        .label_width(label_width)
        .tooltip(truncated.then(|| state.name.clone().into()))
        .occlude()
        .stop_presses()
        .debug_selector(|| "session-switcher-corner".to_owned())
        .on_click(Self::button_action(cx, |view, window, cx| {
            view.toggle_session_list(SessionListAnchor::Corner, window, cx);
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
        let trigger = if icon_only {
            trigger.size(CONTROL_SIZE)
        } else {
            trigger.h(CONTROL_SIZE).w(width)
        };
        Some(
            if open {
                trigger.child(self.session_list_popover.trigger_probe())
            } else {
                trigger
            }
            .into_any_element(),
        )
    }

    /// The painted bounds of what opens the list from `from`, if drawn.
    fn session_opener_bounds(
        &self,
        from: SessionListAnchor,
    ) -> Option<Bounds<Pixels>> {
        match from {
            SessionListAnchor::Corner => self
                .session_corner_bounds
                .get()
                .filter(|_| self.session_corner_drawn.get()),
            SessionListAnchor::Sidebar => self
                .session_sidebar_bounds
                .get()
                .filter(|_| self.session_sidebar_drawn.get()),
            SessionListAnchor::Fallback => None,
        }
    }

    /// The list's anchor: what it opened from while painted, else the
    /// corner switcher or the sidebar's session row, else the menu
    /// button's place.
    pub(super) fn session_list_anchor(
        &self,
        from: SessionListAnchor,
        layout: &ChromeLayout,
    ) -> Bounds<Pixels> {
        self.session_opener_bounds(from)
            .or_else(|| self.session_opener_bounds(SessionListAnchor::Corner))
            .or_else(|| self.session_opener_bounds(SessionListAnchor::Sidebar))
            .unwrap_or_else(|| self.menu_anchor(layout))
    }

    /// Where `open_session_menu` opens the list from: the corner switcher
    /// or the sidebar's session row while drawn, else the fallback.
    fn session_list_opener(&self) -> SessionListAnchor {
        [SessionListAnchor::Corner, SessionListAnchor::Sidebar]
            .into_iter()
            .find(|&from| self.session_opener_bounds(from).is_some())
            .unwrap_or(SessionListAnchor::Fallback)
    }

    /// Whether the open list lost what it opened from this frame.
    pub(super) fn session_list_anchor_went(&self) -> bool {
        match self.session_list {
            None | Some(SessionListAnchor::Fallback) => false,
            Some(from) => self.session_opener_bounds(from).is_none(),
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

    /// The `open_session_menu` command: opens the session list at the
    /// switcher, or focuses its current row when it is already open.
    pub(super) fn open_session_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        let from = self
            .session_list
            .unwrap_or_else(|| self.session_list_opener());
        self.open_session_list(from, window, cx)
    }

    /// Opens the session list from `from` with the window's session's row
    /// focused, or focuses that row when it is already open. An open menu
    /// and the workspace switcher's popover close; a narrow window's sheet
    /// stays, since its session row may be the opener.
    pub(super) fn open_session_list(
        &mut self,
        from: SessionListAnchor,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.session_menu_availability(cx)?;
        if self.palette.is_some() {
            return Err(CommandError::Unavailable(
                "command palette is open".to_owned(),
            ));
        }
        let Some(session) = self.attached_session(cx) else {
            return Err(CommandError::Unavailable(
                "window shows no session".to_owned(),
            ));
        };
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        if matches!(
            self.switcher_mode,
            Some(workspace_panel::SwitcherMode::Popover(_))
        ) {
            self.close_workspace_switcher(window, cx);
        }
        if let Some(terminal) = self.active_view(cx) {
            terminal.update(cx, TerminalView::clear_composition);
        }
        if self.session_list.is_none() {
            self.session_list = Some(from);
            let layout = self.chrome_layout(window);
            self.session_list_popover.open(
                Anchor::Trigger(self.session_list_anchor(from, &layout)),
                false,
            );
            // Only a drawn opener mounts the trigger probe; a fallback
            // opening has none.
            if from == SessionListAnchor::Fallback {
                self.session_list_popover.clear_trigger();
            }
            self.read_exited_sessions(window, cx);
        }
        self.focus_session_row(session, window, cx);
        Ok(CommandOutcome::Completed)
    }

    /// A switcher: closes an open list, otherwise opens it from `from`.
    pub(super) fn toggle_session_list(
        &mut self,
        from: SessionListAnchor,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.session_list.is_some() {
            let refocus = from == SessionListAnchor::Sidebar
                && self.session_sidebar_drawn.get();
            self.close_session_list(window, cx);
            if refocus {
                self.session_row_focus.focus(window, cx);
            } else {
                self.focus_terminal(window, cx);
            }
            return;
        }
        if let Err(error) = self.open_session_list(from, window, cx) {
            self.report_failure("Open Session List", error.to_string(), cx);
        }
    }

    /// Closes the session list with a row menu opened from it; the caller
    /// moves focus. `false` when it was not open.
    pub(super) fn close_session_list(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let _ = self.session_list_popover.close();
        if self.session_list.take().is_none() {
            return false;
        }
        self.drop_row_drag(
            |payload| matches!(payload, Payload::Session(_)),
            cx,
        );
        if self
            .menu
            .as_ref()
            .is_some_and(|menu| matches!(menu.kind, MenuKind::SessionRow(_)))
        {
            self.close_menu(MenuFocusReturn::Terminal, window, cx);
        }
        cx.notify();
        true
    }

    /// Every session, as the list shows them.
    pub(super) fn session_rows(&self, cx: &App) -> Vec<SessionRow> {
        let desktop = cx.global::<Desktop>();
        session_rows(
            desktop.hierarchy.state(),
            &desktop.windows,
            self.window,
            &self.exited_sessions,
        )
    }

    /// The focus handle of `session`'s row.
    fn session_focus(&mut self, session: SessionId, cx: &App) -> FocusHandle {
        self.session_focus
            .entry(session)
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// The list row that holds keyboard focus, if any.
    pub(super) fn focused_session_row(
        &self,
        window: &Window,
    ) -> Option<SessionId> {
        self.session_focus
            .iter()
            .find(|(_, handle)| handle.is_focused(window))
            .map(|(session, _)| *session)
    }

    pub(super) fn focus_session_row(
        &mut self,
        session: SessionId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let handle = self.session_focus(session, cx);
        handle.focus(window, cx);
        if let Some(index) = self
            .session_rows(cx)
            .iter()
            .position(|row| row.session == session)
        {
            self.session_list_scroll.scroll_to_item(index);
        }
        cx.notify();
    }

    /// Hands focus to the terminal when the list row that holds it is no
    /// longer drawn, drops the handles of sessions that went, and ends a
    /// drag whose session went or whose list closed.
    pub(super) fn release_stale_session_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let rows: HashSet<SessionId> = self
            .session_rows(cx)
            .iter()
            .map(|row| row.session)
            .collect();
        let drawn = self.session_list.is_some();
        let focused = self.focused_session_row(window);
        if let Some(focused) = focused
            && (!drawn || !rows.contains(&focused))
        {
            cx.defer_in(window, move |view, window, cx| {
                if view.focused_session_row(window) == Some(focused) {
                    view.focus_terminal(window, cx);
                }
            });
        }
        self.session_focus.retain(|session, _| {
            rows.contains(session) || Some(*session) == focused
        });
        self.session_row_bounds
            .borrow_mut()
            .retain(|session, _| rows.contains(session));
        if let Some(Payload::Session(session)) =
            self.drag.as_ref().map(|drag| drag.payload)
            && (!drawn || !rows.contains(&session))
        {
            self.drag = None;
        }
    }

    /// Runs a `sidebar_*` command on the focused list row; `None` when no
    /// list row holds focus. The arrows, Home, End, Tab, and Shift-Tab walk
    /// the rows; Return shows the focused session; F2 renames it; the menu
    /// key opens its menu; Escape closes the list.
    pub(super) fn run_session_list_command(
        &mut self,
        command: huterm_protocol::CommandId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<Result<CommandOutcome, CommandError>> {
        let focused = self.focused_session_row(window)?;
        let rows: Vec<SessionId> = self
            .session_rows(cx)
            .iter()
            .map(|row| row.session)
            .collect();
        let at = rows.iter().position(|session| *session == focused);
        let step = match command {
            ids::SIDEBAR_NEXT => Some(huterm_ui::roving::Move::Next),
            ids::SIDEBAR_PREVIOUS => Some(huterm_ui::roving::Move::Previous),
            ids::SIDEBAR_FIRST => Some(huterm_ui::roving::Move::First),
            ids::SIDEBAR_LAST => Some(huterm_ui::roving::Move::Last),
            _ => None,
        };
        if let Some(step) = step {
            if let Some(index) = huterm_ui::roving::target(rows.len(), at, step)
            {
                self.focus_session_row(rows[index], window, cx);
            }
            return Some(Ok(CommandOutcome::Completed));
        }
        match command {
            ids::SIDEBAR_EXIT => {
                let from = self.session_list;
                self.close_session_list(window, cx);
                if from == Some(SessionListAnchor::Sidebar)
                    && self.session_sidebar_drawn.get()
                {
                    self.session_row_focus.focus(window, cx);
                } else {
                    self.focus_terminal(window, cx);
                }
                cx.notify();
            }
            ids::SIDEBAR_SHOW => {
                self.run_session_row(focused, RowAction::Show, window, cx);
            }
            ids::SIDEBAR_RENAME => {
                self.run_session_row(focused, RowAction::Rename, window, cx);
            }
            ids::SIDEBAR_MOVE_UP | ids::SIDEBAR_MOVE_DOWN => {
                let up = command == ids::SIDEBAR_MOVE_UP;
                return Some(
                    self.move_session_step(focused, up, window, cx)
                        .or(Ok(CommandOutcome::Completed)),
                );
            }
            ids::SIDEBAR_MENU => {
                let anchor = self
                    .session_row_bounds
                    .borrow()
                    .get(&focused)
                    .copied()
                    .map_or_else(
                        || window.mouse_position(),
                        |bounds| bounds.bottom_left(),
                    );
                self.open_session_row_menu(focused, anchor, true, window, cx);
            }
            _ => return None,
        }
        Some(Ok(CommandOutcome::Completed))
    }

    /// Shows, renames, clears the name of, or closes `session`, as the
    /// palette's commands do with that session named. Showing one closes
    /// the list.
    pub(super) fn run_session_row(
        &mut self,
        session: SessionId,
        action: RowAction,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let command = match action {
            RowAction::Show => {
                if self.close_session_list(window, cx) {
                    self.focus_terminal(window, cx);
                }
                // Showing the shown session changes nothing.
                if self.attached_session(cx) == Some(session) {
                    return;
                }
                ids::SELECT_SESSION
            }
            RowAction::Rename => ids::RENAME_SESSION,
            RowAction::ClearName => ids::CLEAR_SESSION_NAME,
            RowAction::Close => ids::CLOSE_SESSION,
            // Sessions have no pin; their menu never offers one.
            RowAction::TogglePin
            | RowAction::NewWindow
            | RowAction::MoveUp
            | RowAction::MoveDown
            | RowAction::ToSession => return,
        };
        self.run_recorded(
            &CommandInvocation::new(
                command,
                vec![CommandArgument::new(
                    "session",
                    CommandValue::Session(session),
                )],
            ),
            window,
            cx,
        );
    }

    /// Opens `session`'s row menu at `pointer`, replacing any open menu. It
    /// refuses where the tab menu does.
    pub(super) fn open_session_row_menu(
        &mut self,
        session: SessionId,
        pointer: gpui::Point<Pixels>,
        from_keyboard: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.check_available(true).is_err()
            || self.palette.is_some()
            || self.native_menu.is_some()
        {
            return;
        }
        let Some(spec) = self.session_row_menu_spec(session, cx) else {
            return;
        };
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        let contexts = window.context_stack();
        let kind = MenuKind::SessionRow(session);
        let anchor = MenuAnchor::Pointer(pointer);
        if self.open_native_menu(kind.clone(), &spec, anchor, window, cx) {
            return;
        }
        self.mount_menu(
            spec.to_listbox(Platform::current()),
            kind,
            Some(pointer),
            contexts,
            from_keyboard,
            window,
            cx,
        );
    }

    /// A row's context menu, or `None` once the session went.
    pub(super) fn session_row_menu_spec(
        &self,
        session: SessionId,
        cx: &App,
    ) -> Option<MenuSpec> {
        let rows = self.session_rows(cx);
        let index = rows.iter().position(|row| row.session == session)?;
        Some(row_menu(&rows[index], index, rows.len()))
    }

    /// Whether a dismissed row menu of `kind` returns focus to its row: a
    /// session row while the list is open, or a workspace row while the
    /// switcher's popover or the sheet shows it.
    pub(super) fn popover_row_takes_focus(
        &self,
        kind: &MenuKind,
        cx: &App,
    ) -> bool {
        match kind {
            MenuKind::SessionRow(session) => {
                self.session_list.is_some()
                    && self
                        .session_rows(cx)
                        .iter()
                        .any(|row| row.session == *session)
            }
            MenuKind::WorkspaceRow(workspace) => {
                self.switcher_mode.is_some()
                    && self
                        .workspace_rows(cx)
                        .iter()
                        .any(|row| row.workspace == *workspace)
            }
            _ => false,
        }
    }

    /// Focuses the row a row menu of `kind` opened from.
    pub(super) fn focus_menu_row(
        &mut self,
        kind: &MenuKind,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match kind {
            MenuKind::SessionRow(session) => {
                self.focus_session_row(*session, window, cx);
            }
            MenuKind::WorkspaceRow(workspace) => {
                self.focus_workspace_row(*workspace, window, cx);
            }
            _ => {}
        }
    }

    /// Runs a session row menu pick.
    pub(super) fn run_session_pick(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(pick) = parse_pick(id) else {
            self.report_failure(
                "Command failed",
                format!("unknown session menu item `{id}`"),
                cx,
            );
            return;
        };
        let raw = match pick {
            SessionPick::Rename(raw)
            | SessionPick::ClearName(raw)
            | SessionPick::OpenInNewWindow(raw)
            | SessionPick::MoveUp(raw)
            | SessionPick::MoveDown(raw)
            | SessionPick::Close(raw) => raw,
        };
        let Some(session) =
            resolve(cx.global::<Desktop>().hierarchy.state(), raw)
        else {
            self.report_failure(
                "Command failed",
                "Session no longer exists".to_owned(),
                cx,
            );
            return;
        };
        let action = match pick {
            SessionPick::Rename(_) => RowAction::Rename,
            SessionPick::ClearName(_) => RowAction::ClearName,
            SessionPick::Close(_) => RowAction::Close,
            SessionPick::MoveUp(_) | SessionPick::MoveDown(_) => {
                let up = matches!(pick, SessionPick::MoveUp(_));
                if let Err(error) =
                    self.move_session_step(session, up, window, cx)
                {
                    self.report_failure(
                        "Command failed",
                        error.to_string(),
                        cx,
                    );
                }
                return;
            }
            SessionPick::OpenInNewWindow(_) => {
                self.close_session_list(window, cx);
                self.run_recorded(
                    &CommandInvocation::new(
                        ids::OPEN_SESSION_IN_NEW_WINDOW,
                        vec![CommandArgument::new(
                            "session",
                            CommandValue::Session(session),
                        )],
                    ),
                    window,
                    cx,
                );
                return;
            }
        };
        self.run_session_row(session, action, window, cx);
    }

    /// Moves `session` before `before`, or to the end, on a worker. The
    /// projection applies the committed order; this only reports failure.
    pub(super) fn move_session(
        session: SessionId,
        before: Option<SessionId>,
        cx: &mut Context<'_, Self>,
    ) {
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let task = cx
            .background_executor()
            .spawn(async move { runtime.move_session(session, before) });
        let app = cx.to_async();
        cx.spawn(async move |view, cx| {
            let Committed { result, seq } = task.await;
            projected(&app, seq).await;
            let _ = view.update(cx, |view, cx| {
                match result {
                    Err(error) if !terminating(cx) => view.report_failure(
                        "Reorder sessions",
                        format!("Cannot reorder sessions: {error}"),
                        cx,
                    ),
                    // The moved row stays in view, as one moved by key.
                    Ok(()) => {
                        if let Some(index) = view
                            .session_rows(cx)
                            .iter()
                            .position(|row| row.session == session)
                        {
                            view.session_list_scroll.scroll_to_item(index);
                            cx.notify();
                        }
                    }
                    Err(_) => {}
                }
            });
        })
        .detach();
    }

    /// One session row: a press shows it, a drag reorders it, a right press
    /// or its "⋯" button opens its menu.
    #[expect(
        clippy::too_many_lines,
        reason = "one row element and its listeners, read top to bottom"
    )]
    fn session_list_row_element(
        &mut self,
        row: &SessionRow,
        last: bool,
        colors: TabColors,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let session = row.session;
        let focus = self.session_focus(session, cx);
        let swatch = tokens(&self.config.theme);
        // The row's menu is open, native or Huterm's own.
        let targeted = self.menu_targets(&MenuKind::SessionRow(session));
        let drag = self
            .drag
            .as_ref()
            .filter(|drag| drag.dragging())
            .filter(|drag| matches!(drag.payload, Payload::Session(_)));
        let dragged = drag.is_some_and(|drag| {
            drag.payload.row() == Some(RowKey::Session(session))
        });
        let slot = drag.and_then(|drag| match drag.target() {
            Some(DropTarget::RowSlot(before)) => Some(before),
            _ => None,
        });
        let mut badges = Vec::new();
        if row.windows > 0 {
            badges.push(RowBadge {
                icon: Some(Icon::AppWindow),
                text: row.windows.to_string(),
                warning: false,
            });
        }
        if row.exited {
            badges.push(RowBadge {
                icon: None,
                text: "exited".to_owned(),
                warning: true,
            });
        }
        let more = sidebar_rows::more_button(
            format!("session-row-more-{}", session.get()),
            format!("Session actions: {}", row.name),
            targeted,
            colors,
            swatch,
        )
        .on_click(Self::button_action(cx, move |view, window, cx| {
            cx.stop_propagation();
            let anchor = view
                .session_row_bounds
                .borrow()
                .get(&session)
                .copied()
                .map_or_else(
                    || window.mouse_position(),
                    |bounds| bounds.bottom_right(),
                );
            view.open_session_row_menu(session, anchor, false, window, cx);
        }));
        let bounds_cell = Rc::clone(&self.session_row_bounds);
        let element = sidebar_rows::list_row(
            ListRow {
                id: format!("session-row-{}", session.get()),
                icon: Icon::Layers,
                name: row.name.clone(),
                automatic: row.automatic,
                detail: row.detail(),
                accessible: row.accessible_name(),
                selected: self.attached_session(cx) == Some(session),
                targeted,
                dragged,
                drop_target: false,
                tearing_off: false,
                line_above: slot == Some(Some(RowKey::Session(session))),
                line_below: last && !dragged && slot == Some(None),
                badges,
                pinned: false,
                status: None,
            },
            &focus,
            Some(more.into_any_element()),
            colors,
            swatch,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                // The row takes no focus from a press; keyboard focus stays
                // where it was, as on tabs.
                window.prevent_default();
                cx.stop_propagation();
                view.begin_row_drag(
                    RowKey::Session(session),
                    event.position,
                    window,
                    cx,
                );
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                view.cancel_drag(window, cx);
                view.open_session_row_menu(
                    session,
                    event.position,
                    false,
                    window,
                    cx,
                );
                cx.stop_propagation();
            }),
        )
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), _, _| {
                    bounds_cell.borrow_mut().insert(session, bounds);
                },
            )
            .absolute()
            .inset_0(),
        );
        huterm_ui::focus::ring_inset(element, &focus, swatch, window, cx)
            .into_any_element()
    }

    /// The open session list under its anchor, as tall as its content up to
    /// its cap: the "SESSIONS" header with New Session, then the rows.
    pub(super) fn session_list_overlay(
        &mut self,
        layout: &ChromeLayout,
        colors: TabColors,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let from = self.session_list?;
        let viewport = window.viewport_size();
        let swatch = tokens(&self.config.theme);
        let rows = self.session_rows(cx);
        let count = rows.len();
        let row_elements: Vec<AnyElement> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                self.session_list_row_element(
                    row,
                    index + 1 == count,
                    colors,
                    window,
                    cx,
                )
            })
            .collect();
        let view = cx.entity();
        let list = ScrollArea::new("session-rows", &self.session_list_scroll)
            .padding(gpui::Edges {
                bottom: px(6.0),
                ..gpui::Edges::default()
            })
            .children(row_elements)
            .max_height(Some(LIST_ROWS_MAX))
            .render(
                &mut self.session_list_scrollbar,
                colors.scrollbar,
                &view,
                |view| &mut view.session_list_scrollbar,
                cx,
            );
        let header = Self::list_header(
            "SESSIONS",
            self.new_session_button(colors, cx).into_any_element(),
            colors,
        );
        let shown = f32::from(u16::try_from(count.max(1)).unwrap_or(u16::MAX));
        let height = HEADER_HEIGHT
            + ((ROW_HEIGHT + ROW_GAP) * shown + px(6.0)).min(LIST_ROWS_MAX)
            + px(10.0);
        let placement = place(
            Anchor::Trigger(self.session_list_anchor(from, layout)),
            size(LIST_WIDTH, height),
            viewport,
        );
        // The surface stops presses over it, as a menu's does: the guard
        // lets presses inside a non-modal popover through.
        let panel = div()
            .id("session-list-popover")
            .debug_selector(|| "session-list-popover".to_owned())
            .occlude()
            .role(gpui::Role::Dialog)
            .aria_label("Sessions")
            .w(LIST_WIDTH)
            .max_h(placement.max_height)
            .flex()
            .flex_col()
            .overflow_hidden()
            .py(px(4.0))
            .bg(swatch.surface)
            .border_1()
            .border_color(swatch.line)
            .rounded(px(9.0))
            .shadow(swatch.shadow())
            .text_size(tab_bar::TAB_TEXT_SIZE)
            .text_color(colors.foreground)
            .child(header)
            .child(list)
            .children(self.lifted_row(RowList::Sessions, colors, cx));
        self.session_list_popover.render(placement, viewport, panel)
    }

    /// A list's header: its uppercase label and a "+" button.
    pub(super) fn list_header(
        label: &'static str,
        button: AnyElement,
        colors: TabColors,
    ) -> gpui::Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(HEADER_HEIGHT)
            .pl(px(14.0))
            .pr(ROW_MARGIN)
            .gap(px(2.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(typography::ui(10.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors.inactive)
                    .child(label),
            )
            .child(button)
    }

    /// New Session, for the list's header.
    fn new_session_button(
        &self,
        colors: TabColors,
        cx: &Context<'_, Self>,
    ) -> Button {
        let id = "session-new";
        Button::new(id, "New Session", tokens(&self.config.theme))
            .debug_selector(move || id.to_owned())
            .colors(colors.control())
            .icon(
                icon_element(Icon::Plus, colors.inactive)
                    .group_hover(id, |style| {
                        style.text_color(colors.foreground)
                    }),
            )
            .icon_only()
            .group(id)
            .stop_presses()
            .size(px(22.0))
            .rounded(px(6.0))
            .on_click(Self::button_action(cx, |view, window, cx| {
                if view.close_session_list(window, cx) {
                    view.focus_terminal(window, cx);
                }
                view.run_recorded(
                    &CommandInvocation::new(ids::NEW_SESSION, Vec::new()),
                    window,
                    cx,
                );
            }))
    }

    /// Reads which sessions' shells have all exited on a worker, from each
    /// terminal's published status. When it answers, the tags go into the
    /// open list. A later read supersedes an earlier one.
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
            let _ = view.update_in(cx, |view, _, cx| {
                if view.exited_read != generation {
                    return;
                }
                let exited: HashSet<SessionId> = exited.into_iter().collect();
                if view.exited_sessions != exited {
                    view.exited_sessions = exited;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Smoke output for the session switcher: the corner switcher's host
    /// and painted bounds, whether the list is open, where from, and where
    /// it painted, and the session's name.
    pub(super) fn session_control_smoke_state(
        &self,
        prefix: &str,
        window: &Window,
        cx: &App,
    ) -> String {
        // Where the corner session switcher goes this frame, or `Hidden`
        // while it does not show.
        let chrome = self.chrome_layout(window).workspace_chrome;
        let host = if chrome.session() {
            self.corner_host(chrome, false)
        } else {
            workspace_chrome::CornerHost::Hidden
        };
        let rect = |bounds: Option<Bounds<Pixels>>| {
            bounds.map_or_else(
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
            )
        };
        let list = rect(
            self.session_list_popover
                .content_bounds()
                .filter(|_| self.session_list.is_some()),
        );
        let rect = rect(
            self.session_corner_bounds
                .get()
                .filter(|_| self.session_corner_drawn.get()),
        );
        let state = self.session_control_state(cx);
        let from = self
            .session_list
            .map_or_else(|| "none".to_owned(), |from| format!("{from:?}"));
        format!(
            "{prefix}session_host={host:?} {prefix}session_rect={rect} {prefix}session_open={} {prefix}session_list_from={from} {prefix}session_list_rect={list} {prefix}session_name={:?} {prefix}session_automatic={}",
            self.session_list_open(),
            state.as_ref().map_or("", |state| state.name.as_str()),
            state.as_ref().is_some_and(|state| state.automatic),
        )
    }
}
