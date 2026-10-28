//! The Settings window's controls: one per setting kind, and the special
//! controls the override table names.

use gpui::{
    AnyElement, Context, ElementId, SharedString, Window, canvas, div,
    prelude::*, px,
};
use huterm_config::settings::{Choice, Control, Setting, SettingKind};
use huterm_ui::components::number_field::Format;
use huterm_ui::components::select::Select;
use huterm_ui::components::slider::Slider;
use huterm_ui::components::switch::Switch;
use huterm_ui::components::toggle_group::ToggleGroup;
use toml::Value;

use super::super::super::overlay::Swatch;
use super::super::super::settings::values::{
    self, as_number, number, number_value,
};
use super::super::super::settings::{Special, special};
use super::{
    SEGMENTED_MAX, SettingsView, TRI_STATES, modifier_label, offered_modifiers,
    option_label, toggled_modifiers,
};
use crate::config::Theme;
use crate::renderer::rgb_color as color;

/// The longest option label a segmented control shows; longer ones use a
/// select.
const SEGMENT_LABEL_MAX: usize = 16;

/// A slider's width with its readout: room for a 5% to 100% track in 5%
/// steps beside a "100%" readout.
const SLIDER_WIDTH: f32 = 200.0;

/// How a select trigger opens its menu.
pub(super) enum Opener {
    /// A setting's own choices.
    Setting(&'static Setting),
    /// The action an action key names, such as `qprof` or `gcmd:0`.
    Action(String),
}

fn id(name: String) -> ElementId {
    ElementId::Name(name.into())
}

impl SettingsView {
    /// The control for `setting`; `read_only` says why it cannot change,
    /// when it cannot.
    pub(super) fn render_control(
        &mut self,
        setting: &'static Setting,
        read_only: Option<SharedString>,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let locked = read_only.is_some();
        match special(&setting.path) {
            Some(Special::ThemePicker) => {
                return self.render_themes(setting, locked, swatch, cx);
            }
            // Search results lead to the section's editors.
            Some(Special::GlobalShortcuts | Special::QuakeProfiles) => {
                return self
                    .action_button(
                        format!("settings-show-{}", setting.path),
                        "Show",
                        "qgo".to_owned(),
                        swatch,
                        cx,
                    )
                    .into_any_element();
            }
            Some(Special::Keybindings) => {
                return self
                    .action_button(
                        format!("settings-open-{}", setting.path),
                        "Open config.toml",
                        format!("open:{}", setting.path),
                        swatch,
                        cx,
                    )
                    .into_any_element();
            }
            Some(Special::Modifiers) => {
                return self.render_modifiers(setting, read_only, swatch, cx);
            }
            Some(Special::FontPicker) => {
                return self
                    .render_font(setting, read_only, swatch, window, cx);
            }
            Some(Special::Size) => {
                return self.render_field(setting, read_only, window, cx);
            }
            None => {}
        }
        if setting.control == Some(Control::Slider) {
            return self.render_slider(setting, read_only, swatch, cx);
        }
        match &setting.kind {
            SettingKind::Bool if self.has_default(setting) => {
                self.render_switch(setting, read_only, swatch, cx)
            }
            SettingKind::Bool => {
                self.render_optional_bool(setting, read_only, swatch, cx)
            }
            SettingKind::Choice(choices) if segmented(choices) => {
                self.render_segments(setting, choices, read_only, swatch, cx)
            }
            SettingKind::Choice(choices) => {
                self.render_select(setting, choices, locked, swatch, cx)
            }
            SettingKind::Integer { .. }
            | SettingKind::Number { .. }
            | SettingKind::Text { .. } => {
                self.render_field(setting, read_only, window, cx)
            }
            SettingKind::Compound => div().into_any_element(),
        }
    }

    /// Whether the setting has a value when the file leaves it unset.
    pub(super) fn has_default(&self, setting: &Setting) -> bool {
        setting.default.is_some() || self.defaults.get(&setting.path).is_some()
    }

    fn render_switch(
        &mut self,
        setting: &'static Setting,
        read_only: Option<SharedString>,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let on = self
            .value(setting)
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let handle = self.stop(path, cx);
        let focus = handle.clone();
        Switch::new(
            id(format!("settings-switch-{path}")),
            setting.label.clone(),
            on,
            &handle,
            swatch,
        )
        .key_context("SettingsControl")
        .when_some(read_only, |switch, reason| switch.disabled(Some(reason)))
        .on_toggle(Self::listen(cx, move |view, on, window, cx| {
            focus.focus(window, cx);
            view.write(setting, Some(Value::Boolean(on)), cx);
        }))
        .into_any_element()
    }

    /// An optional switch, such as automatic update checks, has a third
    /// state that leaves the choice to its owner.
    fn render_optional_bool(
        &mut self,
        setting: &'static Setting,
        read_only: Option<SharedString>,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let current = self.value(setting).and_then(|value| value.as_bool());
        let handle = self.stop(path, cx);
        let focus = handle.clone();
        TRI_STATES
            .iter()
            .fold(
                ToggleGroup::single(
                    id(format!("settings-tristate-{path}")),
                    setting.label.clone(),
                    &handle,
                    swatch,
                ),
                |group, (label, state)| {
                    group.option(*label, *label, current == *state)
                },
            )
            .key_context("SettingsControl")
            .when_some(read_only, |group, reason| group.disabled(Some(reason)))
            .on_select(Self::listen(
                cx,
                move |view, picked: SharedString, window, cx| {
                    focus.focus(window, cx);
                    let Some((_, state)) = TRI_STATES
                        .iter()
                        .find(|(label, _)| *label == picked.as_ref())
                    else {
                        return;
                    };
                    // The shown state is already chosen.
                    if view.value(setting).and_then(|value| value.as_bool())
                        != *state
                    {
                        view.write(setting, state.map(Value::Boolean), cx);
                    }
                },
            ))
            .into_any_element()
    }

    fn render_segments(
        &mut self,
        setting: &'static Setting,
        choices: &'static [Choice],
        read_only: Option<SharedString>,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let selected = self.value(setting);
        let handle = self.stop(path, cx);
        let focus = handle.clone();
        choices
            .iter()
            .fold(
                ToggleGroup::single(
                    id(format!("settings-segments-{path}")),
                    setting.label.clone(),
                    &handle,
                    swatch,
                ),
                |group, choice| {
                    let on = selected.as_ref().and_then(Value::as_str)
                        == Some(choice.value.as_str());
                    group.option(choice.value.clone(), option_label(choice), on)
                },
            )
            .key_context("SettingsControl")
            .when_some(read_only, |group, reason| group.disabled(Some(reason)))
            .on_select(Self::listen(
                cx,
                move |view, picked: SharedString, window, cx| {
                    focus.focus(window, cx);
                    // The shown choice is already chosen.
                    if view.value(setting).as_ref().and_then(Value::as_str)
                        != Some(picked.as_ref())
                    {
                        let value = Value::String(picked.to_string());
                        view.write(setting, Some(value), cx);
                    }
                },
            ))
            .into_any_element()
    }

    fn render_select(
        &mut self,
        setting: &'static Setting,
        choices: &'static [Choice],
        read_only: bool,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let label = self
            .value(setting)
            .as_ref()
            .and_then(Value::as_str)
            .map_or_else(
                || "Default".to_owned(),
                // A value the menu does not offer, such as a display's
                // `id:` identifier, shows as itself.
                |value| {
                    choices
                        .iter()
                        .find(|choice| choice.value == value)
                        .map_or_else(|| value.to_owned(), option_label)
                },
            );
        self.select(
            format!("settings-select-{path}"),
            path,
            setting.label.clone(),
            label,
            Opener::Setting(setting),
            read_only,
            px(150.0),
            swatch,
            cx,
        )
    }

    /// A select trigger for the menu `key`, showing `value` and named
    /// `label`, at least `min_width` wide, that opens through `opener`. It
    /// is a `huterm-ui` select: pointer presses toggle it, keys reach it
    /// only as `SettingsControl` commands, and its ring shows for keyboard
    /// focus alone.
    #[expect(
        clippy::too_many_arguments,
        reason = "a trigger needs its names, value, opener, and state"
    )]
    pub(super) fn select(
        &mut self,
        id: String,
        key: &str,
        label: String,
        value: String,
        opener: Opener,
        read_only: bool,
        min_width: gpui::Pixels,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let focus = match &opener {
            Opener::Setting(setting) => self.stop(setting.path.as_str(), cx),
            Opener::Action(key) => self.action_stop(key.clone(), cx),
        };
        let open = self
            .menu
            .as_ref()
            .is_some_and(|(_, target, _)| target.key() == key);
        let weak = cx.entity().downgrade();
        let opening = focus.clone();
        let select = Select::new(id, label, value, &focus, swatch)
            .key_context("SettingsControl")
            .min_width(min_width)
            .open(open)
            .on_toggle(move |window, cx| {
                let _ = weak.update(cx, |view, cx| {
                    opening.focus(window, cx);
                    match &opener {
                        Opener::Setting(setting) => {
                            view.open_select(setting, window, cx);
                        }
                        Opener::Action(key) => view.act(key, window, cx),
                    }
                });
            });
        let select = if read_only {
            select.disabled(None)
        } else {
            select
        };
        self.with_menu(key, select.into_any_element())
    }

    /// `control`, a select trigger for the menu `key`, recording its painted
    /// bounds so the menu anchors to it. While `key`'s menu is open the
    /// popover's guard also knows the trigger, so a press on it closes the
    /// menu rather than opening another. The menu itself draws in the
    /// window's popover layer.
    pub(super) fn with_menu(
        &self,
        key: &str,
        control: AnyElement,
    ) -> AnyElement {
        let open = self
            .menu
            .as_ref()
            .is_some_and(|(_, target, _)| target.key() == key);
        let triggers = self.triggers.clone();
        let recorded = key.to_owned();
        div()
            .relative()
            .flex_none()
            .child(control)
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), _, _| {
                        triggers.borrow_mut().insert(recorded.clone(), bounds);
                    },
                )
                .absolute()
                .inset_0(),
            )
            .when(open, |trigger| trigger.child(self.popover.trigger_probe()))
            .into_any_element()
    }

    /// A setting's text or number field: a `huterm-ui` input, or a number
    /// field with its unit and steppers. `read_only` disables it with that
    /// reason, so a press takes no focus and typing does nothing.
    pub(super) fn render_field(
        &mut self,
        setting: &'static Setting,
        read_only: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let field = self.field(setting, window, cx);
        let invalid = self.errors.contains_key(path);
        field.update(cx, |field, cx| {
            field.set_disabled(read_only.is_some(), read_only, cx);
            field.set_invalid(invalid, cx);
        });
        let (width, control) = match self.numbers.get(path) {
            Some(number) => (150.0, number.clone().into_any_element()),
            None => (
                if special(path) == Some(Special::Size) {
                    120.0
                } else {
                    180.0
                },
                field.into_any_element(),
            ),
        };
        div()
            .flex_none()
            .w(px(width))
            .key_context("Palette")
            .child(control)
            .into_any_element()
    }

    /// A family name field with a menu of the installed families.
    fn render_font(
        &mut self,
        setting: &'static Setting,
        read_only: Option<SharedString>,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let locked = read_only.is_some();
        let field = self.render_field(setting, read_only, window, cx);
        let pick = self.select(
            format!("settings-fonts-{path}"),
            path,
            "Installed fonts".to_owned(),
            "Installed".to_owned(),
            Opener::Action(format!("fonts:{path}")),
            locked,
            px(0.0),
            swatch,
            cx,
        );
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .child(field)
            .child(pick)
            .into_any_element()
    }

    /// A slider for a setting whose schema asks for one, with its value as
    /// a readout. A value off the grid shows as is; the arrows, as
    /// `settings_increase` and `settings_decrease`, step it along the grid,
    /// Page Up and Page Down page it, and Home and End move it to its ends.
    /// Its `control = slider` context lets those last four bind on sliders
    /// alone.
    fn render_slider(
        &mut self,
        setting: &'static Setting,
        read_only: Option<SharedString>,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let Some(steps) = values::steps(setting) else {
            return div().into_any_element();
        };
        let path = setting.path.as_str();
        // A pending reset holds no value; it shows the default it restores.
        let value = self
            .value(setting)
            .or_else(|| self.default_value(setting))
            .as_ref()
            .and_then(as_number)
            .unwrap_or_else(|| steps.min());
        let handle = self.stop(path, cx);
        let focus = handle.clone();
        let slider = Slider::new(
            id(format!("settings-slider-{path}")),
            setting.label.clone(),
            value,
            steps,
            &handle,
            swatch,
        )
        .readout(readout(setting, value))
        .key_context("SettingsControl control=slider")
        .when_some(read_only, |slider, reason| slider.disabled(Some(reason)))
        .on_change(Self::listen(
            cx,
            move |view, value: f64, window, cx| {
                focus.focus(window, cx);
                let value = number_value(setting, value);
                if !view.holds(setting, &value) {
                    view.write(setting, Some(value), cx);
                }
            },
        ));
        // A fixed width, as the row's other controls have, leaves the
        // label and description the row's flexible width.
        div()
            .flex_none()
            .w(px(SLIDER_WIDTH))
            .debug_selector(move || format!("settings-slider-{path}"))
            .child(slider)
            .into_any_element()
    }

    /// The link modifiers as a multiple toggle group: one roving tab stop,
    /// whose arrows `SettingsView::run` routes between the chips. The chip
    /// that would leave no modifier refuses, as a link needs one, and says
    /// so; it stays focusable, as the roving stop may rest on it.
    fn render_modifiers(
        &mut self,
        setting: &'static Setting,
        read_only: Option<SharedString>,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let held = match self.value(setting) {
            Some(Value::String(text)) => text,
            _ if cfg!(target_os = "macos") => "cmd".to_owned(),
            _ => "ctrl".to_owned(),
        };
        let mut group = ToggleGroup::multiple(
            id(format!("settings-modifiers-{path}")),
            setting.label.clone(),
            swatch,
        )
        .key_context("SettingsControl");
        for modifier in offered_modifiers() {
            let on = held.split('-').any(|token| token == *modifier);
            let handle =
                self.action_stop(format!("chip:{path}:{modifier}"), cx);
            group =
                group.toggle(*modifier, modifier_label(modifier), on, &handle);
            // A read-only group's own reason speaks for every chip.
            let refuses =
                toggled_modifiers(Some(Value::String(held.clone())), modifier)
                    .is_none();
            if refuses && read_only.is_none() {
                group = group.description("Keep at least one modifier");
            }
        }
        group
            .when_some(read_only, |group, reason| group.disabled(Some(reason)))
            .on_select(Self::listen(
                cx,
                move |view, modifier: SharedString, window, cx| {
                    let key = format!("chip:{path}:{modifier}");
                    if let Some(handle) = view.actions.get(&key) {
                        handle.clone().focus(window, cx);
                    }
                    view.act(&key, window, cx);
                },
            ))
            .into_any_element()
    }

    /// Cards for every theme `[theme] name` can select, and a note on the
    /// color overrides the file sets on top.
    fn render_themes(
        &mut self,
        setting: &'static Setting,
        read_only: bool,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let selected = self.selected_theme();
        let mut cards = div().flex().flex_wrap().gap(px(8.0));
        let themes = self.themes.clone();
        for (name, preview) in &themes {
            let on = *name == selected;
            let key = format!("theme:{name}");
            let handle = self.action_stop(key.clone(), cx).tab_stop(!read_only);
            cards = cards.child(
                theme_card(name, preview.as_ref(), on, read_only, swatch)
                    .track_focus(&handle)
                    .key_context("SettingsControl")
                    .focus(|style| style.shadow(swatch.focus_ring()))
                    .when(!read_only && !on, |card| {
                        card.on_click(cx.listener(
                            move |view, _, window, cx| {
                                handle.focus(window, cx);
                                view.act(&key, window, cx);
                            },
                        ))
                    }),
            );
        }
        let _ = setting;
        let overrides: Vec<String> = self
            .file
            .get("theme")
            .and_then(Value::as_table)
            .map(|table| {
                table.keys().filter(|key| *key != "name").cloned().collect()
            })
            .unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .w_full()
            .when(read_only, |block| block.opacity(0.5))
            .when(!overrides.is_empty(), |block| {
                block.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(swatch.warning_text())
                        .child(format!(
                            "Your [theme] sets {}, which applies on top of any theme.",
                            overrides.join(", ")
                        )),
                )
            })
            .child(cards)
            .into_any_element()
    }
}

/// A theme's card: a strip of its colors over its background, and its
/// name.
fn theme_card(
    name: &str,
    preview: Option<&Theme>,
    on: bool,
    read_only: bool,
    swatch: Swatch,
) -> gpui::Stateful<gpui::Div> {
    let bar = |width: f32, rgb| {
        div().w(px(width)).h(px(4.0)).rounded_full().bg(color(rgb))
    };
    div()
        .id(id(format!("settings-theme-{name}")))
        .w(px(116.0))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .p(px(4.0))
        .rounded(px(7.0))
        .border_1()
        .border_color(if on { swatch.accent } else { swatch.line })
        .when(!on && !read_only, |card| {
            card.hover(|style| style.border_color(swatch.line_strong))
        })
        .children(preview.map(|theme| {
            div()
                .h(px(40.0))
                .rounded(px(4.0))
                .p(px(6.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .bg(color(theme.background))
                .child(bar(50.0, theme.ansi[2]))
                .child(bar(80.0, theme.foreground))
                .child(bar(36.0, theme.ansi[4]))
        }))
        .child(
            div()
                .px(px(2.0))
                .text_size(px(11.5))
                .text_color(swatch.fg)
                .truncate()
                .child(name.to_owned()),
        )
}

/// A slider's readout: the value with its unit, or without one a percent,
/// as the Quake window's fractions read.
fn readout(setting: &Setting, value: f64) -> String {
    if let Some(unit) = &setting.unit {
        return format!("{} {unit}", number(value));
    }
    let percent = Format {
        scale: 100.0,
        decimals: 1,
    };
    format!("{}%", percent.text(value))
}

/// Whether a choice fits a segmented control.
fn segmented(choices: &[Choice]) -> bool {
    choices.len() <= SEGMENTED_MAX
        && choices.iter().all(|choice| {
            option_label(choice).chars().count() <= SEGMENT_LABEL_MAX
        })
}
