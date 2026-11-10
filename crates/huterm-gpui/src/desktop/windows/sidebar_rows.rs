//! What the sidebar's lists share: the two-line row both the workspace and
//! the session lists draw, and its "⋯" button. The callers attach what a
//! press, a right press, or the "⋯" button does; a press starts a drag
//! through `drag`.

use huterm_ui::components::tooltip::Tooltip;
use huterm_ui::typography;

use super::workspace_status::{Dot, RowStatus};
use super::*;

/// The status dot at a row's end, and the pin mark after its name.
const DOT_SIZE: Pixels = px(7.0);
const PIN_SIZE: Pixels = px(11.0);

/// A row, and the space above each.
pub(super) const ROW_HEIGHT: Pixels = px(40.0);
pub(super) const ROW_GAP: Pixels = px(2.0);
/// Space between rows and their list's edges.
pub(super) const ROW_MARGIN: Pixels = px(6.0);

/// A small tag after a row's text, such as the windows that show a session.
pub(super) struct RowBadge {
    pub(super) icon: Option<Icon>,
    /// Its short text; the row's accessible name says what it means.
    pub(super) text: String,
    pub(super) warning: bool,
}

/// What a list row shows.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent look of the row"
)]
pub(super) struct ListRow {
    /// The element id and debug selector.
    pub(super) id: String,
    pub(super) icon: Icon,
    pub(super) name: String,
    /// The name is the automatic one, drawn dimmer.
    pub(super) automatic: bool,
    /// The second line: the row's counts.
    pub(super) detail: String,
    pub(super) accessible: String,
    /// The window shows this item.
    pub(super) selected: bool,
    /// The row's menu is open.
    pub(super) targeted: bool,
    pub(super) dragged: bool,
    /// The drop line above the row, or below the last row.
    pub(super) line_above: bool,
    pub(super) line_below: bool,
    pub(super) badges: Vec<RowBadge>,
    /// The pin mark after the name: a pinned workspace.
    pub(super) pinned: bool,
    /// The status dot at the row's end, with the tooltip naming its tabs.
    pub(super) status: Option<RowStatus>,
}

/// The row: its icon, the name (with a pin mark when pinned) over its
/// detail, then any badges and the status dot, with a press, focus, and
/// drop-line look; `more` is the "⋯" button at its end, which popovers
/// show. Hovering a row with a dot shows its status text, which assistive
/// technology reads as the row's description. The caller adds the
/// listeners and the focus ring.
#[expect(
    clippy::too_many_lines,
    reason = "one element tree, read top to bottom"
)]
pub(super) fn list_row(
    row: ListRow,
    focus: &FocusHandle,
    more: Option<AnyElement>,
    colors: TabColors,
    swatch: Swatch,
) -> gpui::Stateful<gpui::Div> {
    let ListRow {
        id,
        icon,
        name,
        automatic,
        detail,
        accessible,
        selected,
        targeted,
        dragged,
        line_above,
        line_below,
        badges,
        pinned,
        status,
    } = row;
    // Shown, then custom, then automatic names: an automatic name dims to
    // the tab bar's inactive text and no further, so it still reads as
    // enabled in light themes.
    let name_color = if selected {
        colors.foreground
    } else if automatic {
        colors.inactive
    } else {
        colors.foreground.opacity(0.85)
    };
    let line = |above: bool| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .when(above, |line| line.top(-ROW_GAP))
            .when(!above, |line| line.bottom(-ROW_GAP))
            .h(px(2.0))
            .rounded(px(1.0))
            .bg(swatch.accent)
    };
    let selector = id.clone();
    let selector_dot = id.clone();
    let description = status.as_ref().map(|status| status.text.clone());
    let dot = status.map(|status| {
        let color = match status.dot {
            Dot::Running => colors.running,
            Dot::Bell => colors.accent,
            Dot::Exited => colors.inactive,
            Dot::Failed => colors.error,
        };
        div()
            .debug_selector(move || format!("{selector_dot}-dot"))
            .flex_none()
            .size(DOT_SIZE)
            .rounded_full()
            .bg(color)
    });
    div()
        .id(gpui::ElementId::Name(id.into()))
        .role(gpui::Role::Button)
        .aria_label(accessible)
        .when_some(description, |row, text| {
            let tooltip = text.clone();
            // While the row's menu is open, the tooltip goes, and one still
            // waiting for its delay never shows over the menu.
            row.aria_description(text).when(!targeted, |row| {
                row.tooltip(move |_, cx| {
                    Tooltip::view(tooltip.clone(), swatch, cx)
                })
            })
        })
        .aria_selected(selected)
        .track_focus(&focus.clone().tab_stop(selected))
        .key_context("Sidebar")
        .debug_selector(move || selector.clone())
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.0))
        .h(ROW_HEIGHT)
        .mt(ROW_GAP)
        .mx(ROW_MARGIN)
        .pl(px(8.0))
        .pr(px(if more.is_some() { 4.0 } else { 8.0 }))
        .rounded(px(7.0))
        .cursor_pointer()
        .when(dragged, |row| row.opacity(0.5))
        .when(selected, |row| row.bg(colors.active))
        .when(!selected, |row| {
            row.hover(|style| style.bg(colors.hover))
                .when(targeted, |row| row.bg(colors.hover))
        })
        .child(
            icon_element(
                icon,
                if selected {
                    colors.foreground
                } else {
                    colors.inactive
                },
            )
            .size(px(14.0))
            .flex_none(),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_color(name_color)
                                .when(!automatic, |name| {
                                    name.font_weight(gpui::FontWeight::SEMIBOLD)
                                })
                                .child(name),
                        )
                        .when(pinned, |line| {
                            line.child(
                                icon_element(Icon::Pin, colors.inactive)
                                    .size(PIN_SIZE)
                                    .flex_none(),
                            )
                        }),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(typography::ui(10.5))
                        .text_color(colors.inactive)
                        .child(detail),
                ),
        )
        .children(badges.into_iter().map(|badge| {
            let color = if badge.warning {
                swatch.warning
            } else {
                colors.inactive
            };
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(3.0))
                .px(px(4.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(color.opacity(0.5))
                .text_size(typography::ui(10.5))
                .text_color(color)
                .children(badge.icon.map(|icon| {
                    icon_element(icon, color).size(px(11.0)).flex_none()
                }))
                .child(badge.text)
        }))
        .children(dot)
        .children(more)
        .when(line_above, |row| row.child(line(true)))
        .when(line_below, |row| row.child(line(false)))
}

/// The "⋯" button at a popover row's end, which opens the row's menu as a
/// right press does. It never takes focus: the menu key and Shift-F10 open
/// the same menu from the keyboard. It reports the menu as expanded while
/// `open`, which also drops a tooltip still waiting to show, so a quick
/// click does not leave one over the menu.
pub(super) fn more_button(
    id: String,
    label: String,
    open: bool,
    colors: TabColors,
    swatch: Swatch,
) -> Button {
    let group = id.clone();
    let selector = id.clone();
    Button::new(id, label, swatch)
        .debug_selector(move || selector.clone())
        .colors(colors.control())
        .icon(
            icon_element(
                Icon::Ellipsis,
                if open {
                    colors.foreground
                } else {
                    colors.inactive
                },
            )
            .size(px(14.0))
            .group_hover(group.clone(), |style| {
                style.text_color(colors.foreground)
            }),
        )
        .icon_only()
        .expanded(open)
        .group(group)
        .stop_presses()
        .size(px(22.0))
        .rounded(px(5.0))
}
