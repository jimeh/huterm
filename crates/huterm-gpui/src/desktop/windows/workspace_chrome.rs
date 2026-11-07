//! What a window shows of its session's workspaces: nothing, the dropdown
//! leading the tab bar, a side panel, or a horizontal bar. One pure
//! resolution that layout, rendering, hit testing, menus, and smoke state
//! all read, so they never disagree about which chrome a frame has.

use gpui::{Pixels, px};
use huterm_config::{WorkspacePosition, WorkspaceShow};

/// A new window's side panel width.
pub(super) const PANEL_WIDTH: Pixels = px(180.0);
/// The side panel's resize limits, as for the tab sidebar.
pub(super) const PANEL_MIN: Pixels = px(140.0);
pub(super) const PANEL_MAX: Pixels = px(400.0);
/// A top or bottom workspace bar without a title row to take.
pub(super) const BAR_HEIGHT: Pixels = px(32.0);
/// SW1: a side panel that would leave the terminal narrower than this
/// shows as the dropdown instead.
pub(super) const MIN_TERMINAL_WIDTH: Pixels = px(384.0);
/// A vertical tab column's dropdown row above its tabs.
pub(super) const DROPDOWN_ROW: Pixels = px(32.0);
/// Space before the dropdown along a horizontal strip.
pub(super) const DROPDOWN_MARGIN: Pixels = px(4.0);
/// The divider after the dropdown along a horizontal strip, with its gaps.
pub(super) const DROPDOWN_DIVIDER: Pixels = px(9.0);

/// Which side a panel or bar sits on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::desktop) enum Edge {
    Left,
    Top,
    Right,
    Bottom,
}

impl Edge {
    fn of(position: WorkspacePosition) -> Self {
        match position {
            WorkspacePosition::Left => Self::Left,
            WorkspacePosition::Top => Self::Top,
            WorkspacePosition::Right => Self::Right,
            WorkspacePosition::Bottom => Self::Bottom,
        }
    }

    pub(super) fn vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// The workspace chrome a window draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::desktop) enum WorkspaceChrome {
    /// No workspace UI: `never`, or `auto` with one workspace.
    None,
    /// The dropdown leads the tab bar; `lead` is the width it and its
    /// divider take from a horizontal strip.
    Dropdown { lead: Pixels },
    /// A side panel `width` wide on the left or right edge.
    Panel { edge: Edge, width: Pixels },
    /// A row of chips on the top or bottom edge.
    Bar { edge: Edge },
}

impl WorkspaceChrome {
    /// The edge a panel or bar occupies.
    pub(super) fn edge(self) -> Option<Edge> {
        match self {
            Self::Panel { edge, .. } | Self::Bar { edge } => Some(edge),
            Self::None | Self::Dropdown { .. } => None,
        }
    }

    pub(super) fn is_dropdown(self) -> bool {
        matches!(self, Self::Dropdown { .. })
    }

    /// A top bar takes the title row, so `titlebar` tabs drop below it.
    pub(super) fn top_bar(self) -> bool {
        self == Self::Bar { edge: Edge::Top }
    }
}

/// Everything the resolution reads besides the window's geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Inputs {
    pub(super) show: WorkspaceShow,
    /// Workspaces in the window's session.
    pub(super) workspaces: usize,
    /// The window's own collapse state.
    pub(super) collapsed: bool,
    pub(super) position: WorkspacePosition,
    /// The window's preferred panel width.
    pub(super) width: Pixels,
    /// The width the dropdown and its divider take along a strip.
    pub(super) dropdown_lead: Pixels,
}

/// Whether the window shows any workspace UI for `show` and its session's
/// workspace count.
pub(super) fn shown(show: WorkspaceShow, workspaces: usize) -> bool {
    match show {
        WorkspaceShow::Never => false,
        WorkspaceShow::Always => workspaces > 0,
        WorkspaceShow::Auto => workspaces >= 2,
    }
}

/// Resolves the chrome. `terminal_width` lays the window out with a
/// candidate side panel and returns the terminal width it leaves after
/// the tab column, frame, and safe area; below [`MIN_TERMINAL_WIDTH`] the
/// panel shows as the dropdown (SW1). The saved collapse state is never
/// changed by this.
pub(super) fn resolve(
    inputs: Inputs,
    terminal_width: impl FnOnce(WorkspaceChrome) -> Pixels,
) -> WorkspaceChrome {
    if !shown(inputs.show, inputs.workspaces) {
        return WorkspaceChrome::None;
    }
    let dropdown = WorkspaceChrome::Dropdown {
        lead: inputs.dropdown_lead,
    };
    if inputs.collapsed {
        return dropdown;
    }
    let edge = Edge::of(inputs.position);
    if !edge.vertical() {
        return WorkspaceChrome::Bar { edge };
    }
    let panel = WorkspaceChrome::Panel {
        edge,
        width: inputs.width,
    };
    if terminal_width(panel) < MIN_TERMINAL_WIDTH {
        dropdown
    } else {
        panel
    }
}

/// The panel width a window of `available` content width draws for a
/// preferred `width`: the resize limits, then at most half the content.
pub(super) fn panel_width(width: Pixels, available: Pixels) -> Pixels {
    width
        .clamp(PANEL_MIN, PANEL_MAX)
        .min((available * 0.5).max(px(0.0)))
}

#[cfg(test)]
mod tests;
