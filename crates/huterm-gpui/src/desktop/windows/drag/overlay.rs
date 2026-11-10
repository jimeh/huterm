//! What a tab or pane drag draws over the window: the result region of its
//! target, a tab bar slot's marker, the title chip at the pointer, and the
//! source pane's tear-off cue. Smoke state names targets through
//! [`drop_label`].

// User input picks positions here, so a stale index must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use gpui::Hsla;
use huterm_ui::tokens::size;

use super::super::cross_window::{TEAR_OFF_COMPACT, TEAR_OFF_PILL};
use super::super::pane_geometry::{Zone, zone_preview};
use super::super::*;
use super::{DropTarget, Payload, SpringKey};

/// A sidebar list a dragged row lifts in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in super::super) enum RowList {
    Workspaces,
    Sessions,
}

/// How far a drawn drop box sits inside the region it stands for, so it
/// never touches the pane's edges or a neighbouring box.
const DROP_BOX_INSET: Pixels = px(4.0);

/// The box drawn for a drop `region`: inset on every side, keeping at
/// least its centre line in a region too small for the inset. Only the
/// drawing moves; the region itself still decides the drop.
fn drop_box(region: Bounds<Pixels>) -> Bounds<Pixels> {
    let inset = |length: Pixels| DROP_BOX_INSET.min(length / 2.0);
    let (x, y) = (inset(region.size.width), inset(region.size.height));
    Bounds::new(
        point(region.origin.x + x, region.origin.y + y),
        size(region.size.width - x * 2.0, region.size.height - y * 2.0),
    )
}

/// Place the measured drag chip relative to the event pointer.
fn drag_chip_origin(
    pointer: gpui::Point<Pixels>,
    chip: gpui::Size<Pixels>,
    viewport: gpui::Size<Pixels>,
) -> gpui::Point<Pixels> {
    let axis = |pointer: Pixels, extent: Pixels, limit: Pixels| {
        let preferred = pointer + px(12.0);
        let origin = if preferred < px(0.0) || preferred + extent > limit {
            pointer - px(12.0) - extent
        } else {
            preferred
        };
        origin.clamp(px(0.0), (limit - extent).max(px(0.0)))
    };
    point(
        axis(pointer.x, chip.width, viewport.width),
        axis(pointer.y, chip.height, viewport.height),
    )
}

impl WorkspaceView {
    /// What a drag shows over the window: the result region of its target,
    /// the insertion marker of a tab bar slot, and a chip with the dragged
    /// pane's or tab's title at the pointer. It also records the drawn
    /// target for smoke state.
    pub(in super::super) fn drag_overlay(
        &self,
        colors: TabColors,
        window: &Window,
        cx: &App,
    ) -> Vec<gpui::AnyElement> {
        let mut elements = Vec::new();
        *self.drawn_pane_tear_off.borrow_mut() = None;
        // The floating preview draws the chip while it shows one.
        let mut chip = true;
        let (target, pointer, title, source) = if let Some(incoming) =
            &self.incoming
        {
            chip = !super::super::drag_preview::draws_chip(cx);
            (
                incoming.hover.target,
                incoming.hover.pointer,
                incoming.payload.title.clone(),
                None,
            )
        } else if self.cross_outside(cx) {
            *self.drawn_drop.borrow_mut() = Some(if self.tear_off_cue(cx) {
                "tear_off".into()
            } else {
                "none".into()
            });
            if self.tear_off_pill(cx)
                && let Some(frame) =
                    self.dragged_pane().and_then(|pane| self.drawn_frame(pane))
            {
                elements.extend(self.pane_tear_off_cue(frame, colors));
            }
            return elements;
        } else if let Some(local) = self.local_chip(cx) {
            local
        } else {
            *self.drawn_drop.borrow_mut() = None;
            return elements;
        };
        *self.drawn_drop.borrow_mut() = Some(drop_label(target, &self.tabs));
        elements.extend(self.target_elements(target, source, colors, window));
        // A row this window drags lifts in its own list instead
        // (`lifted_row`).
        if self.incoming.is_none()
            && self
                .drag
                .as_ref()
                .is_some_and(|drag| drag.payload.row().is_some())
        {
            chip = false;
        }
        if !chip {
            return elements;
        }
        // Measure the actual chip before choosing its side of the pointer.
        // All local pane, nesting-tab, and incoming drags share this path.
        elements.push(
            gpui::canvas(
                move |_, window, cx| {
                    let viewport = window.viewport_size();
                    let mut chip = super::super::drag_preview::drag_chip(
                        &title,
                        colors,
                        px(240.0).min(viewport.width),
                    )
                    .max_h(viewport.height)
                    .opacity(0.9)
                    .into_any_element();
                    let measured = chip.layout_as_root(
                        size(
                            gpui::AvailableSpace::MaxContent,
                            gpui::AvailableSpace::MaxContent,
                        ),
                        window,
                        cx,
                    );
                    chip.prepaint_at(
                        drag_chip_origin(pointer, measured, viewport),
                        window,
                        cx,
                    );
                    chip
                },
                |_, mut chip, window, cx| chip.paint(window, cx),
            )
            .absolute()
            .inset_0()
            .into_any_element(),
        );
        elements
    }

    /// This window's own gesture as its chip shows it: the target, the
    /// pointer, the title, and the dragged pane's frame. A pane, a tab
    /// nesting into a pane or sprang away from its workspace, and a row
    /// show one; a tab in its own bar draws itself there.
    #[expect(
        clippy::type_complexity,
        reason = "the four values the overlay reads, named at its one caller"
    )]
    pub(in super::super) fn local_chip(
        &self,
        cx: &App,
    ) -> Option<(
        Option<DropTarget>,
        gpui::Point<Pixels>,
        String,
        Option<Bounds<Pixels>>,
    )> {
        let drag = self.drag.as_ref().filter(|drag| drag.dragging())?;
        let title = match drag.payload {
            Payload::Pane { tab, pane, .. } => {
                // A row spring may have hidden the pane's tab.
                let title = self
                    .all_tab_views()
                    .find(|view| view.id == tab)
                    .and_then(|tab| {
                        let pane = tab.pane(pane)?;
                        Some(tab.pane_label(pane, self.config.tabs, cx))
                    })
                    .unwrap_or_default();
                return Some((
                    drag.target(),
                    drag.pointer(),
                    title,
                    self.drawn_frame(pane),
                ));
            }
            Payload::Tab { tab, .. }
                if drag.nesting() || self.drag_away(drag.payload, cx) =>
            {
                self.all_tab_views()
                    .find(|view| view.id == tab)
                    .map(|tab| tab.title(self.config.tabs, cx))
                    .unwrap_or_default()
            }
            Payload::Tab { .. } => return None,
            // A row's target, for the drop label: the row itself lifts in
            // its list (RC2), and the rows draw its slot.
            Payload::Workspace { .. } | Payload::Session(_) => {
                self.row_title(drag.payload, cx)
            }
        };
        Some((drag.target(), drag.pointer(), title, None))
    }

    /// The lifted copy of a row this window drags in `list`'s rows, the
    /// workspace list or the session list: the dragged row's width,
    /// centred on the pointer and kept within the list's visible part,
    /// while the drag is inside the window.
    pub(in super::super) fn lifted_row(
        &self,
        list: RowList,
        colors: TabColors,
        cx: &App,
    ) -> Option<gpui::AnyElement> {
        use super::super::sidebar_rows::{LiftedRow, ROW_HEIGHT, lifted_row};
        let drag = self
            .drag
            .as_ref()
            .filter(|drag| drag.dragging() && !drag.outside)?;
        let (row, scroll) = match (list, drag.payload) {
            (RowList::Workspaces, Payload::Workspace { workspace, .. }) => {
                let row = self
                    .workspace_rows(cx)
                    .into_iter()
                    .find(|row| row.workspace == workspace)?;
                let painted = self
                    .workspace_row_bounds
                    .borrow()
                    .get(&workspace)
                    .copied()?;
                (
                    (
                        LiftedRow {
                            icon: Icon::LayoutGrid,
                            name: row.name.clone(),
                            automatic: row.automatic,
                            detail: row.detail(),
                        },
                        painted,
                    ),
                    self.workspace_drag_scroll(),
                )
            }
            (RowList::Sessions, Payload::Session(session)) => {
                let row = self
                    .session_rows(cx)
                    .into_iter()
                    .find(|row| row.session == session)?;
                let painted =
                    self.session_row_bounds.borrow().get(&session).copied()?;
                (
                    (
                        LiftedRow {
                            icon: Icon::Layers,
                            name: row.name.clone(),
                            automatic: row.automatic,
                            detail: row.detail(),
                        },
                        painted,
                    ),
                    self.session_list_scroll.clone(),
                )
            }
            _ => return None,
        };
        let ((lifted, painted), list) = (row, scroll.bounds());
        let top = (drag.pointer().y - ROW_HEIGHT / 2.0)
            .clamp(list.top(), (list.bottom() - ROW_HEIGHT).max(list.top()));
        Some(lifted_row(
            lifted,
            Bounds::new(
                point(painted.origin.x, top),
                size(painted.size.width, ROW_HEIGHT),
            ),
            colors,
        ))
    }

    /// The name a dragged row shows in other windows' chips and the
    /// floating preview.
    pub(in super::super) fn row_title(
        &self,
        payload: Payload,
        cx: &App,
    ) -> String {
        match payload {
            Payload::Workspace { workspace, .. } => self
                .workspace_rows(cx)
                .into_iter()
                .find(|row| row.workspace == workspace)
                .map(|row| row.name)
                .unwrap_or_default(),
            Payload::Session(session) => self
                .session_rows(cx)
                .into_iter()
                .find(|row| row.session == session)
                .map(|row| row.name)
                .unwrap_or_default(),
            Payload::Tab { .. } | Payload::Pane { .. } => String::new(),
        }
    }

    /// The source pane while a release would tear it off (S1): its contents
    /// dimmed to 45%, a full-contrast dashed outline, and the tear-off pill
    /// centred. Records the pill for smoke state.
    fn pane_tear_off_cue(
        &self,
        frame: Bounds<Pixels>,
        colors: TabColors,
    ) -> Vec<gpui::AnyElement> {
        let outline = drop_box(frame);
        let pill = pane_tear_off_pill(frame);
        *self.drawn_pane_tear_off.borrow_mut() = Some(pill);
        vec![
            // The terminal colour at 55% leaves the contents at 45%.
            div()
                .absolute()
                .left(frame.origin.x)
                .top(frame.origin.y)
                .w(frame.size.width)
                .h(frame.size.height)
                .bg(colors.terminal.opacity(0.55))
                .into_any_element(),
            div()
                .debug_selector(|| "pane-tear-off-outline".to_owned())
                .absolute()
                .left(outline.origin.x)
                .top(outline.origin.y)
                .w(outline.size.width)
                .h(outline.size.height)
                .rounded(px(size::CONTROL_RADIUS))
                .border_1()
                .border_dashed()
                .border_color(colors.foreground)
                .into_any_element(),
            div()
                .debug_selector(|| "pane-tear-off-pill".to_owned())
                .absolute()
                .left(pill.left())
                .top(pill.top())
                .w(pill.size.width)
                .h(pill.size.height)
                .overflow_hidden()
                .whitespace_nowrap()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .bg(colors.active)
                .border_1()
                .border_color(colors.accent)
                .text_color(colors.foreground)
                .child(super::super::cross_window::tear_off_label(pill))
                .into_any_element(),
        ]
    }

    /// The result region of a drag `target`, with the dragged pane's
    /// `source` frame outlined for a swap, or a tab bar slot's marker.
    fn target_elements(
        &self,
        target: Option<DropTarget>,
        source: Option<Bounds<Pixels>>,
        colors: TabColors,
        window: &Window,
    ) -> Vec<gpui::AnyElement> {
        let mut elements = Vec::new();
        let outline = |bounds: Bounds<Pixels>, color: Hsla| {
            let bounds = drop_box(bounds);
            div()
                .absolute()
                .left(bounds.origin.x)
                .top(bounds.origin.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .rounded(px(size::CONTROL_RADIUS))
                .border_2()
                .border_color(color)
                .into_any_element()
        };
        // The result region a release would fill, tinted in `color`.
        let region = |bounds: Bounds<Pixels>, color: Hsla| {
            let bounds = drop_box(bounds);
            div()
                .absolute()
                .left(bounds.origin.x)
                .top(bounds.origin.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .rounded(px(size::CONTROL_RADIUS))
                .bg(color.opacity(0.22))
                .border_2()
                .border_color(color.opacity(0.8))
                .into_any_element()
        };
        match target {
            Some(DropTarget::Pane {
                frame,
                zone,
                refused,
                ..
            }) => {
                let color = if refused { colors.error } else { colors.accent };
                elements.push(region(zone_preview(frame, zone), color));
                if zone == Zone::Centre
                    && let Some(source) = source
                {
                    elements.push(outline(source, color.opacity(0.8)));
                }
            }
            Some(DropTarget::Slot(index)) => {
                let marker = self.tab_strip(window).marker(index);
                elements.push(
                    div()
                        .absolute()
                        .left(marker.origin.x)
                        .top(marker.origin.y)
                        .w(marker.size.width)
                        .h(marker.size.height)
                        .bg(colors.accent)
                        .into_any_element(),
                );
            }
            Some(DropTarget::TitleRow) => {
                let row = self.title_row_layout(&self.chrome_layout(window));
                elements.push(region(row.drop_region, colors.accent));
            }
            Some(DropTarget::Window) => {
                // Beside the docked sidebar, whose list marks slots.
                let area = self.chrome_layout(window).area;
                elements.push(region(area, colors.accent));
            }
            // The armed label draws its own outline, a row slot its line,
            // and a workspace row its drop region.
            Some(
                DropTarget::Spring(_)
                | DropTarget::RowSlot(_)
                | DropTarget::Workspace { .. },
            )
            | None => {}
        }
        elements
    }
}

/// The pill centred in a source pane that would tear off: labelled when
/// the pane has room for it beside a margin, the arrow alone otherwise.
fn pane_tear_off_pill(frame: Bounds<Pixels>) -> Bounds<Pixels> {
    const MARGIN: Pixels = px(6.0);
    let width = if frame.size.width >= TEAR_OFF_PILL + MARGIN * 2.0 {
        TEAR_OFF_PILL
    } else {
        TEAR_OFF_COMPACT
    };
    let height = px(24.0).min(frame.size.height);
    Bounds::new(
        point(
            frame.center().x - width / 2.0,
            frame.center().y - height / 2.0,
        ),
        size(width, height),
    )
}

/// A drop target as smoke state names it: `pane:<index>:<zone>` with the
/// target pane's index in its tab's traversal order, `slot:<index>`,
/// `title_row`, `spring:<tab index>`, `row_slot`, or `none`. A refused
/// zone ends in `:refused`.
pub(in super::super) fn drop_label(
    target: Option<DropTarget>,
    tabs: &[TabView],
) -> String {
    match target {
        None => "none".to_owned(),
        Some(DropTarget::Slot(index)) => format!("slot:{index}"),
        Some(DropTarget::TitleRow) => "title_row".to_owned(),
        Some(DropTarget::RowSlot(_)) => "row_slot".to_owned(),
        Some(DropTarget::Window) => "window".to_owned(),
        Some(DropTarget::Workspace { workspace, spring }) => format!(
            "workspace:{}{}",
            workspace.get(),
            if spring { ":spring" } else { "" }
        ),
        Some(DropTarget::Spring(SpringKey::Workspace(workspace))) => {
            format!("spring_workspace:{}", workspace.get())
        }
        Some(DropTarget::Spring(SpringKey::Tab(tab))) => format!(
            "spring:{}",
            tabs.iter()
                .position(|view| view.id == tab)
                .map_or_else(|| "?".to_owned(), |index| index.to_string())
        ),
        Some(DropTarget::Pane {
            tab,
            pane,
            zone,
            refused,
            ..
        }) => {
            let index = tabs
                .iter()
                .find(|view| view.id == tab)
                .and_then(|view| view.panes.iter().position(|p| p.pane == pane))
                .map_or_else(|| "?".to_owned(), |index| index.to_string());
            let zone = match zone {
                Zone::Centre => "centre",
                Zone::Edge(SplitDirection::Left) => "left",
                Zone::Edge(SplitDirection::Right) => "right",
                Zone::Edge(SplitDirection::Up) => "up",
                Zone::Edge(SplitDirection::Down) => "down",
            };
            let refused = if refused { ":refused" } else { "" };
            format!("pane:{index}:{zone}{refused}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_chip_flips_at_every_edge_and_corner_then_clamps() {
        let chip = size(px(80.0), px(24.0));
        let viewport = size(px(300.0), px(200.0));
        for (pointer, expected) in [
            ((150.0, 100.0), (162.0, 112.0)),
            ((0.0, 100.0), (12.0, 112.0)),
            ((300.0, 100.0), (208.0, 112.0)),
            ((150.0, 0.0), (162.0, 12.0)),
            ((150.0, 200.0), (162.0, 164.0)),
            ((0.0, 0.0), (12.0, 12.0)),
            ((300.0, 0.0), (208.0, 12.0)),
            ((0.0, 200.0), (12.0, 164.0)),
            ((300.0, 200.0), (208.0, 164.0)),
            ((-100.0, -100.0), (0.0, 0.0)),
            ((500.0, 400.0), (220.0, 176.0)),
        ] {
            assert_eq!(
                drag_chip_origin(
                    point(px(pointer.0), px(pointer.1)),
                    chip,
                    viewport
                ),
                point(px(expected.0), px(expected.1)),
                "pointer {pointer:?}",
            );
        }
        assert_eq!(
            drag_chip_origin(
                point(px(1.0), px(1.0)),
                chip,
                size(px(40.0), px(10.0))
            ),
            point(px(0.0), px(0.0))
        );
    }

    #[test]
    fn the_pane_tear_off_pill_centres_and_turns_compact_in_a_narrow_pane() {
        let wide =
            Bounds::new(point(px(100.0), px(50.0)), size(px(300.0), px(200.0)));
        assert_eq!(
            pane_tear_off_pill(wide),
            Bounds::new(
                point(px(186.0), px(138.0)),
                size(TEAR_OFF_PILL, px(24.0))
            )
        );
        let narrow = Bounds::new(
            point(px(0.0), px(0.0)),
            size(TEAR_OFF_PILL + px(11.0), px(20.0)),
        );
        let pill = pane_tear_off_pill(narrow);
        assert_eq!(pill.size, size(TEAR_OFF_COMPACT, px(20.0)));
        assert_eq!(pill.center(), narrow.center());
        let fits = Bounds::new(
            point(px(0.0), px(0.0)),
            size(TEAR_OFF_PILL + px(12.0), px(40.0)),
        );
        assert_eq!(pane_tear_off_pill(fits).size.width, TEAR_OFF_PILL);
    }

    #[test]
    fn a_drop_box_sits_inside_its_region_and_never_inverts() {
        let region =
            Bounds::new(point(px(100.0), px(50.0)), size(px(300.0), px(200.0)));
        assert_eq!(
            drop_box(region),
            Bounds::new(point(px(104.0), px(54.0)), size(px(292.0), px(192.0)))
        );
        let thin =
            Bounds::new(point(px(10.0), px(10.0)), size(px(6.0), px(40.0)));
        let drawn = drop_box(thin);
        assert_eq!(drawn.origin.x, px(13.0));
        assert_eq!(drawn.size.width, px(0.0));
        assert_eq!(drawn.size.height, px(32.0));
    }
}
