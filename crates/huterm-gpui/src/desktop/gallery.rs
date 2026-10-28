//! The component gallery: every `huterm-ui` component in its static states,
//! in the built-in dark theme and a bundled light theme, side by side.
//!
//! It is each component's first consumer and gives `ux-review` and
//! screenshots one place per component. `scripts/ui-components.test.ts`
//! fails when a file under `crates/huterm-ui/src/components/` has no
//! [`entry`] here, so every entry names its component file.

use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, FocusHandle, Focusable as _,
    IntoElement, Render, ScrollHandle, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, size,
};
use huterm_ui::animation::{AnimationSchedule, Wake};
use huterm_ui::components::badge::{Tone, badge};
use huterm_ui::components::menu::{
    MIN_WIDTH, Menu, MenuEvent, Role as MenuRole,
};
use huterm_ui::components::popover::{Anchor, Popover, place};
use huterm_ui::components::scroll_area::ScrollArea;
use huterm_ui::components::select::Select;
use huterm_ui::components::tooltip::Tooltip;
use huterm_ui::components::{kbd, separator, surface};
use huterm_ui::focus;
use huterm_ui::list_scrollbar::ListScrollbar;
use huterm_ui::listbox::{Button, Item, Model, Row};
use huterm_ui::tokens::size;

use super::key_hint::KeyHint;
use super::overlay::{Swatch, tokens};
use crate::config::Theme;
use crate::keymap::Platform;

/// The light theme shown beside the built-in dark one.
const LIGHT_THEME: &str = "tokyo-night-day";

pub(crate) fn run() {
    crate::assets::application().run(|cx| {
        let bounds = Bounds::centered(None, size(px(960.0), px(720.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..WindowOptions::default()
            },
            |_, cx| cx.new(Gallery::new),
        );
        if let Err(error) = opened {
            eprintln!("component gallery failed to open: {error:#}");
            cx.quit();
            return;
        }
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
}

/// One theme's column.
struct Column {
    name: &'static str,
    tokens: Swatch,
    scroll: ScrollHandle,
    scrollbar: ListScrollbar,
    /// A command menu drawn in place, always open.
    menu: Entity<Menu>,
    /// The popover entry's surface, opened by its button.
    popover: Popover,
    /// The select entry: its value, trigger focus, popover, and open menu.
    select: Option<&'static str>,
    select_focus: FocusHandle,
    select_popover: Popover,
    select_menu: Option<Entity<Menu>>,
}

/// The select entry's options.
const SIZES: [&str; 4] = ["Compact", "Comfortable", "Spacious", "Huge"];

fn command_menu() -> Model {
    Model::new(vec![
        Row::Item(Item::new("palette", "Command Palette").shortcut(
            KeyHint::parse("cmd-shift-p", Platform::current()).glyphs(),
        )),
        Row::Separator,
        Row::Item(Item::new("new_tab", "New Tab")),
        Row::Item(
            Item::new("copy_dir", "Copy Directory Path")
                .disabled(Some("Directory unknown".to_owned())),
        ),
        Row::Item(Item::new("notices", "Show Notices").badge(3)),
        Row::Separator,
        Row::Buttons {
            label: "Edit".to_owned(),
            buttons: vec![
                Button::new("copy", "Copy")
                    .disabled(Some("Nothing selected".to_owned())),
                Button::new("paste", "Paste"),
            ],
        },
    ])
}

struct Gallery {
    columns: [Column; 2],
    focus: FocusHandle,
    /// Wakes the gallery at the scrollbars' and menus' next deadline.
    wake: Wake,
}

impl Gallery {
    fn new(cx: &mut Context<'_, Self>) -> Self {
        let light =
            crate::themes::preview(LIGHT_THEME, std::path::Path::new(""))
                .unwrap_or_else(Theme::huterm_dark);
        let mut column = |name, theme: &Theme| {
            let scroll = ScrollHandle::new();
            let swatch = tokens(theme);
            Column {
                name,
                tokens: swatch,
                scrollbar: ListScrollbar::new(scroll.clone()),
                scroll,
                menu: cx.new(|cx| {
                    Menu::new("Window menu", command_menu(), swatch, true, cx)
                }),
                popover: Popover::new(),
                select: Some(SIZES[1]),
                select_focus: cx.focus_handle(),
                select_popover: Popover::new(),
                select_menu: None,
            }
        };
        Self {
            columns: [
                column("huterm-dark", &Theme::huterm_dark()),
                column(LIGHT_THEME, &light),
            ],
            focus: cx.focus_handle(),
            wake: Wake::default(),
        }
    }

    /// Advances the scrollbars, asking for the next frame or a wake at their
    /// next deadline, so an indicator fades without further input.
    fn animate(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let now = Instant::now();
        let mut schedule = AnimationSchedule::IDLE;
        for column in &mut self.columns {
            column.scrollbar.advance(now);
            schedule = schedule.merge(column.scrollbar.schedule(now));
            for menu in std::iter::once(&column.menu).chain(&column.select_menu)
            {
                menu.update(cx, |menu, cx| menu.advance(now, cx));
                schedule = schedule.merge(menu.read(cx).schedule(now));
            }
        }
        self.wake.arm(schedule, window, cx);
    }

    fn column(
        &mut self,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let view = cx.entity();
        let column = &mut self.columns[index];
        let swatch = column.tokens;
        let get: fn(&mut Gallery) -> &mut ListScrollbar = if index == 0 {
            |gallery| &mut gallery.columns[0].scrollbar
        } else {
            |gallery| &mut gallery.columns[1].scrollbar
        };
        let rows = ScrollArea::new(("gallery-scroll", index), &column.scroll)
            .max_height(Some(px(96.0)))
            .children((1..=20).map(|row| {
                div().px(px(8.0)).h(px(24.0)).child(format!("Row {row}"))
            }))
            .render(&mut column.scrollbar, swatch.scrollbar, &view, get, cx);
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .p(px(16.0))
            .bg(swatch.bg)
            .text_color(swatch.fg)
            .text_size(px(12.5))
            .child(div().text_color(swatch.muted).child(column.name))
            .child(badges(index, swatch))
            .child(key_caps(swatch))
            .child(entry(
                "scroll_area",
                "Scroll area",
                swatch,
                div()
                    .w(px(220.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(swatch.line_strong)
                    .child(rows),
            ))
            .child(separators(swatch))
            .child(entry(
                "surface",
                "Surface",
                swatch,
                surface::surface(swatch, size::PANEL_RADIUS)
                    .w(px(220.0))
                    .p(px(12.0))
                    .child("A raised panel"),
            ))
            .child(tooltips(index, swatch, cx))
            .child(entry(
                "menu",
                "Menu",
                swatch,
                div().child(column.menu.clone()),
            ))
            .child(self.popover_entry(index, cx))
            .child(self.select_entry(index, cx))
            .into_any_element()
    }

    /// A button that opens a modal popover; an outside press closes it.
    fn popover_entry(
        &mut self,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let column = &self.columns[index];
        let swatch = column.tokens;
        let popover = column.popover.clone();
        let opener = popover.clone();
        let button = div()
            .id(("popover-trigger", index))
            .relative()
            .px(px(10.0))
            .h(size::CONTROL_HEIGHT)
            .flex()
            .items_center()
            .rounded(px(size::CONTROL_RADIUS))
            .border_1()
            .border_color(swatch.line_strong)
            .child("Open a popover")
            .child(popover.trigger_probe())
            .on_click(cx.listener(move |_, event, _, cx| {
                if huterm_ui::click::from_keyboard(event) {
                    return;
                }
                if let Some(bounds) = opener.trigger_bounds() {
                    opener.open(Anchor::Trigger(bounds), true);
                }
                cx.notify();
            }));
        entry("popover", "Popover", swatch, div().child(button))
    }

    /// A select over [`SIZES`] whose menu opens in a popover.
    fn select_entry(
        &mut self,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let gallery = cx.entity().downgrade();
        let column = &self.columns[index];
        let swatch = column.tokens;
        let value = column.select.unwrap_or("Default");
        let trigger = Select::new(
            ("select", index),
            "Density",
            value,
            &column.select_focus,
            swatch,
        )
        .open(column.select_popover.is_open())
        .popover(&column.select_popover)
        .on_toggle(move |window, cx| {
            let _ = gallery.update(cx, |gallery, cx| {
                gallery.toggle_select(index, window, cx);
            });
        });
        entry("select", "Select", swatch, div().child(trigger))
    }

    fn toggle_select(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let column = &mut self.columns[index];
        if column.select_popover.close() {
            column.select_menu = None;
            cx.notify();
            return;
        }
        let Some(bounds) = column.select_popover.trigger_bounds() else {
            return;
        };
        let rows = SIZES
            .iter()
            .map(|size| Row::Item(Item::new(*size, *size)))
            .collect();
        let swatch = column.tokens;
        let menu = cx.new(|cx| {
            Menu::new("Density", Model::new(rows), swatch, true, cx)
                .with_role(MenuRole::ListBox)
        });
        if let Some(value) = column.select {
            menu.update(cx, |menu, cx| menu.select_item(value, cx));
        }
        cx.subscribe(&menu, move |gallery, _, event: &MenuEvent, cx| {
            let column = &mut gallery.columns[index];
            if let MenuEvent::Picked(value) = event {
                column.select =
                    SIZES.iter().copied().find(|size| *size == value.as_ref());
            }
            let _ = column.select_popover.close();
            column.select_menu = None;
            cx.notify();
        })
        .detach();
        menu.read(cx).focus_handle(cx).focus(window, cx);
        column.select_popover.open(Anchor::Trigger(bounds), true);
        column.select_menu = Some(menu);
        cx.notify();
    }

    /// The open popovers of every column, placed over the window, and the
    /// press guards that close them.
    fn overlays(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Vec<AnyElement> {
        let viewport = window.viewport_size();
        let mut overlays = Vec::new();
        for index in 0..self.columns.len() {
            let column = &self.columns[index];
            let swatch = column.tokens;
            let popover = column.popover.clone();
            let closer = popover.clone();
            overlays.push(
                popover
                    .guard(move |_, window, _| {
                        let _ = closer.close();
                        window.refresh();
                    })
                    .into_any_element(),
            );
            if let Some(anchor) = popover.anchor() {
                let placement =
                    place(anchor, size(px(200.0), px(80.0)), viewport);
                let content = surface::surface(swatch, size::PANEL_RADIUS)
                    .w(px(200.0))
                    .h(px(80.0))
                    .p(px(12.0))
                    .child("Press outside to close");
                overlays.extend(popover.render(placement, viewport, content));
            }
            let gallery = cx.entity().downgrade();
            overlays.push(
                column
                    .select_popover
                    .guard(move |_, window, cx| {
                        let _ = gallery.update(cx, |gallery, cx| {
                            gallery.toggle_select(index, window, cx);
                        });
                    })
                    .into_any_element(),
            );
            if let (Some(anchor), Some(menu)) =
                (column.select_popover.anchor(), column.select_menu.clone())
            {
                let width = match anchor {
                    Anchor::Trigger(bounds) => bounds.size.width.max(MIN_WIDTH),
                    Anchor::Pointer(_) => MIN_WIDTH,
                };
                let height = Menu::content_height(menu.read(cx).model());
                let placement = place(anchor, size(width, height), viewport);
                menu.update(cx, |menu, cx| {
                    menu.set_max_height(Some(placement.max_height), cx);
                });
                overlays.extend(column.select_popover.render(
                    placement,
                    viewport,
                    div().w(width).child(menu),
                ));
            }
        }
        overlays
    }
}

fn badges(index: usize, swatch: Swatch) -> Div {
    entry(
        "badge",
        "Badge",
        swatch,
        row()
            .child(badge(
                ("badge-neutral", index),
                "Default",
                Tone::Neutral,
                swatch,
            ))
            .child(badge(
                ("badge-accent", index),
                "Custom",
                Tone::Accent,
                swatch,
            ))
            .child(badge(("badge-warning", index), "3", Tone::Warning, swatch))
            .child(badge(("badge-danger", index), "!", Tone::Danger, swatch)),
    )
}

fn key_caps(swatch: Swatch) -> Div {
    let glyphs = KeyHint::parse("cmd-shift-p", Platform::current()).glyphs();
    entry(
        "kbd",
        "Kbd",
        swatch,
        row().child(kbd::cap(&glyphs, swatch)).child(
            div()
                .px(px(10.0))
                .h(size::CONTROL_HEIGHT)
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(px(size::CONTROL_RADIUS))
                .bg(swatch.accent)
                .text_color(swatch.on_accent())
                .child("Run")
                .child(kbd::cap_on(&glyphs, swatch.on_accent())),
        ),
    )
}

fn separators(swatch: Swatch) -> Div {
    entry(
        "separator",
        "Separator",
        swatch,
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .w(px(220.0))
            .child("Above")
            .child(separator::horizontal(swatch))
            .child(
                row()
                    .h(px(20.0))
                    .child("Left")
                    .child(separator::vertical(swatch))
                    .child("Right"),
            ),
    )
}

fn tooltips(index: usize, swatch: Swatch, cx: &mut App) -> Div {
    entry(
        "tooltip",
        "Tooltip",
        swatch,
        row().child(Tooltip::view("A tooltip", swatch, cx)).child(
            div()
                .id(("tooltip-target", index))
                .px(px(10.0))
                .h(size::CONTROL_HEIGHT)
                .flex()
                .items_center()
                .rounded(px(size::CONTROL_RADIUS))
                .border_1()
                .border_color(swatch.line_strong)
                .child("Hover me")
                .tooltip(move |_, cx| {
                    Tooltip::view("Shown on hover", swatch, cx)
                }),
        ),
    )
}

/// A labeled gallery entry for the component in
/// `crates/huterm-ui/src/components/<component>.rs`.
fn entry(
    component: &'static str,
    title: &'static str,
    swatch: Swatch,
    content: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .gap(px(6.0))
                .text_size(px(11.5))
                .child(div().text_color(swatch.fg).child(title))
                .child(
                    div()
                        .text_color(swatch.dim)
                        .child(format!("components/{component}.rs")),
                ),
        )
        .child(content)
}

fn row() -> Div {
    div().flex().items_center().gap(px(8.0))
}

impl Render for Gallery {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        self.animate(window, cx);
        let dark = self.column(0, cx);
        let light = self.column(1, cx);
        let overlays = self.overlays(window, cx);
        div()
            .id("gallery")
            .size_full()
            .flex()
            .overflow_y_scroll()
            .track_focus(&self.focus)
            .child(focus::tracker())
            .child(dark)
            .child(light)
            .children(overlays)
    }
}
