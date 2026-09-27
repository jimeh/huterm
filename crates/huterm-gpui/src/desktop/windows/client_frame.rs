//! Linux client-side decorations. With `tabs.position = "titlebar"` an
//! ordinary Linux window asks GPUI for client decorations; a compositing
//! window manager then lets Huterm draw the title row, the window
//! controls, and an invisible resize border with a shadow around the
//! content. Everything here is pure: the decorations GPUI reports, the
//! window's maximized and fullscreen flags, and the viewport decide the
//! frame, so the fallback and the resize-edge mapping are testable without
//! a window.

use gpui::{
    Bounds, CursorStyle, Decorations, Edges, Pixels, ResizeEdge, Size,
    WindowDecorations, point, px, size,
};

use super::TabPosition;
use crate::keymap::Platform;

/// The invisible border around the content that starts a resize, and the
/// room the shadow paints in.
pub(super) const CLIENT_INSET: Pixels = px(10.0);
/// Presses this far from a corner along either edge resize diagonally.
pub(super) const CORNER_REACH: Pixels = px(24.0);
/// The radius of the drawn frame's top corners.
pub(super) const FRAME_RADIUS: Pixels = px(12.0);

/// The decorations a new or reloaded window asks GPUI for. Only an ordinary
/// Linux window whose configured position is `titlebar` draws its own;
/// macOS keeps `AppKit`'s strip and Quake windows keep their frameless path.
pub(super) fn requested_decorations(
    configured: TabPosition,
    platform: Platform,
    quake: bool,
) -> WindowDecorations {
    if platform == Platform::Linux
        && !quake
        && configured == TabPosition::Titlebar
    {
        WindowDecorations::Client
    } else {
        WindowDecorations::Server
    }
}

/// What GPUI reports about the window, sampled together whenever the
/// window's bounds, appearance, or decorations change.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct FrameState {
    /// `Decorations::Server` after the no-compositor fallback, whatever was
    /// requested.
    pub(super) decorations: Decorations,
    pub(super) maximized: bool,
    pub(super) fullscreen: bool,
}

impl FrameState {
    /// Huterm draws the title bar: GPUI kept the client decorations.
    pub(super) fn client_decorations(self) -> bool {
        matches!(self.decorations, Decorations::Client { .. })
    }

    /// The border Huterm owns around the content. Any tiled edge, a
    /// maximized window, or fullscreen drops it entirely, with the shadow
    /// and rounded corners: the window then meets the screen or its
    /// neighbour flush on that side.
    pub(super) fn frame(self) -> ClientFrame {
        match self.decorations {
            Decorations::Client { tiling }
                if !tiling.is_tiled()
                    && !self.maximized
                    && !self.fullscreen =>
            {
                ClientFrame {
                    inset: CLIENT_INSET,
                }
            }
            Decorations::Client { .. } | Decorations::Server => {
                ClientFrame::default()
            }
        }
    }
}

/// The invisible resize border around the content, zero when the window
/// has no drawn frame or sits flush against the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ClientFrame {
    pub(super) inset: Pixels,
}

impl ClientFrame {
    pub(super) fn edges(self) -> Edges<Pixels> {
        Edges::all(self.inset)
    }

    /// The shadow and rounded corners are drawn only around a frame with an
    /// inset to draw them in.
    pub(super) fn decorated(self) -> bool {
        self.inset > px(0.0)
    }

    /// The resize zones in the inset. Each corner takes [`CORNER_REACH`]
    /// along both of its edges as an L-shaped band of two rectangles, so
    /// its inner square stays content: the close control sits in the top
    /// right one. The edges take the rest of the band.
    pub(super) fn resize_zones(
        self,
        viewport: Size<Pixels>,
    ) -> Vec<(ResizeEdge, Bounds<Pixels>)> {
        if !self.decorated() {
            return Vec::new();
        }
        let width = viewport.width.max(px(0.0));
        let height = viewport.height.max(px(0.0));
        let inset = self.inset.min(width / 2.0).min(height / 2.0);
        let corner = (inset + CORNER_REACH).min(width / 2.0).min(height / 2.0);
        let middle_width = (width - corner * 2.0).max(px(0.0));
        let middle_height = (height - corner * 2.0).max(px(0.0));
        let reach = (corner - inset).max(px(0.0));
        let right = width - inset;
        let bottom = height - inset;
        // Corners first: a press in the shared reach resizes diagonally.
        // Each is the band along its horizontal edge plus the band along
        // its vertical edge below or above that.
        let mut zones = vec![
            (
                ResizeEdge::TopLeft,
                Bounds::new(point(px(0.0), px(0.0)), size(corner, inset)),
            ),
            (
                ResizeEdge::TopLeft,
                Bounds::new(point(px(0.0), inset), size(inset, reach)),
            ),
            (
                ResizeEdge::TopRight,
                Bounds::new(
                    point(width - corner, px(0.0)),
                    size(corner, inset),
                ),
            ),
            (
                ResizeEdge::TopRight,
                Bounds::new(point(right, inset), size(inset, reach)),
            ),
            (
                ResizeEdge::BottomLeft,
                Bounds::new(point(px(0.0), bottom), size(corner, inset)),
            ),
            (
                ResizeEdge::BottomLeft,
                Bounds::new(
                    point(px(0.0), height - corner),
                    size(inset, reach),
                ),
            ),
            (
                ResizeEdge::BottomRight,
                Bounds::new(point(width - corner, bottom), size(corner, inset)),
            ),
            (
                ResizeEdge::BottomRight,
                Bounds::new(point(right, height - corner), size(inset, reach)),
            ),
        ];
        zones.extend([
            (
                ResizeEdge::Top,
                Bounds::new(point(corner, px(0.0)), size(middle_width, inset)),
            ),
            (
                ResizeEdge::Bottom,
                Bounds::new(point(corner, bottom), size(middle_width, inset)),
            ),
            (
                ResizeEdge::Left,
                Bounds::new(point(px(0.0), corner), size(inset, middle_height)),
            ),
            (
                ResizeEdge::Right,
                Bounds::new(point(right, corner), size(inset, middle_height)),
            ),
        ]);
        zones
    }

    /// The edge a press at `position` resizes, or `None` inside the
    /// content, from the zones production mounts.
    #[cfg(test)]
    pub(super) fn resize_edge(
        self,
        position: gpui::Point<Pixels>,
        viewport: Size<Pixels>,
    ) -> Option<ResizeEdge> {
        self.resize_zones(viewport)
            .into_iter()
            .find(|(_, bounds)| bounds.contains(&position))
            .map(|(edge, _)| edge)
    }
}

/// The pointer shape over a resize zone.
pub(super) fn resize_cursor(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top => CursorStyle::ResizeUp,
        ResizeEdge::Bottom => CursorStyle::ResizeDown,
        ResizeEdge::Left => CursorStyle::ResizeLeft,
        ResizeEdge::Right => CursorStyle::ResizeRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
            CursorStyle::ResizeUpLeftDownRight
        }
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
            CursorStyle::ResizeUpRightDownLeft
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tab_position::{TabHost, resolve_tab_position};
    use super::*;
    use gpui::Tiling;

    fn client(tiling: Tiling) -> Decorations {
        Decorations::Client { tiling }
    }

    #[test]
    fn only_ordinary_linux_title_bar_windows_request_client_decorations() {
        for configured in [
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Right,
            TabPosition::Titlebar,
        ] {
            for platform in [Platform::Linux, Platform::MacOs] {
                for quake in [false, true] {
                    let expected = if platform == Platform::Linux
                        && !quake
                        && configured == TabPosition::Titlebar
                    {
                        WindowDecorations::Client
                    } else {
                        WindowDecorations::Server
                    };
                    assert_eq!(
                        requested_decorations(configured, platform, quake),
                        expected,
                        "{configured:?} on {platform:?}, quake {quake}"
                    );
                }
            }
        }
    }

    #[test]
    fn reported_decorations_decide_the_title_row_and_the_fallback() {
        let host = |state: FrameState, fullscreen: bool| TabHost {
            platform: Platform::Linux,
            fullscreen,
            quake: false,
            client_decorations: state.client_decorations(),
        };
        // Without a compositor GPUI keeps server decorations: a top bar.
        let fallback = FrameState::default();
        assert!(!fallback.client_decorations());
        assert_eq!(
            resolve_tab_position(TabPosition::Titlebar, host(fallback, false)),
            TabPosition::Top
        );
        // With one, Huterm draws the row, also while maximized or tiled.
        for decorations in [
            client(Tiling::default()),
            client(Tiling::tiled()),
            client(Tiling {
                left: true,
                ..Tiling::default()
            }),
        ] {
            let state = FrameState {
                decorations,
                maximized: true,
                fullscreen: false,
            };
            assert!(state.client_decorations());
            assert_eq!(
                resolve_tab_position(TabPosition::Titlebar, host(state, false)),
                TabPosition::Titlebar
            );
        }
        // Fullscreen hides the row whatever the decorations report.
        let state = FrameState {
            decorations: client(Tiling::default()),
            maximized: false,
            fullscreen: true,
        };
        assert_eq!(
            resolve_tab_position(TabPosition::Titlebar, host(state, true)),
            TabPosition::Top
        );
    }

    #[test]
    fn the_frame_exists_only_for_untiled_windowed_client_decorations() {
        let framed = FrameState {
            decorations: client(Tiling::default()),
            maximized: false,
            fullscreen: false,
        };
        assert_eq!(framed.frame().inset, CLIENT_INSET);
        assert!(framed.frame().decorated());
        assert_eq!(framed.frame().edges(), Edges::all(CLIENT_INSET));
        let flush = [
            FrameState {
                maximized: true,
                ..framed
            },
            FrameState {
                fullscreen: true,
                ..framed
            },
            FrameState {
                decorations: client(Tiling::tiled()),
                ..framed
            },
            FrameState {
                decorations: client(Tiling {
                    right: true,
                    ..Tiling::default()
                }),
                ..framed
            },
            FrameState {
                decorations: Decorations::Server,
                ..framed
            },
        ];
        for state in flush {
            let frame = state.frame();
            assert_eq!(frame, ClientFrame::default(), "{state:?}");
            assert!(!frame.decorated(), "{state:?}");
            assert_eq!(frame.edges(), Edges::default(), "{state:?}");
            assert!(
                frame.resize_zones(size(px(800.0), px(600.0))).is_empty(),
                "{state:?}"
            );
        }
    }

    #[test]
    fn presses_in_the_inset_map_to_their_edge_or_corner() {
        let frame = ClientFrame {
            inset: CLIENT_INSET,
        };
        let viewport = size(px(800.0), px(600.0));
        let edge =
            |x: f32, y: f32| frame.resize_edge(point(px(x), px(y)), viewport);
        // Edges, away from the corner reach.
        assert_eq!(edge(400.0, 0.0), Some(ResizeEdge::Top));
        assert_eq!(edge(400.0, 9.9), Some(ResizeEdge::Top));
        assert_eq!(edge(400.0, 599.0), Some(ResizeEdge::Bottom));
        assert_eq!(edge(400.0, 590.0), Some(ResizeEdge::Bottom));
        assert_eq!(edge(0.0, 300.0), Some(ResizeEdge::Left));
        assert_eq!(edge(9.0, 300.0), Some(ResizeEdge::Left));
        assert_eq!(edge(799.0, 300.0), Some(ResizeEdge::Right));
        assert_eq!(edge(790.0, 300.0), Some(ResizeEdge::Right));
        // Corners: the inset band within 34 points of the corner.
        assert_eq!(edge(0.0, 0.0), Some(ResizeEdge::TopLeft));
        assert_eq!(edge(30.0, 5.0), Some(ResizeEdge::TopLeft));
        assert_eq!(edge(5.0, 30.0), Some(ResizeEdge::TopLeft));
        assert_eq!(edge(795.0, 30.0), Some(ResizeEdge::TopRight));
        assert_eq!(edge(770.0, 5.0), Some(ResizeEdge::TopRight));
        assert_eq!(edge(5.0, 570.0), Some(ResizeEdge::BottomLeft));
        assert_eq!(edge(30.0, 595.0), Some(ResizeEdge::BottomLeft));
        assert_eq!(edge(795.0, 595.0), Some(ResizeEdge::BottomRight));
        assert_eq!(edge(770.0, 595.0), Some(ResizeEdge::BottomRight));
        // Past the corner reach the press is a plain edge. GPUI bounds
        // include their far edge, so the shared boundary point stays a
        // corner.
        assert_eq!(edge(34.0, 5.0), Some(ResizeEdge::TopLeft));
        assert_eq!(edge(35.0, 5.0), Some(ResizeEdge::Top));
        assert_eq!(edge(5.0, 35.0), Some(ResizeEdge::Left));
        // Content, including the corner squares inside the band, is not
        // a resize. The band's far edge itself still counts, as above.
        assert_eq!(edge(10.5, 10.5), None);
        assert_eq!(edge(20.0, 20.0), None);
        assert_eq!(edge(400.0, 300.0), None);
        assert_eq!(edge(789.9, 589.9), None);
        // Without a frame nothing resizes.
        assert_eq!(
            ClientFrame::default()
                .resize_edge(point(px(0.0), px(0.0)), viewport),
            None
        );
        // The zones tile the band and nothing else: every band point maps
        // to one edge, and no content point, including the corner squares
        // inside the band, maps at all.
        let zones = frame.resize_zones(viewport);
        assert_eq!(zones.len(), 12);
        for x in (0..800_u16).step_by(7) {
            for y in (0..600_u16).step_by(7) {
                let position = point(px(f32::from(x)), px(f32::from(y)));
                let hits: Vec<ResizeEdge> = zones
                    .iter()
                    .filter(|(_, bounds)| bounds.contains(&position))
                    .map(|(edge, _)| *edge)
                    .collect();
                let in_band = x < 10 || y < 10 || x >= 790 || y >= 590;
                if in_band {
                    assert!(!hits.is_empty(), "({x}, {y})");
                    assert!(
                        hits.iter().all(|edge| *edge == hits[0]),
                        "({x}, {y})"
                    );
                } else {
                    assert!(hits.is_empty(), "({x}, {y}) {hits:?}");
                }
            }
        }
    }

    #[test]
    fn a_tiny_window_keeps_its_zones_inside_the_viewport() {
        let frame = ClientFrame {
            inset: CLIENT_INSET,
        };
        let viewport = size(px(12.0), px(8.0));
        for (_, bounds) in frame.resize_zones(viewport) {
            assert!(bounds.origin.x >= px(0.0) && bounds.origin.y >= px(0.0));
            assert!(bounds.right() <= viewport.width);
            assert!(bounds.bottom() <= viewport.height);
        }
        assert_eq!(
            frame.resize_edge(point(px(0.0), px(0.0)), viewport),
            Some(ResizeEdge::TopLeft)
        );
    }

    #[test]
    fn diagonal_corners_share_a_cursor_with_their_opposite() {
        assert_eq!(
            resize_cursor(ResizeEdge::TopLeft),
            resize_cursor(ResizeEdge::BottomRight)
        );
        assert_eq!(
            resize_cursor(ResizeEdge::TopRight),
            resize_cursor(ResizeEdge::BottomLeft)
        );
        assert_eq!(resize_cursor(ResizeEdge::Top), CursorStyle::ResizeUp);
        assert_eq!(resize_cursor(ResizeEdge::Right), CursorStyle::ResizeRight);
    }
}
