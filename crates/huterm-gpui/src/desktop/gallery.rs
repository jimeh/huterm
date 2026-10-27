//! The component gallery: every `huterm-ui` component in its static states,
//! in the built-in dark theme and a bundled light theme, side by side.
//!
//! It is each component's first consumer and gives `ux-review` and
//! screenshots one place per component. `scripts/ui-components.test.ts`
//! fails when a file under `crates/huterm-ui/src/components/` has no
//! [`entry`] here, so every entry names its component file.

use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, Context, Div, FocusHandle, IntoElement, Render,
    ScrollHandle, Task, Window, WindowBounds, WindowOptions, div, prelude::*,
    px, size,
};
use huterm_ui::animation::AnimationSchedule;
use huterm_ui::components::badge::{Tone, badge};
use huterm_ui::components::scroll_area::ScrollArea;
use huterm_ui::components::tooltip::Tooltip;
use huterm_ui::components::{kbd, separator, surface};
use huterm_ui::focus;
use huterm_ui::list_scrollbar::ListScrollbar;
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
}

struct Gallery {
    columns: [Column; 2],
    focus: FocusHandle,
    /// The pending wake for the scrollbars' next deadline, such as the end
    /// of an indicator's hold, and that deadline.
    wake: Option<(Instant, Task<()>)>,
}

impl Gallery {
    fn new(cx: &mut Context<'_, Self>) -> Self {
        let light =
            crate::themes::preview(LIGHT_THEME, std::path::Path::new(""))
                .unwrap_or_else(Theme::huterm_dark);
        let column = |name, theme: &Theme| {
            let scroll = ScrollHandle::new();
            Column {
                name,
                tokens: tokens(theme),
                scrollbar: ListScrollbar::new(scroll.clone()),
                scroll,
            }
        };
        Self {
            columns: [
                column("huterm-dark", &Theme::huterm_dark()),
                column(LIGHT_THEME, &light),
            ],
            focus: cx.focus_handle(),
            wake: None,
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
        }
        if schedule.frame {
            window.request_animation_frame();
        }
        match schedule.deadline {
            Some(deadline)
                if self.wake.as_ref().map(|(at, _)| *at) != Some(deadline) =>
            {
                let timer = cx
                    .background_executor()
                    .timer(deadline.saturating_duration_since(now));
                let task = cx.spawn(async move |gallery, cx| {
                    timer.await;
                    let _ = gallery.update(cx, |_, cx| cx.notify());
                });
                self.wake = Some((deadline, task));
            }
            Some(_) => {}
            None => self.wake = None,
        }
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
            .into_any_element()
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
        div()
            .size_full()
            .flex()
            .track_focus(&self.focus)
            .child(focus::tracker())
            .child(dark)
            .child(light)
    }
}
