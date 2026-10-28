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
    IntoElement, Render, ScrollHandle, SharedString, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};
use huterm_ui::animation::{AnimationSchedule, Wake};
use huterm_ui::components::badge::{Tone, badge};
use huterm_ui::components::button::{Button, Variant};
use huterm_ui::components::input::{Chrome, Input};
use huterm_ui::components::input_group::{Addon, InputGroup};
use huterm_ui::components::menu::{
    MIN_WIDTH, Menu, MenuEvent, Role as MenuRole,
};
use huterm_ui::components::number_field::{Format, NumberField};
use huterm_ui::components::popover::{Anchor, Popover, place};
use huterm_ui::components::scroll_area::ScrollArea;
use huterm_ui::components::select::Select;
use huterm_ui::components::slider::Slider;
use huterm_ui::components::switch::Switch;
use huterm_ui::components::toggle_group::ToggleGroup;
use huterm_ui::components::tooltip::Tooltip;
use huterm_ui::components::{kbd, separator, surface};
use huterm_ui::focus;
use huterm_ui::list_scrollbar::ListScrollbar;
use huterm_ui::listbox::{Button as MenuButton, Item, Model, Row};
use huterm_ui::steps::Steps;
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
            |window, cx| cx.new(|cx| Gallery::new(window, cx)),
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
    controls: Controls,
    fields: Fields,
    /// The slider entry's value, a Quake width.
    slider: f64,
}

/// The text entry entries' fields, which hold their own editing state.
struct Fields {
    /// Empty, filled, read-only, disabled, and invalid inputs.
    inputs: [Entity<Input>; 5],
    /// The grouped fields: a link with a clear button, and a size in points.
    link: Entity<Input>,
    size: Entity<Input>,
    number: Entity<NumberField>,
}

impl Fields {
    fn new(
        index: usize,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let field = |id: &str, label: &'static str, cx: &mut App| {
            let id = SharedString::from(format!("{id}-{index}"));
            cx.new(|cx| Input::new(id, label, swatch, cx))
        };
        let inputs = [
            field("input-empty", "Name", cx),
            field("input-filled", "Font", cx),
            field("input-read-only", "Config path", cx),
            field("input-disabled", "Shell", cx),
            field("input-invalid", "Padding", cx),
        ];
        let [empty, filled, read_only, disabled, invalid] = &inputs;
        empty.update(cx, |input, _| input.set_placeholder("Default"));
        filled.update(cx, |input, cx| {
            input.set_text(
                "Berkeley Mono, a family name long enough to scroll",
                cx,
            );
        });
        read_only.update(cx, |input, cx| {
            input.set_text("~/.config/huterm/config.toml", cx);
            input.set_read_only(true, cx);
        });
        disabled.update(cx, |input, cx| {
            input.set_text("/bin/zsh", cx);
            input.set_disabled(true, Some("Set by the profile".into()), cx);
        });
        invalid.update(cx, |input, cx| {
            input.set_text("-4", cx);
            input.set_invalid(true, cx);
        });
        let grouped = |id: &str, label: &'static str, cx: &mut App| {
            let id = SharedString::from(format!("{id}-{index}"));
            cx.new(|cx| {
                Input::new(id, label, swatch, cx).with_chrome(Chrome::Grouped)
            })
        };
        let link = grouped("group-link-field", "Link", cx);
        link.update(cx, |input, cx| input.set_text("https://huterm.dev", cx));
        let size = grouped("group-size-field", "Font size", cx);
        size.update(cx, |input, cx| input.set_text("13", cx));
        let number = cx.new(|cx| {
            let mut field = NumberField::new(
                SharedString::from(format!("number-{index}")),
                "Quake width",
                Steps::new(0.05, 1.0, 0.05),
                Format {
                    scale: 100.0,
                    decimals: 0,
                },
                swatch,
                window,
                cx,
            )
            .with_unit("%");
            field.set_value(Some(0.6), cx);
            field
        });
        Self {
            inputs,
            link,
            size,
            number,
        }
    }
}

/// The handles the column's focusable controls take focus through.
struct Controls {
    switches: [FocusHandle; 3],
    segments: [FocusHandle; 2],
    chips: [FocusHandle; 3],
    sliders: [FocusHandle; 2],
}

impl Controls {
    fn new(cx: &mut App) -> Self {
        Self {
            switches: std::array::from_fn(|_| cx.focus_handle()),
            segments: std::array::from_fn(|_| cx.focus_handle()),
            chips: std::array::from_fn(|_| cx.focus_handle()),
            sliders: std::array::from_fn(|_| cx.focus_handle()),
        }
    }
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
                MenuButton::new("copy", "Copy")
                    .disabled(Some("Nothing selected".to_owned())),
                MenuButton::new("paste", "Paste"),
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
    fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let light =
            crate::themes::preview(LIGHT_THEME, std::path::Path::new(""))
                .unwrap_or_else(Theme::huterm_dark);
        let mut column = |index, name, theme: &Theme| {
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
                controls: Controls::new(cx),
                fields: Fields::new(index, swatch, window, cx),
                slider: 0.6,
            }
        };
        Self {
            columns: [
                column(0, "huterm-dark", &Theme::huterm_dark()),
                column(1, LIGHT_THEME, &light),
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
            .child(buttons(index, swatch))
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
            .child(switches(index, swatch, &column.controls))
            .child(toggle_groups(index, swatch, &column.controls))
            .child(tooltips(index, swatch, cx))
            .child(entry(
                "menu",
                "Menu",
                swatch,
                div().child(column.menu.clone()),
            ))
            .child(self.popover_entry(index, cx))
            .child(self.select_entry(index, cx))
            .child(inputs(swatch, &self.columns[index].fields))
            .child(input_groups(index, swatch, &self.columns[index].fields))
            .child(entry(
                "number_field",
                "Number field",
                swatch,
                div()
                    .w(px(160.0))
                    .child(self.columns[index].fields.number.clone()),
            ))
            .child(self.slider_entry(index, cx))
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

    /// A Quake width slider, and a disabled one.
    fn slider_entry(
        &mut self,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let gallery = cx.entity().downgrade();
        let column = &self.columns[index];
        let swatch = column.tokens;
        let steps = Steps::new(0.05, 1.0, 0.05);
        let [focus, disabled] = &column.controls.sliders;
        let percent = |value: f64| format!("{:.0}%", value * 100.0);
        entry(
            "slider",
            "Slider",
            swatch,
            div()
                .w(px(220.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    Slider::new(
                        ("slider", index),
                        "Quake width",
                        column.slider,
                        steps,
                        focus,
                        swatch,
                    )
                    .readout(percent(column.slider))
                    .on_change(move |value, _, cx| {
                        let _ = gallery.update(cx, |gallery, cx| {
                            gallery.columns[index].slider = value;
                            cx.notify();
                        });
                    }),
                )
                .child(
                    Slider::new(
                        ("slider-disabled", index),
                        "Quake height",
                        0.4,
                        steps,
                        disabled,
                        swatch,
                    )
                    .readout(percent(0.4))
                    .disabled(Some("Quake is off".into())),
                ),
        )
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
        if self.columns[index].select_popover.is_open() {
            self.close_select(index, window, cx);
            return;
        }
        let column = &mut self.columns[index];
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
        cx.subscribe_in(
            &menu,
            window,
            move |gallery, _, event: &MenuEvent, window, cx| {
                if let MenuEvent::Picked(value) = event {
                    gallery.columns[index].select = SIZES
                        .iter()
                        .copied()
                        .find(|size| *size == value.as_ref());
                }
                gallery.close_select(index, window, cx);
            },
        )
        .detach();
        menu.read(cx).focus_handle(cx).focus(window, cx);
        column.select_popover.open(Anchor::Trigger(bounds), true);
        column.select_menu = Some(menu);
        cx.notify();
    }

    /// Closes a column's select menu and returns focus to its trigger.
    fn close_select(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let column = &mut self.columns[index];
        let _ = column.select_popover.close();
        column.select_menu = None;
        column.select_focus.focus(window, cx);
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

fn buttons(index: usize, swatch: Swatch) -> Div {
    let variants = [
        ("Outline", Variant::Outline),
        ("Secondary", Variant::Secondary),
        ("Ghost", Variant::Ghost),
        ("Primary", Variant::Primary),
        ("Delete", Variant::Destructive),
        ("Link", Variant::Link),
    ];
    let glyphs = KeyHint::parse("cmd-r", Platform::current()).glyphs();
    entry(
        "button",
        "Button",
        swatch,
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(row().children(variants.into_iter().map(
                |(label, variant)| {
                    let id = format!("button-{label}-{index}");
                    Button::new(SharedString::from(id), label, swatch)
                        .variant(variant)
                },
            )))
            .child(
                row()
                    .child(
                        Button::new(("button-reload", index), "Reload", swatch)
                            .variant(Variant::Primary)
                            .shortcut(glyphs.clone()),
                    )
                    .child(
                        Button::new(("button-reset", index), "Reset", swatch)
                            .shortcut(glyphs),
                    )
                    .child(
                        Button::new(
                            ("button-disabled", index),
                            "Disabled",
                            swatch,
                        )
                        .disabled(Some("Shown when unavailable".into())),
                    )
                    .child(
                        Button::new(
                            ("button-new-tab", index),
                            "New tab",
                            swatch,
                        )
                        .variant(Variant::Ghost)
                        .icon(
                            gpui::svg()
                                .path("icons/plus.svg")
                                .size(px(14.0))
                                .text_color(swatch.fg),
                        )
                        .icon_only(),
                    ),
            ),
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

fn switches(index: usize, swatch: Swatch, controls: &Controls) -> Div {
    let [on, off, disabled] = &controls.switches;
    entry(
        "switch",
        "Switch",
        swatch,
        row()
            .child(Switch::new(("switch-on", index), "On", true, on, swatch))
            .child(Switch::new(
                ("switch-off", index),
                "Off",
                false,
                off,
                swatch,
            ))
            .child(
                Switch::new(
                    ("switch-disabled", index),
                    "Disabled",
                    true,
                    disabled,
                    swatch,
                )
                .disabled(None),
            ),
    )
}

fn toggle_groups(index: usize, swatch: Swatch, controls: &Controls) -> Div {
    let [segments, disabled] = &controls.segments;
    let chips = ["Cmd", "Alt", "Ctrl"].iter().zip(&controls.chips).fold(
        ToggleGroup::multiple(("chips", index), "Modifiers", swatch),
        |group, (label, focus)| {
            group.toggle(*label, *label, *label != "Ctrl", focus)
        },
    );
    entry(
        "toggle_group",
        "Toggle group",
        swatch,
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                row()
                    .child(
                        ToggleGroup::single(
                            ("segments", index),
                            "Position",
                            segments,
                            swatch,
                        )
                        .option("top", "Top", true)
                        .option("bottom", "Bottom", false),
                    )
                    .child(
                        ToggleGroup::single(
                            ("segments-disabled", index),
                            "Disabled",
                            disabled,
                            swatch,
                        )
                        .option("on", "On", false)
                        .option("off", "Off", true)
                        .disabled(None),
                    ),
            )
            .child(chips),
    )
}

fn inputs(swatch: Swatch, fields: &Fields) -> Div {
    entry(
        "input",
        "Input",
        swatch,
        div()
            .w(px(220.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .children(fields.inputs.iter().cloned()),
    )
}

fn input_groups(index: usize, swatch: Swatch, fields: &Fields) -> Div {
    let link = fields.link.downgrade();
    let icon = |path: &'static str| {
        gpui::svg()
            .path(path)
            .size(px(14.0))
            .text_color(swatch.muted)
    };
    entry(
        "input_group",
        "Input group",
        swatch,
        div()
            .w(px(220.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                InputGroup::new(("group-link", index), &fields.link, swatch)
                    .leading(Addon::Icon(
                        icon("icons/link.svg").into_any_element(),
                    ))
                    .trailing(Addon::Button(
                        Button::new(("group-clear", index), "Clear", swatch)
                            .variant(Variant::Ghost)
                            .icon(icon("icons/x.svg"))
                            .icon_only()
                            .on_click(move |_, cx| {
                                let _ = link.update(cx, |input, cx| {
                                    input.set_text("", cx);
                                });
                            }),
                    )),
            )
            .child(
                InputGroup::new(("group-size", index), &fields.size, swatch)
                    .trailing(Addon::Text("pt".into())),
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
                        .text_color(swatch.muted)
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
