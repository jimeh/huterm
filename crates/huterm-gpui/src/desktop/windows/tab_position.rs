//! Resolves the configured tab position for one window. `titlebar` needs a
//! title-bar row to merge into; without one it becomes a top bar. Every
//! layout, visibility, and placement decision reads the resolved position,
//! so only the merged row itself ever sees `Titlebar`.

use super::TabPosition;
use crate::keymap::Platform;

/// The facts about a window that decide whether it has a title-bar row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TabHost {
    /// macOS windows draw the transparent title strip themselves.
    pub(super) platform: Platform,
    /// The title bar is hidden: fullscreen on either platform, including
    /// non-native fullscreen recovery.
    pub(super) fullscreen: bool,
    /// Quake windows keep their frameless path, even after conversion to a
    /// regular window.
    pub(super) quake: bool,
    /// Linux client-side decorations are active, so Huterm draws the title
    /// bar. Nothing sets this yet; the Linux title-bar slice will.
    pub(super) client_decorations: bool,
}

impl TabHost {
    fn has_title_bar(self) -> bool {
        !self.fullscreen
            && !self.quake
            && (self.platform == Platform::MacOs || self.client_decorations)
    }
}

/// `Titlebar` where the window has a title-bar row; `Top` otherwise. Every
/// other position is returned unchanged.
pub(super) fn resolve_tab_position(
    configured: TabPosition,
    host: TabHost,
) -> TabPosition {
    if configured == TabPosition::Titlebar && !host.has_title_bar() {
        TabPosition::Top
    } else {
        configured
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POSITIONS: [TabPosition; 5] = [
        TabPosition::Top,
        TabPosition::Bottom,
        TabPosition::Left,
        TabPosition::Right,
        TabPosition::Titlebar,
    ];

    #[test]
    fn titlebar_resolves_to_top_wherever_no_title_bar_exists() {
        for configured in POSITIONS {
            for platform in [Platform::Linux, Platform::MacOs] {
                for fullscreen in [false, true] {
                    for quake in [false, true] {
                        for client_decorations in [false, true] {
                            let host = TabHost {
                                platform,
                                fullscreen,
                                quake,
                                client_decorations,
                            };
                            let resolved =
                                resolve_tab_position(configured, host);
                            let expected =
                                if configured != TabPosition::Titlebar {
                                    configured
                                } else if fullscreen || quake {
                                    TabPosition::Top
                                } else if platform == Platform::MacOs
                                    || client_decorations
                                {
                                    TabPosition::Titlebar
                                } else {
                                    TabPosition::Top
                                };
                            assert_eq!(
                                resolved, expected,
                                "{configured:?} on {host:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_merged_row_is_never_vertical() {
        assert!(!TabPosition::Titlebar.vertical());
    }
}
