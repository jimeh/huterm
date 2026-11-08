//! What a window shows of its sidebar, which holds the session's workspaces
//! and the session switcher: docked on its side, or hidden with its button
//! in that corner, beside it the workspace switcher given two or more
//! workspaces and the session switcher when Settings turn it on. One pure resolution that layout, rendering, hit testing,
//! popovers, and smoke state all read, so they never disagree about which
//! chrome a frame has.

use gpui::{Pixels, px};
use huterm_config::SidebarPosition;

use super::TabPosition;
use super::window_menu::{MenuButtonPlacement, menu_button_placement};

/// A new window's sidebar width.
pub(super) const PANEL_WIDTH: Pixels = px(180.0);
/// The sidebar's resize limits, as for the tab column.
pub(super) const PANEL_MIN: Pixels = px(140.0);
pub(super) const PANEL_MAX: Pixels = px(400.0);
/// A docked sidebar that would leave the terminal narrower than this
/// stays hidden, and its button opens it as a sheet instead.
pub(super) const MIN_TERMINAL_WIDTH: Pixels = px(384.0);
/// The row a tab column gives the corner controls above its tabs, and the
/// corner strip a docked sidebar keeps without a title row.
pub(super) const CORNER_ROW: Pixels = px(32.0);
/// Space before the corner controls along a horizontal strip.
pub(super) const CORNER_MARGIN: Pixels = px(4.0);

/// Which side the sidebar docks on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::desktop) enum Edge {
    Left,
    Right,
}

impl Edge {
    pub(super) fn of(position: SidebarPosition) -> Self {
        match position {
            SidebarPosition::Left => Self::Left,
            SidebarPosition::Right => Self::Right,
        }
    }
}

/// The sidebar chrome a window draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::desktop) enum WorkspaceChrome {
    /// The sidebar is hidden. Its button sits in the corner on `edge`, with
    /// the workspace switcher beside it while `switcher` and the session
    /// switcher while `session`. `narrow` means the window wants it shown
    /// but cannot dock it, so the button opens it as a sheet.
    Hidden {
        edge: Edge,
        switcher: bool,
        session: bool,
        narrow: bool,
    },
    /// The sidebar docks `width` wide on `edge`.
    Panel { edge: Edge, width: Pixels },
}

impl WorkspaceChrome {
    /// A hidden left sidebar without switchers, whose layout has no
    /// sidebar band and no corner slot.
    pub(super) const NONE: Self = Self::Hidden {
        edge: Edge::Left,
        switcher: false,
        session: false,
        narrow: false,
    };

    pub(super) fn edge(self) -> Edge {
        match self {
            Self::Hidden { edge, .. } | Self::Panel { edge, .. } => edge,
        }
    }

    pub(super) fn docked(self) -> bool {
        matches!(self, Self::Panel { .. })
    }

    /// The workspace switcher shows beside the sidebar button.
    pub(super) fn switcher(self) -> bool {
        matches!(self, Self::Hidden { switcher: true, .. })
    }

    /// The session switcher shows beside the sidebar button.
    pub(super) fn session(self) -> bool {
        matches!(self, Self::Hidden { session: true, .. })
    }

    /// How many switchers show beside the sidebar button.
    pub(super) fn switchers(self) -> usize {
        usize::from(self.switcher()) + usize::from(self.session())
    }

    /// The window wants the sidebar but is too narrow to dock it.
    pub(super) fn narrow(self) -> bool {
        matches!(self, Self::Hidden { narrow: true, .. })
    }

    /// The docked sidebar on `edge`.
    pub(super) fn docked_on(self, edge: Edge) -> bool {
        matches!(self, Self::Panel { edge: docked, .. } if docked == edge)
    }
}

/// Everything the resolution reads besides the window's geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Inputs {
    /// The window's own visible state.
    pub(super) visible: bool,
    /// Workspaces in the window's session.
    pub(super) workspaces: usize,
    pub(super) position: SidebarPosition,
    /// The window's preferred sidebar width.
    pub(super) width: Pixels,
    /// Settings show the session switcher beside the hidden sidebar's
    /// button.
    pub(super) session_corner: bool,
    /// Settings show the workspace switcher there, given two or more
    /// workspaces.
    pub(super) workspace_corner: bool,
}

/// Resolves the chrome. `terminal_width` lays the window out with a
/// candidate docked sidebar and returns the terminal width it leaves after
/// the tab column, frame, and safe area; below [`MIN_TERMINAL_WIDTH`] the
/// sidebar stays hidden and marked narrow. The saved visible state is
/// never changed by this.
pub(super) fn resolve(
    inputs: Inputs,
    terminal_width: impl FnOnce(WorkspaceChrome) -> Pixels,
) -> WorkspaceChrome {
    let edge = Edge::of(inputs.position);
    let hidden = |narrow| WorkspaceChrome::Hidden {
        edge,
        switcher: inputs.workspace_corner && inputs.workspaces >= 2,
        session: inputs.session_corner,
        narrow,
    };
    if !inputs.visible {
        return hidden(false);
    }
    let panel = WorkspaceChrome::Panel {
        edge,
        width: inputs.width,
    };
    if terminal_width(panel) < MIN_TERMINAL_WIDTH {
        hidden(true)
    } else {
        panel
    }
}

/// The sidebar width a window of `available` content width draws for a
/// preferred `width`: the resize limits, then at most half the content.
pub(super) fn panel_width(width: Pixels, available: Pixels) -> Pixels {
    width
        .clamp(PANEL_MIN, PANEL_MAX)
        .min((available * 0.5).max(px(0.0)))
}

/// Which element holds the sidebar button, and the switchers beside it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CornerHost {
    /// The docked sidebar's own corner strip.
    Sidebar,
    /// The title row: after the traffic lights or leading window buttons on
    /// the left, just inside the session control and menu button on the
    /// right. On the right this includes a right tab column's top strip,
    /// which holds the title row's trailing corner.
    TitleRow,
    /// A horizontal tab bar: its start on the left, its end on the right,
    /// including the merged title-bar row.
    Bar,
    /// A vertical tab column: its top strip beside the traffic lights, or
    /// the row above its tabs, on the left; the new-tab row beside the menu
    /// button on the right. Its switchers take rows above its tabs.
    Column,
    /// Nowhere: no title row and the tab bar is hidden. `toggle_sidebar`
    /// and `open_workspace_switcher` still work.
    Hidden,
}

/// Where the corner controls go. `title_row` is a title row Huterm or macOS
/// draws; `position` is the tab position the corner controls follow;
/// `bar_shown` whether the tab bar is shown now. On the right they follow
/// the menu button's host, just inside it; on the left they take the
/// matching leading spot.
pub(super) fn corner_host(
    chrome: WorkspaceChrome,
    title_row: bool,
    position: TabPosition,
    bar_shown: bool,
) -> CornerHost {
    if chrome.docked() {
        return CornerHost::Sidebar;
    }
    if chrome.edge() == Edge::Right {
        return match menu_button_placement(true, title_row, position, bar_shown)
        {
            MenuButtonPlacement::Hidden => CornerHost::Hidden,
            MenuButtonPlacement::TitleStrip => CornerHost::TitleRow,
            MenuButtonPlacement::BarEnd => CornerHost::Bar,
            MenuButtonPlacement::SplitRow => CornerHost::Column,
        };
    }
    if bar_shown && position == TabPosition::Titlebar {
        CornerHost::Bar
    } else if bar_shown && position == TabPosition::Left {
        CornerHost::Column
    } else if title_row {
        CornerHost::TitleRow
    } else if !bar_shown {
        CornerHost::Hidden
    } else if position.vertical() {
        CornerHost::Column
    } else {
        CornerHost::Bar
    }
}

#[cfg(test)]
mod tests;
