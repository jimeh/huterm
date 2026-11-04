//! The Window Transparency panel: an inspector card at the top right of the
//! terminal area that overrides the four transparency values for this
//! window alone, with no scrim, so the window shows each change as it
//! happens.
//!
//! Anatomy: a title and a close button; one row per value with its label,
//! where its value comes from ("Default", "Profile", or a dot and "This
//! window"), a slider with a readout, and an icon-only Reset whose slot
//! stays while there is nothing to reset; then Reset All and Edit Defaults
//! in Settings. Linux has no Blur row. Without transparency support every
//! slider is disabled and the reason shows above the rows.
//!
//! Overlay: the panel has its own modal `Popover`, separate from the menus'
//! one, so closing a menu leaves it alone. Opening a menu, the palette, or a
//! dialog closes it first, and opening it closes an open menu. A press
//! outside it, including one against a frame drawn before it, closes it and
//! stops there; so do Escape, the command again, deactivating the window,
//! and the window starting to close. Each close returns focus to the active
//! terminal, which is the tab's focused pane when the pane focus was on
//! closed meanwhile.
//!
//! Keyboard: the `panel_*` commands under the `Panel` context. Tab and
//! Shift-Tab move through the enabled controls, the arrows step the focused
//! slider, Page Up, Page Down, Home, End, and Reset act on a slider, and
//! Enter and Space press a button. Each keyboard move, and the slider that
//! takes focus after a Reset or Reset All, scrolls a short panel's body to
//! show the focused control's whole row.
//!
//! Accessibility: a `Group` named "Window Transparency"; each slider is
//! named by its row's label, reports its readout, and describes where its
//! value comes from, and each Reset is named for its row.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, FocusHandle, IntoElement, ParentElement,
    ScrollHandle, Styled, Window, canvas, div, prelude::*, px, svg,
};
use huterm_protocol::{CommandId, ids};
use huterm_ui::components::button::{Button, Variant};
use huterm_ui::components::popover::{Anchor, Placement};
use huterm_ui::components::scroll_area::{ContentBounds, ScrollArea};
use huterm_ui::components::separator;
use huterm_ui::components::slider::Slider;
use huterm_ui::steps::Steps;
use huterm_ui::typography::ui;

use super::{ChromeLayout, CommandError, CommandOutcome, WorkspaceView};
use crate::assets::Icon;
use crate::desktop::overlay::{Swatch, tokens};
use crate::keymap::{PANEL_BUTTON_CONTEXT, PANEL_SLIDER_CONTEXT, Platform};
use crate::transparency::{Key, Layer, Source, Support, Values};

/// The panel's width, at most.
const WIDTH: f32 = 288.0;
/// The space between the panel and the terminal area's edges.
const MARGIN: f32 = 8.0;
/// The readouts' column, so the tracks line up.
const READOUT_WIDTH: f32 = 40.0;
const PADDING_X: f32 = 14.0;

/// The panel's commands, which go to it before anything else.
pub(super) fn is_panel_command(command: CommandId) -> bool {
    matches!(
        command,
        ids::PANEL_CLOSE
            | ids::PANEL_FOCUS_NEXT
            | ids::PANEL_FOCUS_PREVIOUS
            | ids::PANEL_INCREASE
            | ids::PANEL_DECREASE
            | ids::PANEL_PAGE_UP
            | ids::PANEL_PAGE_DOWN
            | ids::PANEL_FIRST
            | ids::PANEL_LAST
            | ids::PANEL_PRESS
            | ids::PANEL_RESET
    )
}

/// One of the panel's focusable controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Control {
    Close,
    Slider(Key),
    Reset(Key),
    ResetAll,
    Settings,
}

/// Where focus goes when the panel closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PanelFocus {
    /// To the active terminal.
    Terminal,
    /// Wherever the closing action puts it, such as a menu or dialog that
    /// is opening.
    Keep,
}

/// The open panel's focus handles, scroll position, and where its rows
/// last painted.
pub(super) struct Panel {
    close: FocusHandle,
    sliders: [FocusHandle; 4],
    resets: [FocusHandle; 4],
    reset_all: FocusHandle,
    settings: FocusHandle,
    scroll: ScrollHandle,
    /// Each row's bounds within the body, by key, recorded as it paints,
    /// so a keyboard move can scroll the row it lands in into view.
    rows: Rc<RefCell<[Option<ContentBounds>; 4]>>,
}

impl Panel {
    fn new(cx: &mut App) -> Self {
        let handle = || cx.focus_handle().tab_stop(true);
        Self {
            close: handle(),
            sliders: [handle(), handle(), handle(), handle()],
            resets: [handle(), handle(), handle(), handle()],
            reset_all: handle(),
            settings: handle(),
            scroll: ScrollHandle::new(),
            rows: Rc::default(),
        }
    }

    fn handle(&self, control: Control) -> &FocusHandle {
        match control {
            Control::Close => &self.close,
            Control::Slider(key) => &self.sliders[index(key)],
            Control::Reset(key) => &self.resets[index(key)],
            Control::ResetAll => &self.reset_all,
            Control::Settings => &self.settings,
        }
    }

    /// The control with keyboard focus, if any.
    pub(super) fn focused(
        &self,
        controls: &[Control],
        window: &Window,
    ) -> Option<Control> {
        controls
            .iter()
            .copied()
            .find(|control| self.handle(*control).is_focused(window))
    }
}

fn index(key: Key) -> usize {
    match key {
        Key::Opacity => 0,
        Key::Background => 1,
        Key::Foreground => 2,
        Key::Blur => 3,
    }
}

/// One row: its value, where that comes from, and its grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Row {
    pub(super) key: Key,
    pub(super) value: f64,
    pub(super) source: Source,
    /// Whether the window's Quake profile sets this key, so Reset brings
    /// back the profile's value rather than the default.
    pub(super) profile: bool,
    pub(super) steps: Steps,
}

/// The rows a platform shows. Linux cannot blur.
pub(super) fn keys(platform: Platform) -> Vec<Key> {
    Key::ALL
        .into_iter()
        .filter(|key| platform == Platform::MacOs || *key != Key::Blur)
        .collect()
}

/// The grid a key's slider steps along: the schema's range and step, as in
/// Settings.
pub(super) fn steps(key: Key) -> Steps {
    huterm_config::settings::SettingsModel::load()
        .ok()
        .and_then(|model| {
            model.setting(&format!("window.{}", key.config_key()))
        })
        .and_then(crate::desktop::settings::values::steps)
        .unwrap_or_else(|| match key {
            Key::Blur => {
                Steps::new(0.0, f64::from(huterm_config::MAX_BLUR), 1.0)
            }
            Key::Background => Steps::new(0.0, 1.0, 0.05),
            Key::Opacity | Key::Foreground => Steps::new(0.1, 1.0, 0.05),
        })
}

/// A slider's readout: a percent for the opacities, and the blur radius
/// or "Off".
pub(super) fn readout(key: Key, value: f64) -> String {
    if key == Key::Blur {
        return if value <= 0.0 {
            "Off".to_owned()
        } else {
            format!("{value:.0}")
        };
    }
    let percent = value * 100.0;
    if (percent - percent.round()).abs() < 0.05 {
        format!("{percent:.0}%")
    } else {
        format!("{percent:.1}%")
    }
}

/// The rows for `values`, which `profile` and `own` layered.
pub(super) fn rows(
    values: Values,
    profile: Layer,
    own: Layer,
    platform: Platform,
) -> Vec<Row> {
    keys(platform)
        .into_iter()
        .map(|key| Row {
            key,
            value: values.get(key),
            source: Source::of(key, profile, own),
            profile: profile.is_set(key),
            steps: steps(key),
        })
        .collect()
}

/// The enabled controls in Tab order: the close button, each row's slider
/// and its Reset when there is something to reset, then the footer.
pub(super) fn controls(rows: &[Row], enabled: bool) -> Vec<Control> {
    let mut controls = vec![Control::Close];
    for row in rows {
        if enabled {
            controls.push(Control::Slider(row.key));
        }
        if enabled && row.source == Source::Window {
            controls.push(Control::Reset(row.key));
        }
    }
    if enabled && rows.iter().any(|row| row.source == Source::Window) {
        controls.push(Control::ResetAll);
    }
    controls.push(Control::Settings);
    controls
}

impl WorkspaceView {
    /// The panel's rows as this window resolves them now.
    pub(super) fn panel_rows(&self, cx: &App) -> Vec<Row> {
        rows(
            self.transparency_values,
            self.profile_transparency(),
            self.own_transparency(cx),
            Platform::current(),
        )
    }

    /// Why the panel's sliders cannot act, if they cannot.
    fn panel_disabled_reason(&self) -> Option<&'static str> {
        self.transparency_support.note()
    }

    fn panel_controls(&self, cx: &App) -> Vec<Control> {
        controls(
            &self.panel_rows(cx),
            self.transparency_support == Support::Available,
        )
    }

    /// The `window_transparency` command: opens the panel, or closes it.
    pub(super) fn toggle_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        if self.panel.is_some() {
            self.close_panel(window, cx);
            return Ok(CommandOutcome::Completed);
        }
        self.check_available(true)?;
        if self.palette.is_some() {
            return Err(CommandError::Unavailable(
                "command palette is open".to_owned(),
            ));
        }
        self.cancel_reorder(window, cx);
        self.cancel_pane_drag(window, cx);
        self.clear_incoming(true, window, cx);
        if let Some(terminal) = self.active_view(cx) {
            terminal.update(cx, super::TerminalView::clear_composition);
        }
        let panel = Panel::new(cx);
        let first = self
            .panel_controls(cx)
            .into_iter()
            .find(|control| matches!(control, Control::Slider(_)))
            .unwrap_or(Control::Close);
        panel.handle(first).clone().focus(window, cx);
        self.panel = Some(panel);
        self.panel_popover
            .open(Anchor::Trigger(self.chrome_layout(window).terminal), true);
        cx.notify();
        Ok(CommandOutcome::Completed)
    }

    /// Closes the panel; `false` when it was not open. The caller redraws.
    pub(super) fn dismiss_panel(
        &mut self,
        focus: PanelFocus,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let _ = self.panel_popover.close();
        if self.panel.take().is_none() {
            return false;
        }
        if focus == PanelFocus::Terminal {
            self.focus_terminal(window, cx);
        }
        true
    }

    /// Closes the panel, returning focus to the terminal, and redraws.
    pub(super) fn close_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.dismiss_panel(PanelFocus::Terminal, window, cx) {
            cx.notify();
        }
    }

    pub(super) fn panel_availability(&self) -> Result<(), CommandError> {
        if self.panel.is_some() {
            Ok(())
        } else {
            Err(CommandError::Unavailable("no panel is open".to_owned()))
        }
    }

    /// Runs a `panel_*` command on the open panel.
    pub(super) fn run_panel_command(
        &mut self,
        command: CommandId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.panel_availability()?;
        let controls = self.panel_controls(cx);
        let focused = self
            .panel
            .as_ref()
            .and_then(|panel| panel.focused(&controls, window));
        match command {
            ids::PANEL_CLOSE => self.close_panel(window, cx),
            ids::PANEL_FOCUS_NEXT | ids::PANEL_FOCUS_PREVIOUS => {
                let forward = command == ids::PANEL_FOCUS_NEXT;
                let next = focused
                    .and_then(|control| {
                        controls.iter().position(|other| *other == control)
                    })
                    .map_or(
                        if forward { 0 } else { controls.len() - 1 },
                        |at| {
                            if forward {
                                (at + 1) % controls.len()
                            } else {
                                (at + controls.len() - 1) % controls.len()
                            }
                        },
                    );
                self.focus_panel_control(controls[next], window, cx);
            }
            ids::PANEL_PRESS => {
                if let Some(control) = focused {
                    self.press_panel_control(control, window, cx);
                }
            }
            _ => {
                let Some(Control::Slider(key)) = focused else {
                    return Ok(CommandOutcome::Completed);
                };
                if command == ids::PANEL_RESET {
                    self.set_transparency_override(key, None, window, cx);
                    return Ok(CommandOutcome::Completed);
                }
                let Some(row) =
                    self.panel_rows(cx).into_iter().find(|row| row.key == key)
                else {
                    return Ok(CommandOutcome::Completed);
                };
                let (steps, value) = (row.steps, row.value);
                let next = match command {
                    ids::PANEL_INCREASE => steps.increment(value),
                    ids::PANEL_DECREASE => steps.decrement(value),
                    ids::PANEL_PAGE_UP => steps.page_up(value),
                    ids::PANEL_PAGE_DOWN => steps.page_down(value),
                    ids::PANEL_FIRST => steps.min(),
                    ids::PANEL_LAST => steps.max(),
                    _ => value,
                };
                if (next - value).abs() > f64::EPSILON {
                    self.set_transparency_override(key, Some(next), window, cx);
                }
            }
        }
        Ok(CommandOutcome::Completed)
    }

    /// Moves focus to `control` and scrolls the body to show its row with
    /// its ring. The header and footer never scroll, and a row's bounds do
    /// not depend on focus, so the last paint's bounds stand.
    fn focus_panel_control(
        &mut self,
        control: Control,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(panel) = &self.panel else {
            return;
        };
        panel.handle(control).clone().focus(window, cx);
        let key = match control {
            Control::Slider(key) | Control::Reset(key) => Some(key),
            Control::Close | Control::ResetAll | Control::Settings => None,
        };
        let row = key.and_then(|key| panel.rows.borrow()[index(key)]);
        if row.is_some_and(|row| ScrollArea::reveal(&panel.scroll, row)) {
            self.panel_scrollbar.show();
        }
        cx.notify();
    }

    /// What a click on a button, or `panel_press` on it, does.
    fn press_panel_control(
        &mut self,
        control: Control,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match control {
            Control::Close => self.close_panel(window, cx),
            Control::Reset(key) => {
                self.set_transparency_override(key, None, window, cx);
                // The Reset goes with its override, so its slider keeps
                // the keyboard's place.
                self.focus_panel_control(Control::Slider(key), window, cx);
            }
            Control::ResetAll => {
                self.replace_transparency_overrides(
                    Layer::default(),
                    window,
                    cx,
                );
                let first = self
                    .panel_controls(cx)
                    .into_iter()
                    .find(|control| matches!(control, Control::Slider(_)))
                    .unwrap_or(Control::Close);
                self.focus_panel_control(first, window, cx);
            }
            Control::Settings => {
                self.close_panel(window, cx);
                cx.defer(|cx| {
                    super::settings_window::open_at_group(
                        cx,
                        huterm_config::settings::SectionId::Appearance,
                        "Transparency",
                    );
                });
            }
            Control::Slider(_) => {}
        }
    }

    /// The open panel at the top right of the terminal area, below the tab
    /// bar or title row, inside the frame and the fullscreen safe area. A
    /// window too short for it scrolls its rows and keeps the footer.
    pub(super) fn render_panel(
        &mut self,
        layout: &ChromeLayout,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let panel = self.panel.as_ref()?;
        let swatch = tokens(&self.config.theme);
        let area = layout.terminal;
        let width = px(WIDTH).min(area.size.width - px(2.0 * MARGIN));
        let placement = Placement {
            origin: gpui::point(
                area.right() - px(MARGIN) - width,
                area.top() + px(MARGIN),
            ),
            opens_up: false,
            align_right: true,
            max_height: (area.size.height - px(2.0 * MARGIN)).max(px(0.0)),
        };
        let rows = self.panel_rows(cx);
        let reason = self.panel_disabled_reason();
        let any_override = rows.iter().any(|row| row.source == Source::Window);
        let header = self.panel_header(panel, swatch, cx);
        let note = reason.map(|reason| {
            div()
                .flex_none()
                .px(px(PADDING_X))
                .pb(px(6.0))
                .text_size(ui(11.5))
                .text_color(swatch.muted)
                .debug_selector(|| "transparency-panel-note".to_owned())
                .child(reason)
        });
        let row_elements: Vec<AnyElement> = rows
            .iter()
            .map(|row| self.panel_row(*row, reason, swatch, cx))
            .collect();
        let view = cx.entity();
        let body = ScrollArea::new("transparency-panel-body", &panel.scroll)
            .fill()
            .child(div().flex().flex_col().pb(px(6.0)).children(row_elements))
            .render(
                &mut self.panel_scrollbar,
                swatch.scrollbar,
                &view,
                |view| &mut view.panel_scrollbar,
                cx,
            );
        let panel = self.panel.as_ref()?;
        let footer = Self::panel_footer(
            panel,
            reason.or((!any_override).then_some("Nothing to reset")),
            swatch,
            cx,
        );
        let content = div()
            .id("transparency-panel")
            .key_context("Panel")
            .role(gpui::Role::Group)
            .aria_label("Window Transparency")
            .debug_selector(|| "transparency-panel".to_owned())
            .w(width)
            .max_h(placement.max_height)
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(swatch.surface)
            .border_1()
            .border_color(swatch.line)
            .rounded(px(10.0))
            .shadow(swatch.shadow())
            .text_color(swatch.fg)
            .child(header)
            .children(note)
            .child(body)
            .child(separator::horizontal(swatch).flex_none())
            .child(footer);
        self.panel_popover
            .render(placement, window.viewport_size(), content)
    }

    /// The title and the close button.
    fn panel_header(
        &self,
        panel: &Panel,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> gpui::Div {
        let _ = self;
        let close = Button::new("transparency-panel-close", "Close", swatch)
            .variant(Variant::Ghost)
            .icon(
                svg()
                    .path(Icon::X.asset_path())
                    .size(px(12.0))
                    .text_color(swatch.muted),
            )
            .icon_only()
            .focus(&panel.close)
            .key_context(PANEL_BUTTON_CONTEXT)
            .debug_selector(|| "transparency-panel-close".to_owned())
            .on_click(Self::button_action(cx, |view, window, cx| {
                view.press_panel_control(Control::Close, window, cx);
            }));
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .pl(px(PADDING_X))
            .pr(px(PADDING_X - 6.0))
            .pt(px(10.0))
            .pb(px(4.0))
            .child(
                div()
                    .text_size(ui(13.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(swatch.fg)
                    .child("Window Transparency"),
            )
            .child(close)
    }

    /// Reset All, disabled for `reset_all_disabled`, and the link to
    /// Settings.
    fn panel_footer(
        panel: &Panel,
        reset_all_disabled: Option<&'static str>,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> gpui::Div {
        let reset_all =
            Button::new("transparency-panel-reset-all", "Reset All", swatch)
                .variant(Variant::Link)
                .button_role()
                .focus(&panel.reset_all)
                .key_context(PANEL_BUTTON_CONTEXT)
                .debug_selector(|| "transparency-panel-reset-all".to_owned())
                .when_some(reset_all_disabled, |button, reason| {
                    button.disabled(Some(reason.into()))
                })
                .on_click(Self::button_action(cx, |view, window, cx| {
                    view.press_panel_control(Control::ResetAll, window, cx);
                }));
        let settings = Button::new(
            "transparency-panel-settings",
            "Edit Defaults in Settings\u{2026}",
            swatch,
        )
        .variant(Variant::Link)
        .button_role()
        .focus(&panel.settings)
        .key_context(PANEL_BUTTON_CONTEXT)
        .debug_selector(|| "transparency-panel-settings".to_owned())
        .on_click(Self::button_action(cx, |view, window, cx| {
            view.press_panel_control(Control::Settings, window, cx);
        }));
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .px(px(PADDING_X - 4.0))
            .py(px(4.0))
            .child(reset_all)
            .child(settings)
    }

    /// A row's icon-only Reset, named for its row.
    fn panel_reset(
        panel: &Panel,
        row: Row,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Button {
        let key = row.key;
        Button::new(
            ("transparency-panel-reset", index(key)),
            format!("Reset {}", key.label()),
            swatch,
        )
        .variant(Variant::Ghost)
        .tooltip(if row.profile {
            "Use the profile value"
        } else {
            "Use the default"
        })
        .icon(
            svg()
                .path(Icon::RotateCcw.asset_path())
                .size(px(12.0))
                .text_color(swatch.muted),
        )
        .icon_only()
        .focus(&panel.resets[index(key)])
        .key_context(PANEL_BUTTON_CONTEXT)
        .debug_selector(move || {
            format!("transparency-panel-reset-{}", key.config_key())
        })
        .on_click(Self::button_action(cx, move |view, window, cx| {
            view.press_panel_control(Control::Reset(key), window, cx);
        }))
    }

    /// One value's row: label and source, then its slider and Reset.
    fn panel_row(
        &self,
        row: Row,
        reason: Option<&'static str>,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let Some(panel) = &self.panel else {
            return div().into_any_element();
        };
        let key = row.key;
        let label = key.label();
        let source = source_label(row.source, swatch);
        let slider = Slider::new(
            ("transparency-panel-slider", index(key)),
            label,
            row.value,
            row.steps,
            &panel.sliders[index(key)],
            swatch,
        )
        .readout(readout(key, row.value))
        .readout_width(px(READOUT_WIDTH))
        .description(source_description(row.source))
        .key_context(PANEL_SLIDER_CONTEXT)
        .when_some(reason, |slider, reason| {
            slider.disabled(Some(reason.into()))
        })
        .on_change({
            let focus = panel.sliders[index(key)].clone();
            let view = cx.entity().downgrade();
            move |value, window, cx| {
                focus.focus(window, cx);
                let _ = view.update(cx, |view, cx| {
                    view.set_transparency_override(
                        key,
                        Some(value),
                        window,
                        cx,
                    );
                });
            }
        });
        let reset = (row.source == Source::Window && reason.is_none())
            .then(|| Self::panel_reset(panel, row, swatch, cx));
        // Reset's slot keeps its size while there is nothing to reset, so
        // the tracks never move.
        let reset_slot = div()
            .flex_none()
            .size(huterm_ui::tokens::size::CONTROL_HEIGHT)
            .children(reset);
        let probe = {
            let rows = panel.rows.clone();
            let scroll = panel.scroll.clone();
            canvas(
                |_, _, _| {},
                move |bounds, (), _, _| {
                    // Recorded while the row paints, at the offset it was
                    // laid out with.
                    rows.borrow_mut()[index(key)] =
                        Some(ContentBounds::record(&scroll, bounds));
                },
            )
            .absolute()
            .inset_0()
        };
        div()
            .relative()
            .flex()
            .flex_col()
            .px(px(PADDING_X))
            .pt(px(6.0))
            .debug_selector(move || {
                format!("transparency-panel-row-{}", key.config_key())
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(ui(12.5))
                    .child(div().text_color(swatch.fg).child(label))
                    .child(div().text_size(ui(11.0)).child(source)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(div().flex_1().min_w_0().child(slider))
                    .child(reset_slot),
            )
            .child(probe)
            .into_any_element()
    }

    /// Smoke and test state: whether the panel is open and which control
    /// has focus.
    pub(super) fn panel_focus(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<Control> {
        let controls = self.panel_controls(cx);
        self.panel.as_ref()?.focused(&controls, window)
    }

    /// The body's visible bounds in the window, while the panel is open.
    #[cfg(test)]
    pub(super) fn panel_viewport(&self) -> Option<gpui::Bounds<gpui::Pixels>> {
        self.panel.as_ref().map(|panel| panel.scroll.bounds())
    }
}

/// A slider's accessible description: where its row's value comes from,
/// which the row shows beside its label.
fn source_description(source: Source) -> &'static str {
    match source {
        Source::Default => "Uses the default",
        Source::Profile => "Set by the Quake profile",
        Source::Window => "Set for this window",
    }
}

/// Where a row's value comes from: "Default", "Profile", or a dot and
/// "This window" for an override.
fn source_label(source: Source, swatch: Swatch) -> gpui::Div {
    match source {
        Source::Window => div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .text_color(swatch.accent_text())
            .child(
                div()
                    .flex_none()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(swatch.accent),
            )
            .child("This window"),
        Source::Profile => div().text_color(swatch.muted).child("Profile"),
        Source::Default => div().text_color(swatch.muted).child("Default"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_report_sources_and_linux_has_no_blur_row() {
        let values = Values {
            opacity: 0.8,
            background: 0.6,
            foreground: 1.0,
            blur: 20,
        };
        let profile = Layer {
            background: Some(0.6),
            ..Layer::default()
        };
        let own = Layer {
            opacity: Some(0.8),
            blur: Some(20),
            ..Layer::default()
        };
        let mac = rows(values, profile, own, Platform::MacOs);
        assert_eq!(
            mac.iter()
                .map(|row| (row.key, row.source))
                .collect::<Vec<_>>(),
            [
                (Key::Opacity, Source::Window),
                (Key::Background, Source::Profile),
                (Key::Foreground, Source::Default),
                (Key::Blur, Source::Window),
            ]
        );
        assert!(mac[1].profile && !mac[0].profile);
        assert_eq!(
            keys(Platform::Linux),
            [Key::Opacity, Key::Background, Key::Foreground]
        );
        assert_eq!(steps(Key::Blur), Steps::new(0.0, 64.0, 1.0).page(6));
        assert_eq!(steps(Key::Opacity).step(), 0.05);
        assert_eq!(
            [
                readout(Key::Opacity, 0.8),
                readout(Key::Background, 0.625),
                readout(Key::Blur, 0.0),
                readout(Key::Blur, 20.0),
            ],
            ["80%", "62.5%", "Off", "20"]
        );
        // Tab reaches each slider, the Resets of overridden rows, Reset
        // All while anything is overridden, and the footer.
        assert_eq!(
            controls(&mac, true),
            [
                Control::Close,
                Control::Slider(Key::Opacity),
                Control::Reset(Key::Opacity),
                Control::Slider(Key::Background),
                Control::Slider(Key::Foreground),
                Control::Slider(Key::Blur),
                Control::Reset(Key::Blur),
                Control::ResetAll,
                Control::Settings,
            ]
        );
        let plain =
            rows(values, Layer::default(), Layer::default(), Platform::MacOs);
        assert!(!controls(&plain, true).contains(&Control::ResetAll));
        assert_eq!(controls(&mac, false), [Control::Close, Control::Settings]);
    }

    #[test]
    fn each_source_has_its_own_description() {
        assert_eq!(
            [Source::Default, Source::Profile, Source::Window]
                .map(source_description),
            [
                "Uses the default",
                "Set by the Quake profile",
                "Set for this window"
            ]
        );
    }
}
