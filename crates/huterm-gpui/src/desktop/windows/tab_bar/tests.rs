use gpui::{point, size};

use super::super::{
    ChromeLayout, WindowFrame, select_notch_shelf, strip_bounds,
    terminal_corner_radius,
};
use super::*;

fn tabs(style: TabStyle, pill_accent: bool, min: f32, max: f32) -> TabsConfig {
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
    let idle = TabStatus::Idle;
    let strip_tabs = tabs(TabStyle::Strip, false, 48.0, 600.0);
    let strip = fit_tab_width(px(20.0), strip_tabs, idle);
    assert_eq!(strip, px(20.0 + 12.0 + 6.0 + 7.0 + 18.0));
    for status in [TabStatus::Exited, TabStatus::Failed, TabStatus::Bell] {
        assert_eq!(
            fit_tab_width(px(20.0), strip_tabs, status),
            strip + ICON_SIZE + STRIP_GAP,
            "{status:?}"
        );
    }
    assert_eq!(
        fit_tab_width(px(20.0), strip_tabs, TabStatus::Busy),
        strip + DOT_SIZE + STRIP_GAP,
        "the running dot takes its own width"
    );
    let pill_tabs = tabs(TabStyle::Pill, false, 48.0, 600.0);
    let pill = fit_tab_width(px(20.0), pill_tabs, idle);
    assert_eq!(pill, px(20.0 + 3.0 + 2.0 + 9.0 + 5.0 + 6.0 + 18.0));
    assert_eq!(
        fit_tab_width(px(20.0), pill_tabs, TabStatus::Exited),
        pill + ICON_SIZE + PILL_GAP
    );
    assert_eq!(
        fit_tab_width(px(20.0), pill_tabs, TabStatus::Busy),
        pill + DOT_SIZE + PILL_GAP
    );
    assert_eq!(
        fit_tab_width(px(20.0), tabs(TabStyle::Pill, true, 48.0, 600.0), idle),
        pill + PILL_ACCENT_PADDING_LEFT - PILL_PADDING_LEFT
    );
    assert_eq!(fit_tab_width(px(20.4), strip_tabs, idle), px(64.0));
    assert_eq!(
        fit_tab_width(px(1.0), tabs(TabStyle::Strip, false, 96.0, 240.0), idle),
        px(96.0)
    );
    assert_eq!(
        fit_tab_width(
            px(900.0),
            tabs(TabStyle::Pill, false, 96.0, 240.0),
            idle
        ),
        px(240.0)
    );
}

#[test]
fn horizontal_dividers_skip_the_first_active_and_following_tabs() {
    let item = |index, activity| TabItem {
        id: TabId::new(1),
        index,
        title: String::new(),
        status: TabStatus::Idle,
        activity,
        flush_start: false,
        targeted: false,
    };
    assert!(item(1, Activity::Inactive).divided());
    assert!(!item(0, Activity::Inactive).divided());
    assert!(!item(2, Activity::Active).divided());
    assert!(!item(3, Activity::FollowsActive).divided());
}

#[test]
fn error_exit_and_bell_take_precedence_over_the_running_dot() {
    let job = Some("cargo");
    assert_eq!(TabStatus::new(false, false, false, None), TabStatus::Idle);
    assert_eq!(
        TabStatus::new(false, false, false, Some("")),
        TabStatus::Idle,
        "an empty name is no foreground program"
    );
    assert_eq!(TabStatus::new(false, false, false, job), TabStatus::Busy);
    assert_eq!(TabStatus::new(false, false, true, job), TabStatus::Bell);
    assert_eq!(TabStatus::new(false, false, true, None), TabStatus::Bell);
    assert_eq!(TabStatus::new(true, false, true, job), TabStatus::Exited);
    assert_eq!(TabStatus::new(true, true, true, job), TabStatus::Failed);
    assert!(TabStatus::Exited.exited());
    assert!(TabStatus::Failed.exited());
    assert!(!TabStatus::Busy.exited());
}

#[test]
fn top_chrome_border_spans_the_terminal_beside_vertical_tab_bars() {
    let viewport = size(px(800.0), px(600.0));
    for position in [TabPosition::Left, TabPosition::Right] {
        let layout = ChromeLayout::new(viewport, px(28.0), position);
        let border = layout
            .top_chrome_border(position, px(28.0), false)
            .expect("titlebar border");
        let terminal = layout.terminal;
        assert_eq!(border.bottom(), terminal.origin.y);
        assert_eq!(border.origin.x, terminal.origin.x);
        assert_eq!(border.size.width, terminal.size.width);
        assert_eq!(border.size.height, px(1.0));
        // Spanning the bar as well covers the whole window width.
        let spanning = layout
            .top_chrome_border(position, px(28.0), true)
            .expect("spanning border");
        assert_eq!(spanning.origin.x, px(0.0));
        assert_eq!(spanning.size.width, viewport.width);
        assert_eq!(spanning.origin.y, border.origin.y);
        // The two lines meet at the terminal's top corner.
        let side = layout.tab_border(position);
        let corner = match position {
            TabPosition::Left => side.right() == border.origin.x,
            _ => side.origin.x == border.right(),
        };
        assert!(corner, "{position:?}: {side:?} vs {border:?}");
        assert!(layout.top_chrome_border(position, px(0.0), false).is_none());
    }
    for position in [TabPosition::Top, TabPosition::Bottom] {
        let layout = ChromeLayout::new(viewport, px(28.0), position);
        assert!(layout.top_chrome_border(position, px(28.0), true).is_none());
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
        let line = layout.top_chrome_border(position, px(28.0), false).unwrap();
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
        assert!(layout.terminal_corner(position, px(0.0), px(4.0)).is_none());
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
    let strip = strip_bounds(tabs, px(0.0), px(0.0), pill(TabPosition::Top));
    // The first pill's own margin plus the lead equals the vertical inset.
    assert_eq!(
        strip.origin.x + PILL_MARGIN_LEFT,
        tabs.origin.x + PILL_INSET
    );
    assert_eq!(strip.right(), tabs.right());
    assert_eq!(strip.size.height, tabs.size.height);
    assert_eq!(
        strip_bounds(tabs, px(0.0), px(0.0), pill(TabPosition::Left)),
        tabs
    );
    // A column keeps its rows below the safe area it spans.
    let inset = strip_bounds(tabs, px(10.0), px(0.0), pill(TabPosition::Left));
    assert_eq!(inset.origin.y, tabs.origin.y + px(10.0));
    assert_eq!(inset.bottom(), tabs.bottom());
    let strip_style = TabsConfig {
        style: TabStyle::Strip,
        ..TabsConfig::default()
    };
    assert_eq!(strip_bounds(tabs, px(0.0), px(0.0), strip_style), tabs);
    let tiny = Bounds::new(tabs.origin, size(px(1.0), px(32.0)));
    assert_eq!(
        strip_bounds(tiny, px(0.0), px(0.0), pill(TabPosition::Bottom))
            .size
            .width,
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
        None,
        WindowFrame::default(),
    );
    assert_eq!(layout.tabs.size.height, px(34.0));
    assert_eq!(layout.terminal.origin.y, px(28.0 + 34.0));
}

#[test]
fn a_notch_shelf_holds_a_top_bar_beside_the_notch_at_full_height() {
    let viewport = size(px(1800.0), px(1169.0));
    let safe_area = gpui::Edges {
        top: px(38.0),
        ..Default::default()
    };
    let shelf = Bounds::new(point(px(0.0), px(0.0)), size(px(790.0), px(38.0)));
    let pill = TabsConfig {
        position: TabPosition::Top,
        style: TabStyle::Pill,
        ..TabsConfig::default()
    };
    let layout = ChromeLayout::for_tabs(
        viewport,
        px(0.0),
        pill,
        px(220.0),
        safe_area,
        Some(shelf),
        WindowFrame::default(),
    );
    // Bottom-aligned in the shelf at the Pill bar's own height.
    assert_eq!(layout.tabs.size, size(px(790.0), px(34.0)));
    assert_eq!(layout.tabs.bottom(), px(38.0));
    assert_eq!(layout.tabs.origin.x, px(0.0));
    // The terminal keeps everything under the safe area except the
    // point the border line uses.
    assert_eq!(layout.terminal.origin.y, px(39.0));
    assert_eq!(layout.terminal.size.height, px(1169.0 - 39.0));
    // A shelf shorter than the bar is ignored: the bar keeps its full
    // height below the safe area instead of shrinking into the shelf.
    let short = ChromeLayout::for_tabs(
        viewport,
        px(0.0),
        pill,
        px(220.0),
        safe_area,
        Some(Bounds::new(
            point(px(0.0), px(0.0)),
            size(px(790.0), px(30.0)),
        )),
        WindowFrame::default(),
    );
    assert_eq!(short.tabs.size.height, px(34.0));
    assert_eq!(short.tabs.origin.y, px(38.0));
    assert_eq!(short.terminal.origin.y, px(38.0 + 34.0));
    // Hiding a shelf bar must not pull the terminal into the notch.
    for presentation in [Presentation::Hidden, Presentation::Overlay] {
        let hidden = layout.present(presentation, TabPosition::Top, 0.0);
        assert_eq!(hidden.terminal, layout.terminal, "{presentation:?}");
    }
    // Other placements ignore the shelf.
    let column = ChromeLayout::for_tabs(
        viewport,
        px(0.0),
        TabsConfig {
            position: TabPosition::Left,
            ..TabsConfig::default()
        },
        px(220.0),
        safe_area,
        Some(shelf),
        WindowFrame::default(),
    );
    assert_eq!(column.tabs.origin.y, px(0.0));
    assert!(column.tabs.size.height > px(1000.0));
}

#[test]
fn close_buttons_show_by_mode_before_any_hover() {
    assert!(!close_button_shown(TabCloseButton::Hover, true));
    assert!(!close_button_shown(TabCloseButton::Hover, false));
    assert!(close_button_shown(TabCloseButton::Active, true));
    assert!(!close_button_shown(TabCloseButton::Active, false));
    assert!(close_button_shown(TabCloseButton::Always, false));
}

#[test]
fn shelf_selection_follows_the_config_and_only_top_bars() {
    use huterm_config::TabNotch;
    let shelves = crate::fullscreen::NotchShelves {
        left: Bounds::new(point(px(0.0), px(0.0)), size(px(790.0), px(38.0))),
        right: Bounds::new(
            point(px(1010.0), px(0.0)),
            size(px(790.0), px(38.0)),
        ),
    };
    let tabs = |position, notch| TabsConfig {
        position,
        notch,
        ..TabsConfig::default()
    };
    assert_eq!(
        select_notch_shelf(
            tabs(TabPosition::Top, TabNotch::Left),
            Some(shelves)
        ),
        Some(shelves.left)
    );
    assert_eq!(
        select_notch_shelf(
            tabs(TabPosition::Top, TabNotch::Right),
            Some(shelves)
        ),
        Some(shelves.right)
    );
    assert!(
        select_notch_shelf(
            tabs(TabPosition::Top, TabNotch::Off),
            Some(shelves)
        )
        .is_none()
    );
    assert!(
        select_notch_shelf(tabs(TabPosition::Top, TabNotch::Left), None)
            .is_none()
    );
    for position in [TabPosition::Bottom, TabPosition::Left, TabPosition::Right]
    {
        assert!(
            select_notch_shelf(tabs(position, TabNotch::Left), Some(shelves))
                .is_none(),
            "{position:?}"
        );
    }
    // A shelf shorter than the bar is not selected, so auto-hide and the
    // shelf background follow the same fallback as the layout.
    let short = crate::fullscreen::NotchShelves {
        left: Bounds::new(shelves.left.origin, size(px(790.0), px(20.0))),
        right: shelves.right,
    };
    assert!(
        select_notch_shelf(tabs(TabPosition::Top, TabNotch::Left), Some(short))
            .is_none()
    );
    assert_eq!(
        select_notch_shelf(
            tabs(TabPosition::Top, TabNotch::Right),
            Some(short)
        ),
        Some(short.right)
    );
}

#[test]
fn vertical_columns_overlay_beside_a_safe_area_without_moving_rows() {
    let viewport = size(px(800.0), px(600.0));
    let safe_area = gpui::Edges {
        top: px(38.0),
        ..Default::default()
    };
    let tabs = TabsConfig {
        position: TabPosition::Left,
        ..TabsConfig::default()
    };
    let reserved = ChromeLayout::for_tabs(
        viewport,
        px(0.0),
        tabs,
        px(220.0),
        safe_area,
        None,
        WindowFrame::default(),
    );
    let overlay =
        reserved.present(Presentation::Overlay, TabPosition::Left, 0.5);
    // The terminal takes the full width under the safe area.
    assert_eq!(overlay.terminal.origin, point(px(0.0), px(38.0)));
    assert_eq!(overlay.terminal.size.width, px(800.0));
    // The column slides in from the left at half progress, still from the
    // screen top, with its rows below the safe area.
    assert_eq!(overlay.tabs.origin.x, -reserved.tabs.size.width * 0.5);
    assert_eq!(overlay.tabs.origin.y, px(0.0));
    assert_eq!(overlay.strip_bounds(tabs).origin.y, px(38.0));
}

#[test]
fn top_chrome_follows_a_visible_or_revealing_tab_bar() {
    for position in [TabPosition::Top, TabPosition::Left, TabPosition::Right] {
        assert!(top_chrome_uses_bar(
            position,
            true,
            Presentation::Reserved,
            0.0
        ));
        assert!(!top_chrome_uses_bar(
            position,
            true,
            Presentation::Hidden,
            1.0
        ));
        assert!(!top_chrome_uses_bar(
            position,
            true,
            Presentation::Overlay,
            0.0
        ));
        assert!(top_chrome_uses_bar(
            position,
            true,
            Presentation::Overlay,
            0.01
        ));
    }
    // Without a titlebar only a top bar colors the safe-area strip.
    assert!(top_chrome_uses_bar(
        TabPosition::Top,
        false,
        Presentation::Reserved,
        0.0
    ));
    for column in [TabPosition::Left, TabPosition::Right] {
        assert!(!top_chrome_uses_bar(
            column,
            false,
            Presentation::Reserved,
            0.0
        ));
    }
    let bottom = TabPosition::Bottom;
    assert!(!top_chrome_uses_bar(
        bottom,
        true,
        Presentation::Reserved,
        0.0
    ));
    assert!(!top_chrome_uses_bar(
        bottom,
        true,
        Presentation::Overlay,
        1.0
    ));
}

#[test]
fn tab_border_sits_on_the_terminal_edge_for_every_placement() {
    let viewport = size(px(800.0), px(600.0));
    for position in [
        TabPosition::Top,
        TabPosition::Bottom,
        TabPosition::Left,
        TabPosition::Right,
        TabPosition::Titlebar,
    ] {
        let layout = ChromeLayout::new(viewport, px(28.0), position);
        let border = layout.tab_border(position);
        let terminal = layout.terminal;
        let touches = match position {
            TabPosition::Top | TabPosition::Titlebar => {
                border.bottom() == terminal.origin.y
            }
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
