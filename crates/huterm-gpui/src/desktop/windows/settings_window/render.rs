//! The Settings window's layout: the sidebar, the banner, and the pane of
//! grouped rows.

use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, SharedString, Stateful,
    Window, div, prelude::*, px, svg,
};
use huterm_config::settings::{SectionId, Setting};
use huterm_ui::components::badge::{Tone as BadgeTone, badge};
use huterm_ui::components::button::{Button, Variant};
use huterm_ui::components::input::Input;
use huterm_ui::components::input_group::{Addon, InputGroup};

use super::super::super::overlay::{Swatch, mono_font_family};
use super::super::super::settings::status::Action;
use super::super::super::settings::{Special, special};
use super::super::Desktop;
use super::{
    SIDEBAR_WIDTH, SettingsView, apply_note, foreign_note, tone_color,
};
use crate::assets::Icon;

fn id(name: String) -> ElementId {
    ElementId::Name(name.into())
}

impl SettingsView {
    /// A `huterm-ui` button reading `label` that runs the action `key`
    /// names, on a pointer click or on `settings_toggle` while it has focus.
    /// Its ring shows for keyboard focus alone.
    pub(super) fn action_button(
        &mut self,
        id: String,
        label: impl Into<SharedString>,
        key: String,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Button {
        let handle = self.action_stop(key.clone(), cx);
        let focus = handle.clone();
        let act = Self::listen(cx, move |view, (), window, cx| {
            focus.focus(window, cx);
            view.act(&key, window, cx);
        });
        Button::new(ElementId::Name(id.into()), label, swatch)
            .focus(&handle)
            .key_context("SettingsControl")
            .on_click(move |window, cx| act((), window, cx))
    }

    /// A component callback that runs `handler` on the view with the value
    /// the component reports, as `cx.listener` does for element events.
    pub(super) fn listen<T: 'static>(
        cx: &Context<'_, Self>,
        handler: impl Fn(&mut Self, T, &mut Window, &mut Context<'_, Self>)
        + 'static,
    ) -> impl Fn(T, &mut Window, &mut App) + 'static {
        let weak = cx.entity().downgrade();
        move |value, window, cx| {
            let _ =
                weak.update(cx, |view, cx| handler(view, value, window, cx));
        }
    }

    /// Makes `element` a keyboard stop that runs the action `key` names,
    /// by click or by `settings_toggle`.
    pub(super) fn actionable(
        &mut self,
        element: Stateful<Div>,
        key: String,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Stateful<Div> {
        let handle = self.action_stop(key.clone(), cx);
        element
            .track_focus(&handle)
            .key_context("SettingsControl")
            .focus(|style| style.shadow(swatch.focus_ring()))
            .on_click(cx.listener(move |view, _, window, cx| {
                handle.focus(window, cx);
                view.act(&key, window, cx);
            }))
    }

    pub(super) fn render_sidebar(
        &mut self,
        swatch: Swatch,
        searching: bool,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let nav = self.render_nav(swatch, searching, cx);
        let search = Self::search_group(
            "settings-search-group",
            &self.search,
            swatch,
            cx,
        );
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .border_r_1()
            .border_color(swatch.line)
            .bg(swatch.surface)
            .child(
                div()
                    .p(px(10.0))
                    .child(div().key_context("Palette").child(search)),
            )
            .child(div().flex_1().child(nav))
            .child(self.render_footer(swatch, cx))
            .into_any_element()
    }

    /// A search field: `input` after a search icon, with a button that
    /// clears it while it has text.
    pub(super) fn search_group(
        id: &'static str,
        input: &Entity<Input>,
        swatch: Swatch,
        cx: &App,
    ) -> InputGroup {
        let icon = |icon: Icon| {
            svg()
                .path(icon.asset_path())
                .size(px(14.0))
                .text_color(swatch.muted)
        };
        let clear = (!input.read(cx).is_empty()).then(|| {
            let input = input.downgrade();
            Button::new(
                ElementId::Name(format!("{id}-clear").into()),
                "Clear search",
                swatch,
            )
            .variant(Variant::Ghost)
            .icon(icon(Icon::X))
            .icon_only()
            .on_click(move |_, cx| {
                let _ = input.update(cx, |input, cx| input.set_text("", cx));
            })
        });
        InputGroup::new(id, input, swatch)
            .leading(Addon::Icon(icon(Icon::Search).into_any_element()))
            .when_some(clear, |group, clear| {
                group.trailing(Addon::Button(clear))
            })
    }

    fn render_nav(
        &mut self,
        swatch: Swatch,
        searching: bool,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let mut nav = div().flex().flex_col().gap(px(1.0)).px(px(8.0));
        for section in self.visible_sections() {
            let handle = self.section_stop(section, cx);
            // A list counts its entries, as each is something the file sets.
            let count: usize = self.model.sections()[section as usize]
                .groups
                .iter()
                .flat_map(|group| &group.settings)
                .map(|setting| match self.file.get(&setting.path) {
                    Some(toml::Value::Array(entries)) => entries.len(),
                    // Profiles count the values they set.
                    Some(_) if setting.path == "quake" => self.profile_values(),
                    Some(_) => 1,
                    None => 0,
                })
                .sum();
            let on = section == self.section && !searching;
            nav = nav.child(
                div()
                    .id(id(format!("settings-section-{}", section.id())))
                    .track_focus(&handle)
                    .key_context("SettingsControl")
                    .flex()
                    .items_center()
                    .h(px(28.0))
                    .px(px(8.0))
                    .rounded(px(6.0))
                    .text_size(px(13.0))
                    .text_color(swatch.fg)
                    .when(on, |row| row.bg(swatch.selection))
                    .when(!on, |row| row.hover(|style| style.bg(swatch.hover)))
                    .focus(|style| style.shadow(swatch.focus_ring()))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.select_section(section, cx);
                    }))
                    .child(div().flex_1().child(section.title()))
                    .when(count > 0, |row| {
                        // The row hovers and takes the selection fill, so
                        // the count draws no chip fill over them.
                        row.child(
                            badge(
                                id(format!("settings-count-{}", section.id())),
                                count.to_string(),
                                BadgeTone::Neutral,
                                swatch,
                            )
                            .bg(gpui::transparent_black())
                            .text_color(if on {
                                swatch.muted_on(swatch.selection)
                            } else {
                                swatch.muted
                            }),
                        )
                    }),
            );
        }
        nav
    }

    /// The file status with its action, and the link that opens the file.
    fn render_footer(
        &mut self,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let line = self.status.line();
        let action = line.action.map(|action| {
            let (id, label, key) = match action {
                Action::Reload => {
                    ("settings-footer-reload", "Reload", "reload:footer")
                }
                Action::Open => {
                    ("settings-footer-open-error", "Open", "open:footer-error")
                }
            };
            self.action_button(id.to_owned(), label, key.to_owned(), swatch, cx)
                .variant(Variant::Link)
                .button_role()
        });
        let open = self
            .action_button(
                "settings-footer-open".to_owned(),
                "Open config.toml",
                "open:footer".to_owned(),
                swatch,
                cx,
            )
            .variant(Variant::Link)
            .button_role();
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .p(px(10.0))
            .border_t_1()
            .border_color(swatch.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(11.5))
                    .text_color(swatch.muted)
                    .child(
                        div()
                            .flex_none()
                            .size(px(7.0))
                            .rounded_full()
                            .bg(tone_color(line.tone, swatch)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(line.text))
                    .children(action),
            )
            .child(open)
    }

    fn render_banner(
        &mut self,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let message = self.status.invalid_message()?.to_owned();
        let open = self.action_button(
            "settings-banner-open".to_owned(),
            "Open config.toml",
            "open:banner".to_owned(),
            swatch,
            cx,
        );
        // With watching off, a fixed file applies only on a reload.
        let watching = cx.global::<Desktop>().config.general.watch_config;
        let reload = (!watching).then(|| {
            self.action_button(
                "settings-banner-reload".to_owned(),
                "Reload",
                "reload:banner".to_owned(),
                swatch,
                cx,
            )
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .mb(px(14.0))
                .p(px(12.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(swatch.danger)
                .bg(swatch.danger.opacity(0.08))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(swatch.fg)
                        .child("config.toml has an error"),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(swatch.muted)
                        .child(
                            "Huterm keeps running with the last valid settings, and Settings is read-only until the file is fixed.",
                        ),
                )
                .child(
                    div()
                        .p(px(8.0))
                        .rounded(px(5.0))
                        .bg(swatch.bg)
                        .font_family(mono_font_family())
                        .text_size(px(11.5))
                        .text_color(swatch.fg)
                        .child(message.trim_end().to_owned()),
                )
                .child(div().flex().gap(px(6.0)).child(open).children(reload))
                .into_any_element(),
        )
    }

    pub(super) fn render_row(
        &mut self,
        setting: &'static Setting,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let read_only = self.read_only_reason(setting);
        let label = self.render_label(setting, swatch, cx);
        // Each row's reset is named for its setting, so the buttons read
        // differently.
        let reset =
            (self.resettable(setting) && read_only.is_none()).then(|| {
                self.action_button(
                    format!("settings-reset-{path}"),
                    format!("Reset {}", setting.label),
                    format!("reset:{path}"),
                    swatch,
                    cx,
                )
                .variant(Variant::Ghost)
                .tooltip("Reset to default")
                .icon(
                    svg()
                        .path(Icon::RotateCcw.asset_path())
                        .size(px(12.0))
                        .text_color(swatch.muted),
                )
                .icon_only()
            });
        let reset = reset.map(|reset| {
            div()
                .flex_none()
                .debug_selector(move || format!("settings-reset-{path}"))
                .child(reset)
        });
        let control =
            self.render_control(setting, read_only, swatch, window, cx);
        // The theme picker's cards need the row's width.
        let wide = special(path) == Some(Special::ThemePicker);
        div()
            .flex()
            .when(wide, |row| row.flex_col().items_start())
            .when(!wide, gpui::Styled::items_center)
            .gap(px(12.0))
            .px(px(14.0))
            .py(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .w_full()
                    .min_w_0()
                    .when(!wide, gpui::Styled::flex_1)
                    .child(label)
                    .children(reset),
            )
            .child(control)
            .into_any_element()
    }

    /// The file's path, with buttons that open it and show it in the file
    /// manager.
    fn render_file_card(
        &mut self,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = self.path.clone();
        let open = self.action_button(
            "settings-file-open".to_owned(),
            "Open",
            "open:card".to_owned(),
            swatch,
            cx,
        );
        let reveal = self.action_button(
            "settings-file-reveal".to_owned(),
            if cfg!(target_os = "macos") {
                "Show in Finder"
            } else {
                "Show in Folder"
            },
            "reveal:card".to_owned(),
            swatch,
            cx,
        );
        div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .mb(px(12.0))
            .px(px(14.0))
            .py(px(10.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(swatch.line)
            .bg(swatch.surface)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(swatch.fg)
                            .font_family(mono_font_family())
                            .truncate()
                            .child(path.display().to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(swatch.muted)
                            .child(
                                "Settings changes only the lines a setting owns; comments, order, and formatting stay as you wrote them.",
                            ),
                    ),
            )
            .child(open)
            .child(reveal)
            .into_any_element()
    }

    /// A row's label: the setting's name with a dot when the file sets it,
    /// then its description, apply note, and any error or conflict.
    fn render_label(
        &mut self,
        setting: &'static Setting,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let path = setting.path.as_str();
        let small = |text: String, color| {
            div().text_size(px(11.5)).text_color(color).child(text)
        };
        let foreign = foreign_note(setting);
        let note = apply_note(setting.apply).filter(|_| foreign.is_none());
        let description = foreign.or_else(|| setting.description.clone());
        let message = if let Some(text) = self
            .conflicts
            .get(path)
            .map(|conflict| conflict.message.clone())
        {
            let mine = self
                .action_button(
                    format!("settings-mine-{path}"),
                    "Use mine",
                    format!("mine:{path}"),
                    swatch,
                    cx,
                )
                .variant(Variant::Link)
                .button_role();
            let file = self
                .action_button(
                    format!("settings-file-{path}"),
                    "Use file",
                    format!("file:{path}"),
                    swatch,
                    cx,
                )
                .variant(Variant::Link)
                .button_role();
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(small(text, swatch.warning_text()))
                    // An invalid file cannot take either choice yet.
                    .when(!self.locked(), |row| row.child(mine).child(file)),
            )
        } else {
            self.errors
                .get(path)
                .map(|error| small(error.clone(), swatch.danger_text()))
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.0))
            .debug_selector(move || format!("settings-label-{path}"))
            .child(self.render_title(setting, swatch))
            .children(description.map(|text| small(text, swatch.muted)))
            .children(note.map(|note| small(note.to_owned(), swatch.muted)))
            .children(message)
    }

    fn render_title(&self, setting: &Setting, swatch: Swatch) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(px(13.0))
            .text_color(swatch.fg)
            .when(self.file.is_set(&setting.path), |line| {
                line.child(
                    div()
                        .flex_none()
                        .size(px(6.0))
                        .rounded_full()
                        .bg(swatch.accent),
                )
            })
            .child(setting.label.clone())
            .when(setting.advanced, |line| {
                line.child(badge(
                    id(format!("settings-advanced-{}", setting.path)),
                    "Advanced",
                    BadgeTone::Neutral,
                    swatch,
                ))
            })
    }

    pub(super) fn render_main(
        &mut self,
        swatch: Swatch,
        query: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let searching = !query.is_empty();
        let mut body =
            div().flex().flex_col().w_full().gap(px(6.0)).p(px(24.0));
        if let Some(banner) = self.render_banner(swatch, cx) {
            body = body.child(banner);
        }
        if searching {
            let mut matched = false;
            for section in self.visible_sections() {
                // Keybindings lists its matching commands instead.
                let groups = if section == SectionId::Keybindings {
                    self.render_key_results(query, swatch, cx)
                        .into_iter()
                        .collect()
                } else {
                    self.render_groups(section, query, swatch, window, cx)
                };
                if groups.is_empty() {
                    continue;
                }
                matched = true;
                body = body
                    .child(
                        div()
                            .text_size(px(14.0))
                            .text_color(swatch.fg)
                            .my(px(4.0))
                            .child(section.title()),
                    )
                    .children(groups);
            }
            if !matched {
                body = body.child(
                    div()
                        .text_size(px(13.0))
                        .text_color(swatch.muted)
                        .child("No settings match."),
                );
            }
        } else {
            let section = self.section;
            body = body
                .child(
                    div()
                        .text_size(px(20.0))
                        .text_color(swatch.fg)
                        .mb(px(8.0))
                        .child(section.title()),
                )
                .children(
                    (section == SectionId::General)
                        .then(|| self.render_file_card(swatch, cx)),
                );
            body = if section == SectionId::Keybindings {
                body.children(self.render_keybindings(swatch, window, cx))
            } else if section == SectionId::Quake {
                body.children(self.render_quake(swatch, window, cx))
            } else {
                body.children(
                    self.render_groups(section, "", swatch, window, cx),
                )
                .children(self.render_advanced(section, swatch, cx))
            };
        }
        div()
            .id("settings-main")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(body)
            .into_any_element()
    }

    /// Each group of `section` with a row that shows and matches `query`,
    /// as a titled card.
    fn render_groups(
        &mut self,
        section: SectionId,
        query: &str,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Vec<AnyElement> {
        let searching = !query.is_empty();
        let mut cards = Vec::new();
        for group in &self.model.sections()[section as usize].groups {
            let settings: Vec<&'static Setting> = group
                .settings
                .iter()
                .filter(|setting| {
                    self.shows(setting, searching)
                        && Self::matches(setting, query)
                })
                .collect();
            if settings.is_empty() {
                continue;
            }
            let mut card = div()
                .flex()
                .flex_col()
                .rounded(px(8.0))
                .border_1()
                .border_color(swatch.line)
                .bg(swatch.surface);
            for (index, setting) in settings.into_iter().enumerate() {
                let row = self.render_row(setting, swatch, window, cx);
                card = card.child(
                    div()
                        .when(index > 0, |row| {
                            row.border_t_1().border_color(swatch.line)
                        })
                        .child(row),
                );
            }
            cards.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .mb(px(12.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(swatch.muted)
                            .child(group.title.to_uppercase()),
                    )
                    .child(card)
                    .into_any_element(),
            );
        }
        cards
    }

    /// The switch that shows a section's advanced settings, when it has any
    /// available here.
    fn render_advanced(
        &mut self,
        section: SectionId,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let has_advanced = self.model.sections()[section as usize]
            .groups
            .iter()
            .flat_map(|group| &group.settings)
            .any(|setting| setting.advanced && self.available(setting));
        if !has_advanced {
            return None;
        }
        let on = self.advanced.contains(&section);
        Some(
            self.action_button(
                "settings-advanced".to_owned(),
                if on {
                    "Hide advanced settings"
                } else {
                    "Show advanced settings"
                },
                format!("advanced:{}", section.id()),
                swatch,
                cx,
            )
            .variant(Variant::Ghost)
            .into_any_element(),
        )
    }
}
