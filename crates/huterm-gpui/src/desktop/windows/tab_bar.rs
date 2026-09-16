//! Theme-derived tab bar colors and tab item presentation.

use gpui::{Div, Hsla, Stateful, Svg, TextRun, svg};
use huterm_config::{TabStyle, TabWidth, TabsConfig};

use crate::assets::Icon;

use super::{
    App, Bounds, CloseTarget, Context, FluentBuilder, InteractiveElement,
    MouseButton, MouseDownEvent, ParentElement, Pixels, Presentation,
    StatefulInteractiveElement, Styled, TAB_HEIGHT, TabId, TabPosition, Theme,
    Window, WorkspaceView, color, div, px,
};

/// Bounds the title width cache so long-lived windows with changing titles
/// cannot grow it without limit.
const TITLE_WIDTH_CACHE_LIMIT: usize = 512;

/// Pills keep 4 points on every side, which makes the Pill bar 34 points
/// tall; the new-tab control and the strip's leading margin share that inset.
pub(super) const PILL_HEIGHT: Pixels = px(26.0);
pub(super) const PILL_INSET: Pixels = px(4.0);
const ICON_SIZE: Pixels = px(12.0);
const CLOSE_SIZE: Pixels = px(18.0);
const STRIP_GAP: Pixels = px(7.0);
const STRIP_PADDING_LEFT: Pixels = px(12.0);
const STRIP_PADDING_RIGHT: Pixels = px(6.0);
const PILL_GAP: Pixels = px(6.0);
const PILL_PADDING_LEFT: Pixels = px(9.0);
/// Leaves room for the active pill's accent bar on every pill, so titles
/// keep their position when the active tab changes.
const PILL_ACCENT_PADDING_LEFT: Pixels = px(14.0);
const PILL_PADDING_RIGHT: Pixels = px(5.0);
pub(super) const PILL_MARGIN: Pixels = px(2.0);
/// Insets shared by vertical rows and the vertical new-tab button so they
/// align in the column.
pub(super) const VERTICAL_ROW_MARGIN_X: Pixels = px(6.0);
pub(super) const VERTICAL_ROW_MARGIN_Y: Pixels = px(1.0);
/// Title text size used both to render and to measure Fit tabs.
pub(super) const TAB_TEXT_SIZE: Pixels = px(13.0);

#[derive(Clone, Copy)]
pub(super) struct TabColors {
    pub(super) bar: Hsla,
    pub(super) active: Hsla,
    pub(super) foreground: Hsla,
    pub(super) inactive: Hsla,
    pub(super) border: Hsla,
    pub(super) accent: Hsla,
    pub(super) terminal: Hsla,
    pub(super) error: Hsla,
}

impl TabColors {
    pub(super) fn new(theme: &Theme) -> Self {
        let ui = theme.ui();
        Self {
            bar: color(ui.tab_bar_background),
            active: color(ui.tab_active_background),
            foreground: color(ui.tab_foreground),
            inactive: color(ui.tab_inactive_foreground),
            border: color(ui.tab_border),
            accent: color(ui.tab_accent),
            terminal: color(theme.background),
            error: color(theme.ansi[1]),
        }
    }

    fn hover(self) -> Hsla {
        self.foreground.opacity(0.04)
    }
}

/// Height of a horizontal tab bar for `tabs`. Strip keeps the 32-point row
/// height that vertical bars also use; Pill adds its insets around the pill.
pub(super) fn tab_bar_height(tabs: TabsConfig) -> Pixels {
    match tabs.style {
        TabStyle::Pill if !tabs.position.vertical() => {
            PILL_HEIGHT + PILL_INSET * 2.0
        }
        TabStyle::Strip | TabStyle::Pill => TAB_HEIGHT,
    }
}

/// A 12-point Lucide icon. SVGs paint with their own text color, not an
/// inherited one, so hover changes must target the icon through a group.
pub(super) fn icon_element(icon: Icon, tint: Hsla) -> Svg {
    svg()
        .path(icon.asset_path())
        .w(ICON_SIZE)
        .h(ICON_SIZE)
        .flex_shrink_0()
        .text_color(tint)
}

/// Whether the region above the terminal (the macOS titlebar or the display
/// safe area above a notch) uses the tab bar background. Only a bar that
/// touches that region shares its color; a bottom bar leaves it to the
/// terminal.
pub(super) fn top_chrome_uses_bar(
    position: TabPosition,
    presentation: Presentation,
    reveal_progress: f32,
) -> bool {
    if position == TabPosition::Bottom {
        return false;
    }
    match presentation {
        Presentation::Reserved => true,
        Presentation::Overlay => reveal_progress > 0.0,
        Presentation::Hidden => false,
    }
}

pub(super) struct TabItem {
    pub(super) id: TabId,
    pub(super) index: usize,
    pub(super) title: String,
    pub(super) exited: bool,
    pub(super) active: bool,
    pub(super) follows_active: bool,
}

impl TabItem {
    /// Horizontal tabs draw a leading divider unless they or their left
    /// neighbor is active, or they are first.
    fn divided(&self) -> bool {
        !self.active && self.index > 0 && !self.follows_active
    }
}

/// Content shared by every tab style: status, title, and close button.
struct TabParts {
    status: Option<Svg>,
    title: Div,
    close: Stateful<Div>,
}

impl WorkspaceView {
    /// Renders one tab at `bounds`, relative to the tab strip.
    pub(super) fn tab_element(
        item: &TabItem,
        bounds: Bounds<Pixels>,
        tabs: TabsConfig,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> Stateful<Div> {
        let position = tabs.position;
        let id = item.id;
        let shell = div()
            .id(("tab", id.get()))
            .group("tab")
            .absolute()
            .left(bounds.origin.x)
            .top(bounds.origin.y)
            .w(bounds.size.width)
            .h(bounds.size.height)
            .flex_shrink_0()
            .flex()
            .items_center()
            .overflow_hidden()
            .cursor_pointer()
            .text_color(if item.active {
                colors.foreground
            } else {
                colors.inactive
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    view.begin_reorder(id, event.position, window, cx);
                    cx.stop_propagation();
                }),
            );
        let parts = TabParts {
            status: item
                .exited
                .then(|| icon_element(Icon::CircleAlert, colors.error)),
            // GPUI caches nowrap text at its first measured width, which
            // skips truncation; a one-line clamp truncates at the final width.
            title: div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .line_clamp(1)
                .when(item.exited, |title| title.opacity(0.6))
                .child(item.title.clone()),
            close: Self::close_tab_button(id, item.active, colors, cx),
        };
        if position.vertical() {
            vertical_tab(shell, parts, item.active, colors)
        } else {
            match tabs.style {
                TabStyle::Strip => {
                    strip_tab(shell, parts, item, position, colors)
                }
                TabStyle::Pill => {
                    pill_tab(shell, parts, item, tabs.pill_accent, colors)
                }
            }
        }
    }

    /// Caches Fit tab widths for `tab_strip`, which cannot read titles
    /// because it has no `App`.
    pub(super) fn measure_tab_widths(&mut self, window: &Window, cx: &App) {
        let tabs = self.config.tabs;
        if tabs.width != TabWidth::Fit || tabs.position.vertical() {
            self.tab_widths.clear();
            return;
        }
        if self.title_widths.len() > TITLE_WIDTH_CACHE_LIMIT {
            self.title_widths.clear();
        }
        let font = window.text_style().font();
        let mut widths = Vec::with_capacity(self.tabs.len());
        for tab in &self.tabs {
            let (title, exited) = tab.label(cx);
            let text = if let Some(width) = self.title_widths.get(&title) {
                *width
            } else {
                let run = TextRun {
                    len: title.len(),
                    font: font.clone(),
                    color: Hsla::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let width = window
                    .text_system()
                    .layout_line(&title, TAB_TEXT_SIZE, &[run], None)
                    .width;
                self.title_widths.insert(title, width);
                width
            };
            widths.push(fit_tab_width(text, tabs, exited));
        }
        self.tab_widths = widths;
    }

    /// Keeps its slot while hidden so hovering never changes tab width.
    fn close_tab_button(
        id: TabId,
        active: bool,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> Stateful<Div> {
        let foreground = colors.foreground;
        div()
            .id(("close-tab", id.get()))
            .group("close-tab")
            .flex_shrink_0()
            .w(CLOSE_SIZE)
            .h(CLOSE_SIZE)
            .rounded(px(4.0))
            .flex()
            .items_center()
            .justify_center()
            .when(!active, |button| {
                button
                    .opacity(0.0)
                    .group_hover("tab", |style| style.opacity(1.0))
            })
            .hover(|style| style.bg(foreground.opacity(0.08)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |view, _, window, cx| {
                cx.stop_propagation();
                view.request_close(CloseTarget::Tab(id), window, cx);
            }))
            .child(
                icon_element(Icon::X, colors.inactive)
                    .group_hover("close-tab", |style| {
                        style.text_color(foreground)
                    }),
            )
    }
}

/// A 3-point accent bar inset along the left edge of an active row or pill.
fn accent_bar(colors: TabColors, inset: Pixels) -> Div {
    div()
        .absolute()
        .left(px(5.0))
        .top(inset)
        .bottom(inset)
        .w(px(3.0))
        .rounded(px(3.0))
        .bg(colors.accent)
}

/// A short vertical line on a horizontal tab's leading edge.
fn divider(colors: TabColors) -> Div {
    div()
        .absolute()
        .left_0()
        .top(px(9.0))
        .bottom(px(9.0))
        .w(px(1.0))
        .bg(colors.border)
}

/// Left and right placement share one row style regardless of `tabs.style`.
fn vertical_tab(
    shell: Stateful<Div>,
    parts: TabParts,
    active: bool,
    colors: TabColors,
) -> Stateful<Div> {
    let (hover, foreground) = (colors.hover(), colors.foreground);
    shell.child(
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .mx(VERTICAL_ROW_MARGIN_X)
            .my(VERTICAL_ROW_MARGIN_Y)
            .rounded(px(7.0))
            .flex()
            .items_center()
            .gap(px(9.0))
            .pl(px(14.0))
            .pr(px(6.0))
            .when(active, |row| {
                row.bg(colors.active).child(accent_bar(colors, px(8.0)))
            })
            .when(!active, |row| {
                row.group_hover("tab", |style| {
                    style.bg(hover).text_color(foreground)
                })
            })
            .children(parts.status)
            .child(parts.title)
            .child(parts.close),
    )
}

/// The active tab merges into the terminal with an accent on its outer edge.
fn strip_tab(
    shell: Stateful<Div>,
    parts: TabParts,
    item: &TabItem,
    position: TabPosition,
    colors: TabColors,
) -> Stateful<Div> {
    let (hover, foreground) = (colors.hover(), colors.foreground);
    shell
        .gap(STRIP_GAP)
        .pl(STRIP_PADDING_LEFT)
        .pr(STRIP_PADDING_RIGHT)
        .when(item.active, |tab| {
            tab.bg(colors.terminal).child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .h(px(2.0))
                    .bg(colors.accent)
                    .when(position == TabPosition::Top, Styled::top_0)
                    .when(position != TabPosition::Top, Styled::bottom_0),
            )
        })
        .when(!item.active, |tab| {
            tab.hover(|style| style.bg(hover).text_color(foreground))
        })
        .when(item.divided(), |tab| tab.child(divider(colors)))
        .children(parts.status)
        .child(parts.title)
        .child(parts.close)
}

/// Rounded tabs separated by dividers. The active pill is raised and, with
/// `accent`, carries the same left accent bar as vertical rows.
fn pill_tab(
    shell: Stateful<Div>,
    parts: TabParts,
    item: &TabItem,
    accent: bool,
    colors: TabColors,
) -> Stateful<Div> {
    let (hover, foreground) = (colors.hover(), colors.foreground);
    let active = item.active;
    shell
        .px(PILL_MARGIN)
        .when(item.divided(), |tab| tab.child(divider(colors)))
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h(PILL_HEIGHT)
                .rounded(px(7.0))
                .flex()
                .items_center()
                .gap(PILL_GAP)
                .pl(pill_padding_left(accent))
                .pr(PILL_PADDING_RIGHT)
                .when(active, |pill| pill.bg(colors.active))
                .when(active && accent, |pill| {
                    pill.child(accent_bar(colors, px(7.0)))
                })
                .when(!active, |pill| {
                    pill.group_hover("tab", |style| {
                        style.bg(hover).text_color(foreground)
                    })
                })
                .children(parts.status)
                .child(parts.title)
                .child(parts.close),
        )
}

fn pill_padding_left(accent: bool) -> Pixels {
    if accent {
        PILL_ACCENT_PADDING_LEFT
    } else {
        PILL_PADDING_LEFT
    }
}

/// The width a horizontal Fit tab needs around `title_width` of text, clamped
/// to the configured bounds. It mirrors the padding, gaps, and slots rendered
/// by `strip_tab` and `pill_tab`.
pub(super) fn fit_tab_width(
    title_width: Pixels,
    tabs: TabsConfig,
    exited: bool,
) -> Pixels {
    let style = tabs.style;
    let status = if exited {
        ICON_SIZE + gap(style)
    } else {
        px(0.0)
    };
    // Every tab has a title and a close button separated by one gap.
    let chrome = match style {
        TabStyle::Strip => STRIP_PADDING_LEFT + STRIP_PADDING_RIGHT,
        TabStyle::Pill => {
            PILL_MARGIN * 2.0
                + pill_padding_left(tabs.pill_accent)
                + PILL_PADDING_RIGHT
        }
    } + gap(style)
        + CLOSE_SIZE
        + status;
    (title_width + chrome)
        .ceil()
        .clamp(px(tabs.min_width), px(tabs.max_width))
}

fn gap(style: TabStyle) -> Pixels {
    match style {
        TabStyle::Strip => STRIP_GAP,
        TabStyle::Pill => PILL_GAP,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{point, size};

    use super::super::{ChromeLayout, strip_bounds, terminal_corner_radius};
    use super::*;

    fn tabs(
        style: TabStyle,
        pill_accent: bool,
        min: f32,
        max: f32,
    ) -> TabsConfig {
        TabsConfig {
            style,
            pill_accent,
            width: TabWidth::Fit,
            min_width: min,
            max_width: max,
            ..TabsConfig::default()
        }
    }

    #[test]
    fn fit_widths_add_style_chrome_and_clamp_to_bounds() {
        let strip_tabs = tabs(TabStyle::Strip, false, 48.0, 600.0);
        let strip = fit_tab_width(px(20.0), strip_tabs, false);
        assert_eq!(strip, px(20.0 + 12.0 + 6.0 + 7.0 + 18.0));
        assert_eq!(
            fit_tab_width(px(20.0), strip_tabs, true),
            strip + ICON_SIZE + STRIP_GAP
        );
        let pill_tabs = tabs(TabStyle::Pill, false, 48.0, 600.0);
        let pill = fit_tab_width(px(20.0), pill_tabs, false);
        assert_eq!(pill, px(20.0 + 2.0 * 2.0 + 9.0 + 5.0 + 6.0 + 18.0));
        assert_eq!(
            fit_tab_width(px(20.0), pill_tabs, true),
            pill + ICON_SIZE + PILL_GAP
        );
        assert_eq!(
            fit_tab_width(
                px(20.0),
                tabs(TabStyle::Pill, true, 48.0, 600.0),
                false
            ),
            pill + PILL_ACCENT_PADDING_LEFT - PILL_PADDING_LEFT
        );
        assert_eq!(fit_tab_width(px(20.4), strip_tabs, false), px(64.0));
        assert_eq!(
            fit_tab_width(
                px(1.0),
                tabs(TabStyle::Strip, false, 96.0, 240.0),
                false
            ),
            px(96.0)
        );
        assert_eq!(
            fit_tab_width(
                px(900.0),
                tabs(TabStyle::Pill, false, 96.0, 240.0),
                false
            ),
            px(240.0)
        );
    }

    #[test]
    fn horizontal_dividers_skip_the_first_active_and_following_tabs() {
        let item = |index, active, follows_active| TabItem {
            id: TabId::new(1),
            index,
            title: String::new(),
            exited: false,
            active,
            follows_active,
        };
        assert!(item(1, false, false).divided());
        assert!(!item(0, false, false).divided());
        assert!(!item(2, true, false).divided());
        assert!(!item(3, false, true).divided());
    }

    #[test]
    fn top_chrome_border_spans_the_terminal_beside_vertical_tab_bars() {
        let viewport = size(px(800.0), px(600.0));
        for position in [TabPosition::Left, TabPosition::Right] {
            let layout = ChromeLayout::new(viewport, px(28.0), position);
            let border = layout
                .top_chrome_border(position, px(28.0))
                .expect("titlebar border");
            let terminal = layout.terminal;
            assert_eq!(border.bottom(), terminal.origin.y);
            assert_eq!(border.origin.x, terminal.origin.x);
            assert_eq!(border.size.width, terminal.size.width);
            assert_eq!(border.size.height, px(1.0));
            // The two lines meet at the terminal's top corner.
            let side = layout.tab_border(position);
            let corner = match position {
                TabPosition::Left => side.right() == border.origin.x,
                _ => side.origin.x == border.right(),
            };
            assert!(corner, "{position:?}: {side:?} vs {border:?}");
            assert!(layout.top_chrome_border(position, px(0.0)).is_none());
        }
        for position in [TabPosition::Top, TabPosition::Bottom] {
            let layout = ChromeLayout::new(viewport, px(28.0), position);
            assert!(layout.top_chrome_border(position, px(28.0)).is_none());
        }
    }

    #[test]
    fn terminal_corner_continues_the_lines_around_the_terminal() {
        let viewport = size(px(800.0), px(600.0));
        for position in [TabPosition::Left, TabPosition::Right] {
            let layout = ChromeLayout::new(viewport, px(28.0), position);
            let corner = layout
                .terminal_corner(position, px(28.0), px(4.0))
                .expect("corner patch");
            let line = layout.top_chrome_border(position, px(28.0)).unwrap();
            let side = layout.tab_border(position);
            assert_eq!(corner.size, size(px(5.0), px(5.0)));
            assert_eq!(corner.origin.y, line.origin.y);
            let flush = match position {
                TabPosition::Left => corner.origin.x == side.origin.x,
                _ => corner.right() == side.right(),
            };
            assert!(flush, "{position:?}: {corner:?} vs {side:?}");
            assert!(
                layout
                    .terminal_corner(position, px(28.0), px(0.0))
                    .is_none()
            );
            assert!(
                layout.terminal_corner(position, px(0.0), px(4.0)).is_none()
            );
        }
        let layout = ChromeLayout::new(viewport, px(28.0), TabPosition::Top);
        assert!(
            layout
                .terminal_corner(TabPosition::Top, px(28.0), px(4.0))
                .is_none()
        );
    }

    #[test]
    fn terminal_corner_radius_stays_inside_the_smaller_padding() {
        let window = |x, y| huterm_config::WindowConfig {
            padding_x: x,
            padding_y: y,
            ..huterm_config::WindowConfig::default()
        };
        assert_eq!(terminal_corner_radius(window(4.0, 4.0)), px(4.0));
        assert_eq!(terminal_corner_radius(window(10.0, 6.5)), px(6.0));
        assert_eq!(terminal_corner_radius(window(0.0, 8.0)), px(0.0));
        assert_eq!(terminal_corner_radius(window(40.0, 40.0)), px(12.0));
    }

    #[test]
    fn pill_strips_start_with_a_leading_margin_matching_the_inset() {
        let tabs =
            Bounds::new(point(px(10.0), px(20.0)), size(px(600.0), px(32.0)));
        let pill = |position| TabsConfig {
            position,
            style: TabStyle::Pill,
            ..TabsConfig::default()
        };
        let strip = strip_bounds(tabs, pill(TabPosition::Top));
        // The first pill's own margin plus the lead equals the vertical inset.
        assert_eq!(strip.origin.x + PILL_MARGIN, tabs.origin.x + PILL_INSET);
        assert_eq!(strip.right(), tabs.right());
        assert_eq!(strip.size.height, tabs.size.height);
        assert_eq!(strip_bounds(tabs, pill(TabPosition::Left)), tabs);
        assert_eq!(strip_bounds(tabs, TabsConfig::default()), tabs);
        let tiny = Bounds::new(tabs.origin, size(px(1.0), px(32.0)));
        assert_eq!(
            strip_bounds(tiny, pill(TabPosition::Bottom)).size.width,
            px(0.0)
        );
    }

    #[test]
    fn pill_bars_grow_around_the_pill_while_others_keep_the_row_height() {
        let tabs = |position, style| TabsConfig {
            position,
            style,
            ..TabsConfig::default()
        };
        assert_eq!(
            tab_bar_height(tabs(TabPosition::Top, TabStyle::Pill)),
            px(34.0)
        );
        assert_eq!(
            tab_bar_height(tabs(TabPosition::Bottom, TabStyle::Pill)),
            px(34.0)
        );
        assert_eq!(
            tab_bar_height(tabs(TabPosition::Top, TabStyle::Strip)),
            TAB_HEIGHT
        );
        assert_eq!(
            tab_bar_height(tabs(TabPosition::Left, TabStyle::Pill)),
            TAB_HEIGHT
        );
        let viewport = size(px(800.0), px(600.0));
        let layout = ChromeLayout::for_tabs(
            viewport,
            px(28.0),
            tabs(TabPosition::Top, TabStyle::Pill),
            px(220.0),
            gpui::Edges::default(),
        );
        assert_eq!(layout.tabs.size.height, px(34.0));
        assert_eq!(layout.terminal.origin.y, px(28.0 + 34.0));
    }

    #[test]
    fn top_chrome_follows_a_visible_or_revealing_tab_bar() {
        for position in
            [TabPosition::Top, TabPosition::Left, TabPosition::Right]
        {
            assert!(top_chrome_uses_bar(position, Presentation::Reserved, 0.0));
            assert!(!top_chrome_uses_bar(position, Presentation::Hidden, 1.0));
            assert!(!top_chrome_uses_bar(position, Presentation::Overlay, 0.0));
            assert!(top_chrome_uses_bar(position, Presentation::Overlay, 0.01));
        }
        let bottom = TabPosition::Bottom;
        assert!(!top_chrome_uses_bar(bottom, Presentation::Reserved, 0.0));
        assert!(!top_chrome_uses_bar(bottom, Presentation::Overlay, 1.0));
    }

    #[test]
    fn tab_border_sits_on_the_terminal_edge_for_every_placement() {
        let viewport = size(px(800.0), px(600.0));
        for position in [
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Right,
        ] {
            let layout = ChromeLayout::new(viewport, px(28.0), position);
            let border = layout.tab_border(position);
            let terminal = layout.terminal;
            let touches = match position {
                TabPosition::Top => border.bottom() == terminal.origin.y,
                TabPosition::Bottom => border.origin.y == terminal.bottom(),
                TabPosition::Left => border.right() == terminal.origin.x,
                TabPosition::Right => border.origin.x == terminal.right(),
            };
            assert!(touches, "{position:?}: {border:?} vs {terminal:?}");
            assert!(
                layout.tabs.contains(&border.origin),
                "{position:?} border leaves the tab bar"
            );
        }
    }
}
