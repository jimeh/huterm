//! The desktop client's saved window layout, as plain types the client
//! converts to and from its own window records.

use huterm_protocol::{PaneId, SessionId, TabId, WorkspaceId};

use crate::InvalidState;
use crate::bounds::{MAX_TABS, MAX_WINDOWS};

/// The desktop windows to reopen, in opening order. Identities are
/// unscoped and name records in the runtime file saved with the layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesktopLayout {
    /// Windows in opening order.
    pub windows: Vec<WindowLayout>,
    /// The index of the regular window that was active, activated last.
    pub active_window: Option<usize>,
}

/// One window's layout.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowLayout {
    /// The session the window shows.
    pub session: SessionId,
    /// The workspace the window shows.
    pub workspace: WorkspaceId,
    /// The active tab.
    pub active_tab: Option<TabId>,
    /// Tabs in activation order, most recent last.
    pub tab_history: Vec<TabId>,
    /// The focused pane of each tab that has one.
    pub focused_panes: Vec<FocusedPane>,
    /// The restorable bounds in global coordinates, which stay windowed
    /// while the window is maximized or fullscreen.
    pub bounds: WindowBounds,
    /// How the window was presented.
    pub mode: WindowMode,
    /// The tab sidebar's width.
    pub sidebar_width: f32,
    /// The Quake profile the window belongs to, if any.
    pub quake_profile: Option<String>,
    /// Per-window appearance overrides, if any.
    pub appearance: Option<WindowAppearance>,
}

/// A tab's focused pane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FocusedPane {
    /// The tab.
    pub tab: TabId,
    /// Its focused pane.
    pub pane: PaneId,
}

/// A window's restorable frame in global coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowBounds {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width, above zero.
    pub width: f32,
    /// Height, above zero.
    pub height: f32,
}

/// How a window was presented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowMode {
    /// At its bounds.
    Windowed,
    /// Maximized.
    Maximized,
    /// Fullscreen.
    Fullscreen,
}

/// A window's transparency overrides; each unset value inherits.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowAppearance {
    /// Overall window opacity.
    pub opacity: Option<f32>,
    /// Background opacity.
    pub background_opacity: Option<f32>,
    /// Text opacity.
    pub foreground_opacity: Option<f32>,
    /// Background blur radius.
    pub blur: Option<u32>,
}

impl DesktopLayout {
    /// Checks the bounds and rules a saved layout meets: at most
    /// [`MAX_WINDOWS`] windows and [`MAX_TABS`] history and focus entries
    /// per window, nonzero identities, an active window that exists,
    /// finite geometry with a positive size, and non-blank profile names.
    ///
    /// # Errors
    /// Returns the first rule the layout breaks.
    pub fn validate(&self) -> Result<(), InvalidState> {
        if self.windows.len() > MAX_WINDOWS {
            return Err(InvalidState::new(format!(
                "more than {MAX_WINDOWS} windows"
            )));
        }
        if self
            .active_window
            .is_some_and(|index| index >= self.windows.len())
        {
            return Err(InvalidState::new("the active window does not exist"));
        }
        self.windows.iter().try_for_each(WindowLayout::validate)
    }
}

impl WindowLayout {
    fn validate(&self) -> Result<(), InvalidState> {
        if self.tab_history.len() > MAX_TABS
            || self.focused_panes.len() > MAX_TABS
        {
            return Err(InvalidState::new(format!(
                "a window lists more than {MAX_TABS} tabs"
            )));
        }
        let ids = [self.session.get(), self.workspace.get()]
            .into_iter()
            .chain(self.active_tab.map(TabId::get))
            .chain(self.tab_history.iter().map(|tab| tab.get()))
            .chain(
                self.focused_panes
                    .iter()
                    .flat_map(|focus| [focus.tab.get(), focus.pane.get()]),
            );
        if ids.into_iter().any(|id| id == 0) {
            return Err(InvalidState::new("a window names identity zero"));
        }
        let WindowBounds {
            x,
            y,
            width,
            height,
        } = self.bounds;
        if ![x, y, width, height, self.sidebar_width]
            .iter()
            .all(|value| value.is_finite())
            || width <= 0.0
            || height <= 0.0
            || self.sidebar_width < 0.0
        {
            return Err(InvalidState::new("a window's geometry is invalid"));
        }
        if self
            .quake_profile
            .as_deref()
            .is_some_and(|name| name.trim().is_empty())
        {
            return Err(InvalidState::new("a Quake profile name is blank"));
        }
        if let Some(appearance) = self.appearance {
            let opacities = [
                appearance.opacity,
                appearance.background_opacity,
                appearance.foreground_opacity,
            ];
            if !opacities.into_iter().flatten().all(f32::is_finite) {
                return Err(InvalidState::new("an opacity is not a number"));
            }
        }
        Ok(())
    }
}
