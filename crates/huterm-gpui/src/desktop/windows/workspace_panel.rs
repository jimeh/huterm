//! The workspace panel, bar, and dropdown: what they draw, the workspace
//! menu they open, and the per-window state they keep (collapse and
//! panel width). [`super::workspace_chrome`] decides which one a window
//! shows; this module draws it and handles its input.

use gpui::{Hsla, TextRun};
use huterm_ui::components::menu_trigger::{self, MenuTrigger};
use huterm_ui::components::scroll_area::ScrollArea;
use huterm_ui::typography;

use super::session_control::SessionLabel;
use super::window_menu::SessionControlPlacement;
use super::workspace_menu::{
    PanelRow, WorkspaceMenuInput, WorkspacePick, WorkspaceRow, parse_pick,
    resolve, row_menu, workspace_menu, workspace_rows,
};
use super::*;

/// The dropdown's widest label; a longer name truncates.
const DROPDOWN_LABEL_MAX: Pixels = px(140.0);

impl WorkspaceView {
    /// The dropdown trigger's drawn width.
    pub(super) fn dropdown_width(&self) -> Pixels {
        menu_trigger::width_for_label(
            self.workspace_label.width.ceil().min(DROPDOWN_LABEL_MAX),
        )
    }

    /// The width the dropdown and its divider take along a strip.
    pub(super) fn dropdown_lead(&self) -> Pixels {
        workspace_chrome::DROPDOWN_MARGIN
            + self.dropdown_width()
            + workspace_chrome::DROPDOWN_DIVIDER
    }

    /// Reads the window's session's workspace count and measures the
    /// shown workspace's name, so layout outside render resolves the same
    /// chrome render draws. Render calls it before laying out.
    pub(super) fn sync_workspace_chrome(&mut self, window: &Window, cx: &App) {
        let state = cx.global::<Desktop>().hierarchy.state();
        self.workspace_count = self
            .attached_session(cx)
            .and_then(|session| state.session_workspaces(session))
            .map_or(0, <[_]>::len);
        self.workspace_chrome_now
            .set(self.workspace_chrome(window.viewport_size()));
        let Some(name) = self
            .workspace_id(cx)
            .and_then(|workspace| state.workspace(workspace))
            .map(|info| info.display_name().to_owned())
        else {
            self.workspace_label = SessionLabel::default();
            return;
        };
        // Measured every frame, as the session control is, so a font or
        // interface size change sizes the dropdown at once.
        let size = typography::ui(menu_trigger::TEXT_SIZE)
            .to_pixels(window.rem_size());
        let mut font = window.text_style().font();
        font.family = Typography::of(cx).family;
        let run = TextRun {
            len: name.len(),
            font,
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let width = window
            .text_system()
            .layout_line(&name, size, &[run], None)
            .width;
        self.workspace_label = SessionLabel { text: name, width };
    }
}

/// A side panel's header: "WORKSPACES", New Workspace, and collapse.
const HEADER_HEIGHT: Pixels = px(28.0);
/// A side panel's rows.
const ROW_HEIGHT: Pixels = px(28.0);
/// A bar's chips.
const CHIP_HEIGHT: Pixels = px(24.0);
/// Space between a panel's rows and its edges.
const ROW_MARGIN: Pixels = px(6.0);
/// The panel's resize grab zone along its inner edge.
const HANDLE_WIDTH: Pixels = px(4.0);

/// Where the workspace menu opened from, for its anchor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WorkspaceMenuAnchor {
    /// The dropdown trigger, else the empty state's button, else the
    /// window menu's fallback anchor.
    Trigger,
    /// The empty state's Switch Workspace… button.
    EmptyState,
}

impl WorkspaceView {
    /// Whether the window shows any workspace UI.
    pub(super) fn workspace_ui_shown(&self) -> bool {
        workspace_chrome::shown(
            self.config.workspaces.show,
            self.workspace_count,
        )
    }

    /// The window's session's workspaces, as the panel, bar, and menus list
    /// them.
    pub(super) fn workspace_rows(&self, cx: &App) -> Vec<WorkspaceRow> {
        let desktop = cx.global::<Desktop>();
        workspace_rows(
            desktop.hierarchy.state(),
            &desktop.windows,
            self.window,
            self.attached_session(cx),
        )
    }

    /// The panel's background: one step darker than the tab bar's, so it
    /// reads as the outer level.
    pub(super) fn workspace_panel_background(colors: TabColors) -> Hsla {
        let bar = colors.bar;
        let lightness = if bar.l > 0.5 {
            bar.l - 0.03
        } else {
            bar.l * 0.8
        };
        Hsla {
            l: lightness.clamp(0.0, 1.0),
            ..bar
        }
    }

    /// The focus handle of `workspace`'s row or chip.
    fn workspace_focus(
        &mut self,
        workspace: WorkspaceId,
        cx: &App,
    ) -> FocusHandle {
        self.workspace_focus
            .entry(workspace)
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// The row or chip that holds keyboard focus, if any.
    pub(super) fn focused_workspace_row(
        &self,
        window: &Window,
    ) -> Option<WorkspaceId> {
        self.workspace_focus
            .iter()
            .find(|(_, handle)| handle.is_focused(window))
            .map(|(workspace, _)| *workspace)
    }

    /// Hands focus to the terminal when the row or chip that holds it is
    /// no longer drawn: its workspace closed or left the session, or the
    /// panel or bar went. A focus handle nothing draws would swallow every
    /// key. Drops the handles of workspaces the session no longer holds.
    pub(super) fn release_stale_row_focus(
        &mut self,
        layout: &ChromeLayout,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let rows: HashSet<WorkspaceId> = self
            .workspace_rows(cx)
            .iter()
            .map(|row| row.workspace)
            .collect();
        let focused = self.focused_workspace_row(window);
        if let Some(focused) = focused
            && (layout.workspace_chrome.edge().is_none()
                || !rows.contains(&focused))
        {
            cx.defer_in(window, move |view, window, cx| {
                if view.focused_workspace_row(window) == Some(focused) {
                    view.focus_terminal(window, cx);
                }
            });
        }
        self.workspace_focus.retain(|workspace, _| {
            rows.contains(workspace) || Some(*workspace) == focused
        });
        self.workspace_row_bounds
            .borrow_mut()
            .retain(|workspace, _| rows.contains(workspace));
        // A workspace the window starts to show, as one New Workspace just
        // made past the end of a long list, scrolls into view.
        let shown = self.workspace_id(cx);
        if layout.workspace_chrome.edge().is_some() {
            if shown != self.workspace_revealed
                && let Some(workspace) = shown
            {
                self.reveal_workspace_row(workspace, cx);
            }
            self.workspace_revealed = shown;
        } else {
            self.workspace_revealed = None;
        }
    }

    /// Whether the window shows a panel or bar, which the panel commands
    /// need.
    fn workspace_band_shown(&self, window: &Window) -> bool {
        self.workspace_chrome(window.viewport_size())
            .edge()
            .is_some()
    }

    pub(super) fn toggle_workspace_panel_availability(
        &self,
    ) -> Result<(), CommandError> {
        if self.config.workspaces.show == huterm_config::WorkspaceShow::Never {
            return Err(CommandError::Unavailable(
                "workspaces are hidden in this window".to_owned(),
            ));
        }
        if !self.workspace_ui_shown() {
            return Err(CommandError::Unavailable(
                "the session has one workspace".to_owned(),
            ));
        }
        Ok(())
    }

    /// X1: collapses the panel or bar to the dropdown, or shows it again.
    /// The terminal keeps its process, focus, and selection; only its size
    /// changes, as for a window resize.
    pub(super) fn toggle_workspace_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.toggle_workspace_panel_availability()?;
        // A row that holds focus goes with its panel.
        let focused = self.focused_workspace_row(window).is_some();
        self.workspace_collapsed = !self.workspace_collapsed;
        cx.global_mut::<Desktop>()
            .windows
            .set_workspace_collapsed(self.window, self.workspace_collapsed);
        if self
            .menu
            .as_ref()
            .is_some_and(|menu| matches!(menu.kind, MenuKind::WorkspaceRow(_)))
        {
            self.close_menu(MenuFocusReturn::Keep, window, cx);
        }
        self.relayout_terminals(window, cx);
        if focused {
            self.focus_terminal(window, cx);
        }
        cx.notify();
        Ok(CommandOutcome::Completed)
    }

    /// Hands every terminal view the window's current geometry, as a
    /// window resize does.
    pub(super) fn relayout_terminals(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        for (view, geometry) in self.pane_views_with_geometry(window, cx) {
            view.update(cx, |view, cx| {
                view.geometry = geometry;
                if view.visible {
                    view.resize_if_needed(window);
                }
                cx.notify();
            });
        }
    }

    pub(super) fn focus_workspace_panel_availability(
        &self,
    ) -> Result<(), CommandError> {
        self.toggle_workspace_panel_availability()?;
        if self.workspace_chrome_unsized().edge().is_none() {
            return Err(CommandError::Unavailable(
                "the workspace panel is collapsed".to_owned(),
            ));
        }
        Ok(())
    }

    /// Moves keyboard focus to the shown workspace's row or chip.
    pub(super) fn focus_workspace_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.focus_workspace_panel_availability()?;
        if !self.workspace_band_shown(window) {
            return Err(CommandError::Unavailable(
                "the window is too narrow for the workspace panel".to_owned(),
            ));
        }
        let Some(workspace) = self.workspace_id(cx) else {
            return Err(CommandError::Unavailable(
                "window shows no workspace".to_owned(),
            ));
        };
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        self.focus_workspace_row(workspace, window, cx);
        Ok(CommandOutcome::Completed)
    }

    fn focus_workspace_row(
        &mut self,
        workspace: WorkspaceId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let handle = self.workspace_focus(workspace, cx);
        handle.focus(window, cx);
        self.reveal_workspace_row(workspace, cx);
        cx.notify();
    }

    /// Scrolls the panel or bar so `workspace`'s row or chip shows.
    pub(super) fn reveal_workspace_row(
        &mut self,
        workspace: WorkspaceId,
        cx: &App,
    ) {
        if let Some(index) = self
            .workspace_rows(cx)
            .iter()
            .position(|row| row.workspace == workspace)
        {
            self.workspace_scroll.scroll_to_item(index);
        }
    }

    /// Runs a `workspace_panel_*` command on the focused row; `None` when
    /// no row holds focus, so the command is not this panel's.
    pub(super) fn run_workspace_panel_command(
        &mut self,
        command: huterm_protocol::CommandId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<Result<CommandOutcome, CommandError>> {
        let focused = self.focused_workspace_row(window)?;
        let rows = self.workspace_rows(cx);
        let at = rows.iter().position(|row| row.workspace == focused);
        let step = match command {
            ids::WORKSPACE_PANEL_NEXT => Some(huterm_ui::roving::Move::Next),
            ids::WORKSPACE_PANEL_PREVIOUS => {
                Some(huterm_ui::roving::Move::Previous)
            }
            ids::WORKSPACE_PANEL_FIRST => Some(huterm_ui::roving::Move::First),
            ids::WORKSPACE_PANEL_LAST => Some(huterm_ui::roving::Move::Last),
            _ => None,
        };
        if let Some(step) = step {
            if let Some(index) = huterm_ui::roving::target(rows.len(), at, step)
            {
                self.focus_workspace_row(rows[index].workspace, window, cx);
            }
            return Some(Ok(CommandOutcome::Completed));
        }
        let result = match command {
            ids::WORKSPACE_PANEL_SHOW => {
                self.run_workspace_row(focused, RowAction::Show, window, cx);
                Ok(CommandOutcome::Completed)
            }
            ids::WORKSPACE_PANEL_RENAME => {
                self.run_workspace_row(focused, RowAction::Rename, window, cx);
                Ok(CommandOutcome::Completed)
            }
            ids::WORKSPACE_PANEL_MENU => {
                let anchor = self
                    .workspace_row_bounds
                    .borrow()
                    .get(&focused)
                    .copied()
                    .map_or_else(
                        || window.mouse_position(),
                        |bounds| bounds.bottom_left(),
                    );
                self.open_workspace_row_menu(focused, anchor, true, window, cx);
                Ok(CommandOutcome::Completed)
            }
            ids::WORKSPACE_PANEL_EXIT => {
                self.focus_terminal(window, cx);
                cx.notify();
                Ok(CommandOutcome::Completed)
            }
            _ => return None,
        };
        Some(result)
    }

    /// Shows, renames, clears the name of, or closes `workspace`, as the
    /// palette's commands do with that workspace named.
    pub(super) fn run_workspace_row(
        &mut self,
        workspace: WorkspaceId,
        action: RowAction,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let command = match action {
            RowAction::Show => {
                // Showing the shown workspace changes nothing.
                if self.workspace_id(cx) == Some(workspace) {
                    return;
                }
                ids::SELECT_WORKSPACE
            }
            RowAction::Rename => ids::RENAME_WORKSPACE,
            RowAction::ClearName => ids::CLEAR_WORKSPACE_NAME,
            RowAction::Close => ids::CLOSE_WORKSPACE,
        };
        let invocation = CommandInvocation::new(
            command,
            vec![CommandArgument::new(
                "workspace",
                CommandValue::Workspace(workspace),
            )],
        );
        self.run_recorded(&invocation, window, cx);
    }

    /// Runs `invocation` as a menu or palette pick does, recording it as
    /// recent and frequent, and reports a failure.
    pub(super) fn run_recorded(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match self.invoke_interactive(invocation, window, cx) {
            Ok(_) => {
                self.recent.record(invocation.id);
                cx.global_mut::<Desktop>().frequency.record(invocation.id);
            }
            Err(error) => {
                self.report_failure("Command failed", error.to_string(), cx);
            }
        }
        cx.notify();
    }

    /// Whether the workspace menu is open, Huterm's or native.
    pub(super) fn workspace_menu_open(&self) -> bool {
        self.menu
            .as_ref()
            .is_some_and(|menu| menu.kind == MenuKind::Workspace)
            || self.native_menu_kind() == Some(&MenuKind::Workspace)
    }

    pub(super) fn workspace_menu_availability(
        &self,
        cx: &App,
    ) -> Result<(), CommandError> {
        self.check_available(false)?;
        if self.workspace_id(cx).is_none() {
            return Err(CommandError::Unavailable(
                "window shows no workspace".to_owned(),
            ));
        }
        Ok(())
    }

    /// The workspace menu's rows from the projection, the window model, and
    /// the chrome render last resolved, so a refresh between frames keeps
    /// the panel row.
    pub(super) fn workspace_menu_spec(&self, cx: &App) -> MenuSpec {
        let panel = if !self.workspace_ui_shown()
            || self.config.workspaces.show
                == huterm_config::WorkspaceShow::Never
        {
            PanelRow::None
        } else if self.workspace_collapsed {
            PanelRow::Show
        } else if self.workspace_chrome_now.get().edge().is_some() {
            PanelRow::Collapse
        } else {
            PanelRow::None
        };
        workspace_menu(&WorkspaceMenuInput {
            rows: self.workspace_rows(cx),
            current: self.workspace_id(cx),
            panel,
        })
    }

    /// The workspace menu's anchor.
    pub(super) fn workspace_menu_anchor(
        &self,
        layout: &ChromeLayout,
    ) -> Bounds<Pixels> {
        let trigger = match self.workspace_menu_from {
            WorkspaceMenuAnchor::Trigger => self.workspace_trigger_bounds.get(),
            WorkspaceMenuAnchor::EmptyState => None,
        };
        trigger
            .or_else(|| self.empty_state_switch_bounds())
            .unwrap_or_else(|| self.menu_anchor(layout))
    }

    /// Opens the workspace menu, or focuses it when it is already open;
    /// another open menu is replaced. `from_keyboard` selects the first
    /// row.
    pub(super) fn open_workspace_menu(
        &mut self,
        from_keyboard: bool,
        from: WorkspaceMenuAnchor,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.workspace_menu_availability(cx)?;
        if self.native_menu.is_some() {
            return Ok(CommandOutcome::Completed);
        }
        if let Some(menu) = &self.menu
            && menu.kind == MenuKind::Workspace
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
        self.workspace_menu_from = from;
        let contexts = window.context_stack();
        let spec = self.workspace_menu_spec(cx);
        let layout = self.chrome_layout(window);
        // The popover opens upward from the window's lower half, as from a
        // bottom bar.
        let anchor = MenuAnchor::Below(self.workspace_menu_anchor(&layout));
        if self.open_native_menu(MenuKind::Workspace, &spec, anchor, window, cx)
        {
            return Ok(CommandOutcome::Completed);
        }
        self.mount_menu(
            spec.to_listbox(Platform::current()),
            MenuKind::Workspace,
            None,
            contexts,
            from_keyboard,
            window,
            cx,
        );
        Ok(CommandOutcome::Completed)
    }

    /// The dropdown trigger: closes an open workspace menu, otherwise
    /// opens it with no selection.
    fn toggle_workspace_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self
            .menu
            .as_ref()
            .is_some_and(|menu| menu.kind == MenuKind::Workspace)
        {
            self.close_menu(MenuFocusReturn::Terminal, window, cx);
            return;
        }
        if let Err(error) = self.open_workspace_menu(
            false,
            WorkspaceMenuAnchor::Trigger,
            window,
            cx,
        ) {
            self.report_failure("Open Workspace Menu", error.to_string(), cx);
        }
    }

    /// Opens `workspace`'s row context menu at `pointer`, replacing any
    /// open menu. It refuses where the tab menu does.
    pub(super) fn open_workspace_row_menu(
        &mut self,
        workspace: WorkspaceId,
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
        let Some(spec) = self.workspace_row_menu_spec(workspace, cx) else {
            return;
        };
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        let contexts = window.context_stack();
        let kind = MenuKind::WorkspaceRow(workspace);
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

    /// A row's context menu, or `None` once the workspace left the
    /// window's session.
    pub(super) fn workspace_row_menu_spec(
        &self,
        workspace: WorkspaceId,
        cx: &App,
    ) -> Option<MenuSpec> {
        let rows = self.workspace_rows(cx);
        let row = rows.iter().find(|row| row.workspace == workspace)?;
        Some(row_menu(row, rows.len()))
    }

    /// Runs a workspace menu or row menu pick.
    pub(super) fn run_workspace_pick(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let state = cx.global::<Desktop>().hierarchy.state();
        let Some(pick) = parse_pick(id) else {
            self.report_failure(
                "Command failed",
                format!("unknown workspace menu item `{id}`"),
                cx,
            );
            return;
        };
        let target = |raw: u64| resolve(state, raw);
        let row = |raw| (raw, target(raw));
        let (action, (_, workspace)) = match pick {
            WorkspacePick::Command(command) => {
                self.run_recorded(
                    &CommandInvocation::new(command, Vec::new()),
                    window,
                    cx,
                );
                return;
            }
            WorkspacePick::Select(raw) => (RowAction::Show, row(raw)),
            WorkspacePick::Rename(raw) => (RowAction::Rename, row(raw)),
            WorkspacePick::ClearName(raw) => (RowAction::ClearName, row(raw)),
            WorkspacePick::Close(raw) => (RowAction::Close, row(raw)),
        };
        let Some(workspace) = workspace else {
            self.report_failure(
                "Command failed",
                "Workspace no longer exists".to_owned(),
                cx,
            );
            return;
        };
        self.run_workspace_row(workspace, action, window, cx);
    }

    /// The dropdown trigger (C1): the workspace icon, the shown
    /// workspace's name, dim when automatic, and a chevron. It records its
    /// painted bounds for the menu's anchor and never holds focus.
    pub(super) fn workspace_dropdown_element(
        &self,
        width: Pixels,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let state = cx.global::<Desktop>().hierarchy.state();
        let info = state.workspace(self.workspace_id(cx)?)?;
        let name = info.display_name().to_owned();
        let automatic = info.custom_name.is_none();
        let open = self.workspace_menu_open();
        let bounds_cell = Rc::clone(&self.workspace_trigger_bounds);
        let label_width = (width - menu_trigger::width_for_label(px(0.0)))
            .max(px(0.0))
            .min(self.workspace_label.width.ceil());
        let truncated = self.workspace_label.width.ceil() > label_width;
        let trigger = MenuTrigger::new(
            "workspace-dropdown",
            format!("Workspace: {name}"),
            name.clone(),
            Icon::LayoutGrid.asset_path(),
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
        .label_width(label_width)
        .tooltip(truncated.then(|| name.into()))
        .occlude()
        .stop_presses()
        .debug_selector(|| "workspace-dropdown".to_owned())
        .on_click(Self::button_action(cx, |view, window, cx| {
            view.toggle_workspace_menu(window, cx);
            cx.stop_propagation();
        }))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), _, _| bounds_cell.set(Some(bounds)),
            )
            .absolute()
            .inset_0(),
        )
        .h(CONTROL_SIZE);
        let trigger = if automatic {
            trigger.muted(colors.inactive)
        } else {
            trigger
        };
        Some(
            if open {
                trigger.child(self.popover.trigger_probe())
            } else {
                trigger
            }
            .into_any_element(),
        )
    }

    /// One workspace row (P1) or chip (H1): click shows it, a double click
    /// renames it, a right press opens its menu. A button named
    /// "Workspace: name, n tabs", selected while shown.
    #[expect(
        clippy::too_many_lines,
        reason = "one element tree, read top to bottom"
    )]
    fn workspace_row_element(
        &mut self,
        row: &WorkspaceRow,
        chip: bool,
        colors: TabColors,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let workspace = row.workspace;
        let shown = self.workspace_id(cx) == Some(workspace);
        let focus = self.workspace_focus(workspace, cx);
        let swatch = tokens(&self.config.theme);
        let targeted = self
            .menu
            .as_ref()
            .is_some_and(|menu| menu.kind == MenuKind::WorkspaceRow(workspace));
        let bounds_cell = Rc::clone(&self.workspace_row_bounds);
        // Shown, then custom, then automatic names: an automatic name
        // dims to the tab bar's inactive text and no further, so it still
        // reads as enabled in light themes.
        let name_color = if shown {
            colors.foreground
        } else if row.automatic {
            colors.inactive
        } else {
            colors.foreground.opacity(0.85)
        };
        let element = div()
            .id(gpui::ElementId::Name(
                format!("workspace-row-{}", workspace.get()).into(),
            ))
            .role(gpui::Role::Button)
            .aria_label(row.accessible_name())
            .aria_selected(shown)
            .track_focus(&focus.clone().tab_stop(shown))
            .key_context("WorkspacePanel")
            .debug_selector(move || {
                format!("workspace-row-{}", workspace.get())
            })
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .when(chip, |chip_row| {
                chip_row.h(CHIP_HEIGHT).px(px(8.0)).rounded(px(6.0))
            })
            .when(!chip, |list_row| {
                list_row
                    .h(ROW_HEIGHT)
                    .mx(ROW_MARGIN)
                    .px(px(8.0))
                    .rounded(px(6.0))
            })
            .cursor_pointer()
            .when(shown, |row| row.bg(colors.active))
            .when(!shown, |row| {
                row.hover(|style| style.bg(colors.hover))
                    .when(targeted, |row| row.bg(colors.hover))
            })
            .child(
                icon_element(
                    Icon::LayoutGrid,
                    if shown {
                        colors.foreground
                    } else {
                        colors.inactive
                    },
                )
                .size(px(13.0))
                .flex_none(),
            )
            .child(
                div()
                    .when(!chip, Styled::flex_1)
                    .min_w_0()
                    .truncate()
                    .text_color(name_color)
                    .child(row.name.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(typography::ui(11.0))
                    .text_color(colors.inactive)
                    .child(row.tabs.to_string()),
            )
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                // The row takes no focus from a press; keyboard focus
                // stays where it was, as on tabs. A chip in the title row
                // keeps the press from its move and double-click.
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(cx.listener(
                move |view, event: &gpui::ClickEvent, window, cx| {
                    cx.stop_propagation();
                    if event.click_count() >= 2 {
                        view.run_workspace_row(
                            workspace,
                            RowAction::Rename,
                            window,
                            cx,
                        );
                    } else {
                        view.run_workspace_row(
                            workspace,
                            RowAction::Show,
                            window,
                            cx,
                        );
                    }
                },
            ))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    view.open_workspace_row_menu(
                        workspace,
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
                        bounds_cell.borrow_mut().insert(workspace, bounds);
                    },
                )
                .absolute()
                .inset_0(),
            );
        huterm_ui::focus::ring_inset(element, &focus, swatch, window, cx)
            .into_any_element()
    }

    /// A 22-point square button in the panel header or bar end.
    fn workspace_control(
        id: &'static str,
        label: &'static str,
        icon: Icon,
        colors: TabColors,
        swatch: huterm_ui::tokens::Tokens,
        cx: &Context<'_, Self>,
        run: impl Fn(&mut Self, &mut Window, &mut Context<'_, Self>) + 'static,
    ) -> Button {
        Button::new(id, label, swatch)
            .debug_selector(move || id.to_owned())
            .colors(colors.control())
            .icon(
                icon_element(icon, colors.inactive).group_hover(id, |style| {
                    style.text_color(colors.foreground)
                }),
            )
            .icon_only()
            .group(id)
            .stop_presses()
            .size(px(22.0))
            .rounded(px(6.0))
            .on_click(Self::button_action(cx, run))
    }

    /// The panel's New Workspace and collapse buttons.
    fn workspace_controls(
        &self,
        colors: TabColors,
        cx: &Context<'_, Self>,
    ) -> [Button; 2] {
        let swatch = tokens(&self.config.theme);
        let collapse = match self.config.workspaces.position {
            huterm_config::WorkspacePosition::Left => Icon::PanelLeftClose,
            huterm_config::WorkspacePosition::Right => Icon::PanelRightClose,
            huterm_config::WorkspacePosition::Top => Icon::PanelTopClose,
            huterm_config::WorkspacePosition::Bottom => Icon::PanelBottomClose,
        };
        [
            Self::workspace_control(
                "workspace-new",
                "New Workspace",
                Icon::Plus,
                colors,
                swatch,
                cx,
                |view, window, cx| {
                    view.run_recorded(
                        &CommandInvocation::new(ids::NEW_WORKSPACE, Vec::new()),
                        window,
                        cx,
                    );
                },
            ),
            Self::workspace_control(
                "workspace-collapse",
                "Collapse Workspace Panel",
                collapse,
                colors,
                swatch,
                cx,
                |view, window, cx| {
                    if let Err(error) = view.toggle_workspace_panel(window, cx)
                    {
                        view.report_failure(
                            "Command failed",
                            error.to_string(),
                            cx,
                        );
                    }
                },
            ),
        ]
    }

    /// The side panel (P1): a strip above its rows that keeps the title
    /// row's gestures, the "WORKSPACES" header with New Workspace and
    /// collapse, and the rows, which scroll under the header with the tab
    /// column's overlay scrollbar. Its inner edge resizes it.
    #[expect(
        clippy::too_many_lines,
        reason = "one element tree, read top to bottom"
    )]
    pub(super) fn workspace_panel_element(
        &mut self,
        layout: &ChromeLayout,
        colors: TabColors,
        session: Option<AnyElement>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let WorkspaceChrome::Panel { edge, .. } = layout.workspace_chrome
        else {
            return None;
        };
        let band = layout.workspace?;
        let left = edge == workspace_chrome::Edge::Left;
        let rows = self.workspace_rows(cx);
        let row_elements: Vec<AnyElement> = rows
            .iter()
            .map(|row| {
                self.workspace_row_element(row, false, colors, window, cx)
            })
            .collect();
        let [new, collapse] = self.workspace_controls(colors, cx);
        let view = cx.entity();
        // Rows are the scrolled region's direct children, so revealing one
        // indexes it.
        let list = ScrollArea::new("workspace-rows", &self.workspace_scroll)
            .fill()
            .padding(gpui::Edges {
                bottom: px(6.0),
                ..gpui::Edges::default()
            })
            .children(row_elements)
            .render(
                &mut self.workspace_scrollbar,
                colors.scrollbar,
                &view,
                |view| &mut view.workspace_scrollbar,
                cx,
            );
        let titled = self.title_row() > px(0.0) || layout.frame.controls;
        let strip = layout.workspace_inset;
        let header = div()
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
                    .child("WORKSPACES"),
            )
            .child(new)
            .child(collapse);
        let handle_x = if left {
            band.size.width - HANDLE_WIDTH
        } else {
            px(0.0)
        };
        let panel = div()
            .id("workspace-panel")
            .debug_selector(|| "workspace-panel".to_owned())
            .role(gpui::Role::Group)
            .aria_label("Workspaces")
            .absolute()
            .left(band.origin.x)
            .top(band.origin.y)
            .w(band.size.width)
            .h(band.size.height)
            .bg(Self::workspace_panel_background(colors))
            .occlude()
            .flex()
            .flex_col()
            .text_size(tab_bar::TAB_TEXT_SIZE)
            .child({
                let top =
                    div().id("workspace-panel-strip").flex_none().h(strip);
                if titled {
                    self.title_row_gestures(
                        top.window_control_area(WindowControlArea::Drag),
                    )
                    .into_any_element()
                } else {
                    top.into_any_element()
                }
            })
            .children(session)
            .child(header)
            .child(list)
            .child(
                // The inner edge's divider.
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .when(left, Styled::right_0)
                    .when(!left, Styled::left_0)
                    .w(px(1.0))
                    .bg(colors.border),
            )
            .child(
                div()
                    .id("workspace-panel-resize")
                    .absolute()
                    .top_0()
                    .left(handle_x)
                    .w(HANDLE_WIDTH)
                    .h(band.size.height)
                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, window, cx| {
                            view.cancel_reorder(window, cx);
                            view.cancel_pane_drag(window, cx);
                            view.resizing_workspace = true;
                            cx.stop_propagation();
                        }),
                    ),
            );
        Some(panel.into_any_element())
    }

    /// The chips (H1) a horizontal bar shows, scrolling under floating
    /// arrows, with New Workspace and collapse pinned at the end.
    #[expect(
        clippy::too_many_lines,
        reason = "one element tree, read top to bottom"
    )]
    pub(super) fn workspace_chips_element(
        &mut self,
        colors: TabColors,
        session: Option<AnyElement>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let rows = self.workspace_rows(cx);
        let chips: Vec<AnyElement> = rows
            .iter()
            .map(|row| {
                self.workspace_row_element(row, true, colors, window, cx)
            })
            .collect();
        let [new, collapse] = self.workspace_controls(colors, cx);
        let swatch = tokens(&self.config.theme);
        let scroll = self.workspace_scroll.clone();
        let offset = scroll.offset().x;
        let max = scroll.max_offset().x;
        let arrow = |forward: bool, cx: &Context<'_, Self>| {
            let scroll = scroll.clone();
            Button::new(
                if forward {
                    "workspace-chips-forward"
                } else {
                    "workspace-chips-backward"
                },
                if forward {
                    "Scroll Workspaces Right"
                } else {
                    "Scroll Workspaces Left"
                },
                swatch,
            )
            .colors(ButtonColors {
                fill: Some(colors.bar),
                ..colors.control()
            })
            .icon(icon_element(
                if forward {
                    Icon::ChevronRight
                } else {
                    Icon::ChevronLeft
                },
                colors.inactive,
            ))
            .icon_only()
            .stop_presses()
            .absolute()
            .top(px(1.0))
            .when(forward, Styled::right_0)
            .when(!forward, Styled::left_0)
            .size(px(22.0))
            .rounded(px(6.0))
            .on_click(Self::button_action(cx, move |_, _, cx| {
                let bounds = scroll.bounds();
                let step = bounds.size.width * 0.75;
                let current = scroll.offset();
                let x = if forward {
                    current.x - step
                } else {
                    current.x + step
                }
                .clamp(-scroll.max_offset().x, px(0.0));
                scroll.set_offset(point(x, current.y));
                cx.notify();
            }))
        };
        let back = (offset < px(0.0)).then(|| arrow(false, cx));
        let forward = (offset > -max && max > px(0.0)).then(|| arrow(true, cx));
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .gap(px(4.0))
            .children(session.map(|session| {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.0))
                    .child(session)
                    .child(div().w(px(1.0)).h(px(16.0)).bg(colors.border))
            }))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h(CHIP_HEIGHT + px(2.0))
                    .child(
                        div()
                            .id("workspace-chips")
                            .debug_selector(|| "workspace-chips".to_owned())
                            .role(gpui::Role::Group)
                            .aria_label("Workspaces")
                            .size_full()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .px(px(1.0))
                            .overflow_x_scroll()
                            .track_scroll(&self.workspace_scroll)
                            .children(chips),
                    )
                    .children(back)
                    .children(forward),
            )
            .child(new)
            .child(collapse)
            .into_any_element()
    }

    /// A bar on its own band (H1): at the bottom, or at the top of a window
    /// without a title row. A bar that takes the title row draws inside it.
    pub(super) fn workspace_bar_element(
        &mut self,
        layout: &ChromeLayout,
        colors: TabColors,
        session: Option<AnyElement>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let WorkspaceChrome::Bar { edge } = layout.workspace_chrome else {
            return None;
        };
        let band = layout.workspace?;
        let top = edge == workspace_chrome::Edge::Top;
        let chips = self.workspace_chips_element(colors, session, window, cx);
        Some(
            div()
                .id("workspace-bar")
                .debug_selector(|| "workspace-bar".to_owned())
                .absolute()
                .left(band.origin.x)
                .top(band.origin.y)
                .w(band.size.width)
                .h(band.size.height)
                .bg(Self::workspace_panel_background(colors))
                .occlude()
                .flex()
                .items_center()
                .pl(px(6.0))
                .pr(px(6.0))
                .text_size(tab_bar::TAB_TEXT_SIZE)
                .child(chips)
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .when(top, Styled::bottom_0)
                        .when(!top, Styled::top_0)
                        .h(px(1.0))
                        .bg(colors.border),
                )
                .into_any_element(),
        )
    }

    /// Resizes the side panel from its inner edge, with the tab sidebar's
    /// limits; the width is per window.
    pub(super) fn resize_workspace_panel(
        &mut self,
        pointer: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let layout = self.chrome_layout(window);
        let Some(band) = layout.workspace else {
            return;
        };
        let left = layout.workspace_chrome.edge()
            == Some(workspace_chrome::Edge::Left);
        let desired = if left {
            pointer.x - band.origin.x
        } else {
            band.right() - pointer.x
        };
        self.workspace_width = desired
            .clamp(workspace_chrome::PANEL_MIN, workspace_chrome::PANEL_MAX);
        cx.global_mut::<Desktop>()
            .windows
            .set_workspace_width(self.window, self.workspace_width);
        self.relayout_terminals(window, cx);
        cx.notify();
    }

    /// Smoke output for the workspace chrome: which chrome shows, its band,
    /// the dropdown's painted bounds, the shown workspace, and the count.
    pub(super) fn workspace_smoke_state(
        &self,
        prefix: &str,
        window: &Window,
        cx: &App,
    ) -> String {
        let layout = self.chrome_layout(window);
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
        let chrome = match layout.workspace_chrome {
            WorkspaceChrome::None => "none".to_owned(),
            WorkspaceChrome::Dropdown { .. } => "dropdown".to_owned(),
            WorkspaceChrome::Panel { edge, .. } => format!("panel_{edge:?}"),
            WorkspaceChrome::Bar { edge } => format!("bar_{edge:?}"),
        }
        .to_lowercase();
        // Rows in session order while a panel or bar draws them.
        let rows = if layout.workspace_chrome.edge().is_some() {
            let painted = self.workspace_row_bounds.borrow();
            let rows: Vec<String> = self
                .workspace_rows(cx)
                .iter()
                .map(|row| rect(painted.get(&row.workspace).copied()))
                .collect();
            rows.join(";")
        } else {
            "none".to_owned()
        };
        format!(
            "{prefix}workspace_chrome={chrome} {prefix}workspace_band={} {prefix}workspace_rows={rows} {prefix}workspace_dropdown={} {prefix}workspace_collapsed={} {prefix}workspace_count={} {prefix}workspace_menu_open={} {prefix}workspace_shown={}",
            rect(layout.workspace),
            rect(
                self.workspace_trigger_bounds
                    .get()
                    .filter(|_| self.workspace_trigger_drawn.get())
            ),
            self.workspace_collapsed,
            self.workspace_count,
            self.workspace_menu_open(),
            self.workspace_id(cx).map_or(0, WorkspaceId::get),
        )
    }
}

impl WorkspaceView {
    /// Whether a right workspace panel, as render last resolved it, holds
    /// the title row's trailing corner.
    pub(super) fn right_panel_holds_corner(&self) -> bool {
        (self.title_row() > px(0.0) || self.window_frame().controls)
            && matches!(
                self.workspace_chrome_now.get(),
                WorkspaceChrome::Panel {
                    edge: workspace_chrome::Edge::Right,
                    ..
                }
            )
    }

    /// Whether a side workspace panel holds the title row's leading or
    /// trailing corner: only in a window with a title row.
    pub(super) fn panel_corners(&self, layout: &ChromeLayout) -> (bool, bool) {
        if self.title_row() <= px(0.0) && !layout.frame.controls {
            return (false, false);
        }
        match layout.workspace_chrome {
            WorkspaceChrome::Panel { edge, .. } => (
                edge == workspace_chrome::Edge::Left,
                edge == workspace_chrome::Edge::Right,
            ),
            _ => (false, false),
        }
    }

    /// The title row's corner controls a side panel holds, drawn over its
    /// top strip: the window buttons at its corner, and in the trailing
    /// corner the title-bar session control and the menu button.
    #[expect(
        clippy::too_many_arguments,
        reason = "the corner shows what the title row would have"
    )]
    pub(super) fn panel_corner_controls(
        &self,
        layout: &ChromeLayout,
        lead_held: bool,
        trail_held: bool,
        session: SessionControlPlacement,
        menu: MenuButtonPlacement,
        colors: TabColors,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Vec<AnyElement> {
        let mut controls = Vec::new();
        let content = layout.content;
        let titlebar = self.title_row();
        let frame = layout.frame;
        let row = |element: AnyElement, left: bool| {
            div()
                .absolute()
                .top(content.origin.y)
                .h(titlebar)
                .when(left, |row| row.left(content.origin.x))
                .when(!left, |row| {
                    row.right(window.viewport_size().width - content.right())
                })
                .flex()
                .items_center()
                .child(element)
                .into_any_element()
        };
        if lead_held && !frame.buttons.leading.is_empty() {
            let buttons = self
                .window_controls_element(
                    frame.buttons.leading,
                    true,
                    colors,
                    window,
                    cx,
                )
                .into_any_element();
            controls.push(row(buttons, true));
        }
        if trail_held {
            let mut group = div().flex().items_center();
            if session.host == SessionControlHost::TitleStrip
                && let Some(control) =
                    self.session_control_element(session, colors, cx)
            {
                group = group.child(control.mr(
                    if menu == MenuButtonPlacement::TitleStrip {
                        CONTROL_SLOT - CONTROL_SIZE
                    } else {
                        CONTROL_INSET
                    },
                ));
            }
            if menu == MenuButtonPlacement::TitleStrip {
                group = group.child(
                    self.menu_button_element(colors, cx).mr(CONTROL_INSET),
                );
            }
            if !frame.buttons.trailing.is_empty() {
                group = group.child(self.window_controls_element(
                    frame.buttons.trailing,
                    false,
                    colors,
                    window,
                    cx,
                ));
            }
            controls.push(row(group.into_any_element(), false));
        }
        controls
    }
}

/// What a row press, a row menu pick, or a panel key does to a workspace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RowAction {
    Show,
    Rename,
    ClearName,
    Close,
}
