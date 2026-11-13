//! The sidebar (internally the workspace panel): its workspace rows and
//! session switcher, the sidebar button and workspace switcher in the
//! window's corner, the switcher's popover and a narrow window's sheet,
//! row drag and drop, and the per-window state it keeps (visible and
//! width). [`super::workspace_chrome`] decides what a window shows; this
//! module draws it and handles its input.

use gpui::Hsla;
use huterm_config::{SessionSwitcherPosition, SwitcherStyle};
use huterm_ui::components::menu_trigger::{self, MenuTrigger};
use huterm_ui::components::popover::Placement;
use huterm_ui::components::scroll_area::ScrollArea;
use huterm_ui::typography;

use super::drag::{DropTarget, Gesture, Payload, RowKey, RowList};
use super::session_control::{SessionLabel, SessionListAnchor, switcher_label};
use super::sidebar_rows::{self, ListRow, ROW_GAP, ROW_HEIGHT};
use super::workspace_chrome::{CornerHost, Edge};
use super::workspace_menu::{
    RowPlace, WorkspacePick, WorkspaceRow, parse_pick, resolve, row_menu,
    workspace_rows,
};
use super::*;

/// A corner switcher's widest label; a longer name truncates.
pub(super) const SWITCHER_LABEL_MAX: Pixels = px(140.0);
/// The gap between the sidebar button and the switchers beside it.
pub(super) const CORNER_GAP: Pixels = px(4.0);
/// A list's header: "WORKSPACES" with New Workspace, or "SESSIONS".
pub(super) const HEADER_HEIGHT: Pixels = px(28.0);
/// The sidebar's session row, with its caption.
const SESSION_ROW_HEIGHT: Pixels = px(40.0);
/// The sidebar's resize grab zone along its inner edge.
const HANDLE_WIDTH: Pixels = px(4.0);
/// The workspace switcher's popover.
const POPOVER_WIDTH: Pixels = px(260.0);
/// The popover's rows scroll beyond this.
const POPOVER_ROWS_MAX: Pixels = px(300.0);

/// Where the workspace switcher's popover opened from, for its anchor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SwitcherAnchor {
    /// The workspace switcher beside the sidebar button.
    Switcher,
    /// The empty state's Switch Workspace… button.
    EmptyState,
    /// The sidebar button, from `open_workspace_switcher` while the window
    /// shows no switcher.
    Button,
    /// The menu button's place, from `open_workspace_switcher` while the
    /// window draws neither the switcher nor the sidebar button.
    Fallback,
}

/// What the workspace switcher shows while open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SwitcherMode {
    /// The popover, as tall as its content.
    Popover(SwitcherAnchor),
    /// A narrow window's sheet, the sidebar over the terminal.
    Sheet,
}

/// Where a sidebar's content draws.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Surface {
    Docked,
    Popover,
    Sheet,
}

/// Whether running `command` leaves an open switcher or session list open:
/// their own keys and the commands that open them or move focus into them.
/// Every other command closes them, as it closes an open menu.
pub(super) fn keeps_switcher(command: huterm_protocol::CommandId) -> bool {
    matches!(
        command,
        ids::SIDEBAR_NEXT
            | ids::SIDEBAR_PREVIOUS
            | ids::SIDEBAR_FIRST
            | ids::SIDEBAR_LAST
            | ids::SIDEBAR_SHOW
            | ids::SIDEBAR_RENAME
            | ids::SIDEBAR_MENU
            | ids::SIDEBAR_MOVE_UP
            | ids::SIDEBAR_MOVE_DOWN
            | ids::SIDEBAR_EXIT
            | ids::OPEN_WORKSPACE_SWITCHER
            | ids::OPEN_SESSION_MENU
            | ids::TOGGLE_SIDEBAR
            | ids::FOCUS_SIDEBAR
    )
}

/// One stop of the sidebar's keyboard walk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SidebarItem {
    Row(WorkspaceId),
    Session,
}

impl WorkspaceView {
    /// The workspace switcher's drawn width.
    pub(super) fn switcher_width(&self) -> Pixels {
        menu_trigger::width_for_label(
            self.workspace_label.width.ceil().min(SWITCHER_LABEL_MAX),
        )
    }

    /// The room the corner controls may take in a window whose content is
    /// `content_width` wide: past it the switchers show only their icons.
    pub(super) fn corner_room(content_width: Pixels) -> Pixels {
        content_width * 0.3
    }

    /// The switchers' width beside the sidebar button with their names,
    /// each after a gap.
    fn named_switchers_width(&self, chrome: WorkspaceChrome) -> Pixels {
        let session = if chrome.session() {
            CORNER_GAP + self.session_switcher_width()
        } else {
            px(0.0)
        };
        let workspace = if chrome.switcher() {
            CORNER_GAP + self.switcher_width()
        } else {
            px(0.0)
        };
        session + workspace
    }

    /// Whether the switchers show only their icons in a window whose
    /// content is `content_width` wide: as Settings ask, or where their
    /// names would take more than the corner's room. Reservation and
    /// drawing both read this.
    fn switchers_icon_only(
        &self,
        chrome: WorkspaceChrome,
        content_width: Pixels,
    ) -> bool {
        self.config.sidebar.switcher_style == SwitcherStyle::Icon
            || Self::corner_room(content_width)
                < CONTROL_SIZE + self.named_switchers_width(chrome)
    }

    /// The width the sidebar button and, beside it, the switchers take
    /// along a strip in a window whose content is `content_width` wide;
    /// `inline` is false where the switchers take rows instead.
    pub(super) fn corner_controls_width(
        &self,
        chrome: WorkspaceChrome,
        inline: bool,
        content_width: Pixels,
    ) -> Pixels {
        let switchers = if !inline {
            px(0.0)
        } else if self.switchers_icon_only(chrome, content_width) {
            let count =
                f32::from(u8::try_from(chrome.switchers()).unwrap_or(u8::MAX));
            (CORNER_GAP + CONTROL_SIZE) * count
        } else {
            self.named_switchers_width(chrome)
        };
        CONTROL_SIZE + switchers
    }

    /// Whether a right tab column's top strip holds the right corner's
    /// sidebar button, with the switchers in rows above its tabs as a left
    /// column has them; the strip is too narrow for both.
    pub(super) fn right_column_strip(
        &self,
        host: CornerHost,
        chrome: WorkspaceChrome,
    ) -> bool {
        host == CornerHost::TitleRow
            && chrome.edge() == Edge::Right
            && self.layout_tabs().position == TabPosition::Right
            && self.presentation() != Presentation::Hidden
    }

    /// Where the corner controls go. `geometry` asks as the tab bar's
    /// geometry does, which reserves their slot whether or not an overlay
    /// bar is revealed; otherwise as this frame draws them.
    pub(super) fn corner_host(
        &self,
        chrome: WorkspaceChrome,
        geometry: bool,
    ) -> CornerHost {
        let bar_shown = if geometry {
            self.presentation() != Presentation::Hidden
        } else {
            self.bar_drawn()
        };
        workspace_chrome::corner_host(
            chrome,
            self.title_row() > px(0.0),
            self.corner_tab_position(),
            bar_shown,
        )
    }

    /// The tab bar's share for the corner controls under `chrome`, in a
    /// window whose content is `content_width` wide.
    pub(super) fn corner_slot(
        &self,
        chrome: WorkspaceChrome,
        content_width: Pixels,
    ) -> Option<CornerSlot> {
        let titled = self.title_row() > px(0.0);
        let host = self.corner_host(chrome, true);
        let switchers = u8::try_from(chrome.switchers()).unwrap_or(u8::MAX);
        // A column gives each switcher a row; a left column without a title
        // row also holds the sidebar button, in the first.
        let rows = match (host, chrome.edge()) {
            (CornerHost::Bar, Edge::Left) => {
                return Some(CornerSlot::Lead(self.corner_controls_width(
                    chrome,
                    true,
                    content_width,
                )));
            }
            (CornerHost::Column, Edge::Left) if !titled => switchers.max(1),
            (CornerHost::Column, _) => switchers,
            (CornerHost::TitleRow, Edge::Right)
                if self.right_column_strip(host, chrome) =>
            {
                switchers
            }
            _ => 0,
        };
        (rows > 0).then_some(CornerSlot::Rows(rows))
    }

    /// The length the corner controls take from the end of the host that
    /// holds the menu button, on the right, in a window whose content is
    /// `content_width` wide; zero elsewhere.
    pub(super) fn corner_trailing(
        &self,
        host: CornerHost,
        chrome: WorkspaceChrome,
        content_width: Pixels,
    ) -> Pixels {
        if chrome.edge() != Edge::Right || chrome.docked() {
            return px(0.0);
        }
        match host {
            CornerHost::TitleRow | CornerHost::Bar => {
                let inline = !self.right_column_strip(host, chrome);
                self.corner_controls_width(chrome, inline, content_width)
                    + CORNER_GAP
            }
            CornerHost::Column => CONTROL_SLOT,
            CornerHost::Sidebar | CornerHost::Hidden => px(0.0),
        }
    }

    /// The length the corner controls take after the traffic lights or
    /// leading window buttons of the title row, on the left, in a window
    /// whose content is `content_width` wide; zero elsewhere.
    pub(super) fn corner_leading(
        &self,
        host: CornerHost,
        chrome: WorkspaceChrome,
        content_width: Pixels,
    ) -> Pixels {
        if chrome.edge() == Edge::Left && host == CornerHost::TitleRow {
            self.corner_controls_width(chrome, true, content_width) + CORNER_GAP
        } else {
            px(0.0)
        }
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
        let name = self
            .workspace_id(cx)
            .and_then(|workspace| state.workspace(workspace))
            .map(|info| info.display_name().to_owned());
        // Measured every frame, as the session's name is, so a font or
        // interface size change sizes the switcher at once.
        self.workspace_label = name
            .map_or_else(SessionLabel::default, |name| {
                switcher_label(name, window, cx)
            });
        self.workspace_chrome_now
            .set(self.workspace_chrome(window.viewport_size()));
    }

    /// The window's session's workspaces, as the sidebar and the switcher
    /// list them.
    pub(super) fn workspace_rows(&self, cx: &App) -> Vec<WorkspaceRow> {
        workspace_rows(
            cx.global::<Desktop>().hierarchy.state(),
            self.attached_session(cx),
        )
    }

    /// The sidebar's background: one step darker than the tab bar's, so it
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

    /// The focus handle of `workspace`'s row.
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

    /// The row that holds keyboard focus, if any.
    pub(super) fn focused_workspace_row(
        &self,
        window: &Window,
    ) -> Option<WorkspaceId> {
        self.workspace_focus
            .iter()
            .find(|(_, handle)| handle.is_focused(window))
            .map(|(workspace, _)| *workspace)
    }

    /// Whether any surface draws the rows this frame: the docked sidebar,
    /// the switcher's popover, or the sheet.
    pub(super) fn rows_drawn(&self, layout: &ChromeLayout) -> bool {
        layout.workspace_chrome.docked() || self.switcher_mode.is_some()
    }

    /// Hands focus to the terminal when the row that holds it is no longer
    /// drawn: its workspace closed or left the session, or the sidebar or
    /// popover went. A focus handle nothing draws would swallow every key.
    /// Drops the handles of workspaces the session no longer holds, ends a
    /// drag whose workspace went, and scrolls a newly shown workspace's row
    /// into view.
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
        let drawn = self.rows_drawn(layout);
        if !drawn && self.session_row_focus.is_focused(window) {
            cx.defer_in(window, |view, window, cx| {
                if view.session_row_focus.is_focused(window) {
                    view.focus_terminal(window, cx);
                }
            });
        }
        let focused = self.focused_workspace_row(window);
        if let Some(focused) = focused
            && (!drawn || !rows.contains(&focused))
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
        // A drag whose workspace or session went, or whose rows no longer
        // draw, ends without moving anything (W41).
        let attached = self.attached_session(cx);
        self.drop_row_drag(
            |payload| {
                matches!(payload, Payload::Workspace { session, workspace }
                    if !drawn
                        || !rows.contains(&workspace)
                        || attached != Some(session))
            },
            cx,
        );
        // A workspace the window starts to show, as one New Workspace just
        // made past the end of a long list, scrolls into view.
        let shown = self.workspace_id(cx);
        if drawn {
            if shown != self.workspace_revealed
                && let Some(workspace) = shown
            {
                self.reveal_workspace_row(workspace, cx);
            }
            self.workspace_revealed = shown;
            self.retry_reveal(window, cx);
        } else {
            self.workspace_revealed = None;
            self.reveal_pending = None;
        }
    }

    /// Repeats a reveal its list took before its first layout, which GPUI
    /// dropped: the list's first prepaint has no overflow to scroll yet.
    /// A list that has not laid out asks for one more frame; one that still
    /// has no height after it gives the reveal up.
    fn retry_reveal(&mut self, window: &mut Window, cx: &App) {
        let Some((workspace, waited)) = self.reveal_pending else {
            return;
        };
        let scroll = self.workspace_drag_scroll();
        if scroll.bounds().size.height <= px(0.0) {
            if waited {
                self.reveal_pending = None;
            } else {
                self.reveal_pending = Some((workspace, true));
                window.request_animation_frame();
            }
            return;
        }
        self.reveal_pending = None;
        if let Some(index) = self
            .workspace_rows(cx)
            .iter()
            .position(|row| row.workspace == workspace)
        {
            scroll.scroll_to_item(index);
        }
    }

    /// The `toggle_sidebar` command. A docked sidebar hides; an open sheet
    /// closes; otherwise the sidebar docks when the window is wide enough,
    /// and opens as a sheet when it is not. The sheet never changes the
    /// saved visible state. The terminal keeps its process, focus, and
    /// selection; only its size changes, as for a window resize.
    pub(super) fn toggle_sidebar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        // A row that holds focus goes with its sidebar.
        let focused = self.focused_workspace_row(window).is_some()
            || self.session_row_focus.is_focused(window);
        let viewport = window.viewport_size();
        let docked = self.workspace_chrome(viewport).docked();
        let sheet = self.switcher_mode == Some(SwitcherMode::Sheet);
        self.close_workspace_switcher(window, cx);
        if self
            .menu
            .as_ref()
            .is_some_and(|menu| matches!(menu.kind, MenuKind::WorkspaceRow(_)))
        {
            self.close_menu(MenuFocusReturn::Keep, window, cx);
        }
        if docked {
            self.set_workspace_visible(false, cx);
        } else if sheet {
            // The sheet closed; the saved state stays as it was.
        } else if self.workspace_chrome_for(viewport, true).docked() {
            self.set_workspace_visible(true, cx);
        } else {
            self.open_sheet(window, cx);
        }
        if focused && self.switcher_mode.is_none() {
            self.focus_terminal(window, cx);
        }
        self.relayout_terminals(window, cx);
        cx.notify();
    }

    /// Opens a narrow window's sheet with the shown row focused.
    fn open_sheet(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.switcher_mode = Some(SwitcherMode::Sheet);
        self.switcher_popover
            .open(Anchor::Trigger(self.chrome_layout(window).terminal), false);
        if let Some(workspace) = self.workspace_id(cx) {
            self.focus_workspace_row(workspace, window, cx);
        }
    }

    /// Records the window's visible state, saved with its layout.
    fn set_workspace_visible(&mut self, visible: bool, cx: &mut App) {
        self.workspace_visible = visible;
        cx.global_mut::<Desktop>()
            .windows
            .set_workspace_visible(self.window, visible);
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

    pub(super) fn focus_sidebar_availability(
        &self,
        cx: &App,
    ) -> Result<(), CommandError> {
        if self.workspace_id(cx).is_none() {
            return Err(CommandError::Unavailable(
                "window shows no workspace".to_owned(),
            ));
        }
        let chrome = self.workspace_chrome_now.get();
        if !chrome.docked() && !chrome.narrow() && self.switcher_mode.is_none()
        {
            return Err(CommandError::Unavailable(
                "the sidebar is hidden".to_owned(),
            ));
        }
        Ok(())
    }

    /// Moves keyboard focus to the shown workspace's row: in the docked
    /// sidebar, the open switcher, or the sheet a narrow window opens for
    /// it.
    pub(super) fn focus_sidebar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.focus_sidebar_availability(cx)?;
        let Some(workspace) = self.workspace_id(cx) else {
            return Err(CommandError::Unavailable(
                "window shows no workspace".to_owned(),
            ));
        };
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        if !self.workspace_chrome(window.viewport_size()).docked()
            && self.switcher_mode.is_none()
        {
            self.open_sheet(window, cx);
            return Ok(CommandOutcome::Completed);
        }
        self.focus_workspace_row(workspace, window, cx);
        Ok(CommandOutcome::Completed)
    }

    pub(super) fn focus_workspace_row(
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

    /// Scrolls the sidebar and the switcher so `workspace`'s row shows.
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
            self.switcher_scroll.scroll_to_item(index);
            // A drawn list that has not laid out yet drops the request.
            if self.workspace_drag_scroll().bounds().size.height <= px(0.0) {
                self.reveal_pending = Some((workspace, false));
            }
        }
    }

    /// The sidebar's keyboard stops in visual order: the workspace rows,
    /// with the session row after them at the bottom or before them at the
    /// top. The workspace switcher's popover has no session row.
    fn sidebar_items(&self, cx: &App) -> Vec<SidebarItem> {
        let mut items: Vec<SidebarItem> = self
            .workspace_rows(cx)
            .iter()
            .map(|row| SidebarItem::Row(row.workspace))
            .collect();
        let popover =
            matches!(self.switcher_mode, Some(SwitcherMode::Popover(_)));
        if !popover && self.session_control_state(cx).is_some() {
            if self.config.sidebar.session_switcher
                == SessionSwitcherPosition::Top
            {
                items.insert(0, SidebarItem::Session);
            } else {
                items.push(SidebarItem::Session);
            }
        }
        items
    }

    /// The sidebar stop that holds keyboard focus, if any.
    fn focused_sidebar_item(&self, window: &Window) -> Option<SidebarItem> {
        if self.session_row_focus.is_focused(window) {
            return Some(SidebarItem::Session);
        }
        self.focused_workspace_row(window).map(SidebarItem::Row)
    }

    /// Runs a `sidebar_*` command on the focused row or session switcher;
    /// `None` when neither holds focus, so the command is not the
    /// sidebar's. The arrows, Home, End, Tab, and Shift-Tab walk the rows
    /// and the session switcher; Return opens the focused one.
    pub(super) fn run_sidebar_command(
        &mut self,
        command: huterm_protocol::CommandId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<Result<CommandOutcome, CommandError>> {
        // The session list's rows take the keys while one holds focus.
        if let Some(result) = self.run_session_list_command(command, window, cx)
        {
            return Some(result);
        }
        let focused = self.focused_sidebar_item(window)?;
        let items = self.sidebar_items(cx);
        let at = items.iter().position(|item| *item == focused);
        let step = match command {
            ids::SIDEBAR_NEXT => Some(huterm_ui::roving::Move::Next),
            ids::SIDEBAR_PREVIOUS => Some(huterm_ui::roving::Move::Previous),
            ids::SIDEBAR_FIRST => Some(huterm_ui::roving::Move::First),
            ids::SIDEBAR_LAST => Some(huterm_ui::roving::Move::Last),
            _ => None,
        };
        if let Some(step) = step {
            match huterm_ui::roving::target(items.len(), at, step)
                .map(|index| items[index])
            {
                Some(SidebarItem::Row(workspace)) => {
                    self.focus_workspace_row(workspace, window, cx);
                }
                Some(SidebarItem::Session) => {
                    self.session_row_focus.focus(window, cx);
                    cx.notify();
                }
                None => {}
            }
            return Some(Ok(CommandOutcome::Completed));
        }
        if command == ids::SIDEBAR_EXIT {
            self.close_workspace_switcher(window, cx);
            self.focus_terminal(window, cx);
            cx.notify();
            return Some(Ok(CommandOutcome::Completed));
        }
        let SidebarItem::Row(focused) = focused else {
            // The session row opens the session list; the row commands have
            // nothing to rename or offer there.
            if command == ids::SIDEBAR_SHOW || command == ids::SIDEBAR_MENU {
                self.toggle_session_list(
                    SessionListAnchor::Sidebar,
                    window,
                    cx,
                );
            }
            return Some(Ok(CommandOutcome::Completed));
        };
        let result = match command {
            ids::SIDEBAR_SHOW => {
                self.run_workspace_row(focused, RowAction::Show, window, cx);
                Ok(CommandOutcome::Completed)
            }
            // The row's focus handle follows its workspace, so focus stays
            // on the moved row.
            ids::SIDEBAR_MOVE_UP | ids::SIDEBAR_MOVE_DOWN => {
                let up = command == ids::SIDEBAR_MOVE_UP;
                match self.move_workspace_step(Some(focused), up, window, cx) {
                    Err(CommandError::Unavailable(_)) => {
                        Ok(CommandOutcome::Completed)
                    }
                    result => result,
                }
            }
            ids::SIDEBAR_RENAME => {
                self.run_workspace_row(focused, RowAction::Rename, window, cx);
                Ok(CommandOutcome::Completed)
            }
            ids::SIDEBAR_MENU => {
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
            _ => return None,
        };
        Some(result)
    }

    /// Shows, renames, clears the name of, pins or unpins, or closes
    /// `workspace`, as the palette's commands do with that workspace named.
    /// Showing one from the switcher's popover or the sheet closes it.
    pub(super) fn run_workspace_row(
        &mut self,
        workspace: WorkspaceId,
        action: RowAction,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let command = match action {
            RowAction::Show => {
                if self.close_workspace_switcher(window, cx) {
                    self.focus_terminal(window, cx);
                }
                // Showing the shown workspace changes nothing.
                if self.workspace_id(cx) == Some(workspace) {
                    return;
                }
                ids::SELECT_WORKSPACE
            }
            RowAction::Rename => ids::RENAME_WORKSPACE,
            RowAction::ClearName => ids::CLEAR_WORKSPACE_NAME,
            RowAction::TogglePin => ids::TOGGLE_WORKSPACE_PIN,
            RowAction::NewWindow => ids::MOVE_WORKSPACE_TO_NEW_WINDOW,
            RowAction::MoveUp => ids::MOVE_WORKSPACE_UP,
            RowAction::MoveDown => ids::MOVE_WORKSPACE_DOWN,
            RowAction::ToSession => ids::MOVE_WORKSPACE_TO_SESSION,
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

    /// Whether the workspace switcher's popover or the sheet is open.
    pub(super) fn workspace_switcher_open(&self) -> bool {
        self.switcher_mode.is_some()
    }

    pub(super) fn workspace_switcher_availability(
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

    /// The popover's anchor: the control it opened from while painted,
    /// else the switcher, the empty state's button, or the sidebar button,
    /// else the menu button's place.
    fn switcher_anchor(
        &self,
        from: SwitcherAnchor,
        layout: &ChromeLayout,
    ) -> Bounds<Pixels> {
        let switcher = self
            .workspace_trigger_bounds
            .get()
            .filter(|_| self.workspace_trigger_drawn.get());
        let button = self
            .sidebar_button_bounds
            .get()
            .filter(|_| self.sidebar_button_drawn.get());
        let first = match from {
            SwitcherAnchor::Switcher => switcher,
            SwitcherAnchor::EmptyState => self.empty_state_switch_bounds(),
            SwitcherAnchor::Button => button,
            SwitcherAnchor::Fallback => None,
        };
        first
            .or(switcher)
            .or_else(|| self.empty_state_switch_bounds())
            .or(button)
            .unwrap_or_else(|| self.menu_anchor(layout))
    }

    /// Whether the open switcher lost what it opened from this frame: the
    /// popover's switcher, sidebar button, or empty state button, or the
    /// sheet's narrow window, which a docked sidebar replaces or which grew
    /// wide enough to dock one.
    pub(super) fn switcher_anchor_went(
        &self,
        layout: &ChromeLayout,
        window: &Window,
    ) -> bool {
        match self.switcher_mode {
            None => false,
            Some(SwitcherMode::Sheet) => {
                layout.workspace_chrome.docked()
                    || self
                        .workspace_chrome_for(window.viewport_size(), true)
                        .docked()
            }
            Some(SwitcherMode::Popover(from)) => {
                layout.workspace_chrome.docked()
                    || match from {
                        SwitcherAnchor::Switcher => {
                            !self.workspace_trigger_drawn.get()
                        }
                        SwitcherAnchor::Button => {
                            !self.sidebar_button_drawn.get()
                        }
                        SwitcherAnchor::EmptyState => {
                            self.empty_state_switch_bounds().is_none()
                        }
                        SwitcherAnchor::Fallback => false,
                    }
            }
        }
    }

    /// Opens the workspace switcher's popover with the shown row focused,
    /// or focuses that row when it is already open; an open menu closes.
    /// While the sidebar is docked its shown row takes focus instead.
    pub(super) fn open_workspace_switcher(
        &mut self,
        from: SwitcherAnchor,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.workspace_switcher_availability(cx)?;
        if self.palette.is_some() {
            return Err(CommandError::Unavailable(
                "command palette is open".to_owned(),
            ));
        }
        let Some(workspace) = self.workspace_id(cx) else {
            return Err(CommandError::Unavailable(
                "window shows no workspace".to_owned(),
            ));
        };
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        self.close_session_list(window, cx);
        if let Some(terminal) = self.active_view(cx) {
            terminal.update(cx, TerminalView::clear_composition);
        }
        let layout = self.chrome_layout(window);
        if !layout.workspace_chrome.docked() && self.switcher_mode.is_none() {
            self.switcher_mode = Some(SwitcherMode::Popover(from));
            self.switcher_popover.open(
                Anchor::Trigger(self.switcher_anchor(from, &layout)),
                false,
            );
            // Only a drawn opener mounts the trigger probe; a fallback
            // opening has none.
            if from == SwitcherAnchor::Fallback {
                self.switcher_popover.clear_trigger();
            }
        }
        self.focus_workspace_row(workspace, window, cx);
        Ok(CommandOutcome::Completed)
    }

    /// The switcher control: closes an open popover, otherwise opens it.
    fn toggle_workspace_switcher(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.close_workspace_switcher(window, cx) {
            self.focus_terminal(window, cx);
            return;
        }
        if let Err(error) =
            self.open_workspace_switcher(SwitcherAnchor::Switcher, window, cx)
        {
            self.report_failure(
                "Open Workspace Switcher",
                error.to_string(),
                cx,
            );
        }
    }

    /// Closes the switcher's popover or the sheet, with a session list or
    /// row menu opened from it; the caller moves focus. `false` when
    /// neither was open.
    pub(super) fn close_workspace_switcher(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let _ = self.switcher_popover.close();
        if self.switcher_mode.take().is_none() {
            return false;
        }
        self.drop_row_drag(
            |payload| matches!(payload, Payload::Workspace { .. }),
            cx,
        );
        // A menu opened from its rows goes with them, and hands focus back
        // to the terminal, since nothing it opened from is left to take it.
        // So does a session list opened from the sheet's session row.
        if self
            .menu
            .as_ref()
            .is_some_and(|menu| matches!(menu.kind, MenuKind::WorkspaceRow(_)))
        {
            self.close_menu(MenuFocusReturn::Terminal, window, cx);
        }
        if self.session_list == Some(SessionListAnchor::Sidebar)
            && !self.workspace_chrome_now.get().docked()
        {
            self.close_session_list(window, cx);
        }
        cx.notify();
        true
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
        let index = rows.iter().position(|row| row.workspace == workspace)?;
        let session = self.attached_session(cx);
        let other_session = cx
            .global::<Desktop>()
            .hierarchy
            .state()
            .sessions()
            .iter()
            .any(|other| Some(*other) != session);
        Some(row_menu(
            &rows[index],
            RowPlace {
                index,
                count: rows.len(),
                other_session,
            },
        ))
    }

    /// Runs a row menu pick.
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
        let (action, raw) = match pick {
            WorkspacePick::Rename(raw) => (RowAction::Rename, raw),
            WorkspacePick::ClearName(raw) => (RowAction::ClearName, raw),
            WorkspacePick::TogglePin(raw) => (RowAction::TogglePin, raw),
            WorkspacePick::NewWindow(raw) => (RowAction::NewWindow, raw),
            WorkspacePick::MoveUp(raw) => (RowAction::MoveUp, raw),
            WorkspacePick::MoveDown(raw) => (RowAction::MoveDown, raw),
            WorkspacePick::ToSession(raw) => (RowAction::ToSession, raw),
            WorkspacePick::Close(raw) => (RowAction::Close, raw),
        };
        let Some(workspace) = resolve(state, raw) else {
            self.report_failure(
                "Command failed",
                "Workspace no longer exists".to_owned(),
                cx,
            );
            return;
        };
        self.run_workspace_row(workspace, action, window, cx);
    }

    /// The window's session's workspaces in order.
    pub(super) fn workspace_order(&self, cx: &App) -> Vec<WorkspaceId> {
        self.workspace_rows(cx)
            .iter()
            .map(|row| row.workspace)
            .collect()
    }

    /// The scroll handle of the list a workspace row drag runs in.
    pub(super) fn workspace_drag_scroll(&self) -> gpui::ScrollHandle {
        if self.switcher_mode.is_some() {
            self.switcher_scroll.clone()
        } else {
            self.workspace_scroll.clone()
        }
    }

    /// Moves `workspace` of `session` into session `to`, its own for a
    /// reorder, before `before` or to the end, on a worker, with the window
    /// busy and Quit and checkpoints held (`hold_move`) until it settles.
    /// The projection applies the committed order,
    /// and a window that showed a workspace that left its session follows
    /// through its emptied close; this reports failure and scrolls a
    /// reordered row into view.
    pub(super) fn move_workspace(
        &mut self,
        session: SessionId,
        workspace: WorkspaceId,
        to: SessionId,
        before: Option<WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        self.busy = true;
        super::workspace_moves::hold_move(cx, self.window);
        let task = cx.background_executor().spawn(async move {
            #[cfg(all(test, feature = "test-support"))]
            if runtime.worker_hooks.enter(worker_hooks::Stage::Move).await {
                return Committed {
                    result: Err(RuntimeError::Stopped.into()),
                    seq: runtime.lock().hierarchy_seq(),
                };
            }
            runtime.move_workspace(session, workspace, to, before)
        });
        let app = cx.to_async();
        let (title, verb) = if to == session {
            ("Reorder workspace", "reorder")
        } else {
            ("Move workspace", "move")
        };
        cx.spawn_in(window, async move |view, cx| {
            let Committed { result, seq } = task.await;
            projected(&app, seq).await;
            let _ = view.update_in(cx, |view, window, cx| {
                view.busy = false;
                match result {
                    Err(error) if !terminating(cx) => view.report_failure(
                        title,
                        format!("Cannot {verb} workspace: {error}"),
                        cx,
                    ),
                    Ok(()) if to == session => {
                        view.reveal_workspace_row(workspace, cx);
                    }
                    _ => {}
                }
                view.reconcile_own(window, cx);
                super::workspace_moves::release_move(cx, view.window);
                view.resume_close(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The sidebar button: shows or hides the sidebar, or opens the sheet
    /// in a narrow window. It records its painted bounds and never holds
    /// focus.
    pub(super) fn sidebar_button_element(
        &self,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> Button {
        let chrome = self.workspace_chrome_now.get();
        let shown =
            chrome.docked() || self.switcher_mode == Some(SwitcherMode::Sheet);
        let label = if shown {
            "Hide Sidebar"
        } else {
            "Show Sidebar"
        };
        let icon = match chrome.edge() {
            Edge::Left => Icon::PanelLeft,
            Edge::Right => Icon::PanelRight,
        };
        let bounds_cell = Rc::clone(&self.sidebar_button_bounds);
        self.sidebar_button_drawn.set(true);
        // The sheet, or a popover the button anchors, takes it as its
        // trigger, so a press on it toggles rather than counting as outside.
        let trigger = matches!(
            self.switcher_mode,
            Some(
                SwitcherMode::Sheet
                    | SwitcherMode::Popover(SwitcherAnchor::Button)
            )
        );
        Button::new("sidebar-button", label, tokens(&self.config.theme))
            .colors(colors.control())
            .icon(
                icon_element(
                    icon,
                    if shown {
                        colors.foreground
                    } else {
                        colors.inactive
                    },
                )
                .w(px(14.0))
                .h(px(14.0))
                .group_hover("sidebar-button", |style| {
                    style.text_color(colors.foreground)
                }),
            )
            .icon_only()
            .toggled(shown)
            .group("sidebar-button")
            .debug_selector(|| "sidebar-button".to_owned())
            .occlude()
            .stop_presses()
            .size(CONTROL_SIZE)
            .rounded_md()
            .on_click(Self::button_action(cx, |view, window, cx| {
                view.toggle_sidebar(window, cx);
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
            .when(trigger, |button| {
                button.child(self.switcher_popover.trigger_probe())
            })
    }

    /// The workspace switcher: the workspace icon, the shown workspace's
    /// name, dim when automatic, and a chevron, `width` wide, or the icon
    /// alone. It records its painted bounds for the popover's anchor and
    /// never holds focus.
    pub(super) fn workspace_switcher_element(
        &self,
        width: Pixels,
        icon_only: bool,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let state = cx.global::<Desktop>().hierarchy.state();
        let info = state.workspace(self.workspace_id(cx)?)?;
        let name = info.display_name().to_owned();
        let automatic = info.custom_name.is_none();
        // Only a popover opened from the switcher shows it open and takes
        // it as the trigger; another opener keeps that for itself.
        let open = self.switcher_mode
            == Some(SwitcherMode::Popover(SwitcherAnchor::Switcher));
        let bounds_cell = Rc::clone(&self.workspace_trigger_bounds);
        self.workspace_trigger_drawn.set(true);
        let label_width = (width - menu_trigger::width_for_label(px(0.0)))
            .max(px(0.0))
            .min(self.workspace_label.width.ceil());
        let truncated =
            icon_only || self.workspace_label.width.ceil() > label_width;
        let trigger = MenuTrigger::new(
            "workspace-switcher",
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
        .collapsed(icon_only)
        .label_width(label_width)
        .tooltip(truncated.then(|| name.into()))
        .occlude()
        .stop_presses()
        .debug_selector(|| "workspace-switcher".to_owned())
        .on_click(Self::button_action(cx, |view, window, cx| {
            view.toggle_workspace_switcher(window, cx);
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
        let trigger = if automatic {
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
                trigger.child(self.switcher_popover.trigger_probe())
            } else {
                trigger
            }
            .into_any_element(),
        )
    }

    /// The sidebar button with the switchers on its inner side, the
    /// session switcher first, as one row for a title row, a tab bar, or a
    /// strip, in a window whose content is `content_width` wide; the
    /// switchers show their icons alone where
    /// [`Self::corner_controls_width`] reserves only that.
    pub(super) fn corner_controls_element(
        &self,
        chrome: WorkspaceChrome,
        inline_switchers: bool,
        content_width: Pixels,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> gpui::Div {
        let button = self.sidebar_button_element(colors, cx);
        let icon_only = self.switchers_icon_only(chrome, content_width);
        let mut switchers = Vec::new();
        if inline_switchers && chrome.session() {
            switchers.extend(self.session_switcher_element(
                self.session_switcher_width(),
                icon_only,
                colors,
                cx,
            ));
        }
        if inline_switchers && chrome.switcher() {
            switchers.extend(self.workspace_switcher_element(
                self.switcher_width(),
                icon_only,
                colors,
                cx,
            ));
        }
        let row = div().flex().flex_none().items_center().gap(CORNER_GAP);
        match chrome.edge() {
            Edge::Left => row.child(button).children(switchers),
            Edge::Right => {
                row.children(switchers.into_iter().rev()).child(button)
            }
        }
    }

    /// The switchers filling a tab column's rows above its tabs, the session
    /// switcher first, with the sidebar button before the first when the
    /// column holds it.
    pub(super) fn column_corner_element(
        &self,
        chrome: WorkspaceChrome,
        with_button: bool,
        width: Pixels,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> gpui::Div {
        let switcher = |view: &Self,
                        session: bool,
                        room: Pixels,
                        cx: &mut Context<'_, Self>| {
            let icon_only = view.config.sidebar.switcher_style
                == SwitcherStyle::Icon
                || room < menu_trigger::width_for_label(px(28.0));
            if session {
                view.session_switcher_element(room, icon_only, colors, cx)
            } else {
                view.workspace_switcher_element(room, icon_only, colors, cx)
            }
        };
        let kinds: Vec<bool> = [chrome.session(), chrome.switcher()]
            .into_iter()
            .zip([true, false])
            .filter_map(|(shown, session)| shown.then_some(session))
            .collect();
        let button =
            with_button.then(|| self.sidebar_button_element(colors, cx));
        let first_room = if with_button {
            width - CONTROL_SIZE - CORNER_GAP
        } else {
            width
        }
        .max(px(0.0));
        let mut rows = Vec::new();
        let mut first = div()
            .flex()
            .items_center()
            .gap(CORNER_GAP)
            .w(width)
            .h(CONTROL_SIZE)
            .children(button);
        let mut kinds = kinds.into_iter();
        if let Some(session) = kinds.next() {
            first = first.children(switcher(self, session, first_room, cx));
        }
        rows.push(first);
        for session in kinds {
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .w(width)
                    .h(CONTROL_SIZE)
                    .children(switcher(self, session, width, cx)),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(workspace_chrome::CORNER_MARGIN)
            .w(width)
            .children(rows)
    }

    /// `workspace`'s status dot in this window: bells this window has not
    /// seen mark a workspace it does not show; the one it shows marks none,
    /// since its tabs do.
    pub(super) fn workspace_row_status(
        &self,
        workspace: WorkspaceId,
        cx: &App,
    ) -> Option<workspace_status::RowStatus> {
        let desktop = cx.global::<Desktop>();
        let bells = (self.workspace_id(cx) != Some(workspace))
            .then(|| desktop.windows.record(self.window))
            .flatten()
            .map(|record| &record.bells);
        workspace_status::workspace_status(
            desktop.hierarchy.state(),
            workspace,
            &desktop.statuses,
            bells,
            self.config.tabs,
        )
    }

    /// One workspace row: a press shows it, a drag reorders it, a double
    /// click renames it, a right press opens its menu, and in the popover
    /// its "⋯" button does too. A button named "Workspace: name, n tabs",
    /// selected while shown, with a pin mark when pinned and a status dot
    /// whose tooltip names the tabs.
    #[expect(
        clippy::too_many_lines,
        reason = "one row element and its listeners, read top to bottom"
    )]
    fn workspace_row_element(
        &mut self,
        row: &WorkspaceRow,
        last: bool,
        surface: Surface,
        colors: TabColors,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let workspace = row.workspace;
        let focus = self.workspace_focus(workspace, cx);
        let swatch = tokens(&self.config.theme);
        // The row's menu is open, native or Huterm's own.
        let targeted = self.menu_targets(&MenuKind::WorkspaceRow(workspace));
        let gesture = self.drag.as_ref().filter(|drag| drag.dragging());
        let drag = gesture
            .filter(|drag| matches!(drag.payload, Payload::Workspace { .. }));
        let dragged = drag.is_some_and(|drag| {
            drag.payload.row() == Some(RowKey::Workspace(workspace))
        });
        // This window's own workspace drag, or another window's over it.
        let incoming = self.incoming.as_ref().filter(|incoming| {
            matches!(incoming.payload.drag, Payload::Workspace { .. })
        });
        let slot = drag
            .and_then(Gesture::target)
            .or_else(|| incoming.and_then(|incoming| incoming.hover.target))
            .and_then(|target| match target {
                DropTarget::RowSlot(before) => Some(before),
                _ => None,
            });
        // A tab or pane over this row, here or from another window.
        let target = gesture
            .filter(|drag| drag.payload.in_bar() && !drag.outside)
            .and_then(Gesture::target)
            .or_else(|| {
                self.incoming
                    .as_ref()
                    .and_then(|incoming| incoming.hover.target)
            });
        let drop_target = matches!(
            target,
            Some(DropTarget::Workspace { workspace: over, .. }) if over == workspace
        );
        let tearing_off = dragged && self.tear_off_pill(cx);
        let bounds_cell = Rc::clone(&self.workspace_row_bounds);
        let more = (surface == Surface::Popover).then(|| {
            sidebar_rows::more_button(
                format!("workspace-row-more-{}", workspace.get()),
                format!("Workspace actions: {}", row.name),
                targeted,
                colors,
                swatch,
            )
            .on_click(Self::button_action(cx, move |view, window, cx| {
                cx.stop_propagation();
                let anchor = view
                    .workspace_row_bounds
                    .borrow()
                    .get(&workspace)
                    .copied()
                    .map_or_else(
                        || window.mouse_position(),
                        |bounds| bounds.bottom_right(),
                    );
                view.open_workspace_row_menu(
                    workspace, anchor, false, window, cx,
                );
            }))
            .into_any_element()
        });
        let element = sidebar_rows::list_row(
            ListRow {
                id: format!("workspace-row-{}", workspace.get()),
                icon: Icon::LayoutGrid,
                name: row.name.clone(),
                automatic: row.automatic,
                detail: row.detail(),
                accessible: row.accessible_name(),
                selected: self.workspace_id(cx) == Some(workspace),
                targeted,
                dragged,
                drop_target,
                tearing_off,
                line_above: slot == Some(Some(RowKey::Workspace(workspace))),
                line_below: last && !dragged && slot == Some(None),
                badges: Vec::new(),
                pinned: row.pinned,
                status: self.workspace_row_status(workspace, cx),
            },
            &focus,
            more,
            colors,
            swatch,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                // The row takes no focus from a press; keyboard focus
                // stays where it was, as on tabs.
                window.prevent_default();
                cx.stop_propagation();
                if event.click_count >= 2 {
                    view.cancel_drag(window, cx);
                    view.run_workspace_row(
                        workspace,
                        RowAction::Rename,
                        window,
                        cx,
                    );
                } else {
                    view.begin_row_drag(
                        RowKey::Workspace(workspace),
                        event.position,
                        window,
                        cx,
                    );
                }
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                view.cancel_drag(window, cx);
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

    /// The sidebar's session row: the layers icon, the session's name over
    /// its workspace and tab counts, and an up-down chevron. It opens the
    /// session list.
    fn session_row_element(
        &self,
        colors: TabColors,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let state = self.session_control_state(cx)?;
        let caption = self
            .session_rows(cx)
            .iter()
            .find(|row| row.session == state.session)
            .map(super::session_menu::SessionRow::detail)
            .unwrap_or_default();
        let bounds_cell = Rc::clone(&self.session_sidebar_bounds);
        self.session_sidebar_drawn.set(true);
        let open = self.session_list == Some(SessionListAnchor::Sidebar);
        let name_color = if state.automatic {
            colors.inactive
        } else {
            colors.foreground
        };
        let focus = self.session_row_focus.clone();
        let row = div()
            .id("session-switcher")
            .debug_selector(|| "session-switcher".to_owned())
            .role(gpui::Role::Button)
            .aria_label(format!("{}, {caption}", state.accessible_name()))
            .aria_expanded(open)
            .track_focus(&focus.clone().tab_stop(true))
            .key_context("Sidebar")
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .h(SESSION_ROW_HEIGHT)
            .mx(sidebar_rows::ROW_MARGIN)
            .px(px(8.0))
            .rounded(px(7.0))
            .cursor_pointer()
            .when(open, |row| row.bg(colors.active))
            .when(!open, |row| row.hover(|style| style.bg(colors.hover)))
            .child(icon_element(Icon::Layers, name_color).size(px(14.0)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_color(name_color)
                            .when(!state.automatic, |name| {
                                name.font_weight(gpui::FontWeight::SEMIBOLD)
                            })
                            .child(state.name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(typography::ui(10.5))
                            .text_color(colors.inactive)
                            .child(caption),
                    ),
            )
            .child(
                icon_element(Icon::ChevronsUpDown, colors.inactive)
                    .size(px(12.0))
                    .flex_none(),
            )
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |view, _, window, cx| {
                cx.stop_propagation();
                view.toggle_session_list(
                    SessionListAnchor::Sidebar,
                    window,
                    cx,
                );
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), _, _| bounds_cell.set(Some(bounds)),
                )
                .absolute()
                .inset_0(),
            );
        let row = if open {
            row.child(self.session_list_popover.trigger_probe())
        } else {
            row
        };
        Some(
            huterm_ui::focus::ring_inset(
                row,
                &focus,
                tokens(&self.config.theme),
                window,
                cx,
            )
            .into_any_element(),
        )
    }

    /// New Workspace, for the sidebar's header.
    fn new_workspace_button(
        &self,
        colors: TabColors,
        cx: &Context<'_, Self>,
    ) -> Button {
        let id = "workspace-new";
        Button::new(id, "New Workspace", tokens(&self.config.theme))
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
                view.run_recorded(
                    &CommandInvocation::new(ids::NEW_WORKSPACE, Vec::new()),
                    window,
                    cx,
                );
            }))
    }

    /// The sidebar's content on any surface: the session row at the top or
    /// bottom, except in the popover, the "WORKSPACES" header with New
    /// Workspace, and the rows, which scroll with the tab column's overlay
    /// scrollbar. The rows grow to fill a docked sidebar or the sheet and
    /// stay as tall as they need in the popover, up to its cap.
    fn sidebar_content(
        &mut self,
        surface: Surface,
        colors: TabColors,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Vec<AnyElement> {
        let rows = self.workspace_rows(cx);
        let count = rows.len();
        let row_elements: Vec<AnyElement> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                self.workspace_row_element(
                    row,
                    index + 1 == count,
                    surface,
                    colors,
                    window,
                    cx,
                )
            })
            .collect();
        let view = cx.entity();
        let docked = surface == Surface::Docked;
        let handle = if docked {
            self.workspace_scroll.clone()
        } else {
            self.switcher_scroll.clone()
        };
        // Rows are the scrolled region's direct children, so revealing one
        // indexes it.
        let list = ScrollArea::new(
            if docked {
                "workspace-rows"
            } else {
                "switcher-rows"
            },
            &handle,
        )
        .padding(gpui::Edges {
            bottom: px(6.0),
            ..gpui::Edges::default()
        })
        .children(row_elements);
        let list = if surface == Surface::Popover {
            list.max_height(Some(POPOVER_ROWS_MAX))
        } else {
            list.fill()
        };
        let list = if docked {
            list.render(
                &mut self.workspace_scrollbar,
                colors.scrollbar,
                &view,
                |view| &mut view.workspace_scrollbar,
                cx,
            )
        } else {
            list.render(
                &mut self.switcher_scrollbar,
                colors.scrollbar,
                &view,
                |view| &mut view.switcher_scrollbar,
                cx,
            )
        };
        let header = Self::list_header(
            "WORKSPACES",
            self.new_workspace_button(colors, cx).into_any_element(),
            colors,
        );
        // The popover's session switcher sits beside it in the corner.
        let session = if surface == Surface::Popover {
            None
        } else {
            self.session_row_element(colors, window, cx)
        };
        let top = self.config.sidebar.session_switcher
            == SessionSwitcherPosition::Top;
        let session = session.map(|session| {
            if top {
                div().flex_none().pt(px(2.0)).child(session)
            } else {
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(colors.border)
                    .py(px(6.0))
                    .child(session)
            }
            .into_any_element()
        });
        let mut content = Vec::with_capacity(3);
        let mut session = session;
        if top {
            content.extend(session.take());
        }
        content.push(header.into_any_element());
        content.push(list.into_any_element());
        content.extend(session);
        content.extend(self.lifted_row(RowList::Workspaces, colors, cx));
        content
    }

    /// The docked sidebar: a corner strip that keeps the title row's
    /// gestures, then its content. Its inner edge resizes it. Without a
    /// title row the strip holds the sidebar button itself.
    pub(super) fn workspace_panel_element(
        &mut self,
        layout: &ChromeLayout,
        colors: TabColors,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let WorkspaceChrome::Panel { edge, .. } = layout.workspace_chrome
        else {
            return None;
        };
        let band = layout.workspace?;
        let left = edge == Edge::Left;
        let content = self.sidebar_content(Surface::Docked, colors, window, cx);
        let titled = self.title_row() > px(0.0) || layout.frame.controls;
        let strip = layout.workspace_inset;
        let handle_x = if left {
            band.size.width - HANDLE_WIDTH
        } else {
            px(0.0)
        };
        // Without a title row the button keeps the inset it has while the
        // sidebar is hidden: a tab column's row margin, or a bar's corner
        // margin.
        let inset = if self.layout_tabs().position.vertical() {
            tab_bar::VERTICAL_ROW_MARGIN_X
        } else {
            workspace_chrome::CORNER_MARGIN
        };
        let corner = div()
            .id("workspace-panel-strip")
            .flex_none()
            .h(strip)
            .flex()
            .items_center()
            .px(inset)
            .when(!left, Styled::justify_end);
        let corner = if titled {
            // The title row's corner controls draw over this strip.
            self.title_row_gestures(
                corner.window_control_area(WindowControlArea::Drag),
            )
            .into_any_element()
        } else {
            corner
                .child(self.sidebar_button_element(colors, cx))
                .into_any_element()
        };
        let panel = div()
            .id("workspace-panel")
            .debug_selector(|| "workspace-panel".to_owned())
            .role(gpui::Role::Group)
            .aria_label("Sidebar")
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
            .child(corner)
            .children(content)
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
                            view.cancel_drag(window, cx);
                            view.resizing_workspace = true;
                            cx.stop_propagation();
                        }),
                    ),
            );
        Some(panel.into_any_element())
    }

    /// The open switcher: the popover under its anchor, as tall as its
    /// content, or the sheet over the terminal on the sidebar's side,
    /// below the title row.
    pub(super) fn workspace_switcher_overlay(
        &mut self,
        layout: &ChromeLayout,
        colors: TabColors,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let mode = self.switcher_mode?;
        let viewport = window.viewport_size();
        let swatch = tokens(&self.config.theme);
        match mode {
            SwitcherMode::Popover(from) => {
                let anchor = self.switcher_anchor(from, layout);
                let rows = f32::from(
                    u16::try_from(self.workspace_count.max(1))
                        .unwrap_or(u16::MAX),
                );
                let height = HEADER_HEIGHT
                    + ((ROW_HEIGHT + ROW_GAP) * rows + px(6.0))
                        .min(POPOVER_ROWS_MAX)
                    + px(10.0);
                let placement = place(
                    Anchor::Trigger(anchor),
                    size(POPOVER_WIDTH, height),
                    viewport,
                );
                let content =
                    self.sidebar_content(Surface::Popover, colors, window, cx);
                // The surface stops presses over it, as a menu's does.
                let panel = div()
                    .id("workspace-switcher-popover")
                    .debug_selector(|| "workspace-switcher-popover".to_owned())
                    .occlude()
                    .role(gpui::Role::Dialog)
                    .aria_label("Workspaces")
                    .w(POPOVER_WIDTH)
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
                    .children(content);
                self.switcher_popover.render(placement, viewport, panel)
            }
            SwitcherMode::Sheet => {
                let content = layout.content;
                let mut top = content.origin.y
                    + self.title_row()
                    + self.fullscreen_insets.top.max(px(0.0));
                // Below a top tab bar too, which may hold the sidebar
                // button: the sheet never covers the button that closes it.
                if matches!(
                    self.layout_tabs().position,
                    TabPosition::Top | TabPosition::Titlebar
                ) && self.bar_drawn()
                {
                    top = top.max(layout.tabs.bottom());
                }
                let width = workspace_chrome::panel_width(
                    self.workspace_width,
                    content.size.width,
                );
                let left = match layout.workspace_chrome.edge() {
                    Edge::Left => content.origin.x,
                    Edge::Right => content.right() - width,
                };
                let height = (content.bottom() - top).max(px(0.0));
                let placement = Placement {
                    origin: point(left, top),
                    opens_up: false,
                    align_right: false,
                    max_height: height,
                };
                let body =
                    self.sidebar_content(Surface::Sheet, colors, window, cx);
                let sheet = div()
                    .id("workspace-sidebar-sheet")
                    .debug_selector(|| "workspace-sidebar-sheet".to_owned())
                    .occlude()
                    .role(gpui::Role::Dialog)
                    .aria_label("Sidebar")
                    .w(width)
                    .h(height)
                    .flex()
                    .flex_col()
                    .pt(px(4.0))
                    .bg(Self::workspace_panel_background(colors))
                    .shadow(swatch.shadow())
                    .text_size(tab_bar::TAB_TEXT_SIZE)
                    .text_color(colors.foreground)
                    .children(body);
                self.switcher_popover.render(placement, viewport, sheet)
            }
        }
    }

    /// Resizes the sidebar from its inner edge, with the tab column's
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
        let desired = match layout.workspace_chrome.edge() {
            Edge::Left => pointer.x - band.origin.x,
            Edge::Right => band.right() - pointer.x,
        };
        // The terminal keeps the width docking needs, so a drag never
        // undocks the sidebar under the pointer.
        let fits = band.size.width
            + (layout.terminal.size.width
                - workspace_chrome::MIN_TERMINAL_WIDTH)
                .max(px(0.0));
        self.workspace_width = desired
            .clamp(workspace_chrome::PANEL_MIN, workspace_chrome::PANEL_MAX)
            .min(fits.max(workspace_chrome::PANEL_MIN));
        cx.global_mut::<Desktop>()
            .windows
            .set_workspace_width(self.window, self.workspace_width);
        self.relayout_terminals(window, cx);
        cx.notify();
    }

    /// Smoke output for the sidebar: which chrome shows, its band, the rows'
    /// painted bounds, the switcher's and the sidebar button's, the visible
    /// state, the count, what the switcher shows, and the shown workspace.
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
            WorkspaceChrome::Hidden {
                edge, narrow: true, ..
            } => format!("narrow_{edge:?}"),
            WorkspaceChrome::Hidden { edge, .. } => format!("hidden_{edge:?}"),
            WorkspaceChrome::Panel { edge, .. } => format!("panel_{edge:?}"),
        }
        .to_lowercase();
        // Rows in session order while a surface draws them.
        let rows = if self.rows_drawn(&layout) {
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
        let switcher = match self.switcher_mode {
            None => "none",
            Some(SwitcherMode::Popover(_)) => "popover",
            Some(SwitcherMode::Sheet) => "sheet",
        };
        format!(
            "{prefix}workspace_chrome={chrome} {prefix}workspace_band={} {prefix}workspace_rows={rows} {prefix}workspace_switcher={} {prefix}sidebar_button={} {prefix}sidebar_visible={} {prefix}workspace_count={} {prefix}workspace_switcher_open={switcher} {prefix}workspace_shown={}",
            rect(layout.workspace),
            rect(
                self.workspace_trigger_bounds
                    .get()
                    .filter(|_| self.workspace_trigger_drawn.get())
            ),
            rect(
                self.sidebar_button_bounds
                    .get()
                    .filter(|_| self.sidebar_button_drawn.get())
            ),
            self.workspace_visible,
            self.workspace_count,
            self.workspace_id(cx).map_or(0, WorkspaceId::get),
        )
    }
}

impl WorkspaceView {
    /// Whether a docked right sidebar, as render last resolved it, holds
    /// the title row's trailing corner.
    pub(super) fn right_panel_holds_corner(&self) -> bool {
        (self.title_row() > px(0.0) || self.window_frame().controls)
            && self.workspace_chrome_now.get().docked_on(Edge::Right)
    }

    /// Whether a docked sidebar holds the title row's leading or trailing
    /// corner: only in a window with a title row.
    pub(super) fn panel_corners(&self, layout: &ChromeLayout) -> (bool, bool) {
        if self.title_row() <= px(0.0) && !layout.frame.controls {
            return (false, false);
        }
        (
            layout.workspace_chrome.docked_on(Edge::Left),
            layout.workspace_chrome.docked_on(Edge::Right),
        )
    }

    /// The title row's corner controls a docked sidebar holds, drawn over
    /// its corner strip: the window buttons at its corner and the sidebar
    /// button beside them, and in the trailing corner the title-bar session
    /// control and the menu button.
    #[expect(
        clippy::too_many_arguments,
        reason = "the corner shows what the title row would have"
    )]
    pub(super) fn panel_corner_controls(
        &self,
        layout: &ChromeLayout,
        lead_held: bool,
        trail_held: bool,
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
        if lead_held {
            let mut group = div().flex().items_center();
            if frame.buttons.leading.is_empty() {
                // After the traffic lights, or a small lead in the row
                // Huterm draws.
                group = group.pl(if frame.controls {
                    TITLE_ROW_LEAD
                } else {
                    frame.traffic_light_inset()
                });
            } else {
                group = group.child(self.window_controls_element(
                    frame.buttons.leading,
                    true,
                    colors,
                    window,
                    cx,
                ));
            }
            group = group.child(self.sidebar_button_element(colors, cx));
            controls.push(row(group.into_any_element(), true));
        }
        if trail_held {
            let mut group = div()
                .flex()
                .items_center()
                .gap(CORNER_GAP)
                .mr(CONTROL_INSET);
            group = group.child(self.sidebar_button_element(colors, cx));
            if menu == MenuButtonPlacement::TitleStrip {
                group = group.child(self.menu_button_element(colors, cx));
            }
            let mut outer = div().flex().items_center().child(group);
            if !frame.buttons.trailing.is_empty() {
                outer = outer.child(self.window_controls_element(
                    frame.buttons.trailing,
                    false,
                    colors,
                    window,
                    cx,
                ));
            }
            controls.push(row(outer.into_any_element(), false));
        }
        controls
    }
}

/// What a row press, a row menu pick, or a sidebar key does to a workspace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RowAction {
    Show,
    Rename,
    ClearName,
    /// Workspaces only: pins or unpins the row's workspace.
    TogglePin,
    /// Workspaces only: moves the row's workspace into a new window.
    NewWindow,
    /// Workspaces only: moves the row's workspace one place up or down.
    MoveUp,
    MoveDown,
    /// Workspaces only: moves the row's workspace to another session,
    /// which the palette asks for.
    ToSession,
    Close,
}
