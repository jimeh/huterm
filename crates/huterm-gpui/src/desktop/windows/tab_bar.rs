//! Theme-derived tab bar colors and tab item presentation.

use gpui::{Div, Hsla, Stateful, Svg, TextRun, svg};
use huterm_config::{Rgba, TabCloseButton, TabStyle, TabWidth, TabsConfig};

use crate::assets::Icon;
use crate::desktop::overlay::accent_bar;
use crate::renderer::rgba_color;
use crate::ui::scrollbar::ScrollbarColors;

use super::super::scrollbar_colors;
use super::{
    App, Bounds, CloseTarget, Context, FluentBuilder, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels,
    Presentation, StatefulInteractiveElement, Styled, TAB_HEIGHT, TabId,
    TabPosition, Theme, Window, WorkspaceView, color, div, px,
};

/// Bounds the title width cache so long-lived windows with changing titles
/// cannot grow it without limit.
const TITLE_WIDTH_CACHE_LIMIT: usize = 512;

/// Pills keep 4 points on every side, which makes the Pill bar 34 points
/// tall; the new-tab control and the strip's leading margin share that inset.
pub(super) const PILL_HEIGHT: Pixels = px(26.0);
pub(super) const PILL_INSET: Pixels = px(4.0);
const ICON_SIZE: Pixels = px(12.0);
/// The running dot shown while a program holds the foreground.
const DOT_SIZE: Pixels = px(6.0);
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
/// Pills leave one more point on the left than the right, so the 1-point
/// divider drawn at a tab's leading edge sits centered between neighbors.
pub(super) const PILL_MARGIN_LEFT: Pixels = px(3.0);
pub(super) const PILL_MARGIN_RIGHT: Pixels = px(2.0);
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
    /// The running dot: the theme's ANSI yellow.
    pub(super) running: Hsla,
    /// Overlay on a hovered tab.
    pub(super) hover: Hsla,
    /// Overlay on a hovered control, twice as strong as a tab's.
    pub(super) control_hover: Hsla,
    /// Overlay on a control held down, three times a tab's hover.
    pub(super) control_pressed: Hsla,
    pub(super) scrollbar: ScrollbarColors,
}

impl TabColors {
    pub(super) fn new(theme: &Theme) -> Self {
        let ui = theme.ui();
        let hover = ui.tab_hover_background;
        let control_hover = Rgba {
            alpha: hover.alpha.saturating_mul(2),
            ..hover
        };
        let control_pressed = Rgba {
            alpha: hover.alpha.saturating_mul(3),
            ..hover
        };
        Self {
            bar: color(ui.tab_bar_background),
            active: color(ui.tab_active_background),
            foreground: color(ui.tab_foreground),
            inactive: color(ui.tab_inactive_foreground),
            border: color(ui.tab_border),
            accent: color(ui.tab_accent),
            terminal: color(theme.background),
            error: color(theme.ansi[1]),
            running: color(theme.ansi[3]),
            hover: rgba_color(hover),
            control_hover: rgba_color(control_hover),
            control_pressed: rgba_color(control_pressed),
            scrollbar: scrollbar_colors(theme),
        }
    }

    fn hover(self) -> Hsla {
        self.hover
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
/// touches that region shares its color: a bottom bar leaves it to the
/// terminal, and a vertical column shares only a real titlebar, since under
/// a notch the column itself runs to the screen top beside a terminal-colored
/// strip.
pub(super) fn top_chrome_uses_bar(
    position: TabPosition,
    titlebar: bool,
    presentation: Presentation,
    reveal_progress: f32,
) -> bool {
    if position == TabPosition::Bottom || (position.vertical() && !titlebar) {
        return false;
    }
    match presentation {
        Presentation::Reserved => true,
        Presentation::Overlay => reveal_progress > 0.0,
        Presentation::Hidden => false,
    }
}

/// A tab's relation to the window's active tab.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Activity {
    Active,
    /// Immediately after the active tab along the strip.
    FollowsActive,
    Inactive,
}

/// What a tab's leading indicator reports, in precedence order from the
/// least to the most urgent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TabStatus {
    /// A shell waiting at its prompt; no indicator.
    Idle,
    /// A program holds the foreground; the running dot.
    Busy,
    Bell,
    Exited,
    Failed,
}

impl TabStatus {
    /// `foreground` is the metadata's foreground process, published only
    /// while a program holds the foreground; a name there means Busy.
    pub(super) fn new(
        exited: bool,
        failed: bool,
        bell: bool,
        foreground: Option<&str>,
    ) -> Self {
        if failed {
            Self::Failed
        } else if exited {
            Self::Exited
        } else if bell {
            Self::Bell
        } else if foreground.is_some_and(|process| !process.is_empty()) {
            Self::Busy
        } else {
            Self::Idle
        }
    }

    /// Whether the root process has exited, with or without failure.
    pub(super) fn exited(self) -> bool {
        matches!(self, Self::Exited | Self::Failed)
    }

    /// The width the indicator takes before the title, without its gap.
    fn indicator_width(self) -> Pixels {
        match self {
            Self::Idle => px(0.0),
            Self::Busy => DOT_SIZE,
            Self::Bell | Self::Exited | Self::Failed => ICON_SIZE,
        }
    }
}

pub(super) struct TabItem {
    pub(super) id: TabId,
    pub(super) index: usize,
    pub(super) title: String,
    pub(super) status: TabStatus,
    pub(super) activity: Activity,
    /// The first tab while the strip is scrolled to its start, so its edge
    /// meets the bar's own edge.
    pub(super) flush_start: bool,
    /// The tab whose context menu is open, outlined while it stays open.
    pub(super) targeted: bool,
}

impl TabItem {
    fn active(&self) -> bool {
        self.activity == Activity::Active
    }

    /// Tabs draw a leading divider unless they or their preceding neighbor
    /// is active, or they are first.
    fn divided(&self) -> bool {
        self.activity == Activity::Inactive && self.index > 0
    }
}

/// Content shared by every tab style: status, title, and close button.
struct TabParts {
    status: Option<gpui::AnyElement>,
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
        // The merged title-bar row draws its tabs as a top bar.
        let position = if tabs.position == TabPosition::Titlebar {
            TabPosition::Top
        } else {
            tabs.position
        };
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
            .text_color(if item.active() {
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
            )
            // A right press opens the tab's menu at the pointer without
            // activating the tab; it never reaches the reorder listener.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    view.open_tab_menu(id, event.position, window, cx);
                    cx.stop_propagation();
                }),
            )
            .when(item.targeted, |shell| {
                shell.child(
                    div()
                        .absolute()
                        .inset(px(1.0))
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(colors.accent),
                )
            });
        let parts = TabParts {
            status: match item.status {
                TabStatus::Exited | TabStatus::Failed => Some(
                    icon_element(Icon::CircleAlert, colors.error)
                        .into_any_element(),
                ),
                TabStatus::Bell => Some(
                    icon_element(Icon::Bell, colors.accent).into_any_element(),
                ),
                TabStatus::Busy => Some(running_dot(colors).into_any_element()),
                TabStatus::Idle => None,
            },
            // GPUI caches nowrap text at its first measured width, which
            // skips truncation; a one-line clamp truncates at the final width.
            title: div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .line_clamp(1)
                .when(item.status == TabStatus::Exited, |title| {
                    title.opacity(0.6)
                })
                .child(item.title.clone()),
            close: Self::close_tab_button(
                id,
                close_button_shown(tabs.close_button, item.active()),
                colors,
                cx,
            ),
        };
        match (tabs.style, position.vertical()) {
            (TabStyle::Strip, true) => {
                vertical_strip_tab(shell, parts, item, position, colors)
            }
            (TabStyle::Strip, false) => {
                strip_tab(shell, parts, item, position, colors)
            }
            (TabStyle::Pill, true) => {
                vertical_pill_tab(shell, parts, item, tabs.pill_accent, colors)
            }
            (TabStyle::Pill, false) => {
                pill_tab(shell, parts, item, tabs.pill_accent, colors)
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
            let (title, status) = tab.label(self.config.tabs, cx);
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
            widths.push(fit_tab_width(text, tabs, status));
        }
        self.tab_widths = widths;
    }

    /// Visible when `shown` or while the tab is hovered. Keeps its slot while
    /// hidden so hovering never changes tab width.
    fn close_tab_button(
        id: TabId,
        shown: bool,
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
            .when(!shown, |button| {
                button
                    .opacity(0.0)
                    .group_hover("tab", |style| style.opacity(1.0))
            })
            .hover(|style| style.bg(colors.control_hover))
            .active(|style| style.bg(colors.control_pressed))
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

/// Whether a tab shows its close button without being hovered.
fn close_button_shown(mode: TabCloseButton, active: bool) -> bool {
    match mode {
        TabCloseButton::Hover => false,
        TabCloseButton::Active => active,
        TabCloseButton::Always => true,
    }
}

/// The accent bar inset along the left edge of an active row or pill.
fn tab_accent_bar(colors: TabColors, inset: Pixels) -> Div {
    accent_bar(colors.accent, px(5.0), inset)
}

/// The running dot: a small yellow circle before the title.
fn running_dot(colors: TabColors) -> Div {
    div()
        .flex_shrink_0()
        .w(DOT_SIZE)
        .h(DOT_SIZE)
        .rounded_full()
        .bg(colors.running)
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

/// Left and right Pill placement: a rounded row inset in the column, with
/// the same optional accent bar as horizontal pills.
fn vertical_pill_tab(
    shell: Stateful<Div>,
    parts: TabParts,
    item: &TabItem,
    accent: bool,
    colors: TabColors,
) -> Stateful<Div> {
    let (hover, foreground) = (colors.hover(), colors.foreground);
    let active = item.active();
    shell.child(
        div()
            .relative()
            .flex_1()
            .min_w_0()
            // An explicit height keeps the margins inside the slot; a full
            // height plus margins would overflow it and lose the inset.
            .h(TAB_HEIGHT - VERTICAL_ROW_MARGIN_Y * 2.0)
            .mx(VERTICAL_ROW_MARGIN_X)
            .my(VERTICAL_ROW_MARGIN_Y)
            .rounded(px(7.0))
            .flex()
            .items_center()
            .gap(PILL_GAP)
            .pl(pill_padding_left(accent))
            .pr(PILL_PADDING_RIGHT)
            .when(active, |row| row.bg(colors.active))
            .when(active && accent, |row| {
                row.child(tab_accent_bar(colors, px(8.0)))
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

/// Left and right Strip placement: the active row spans the column and
/// merges into the terminal, with the accent line on the window edge and
/// `tab_border` lines above and below because the open edge is tall. A
/// flush first row omits its top line; the bar's top border serves instead.
fn vertical_strip_tab(
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
        .when(item.active(), |tab| {
            tab.bg(colors.terminal)
                .border_color(colors.border)
                .border_b(px(1.0))
                .when(!item.flush_start, |tab| tab.border_t(px(1.0)))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .w(px(2.0))
                        .bg(colors.accent)
                        .when(position == TabPosition::Left, Styled::left_0)
                        .when(position != TabPosition::Left, Styled::right_0),
                )
        })
        .when(!item.active(), |tab| {
            tab.hover(|style| style.bg(hover).text_color(foreground))
        })
        .children(parts.status)
        .child(parts.title)
        .child(parts.close)
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
        .when(item.active(), |tab| {
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
        .when(!item.active(), |tab| {
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
    let active = item.active();
    shell
        .pl(PILL_MARGIN_LEFT)
        .pr(PILL_MARGIN_RIGHT)
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
                    pill.child(tab_accent_bar(colors, px(7.0)))
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
/// by `strip_tab` and `pill_tab`, including the indicator `status` draws.
pub(super) fn fit_tab_width(
    title_width: Pixels,
    tabs: TabsConfig,
    status: TabStatus,
) -> Pixels {
    let style = tabs.style;
    let status = if status == TabStatus::Idle {
        px(0.0)
    } else {
        status.indicator_width() + gap(style)
    };
    // Every tab has a title and a close button separated by one gap.
    let chrome = match style {
        TabStyle::Strip => STRIP_PADDING_LEFT + STRIP_PADDING_RIGHT,
        TabStyle::Pill => {
            PILL_MARGIN_LEFT
                + PILL_MARGIN_RIGHT
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
mod tests;
