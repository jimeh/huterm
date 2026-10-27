//! The Settings window's controls: one per setting kind, and the special
//! controls the override table names.

use gpui::{
    AnyElement, Context, ElementId, Focusable as _, Window, anchored, deferred,
    div, prelude::*, px,
};
use huterm_config::settings::{Choice, Setting, SettingKind};
use toml::Value;

use super::super::super::overlay::Swatch;
use super::super::super::settings::{Special, special};
use super::{
    SEGMENTED_MAX, SettingsView, button, modifier_label, option_label,
};
use crate::config::Theme;
use crate::renderer::rgb_color as color;

/// The longest option label a segmented control shows; longer ones use a
/// select.
const SEGMENT_LABEL_MAX: usize = 16;

fn id(name: String) -> ElementId {
    ElementId::Name(name.into())
}

impl SettingsView {
    pub(super) fn render_control(
        &mut self,
        setting: &'static Setting,
        read_only: bool,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        match special(&setting.path) {
            Some(Special::ThemePicker) => {
                return self.render_themes(setting, read_only, swatch, cx);
            }
            Some(
                Special::Keybindings
                | Special::GlobalShortcuts
                | Special::QuakeProfiles,
            ) => {
                let element =
                    button(format!("settings-open-{}", setting.path), swatch)
                        .child("Open config.toml");
                return self
                    .actionable(
                        element,
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
                return self
                    .render_field(setting, read_only, swatch, window, cx);
            }
            None => {}
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
                self.render_select(setting, choices, read_only, swatch, cx)
            }
            SettingKind::Integer { .. }
            | SettingKind::Number { .. }
            | SettingKind::Text { .. } => {
                self.render_field(setting, read_only, swatch, window, cx)
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
        read_only: bool,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let on = self
            .value(setting)
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let handle = self.stop(path, cx).tab_stop(!read_only);
        div()
            .id(id(format!("settings-switch-{path}")))
            .track_focus(&handle)
            .key_context("SettingsControl")
            .flex_none()
            .w(px(34.0))
            .h(px(20.0))
            .p(px(2.0))
            .rounded_full()
            .bg(if on {
                swatch.accent
            } else {
                swatch.line_strong
            })
            .focus(|style| style.shadow(swatch.focus_ring()))
            .when(read_only, |control| control.opacity(0.5))
            .child(
                div()
                    .size(px(16.0))
                    .rounded_full()
                    .bg(swatch.bg)
                    .when(on, |knob| knob.ml(px(14.0))),
            )
            .when(!read_only, |control| {
                control.on_click(cx.listener(move |view, _, window, cx| {
                    view.stop(path, cx).focus(window, cx);
                    view.write(setting, Some(Value::Boolean(!on)), cx);
                }))
            })
            .into_any_element()
    }

    /// An optional switch, such as automatic update checks, has a third
    /// state that leaves the choice to its owner.
    fn render_optional_bool(
        &mut self,
        setting: &'static Setting,
        read_only: bool,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let current = self.value(setting).and_then(|value| value.as_bool());
        let handle = self.stop(path, cx).tab_stop(!read_only);
        let mut group =
            segment_group(format!("settings-tristate-{path}"), swatch)
                .track_focus(&handle)
                .key_context("SettingsControl");
        for (label, state) in
            [("Default", None), ("On", Some(true)), ("Off", Some(false))]
        {
            let on = current == state;
            group = group.child(
                segment(
                    format!("settings-{path}-{label}"),
                    on,
                    read_only,
                    swatch,
                )
                .child(label)
                .when(!read_only && !on, |segment| {
                    segment.on_click(cx.listener(move |view, _, window, cx| {
                        view.stop(path, cx).focus(window, cx);
                        view.write(setting, state.map(Value::Boolean), cx);
                    }))
                }),
            );
        }
        group
            .when(read_only, |group| group.opacity(0.5))
            .into_any_element()
    }

    fn render_segments(
        &mut self,
        setting: &'static Setting,
        choices: &'static [Choice],
        read_only: bool,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let selected = self.value(setting);
        let handle = self.stop(path, cx).tab_stop(!read_only);
        let mut group =
            segment_group(format!("settings-segments-{path}"), swatch)
                .track_focus(&handle)
                .key_context("SettingsControl");
        for choice in choices {
            let on = selected.as_ref().and_then(Value::as_str)
                == Some(choice.value.as_str());
            let value = Value::String(choice.value.clone());
            group = group.child(
                segment(
                    format!("settings-{path}-{}", choice.value),
                    on,
                    read_only,
                    swatch,
                )
                .child(option_label(choice))
                .when(!read_only && !on, |segment| {
                    segment.on_click(cx.listener(move |view, _, window, cx| {
                        view.stop(path, cx).focus(window, cx);
                        view.write(setting, Some(value.clone()), cx);
                    }))
                }),
            );
        }
        group
            .when(read_only, |group| group.opacity(0.5))
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
            .and_then(|value| {
                choices.iter().find(|choice| choice.value == value)
            })
            .map_or_else(|| "Default".to_owned(), option_label);
        let handle = self.stop(path, cx).tab_stop(!read_only);
        let trigger = select_trigger(format!("settings-select-{path}"), swatch)
            .track_focus(&handle)
            .key_context("SettingsControl")
            .when(read_only, |control| control.opacity(0.5))
            .child(div().flex_1().child(label))
            .child(caret(swatch))
            .when(!read_only, |control| {
                control.on_click(cx.listener(move |view, _, window, cx| {
                    view.stop(path, cx).focus(window, cx);
                    view.open_select(setting, window, cx);
                }))
            });
        self.with_menu(path, trigger.into_any_element())
    }

    /// `control` with the select menu open beneath it when it is `path`'s.
    fn with_menu(&self, path: &str, control: AnyElement) -> AnyElement {
        let menu = self
            .menu
            .as_ref()
            .filter(|(_, open, _)| open.path == path)
            .map(|(menu, _, _)| menu.clone());
        div()
            .relative()
            .flex_none()
            .child(control)
            .children(menu.map(|menu| {
                deferred(
                    anchored()
                        .snap_to_window_with_margin(px(8.0))
                        .child(div().mt(px(28.0)).child(menu)),
                )
                .with_priority(1)
            }))
            .into_any_element()
    }

    pub(super) fn render_field(
        &mut self,
        setting: &'static Setting,
        read_only: bool,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let field = self.field(setting, window, cx);
        let _ = field.read(cx).focus_handle(cx).tab_stop(!read_only);
        let width = match (&setting.kind, special(path)) {
            (SettingKind::Text { .. }, _) => 180.0,
            (_, Some(Special::Size)) => 120.0,
            _ => 90.0,
        };
        let invalid = self.errors.contains_key(path);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .w(px(width))
                    .h(px(24.0))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(if invalid {
                        swatch.danger
                    } else {
                        swatch.line_strong
                    })
                    .bg(swatch.bg)
                    .text_size(px(12.5))
                    .key_context("Palette")
                    .when(read_only, |control| control.opacity(0.5))
                    .child(field),
            )
            .when_some(setting.unit.clone(), |row, unit| {
                row.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(swatch.muted)
                        .child(unit),
                )
            })
            .into_any_element()
    }

    /// A family name field with a menu of the installed families.
    fn render_font(
        &mut self,
        setting: &'static Setting,
        read_only: bool,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let field = self.render_field(setting, read_only, swatch, window, cx);
        let key = format!("fonts:{path}");
        let handle = self.action_stop(key.clone(), cx).tab_stop(!read_only);
        let pick = select_trigger(format!("settings-fonts-{path}"), swatch)
            .track_focus(&handle)
            .key_context("SettingsControl")
            .min_w(px(0.0))
            .when(read_only, |control| control.opacity(0.5))
            .child("Installed")
            .child(caret(swatch))
            .when(!read_only, |control| {
                control.on_click(cx.listener(move |view, _, window, cx| {
                    handle.focus(window, cx);
                    view.act(&key, window, cx);
                }))
            });
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .child(field)
            .child(self.with_menu(path, pick.into_any_element()))
            .into_any_element()
    }

    fn render_modifiers(
        &mut self,
        setting: &'static Setting,
        read_only: bool,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let current = self.value(setting);
        let held = match &current {
            Some(Value::String(text)) => text.clone(),
            _ if cfg!(target_os = "macos") => "cmd".to_owned(),
            _ => "ctrl".to_owned(),
        };
        // macOS rejects Ctrl: Control-click opens the context menu.
        let offered: &[&str] = if cfg!(target_os = "macos") {
            &["cmd", "alt", "shift"]
        } else {
            &["ctrl", "alt", "shift"]
        };
        let mut chips = div().flex().flex_none().gap(px(4.0));
        for modifier in offered {
            let on = held.split('-').any(|token| token == *modifier);
            // A link needs at least one modifier.
            let enabled = !read_only
                && super::toggled_modifiers(current.clone(), modifier)
                    .is_some();
            let key = format!("chip:{path}:{modifier}");
            let handle = self.action_stop(key.clone(), cx).tab_stop(enabled);
            chips = chips.child(
                div()
                    .id(id(format!("settings-{path}-{modifier}")))
                    .track_focus(&handle)
                    .key_context("SettingsControl")
                    .focus(|style| style.shadow(swatch.focus_ring()))
                    .px(px(8.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(if on {
                        swatch.accent
                    } else {
                        swatch.line_strong
                    })
                    .bg(if on { swatch.selection } else { swatch.bg })
                    .text_size(px(12.0))
                    .text_color(swatch.fg)
                    .child(modifier_label(modifier))
                    .when(read_only, |chip| chip.opacity(0.5))
                    .when(enabled, |chip| {
                        chip.on_click(cx.listener(
                            move |view, _, window, cx| {
                                handle.focus(window, cx);
                                view.act(&key, window, cx);
                            },
                        ))
                    }),
            );
        }
        chips.into_any_element()
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
                        .text_color(swatch.warning)
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

/// Whether a choice fits a segmented control.
fn segmented(choices: &[Choice]) -> bool {
    choices.len() <= SEGMENTED_MAX
        && choices.iter().all(|choice| {
            option_label(choice).chars().count() <= SEGMENT_LABEL_MAX
        })
}

fn segment_group(name: String, swatch: Swatch) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id(name))
        .flex()
        .flex_none()
        .rounded(px(6.0))
        .border_1()
        .border_color(swatch.line_strong)
        .focus(|style| style.shadow(swatch.focus_ring()))
}

fn segment(
    name: String,
    on: bool,
    read_only: bool,
    swatch: Swatch,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id(name))
        .px(px(10.0))
        .h(px(22.0))
        .flex()
        .items_center()
        .text_size(px(12.0))
        .text_color(swatch.fg)
        .when(on, |segment| segment.bg(swatch.selection))
        .when(!on && !read_only, |segment| {
            segment.hover(|style| style.bg(swatch.hover))
        })
}

fn select_trigger(name: String, swatch: Swatch) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id(name))
        .flex()
        .items_center()
        .gap(px(8.0))
        .min_w(px(150.0))
        .h(px(24.0))
        .px(px(10.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(swatch.line_strong)
        .bg(swatch.bg)
        .text_size(px(12.0))
        .text_color(swatch.fg)
        .focus(|style| style.shadow(swatch.focus_ring()))
}

fn caret(swatch: Swatch) -> gpui::Div {
    div().flex_none().text_color(swatch.dim).child("▾")
}
