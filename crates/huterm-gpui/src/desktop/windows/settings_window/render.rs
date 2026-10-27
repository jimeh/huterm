//! The Settings window's layout: the sidebar, the banner, and the pane of
//! grouped rows.

use gpui::{AnyElement, Context, Div, ElementId, Window, div, prelude::*, px};
use huterm_config::settings::{
    Platform as SettingPlatform, SectionId, Setting,
};

use super::super::super::overlay::{Swatch, mono_font_family};
use super::super::super::settings::status::Action;
use super::super::super::settings::{Special, special};
use super::{
    SIDEBAR_WIDTH, SettingsView, apply_note, button, current, link, tone_color,
};

fn id(name: String) -> ElementId {
    ElementId::Name(name.into())
}

impl SettingsView {
    pub(super) fn render_sidebar(
        &mut self,
        swatch: Swatch,
        searching: bool,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let nav = self.render_nav(swatch, searching, cx);
        let search = self.search.clone();
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
                div().p(px(10.0)).child(
                    div()
                        .h(px(28.0))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(swatch.line_strong)
                        .bg(swatch.bg)
                        .text_size(px(12.5))
                        .key_context("Palette")
                        .child(search),
                ),
            )
            .child(div().flex_1().child(nav))
            .child(self.render_footer(swatch))
            .into_any_element()
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
            let count = self.model.sections()[section as usize]
                .groups
                .iter()
                .flat_map(|group| &group.settings)
                .filter(|setting| self.file.is_set(&setting.path))
                .count();
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
                        row.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(swatch.muted)
                                .child(count.to_string()),
                        )
                    }),
            );
        }
        nav
    }

    /// The file status with its action, and the link that opens the file.
    fn render_footer(&self, swatch: Swatch) -> Div {
        let line = self.status.line();
        let action = line.action.map(|action| match action {
            Action::Reload => link("settings-footer-reload", swatch)
                .child("Reload")
                .on_click(|_, _, cx| {
                    let _ = super::super::reload(cx);
                }),
            Action::Open => link("settings-footer-open-error", swatch)
                .child("Open")
                .on_click(|_, _, cx| super::super::open_config_file(cx)),
        });
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
            .child(
                link("settings-footer-open", swatch)
                    .child("Open config.toml")
                    .on_click(|_, _, cx| super::super::open_config_file(cx)),
            )
    }

    fn render_banner(&self, swatch: Swatch) -> Option<AnyElement> {
        let message = self.status.invalid_message()?;
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
                .child(
                    div().flex().child(
                        button("settings-banner-open", swatch)
                            .child("Open config.toml")
                            .on_click(|_, _, cx| {
                                super::super::open_config_file(cx);
                            }),
                    ),
                )
                .into_any_element(),
        )
    }

    fn render_row(
        &mut self,
        setting: &'static Setting,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let foreign = setting
            .platform
            .is_some_and(|platform| platform != current());
        let read_only = foreign || self.status.invalid();
        let label = self.render_label(setting, foreign, swatch, cx);
        let control =
            self.render_control(setting, read_only, swatch, window, cx);
        let reset_stop = self.reset_stop(path, cx);
        let reset = (self.file.is_set(path) && !read_only).then(|| {
            div()
                .id(id(format!("settings-reset-{path}")))
                .track_focus(&reset_stop)
                .key_context("SettingsControl")
                .focus(|style| style.shadow(swatch.focus_ring()))
                .flex_none()
                .px(px(6.0))
                .h(px(22.0))
                .flex()
                .items_center()
                .rounded(px(5.0))
                .text_size(px(11.5))
                .text_color(swatch.muted)
                .hover(|style| style.bg(swatch.hover))
                .child("Reset")
                .on_click(
                    cx.listener(move |view, _, _, cx| view.reset(setting, cx)),
                )
        });
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
    fn render_file_card(&self, swatch: Swatch) -> AnyElement {
        let path = self.path.clone();
        let reveal = if cfg!(target_os = "macos") {
            "Show in Finder"
        } else {
            "Show in Folder"
        };
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
            .child(
                button("settings-file-open", swatch)
                    .child("Open")
                    .on_click(|_, _, cx| super::super::open_config_file(cx)),
            )
            .child(
                button("settings-file-reveal", swatch)
                    .child(reveal)
                    .on_click(move |_, _, cx| cx.reveal_path(&path)),
            )
            .into_any_element()
    }

    /// A row's label: the setting's name with a dot when the file sets it,
    /// then its description, apply note, and any error or conflict.
    fn render_label(
        &self,
        setting: &'static Setting,
        foreign: bool,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let path = setting.path.as_str();
        let small = |text: String, color| {
            div().text_size(px(11.5)).text_color(color).child(text)
        };
        let description = if foreign {
            let platform = match setting.platform {
                Some(SettingPlatform::Macos) => "macOS",
                _ => "Linux",
            };
            Some(format!(
                "{platform} only. Set in config.toml and ignored here."
            ))
        } else {
            setting.description.clone()
        };
        let message = if let Some(conflict) = self.conflicts.get(path) {
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(small(conflict.message.clone(), swatch.warning))
                    .child(
                        link(format!("settings-mine-{path}"), swatch)
                            .child("Use mine")
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.keep_mine(path, cx);
                            })),
                    )
                    .child(
                        link(format!("settings-file-{path}"), swatch)
                            .child("Use file")
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.keep_file(path, cx);
                            })),
                    ),
            )
        } else {
            self.errors
                .get(path)
                .map(|error| small(error.clone(), swatch.danger))
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.0))
            .child(self.render_title(setting, swatch))
            .children(description.map(|text| small(text, swatch.muted)))
            .children(
                apply_note(setting.apply)
                    .filter(|_| !foreign)
                    .map(|note| small(note.to_owned(), swatch.dim)),
            )
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
                line.child(
                    div()
                        .px(px(5.0))
                        .rounded(px(4.0))
                        .bg(swatch.chip)
                        .text_size(px(10.5))
                        .text_color(swatch.muted)
                        .child("Advanced"),
                )
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
        if let Some(banner) = self.render_banner(swatch) {
            body = body.child(banner);
        }
        if searching {
            let mut matched = false;
            for section in self.visible_sections() {
                let groups =
                    self.render_groups(section, query, swatch, window, cx);
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
                        .then(|| self.render_file_card(swatch)),
                )
                .children(self.render_groups(section, "", swatch, window, cx))
                .children(self.render_advanced(section, swatch, cx));
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
        &self,
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
            div()
                .id("settings-advanced")
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(12.0))
                .text_color(swatch.muted)
                .hover(|style| style.text_color(swatch.fg))
                .child(if on {
                    "Hide advanced settings"
                } else {
                    "Show advanced settings"
                })
                .on_click(cx.listener(move |view, _, _, cx| {
                    if !view.advanced.remove(&section) {
                        view.advanced.insert(section);
                    }
                    cx.notify();
                }))
                .into_any_element(),
        )
    }
}
