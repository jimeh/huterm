//! The Settings window's layout: the sidebar, the section header, the
//! banner, and the pane of grouped rows.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, FocusHandle,
    InteractiveElement, MouseButton, SharedString, Window, canvas, div,
    prelude::*, px, svg,
};
use huterm_config::settings::{SectionId, Setting};
use huterm_ui::components::alert::{Alert, Tone as AlertTone};
use huterm_ui::components::badge::{Tone as BadgeTone, badge};
use huterm_ui::components::button::{Button, Variant};
use huterm_ui::components::card::Card;
use huterm_ui::components::field::Field;
use huterm_ui::components::input::Input;
use huterm_ui::components::input_group::{Addon, InputGroup};
use huterm_ui::components::item::Item;
use huterm_ui::components::prose::Prose;
use huterm_ui::components::scroll_area::ScrollArea;
use huterm_ui::components::tabs::Tabs;
use huterm_ui::typography::ui;

use super::super::super::overlay::{Swatch, mono_font_family};
use super::super::super::settings::status::{Action, Tone};
use super::super::super::settings::{Special, special};
use super::super::Desktop;
use super::{
    HEADER_HEIGHT, SIDEBAR_WIDTH, SettingsView, apply_note, foreign_note,
    tone_color,
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

    /// The gestures of the title strip's empty space on macOS, where the
    /// sidebar and the section header replace `AppKit`'s title bar, as a
    /// terminal window's title row has: a primary drag moves the window
    /// and a double-click runs the title-bar action. Elsewhere the window
    /// manager's title bar does both.
    ///
    /// The move starts on the first drag motion after the press, not on
    /// the press itself: `AppKit`'s window drag takes over the pointer, which
    /// would swallow the release and keep a second press from counting as
    /// the double-click.
    fn title_strip<E: InteractiveElement>(&self, strip: E) -> E {
        if !cfg!(target_os = "macos") {
            return strip;
        }
        let pressed = Rc::clone(&self.title_press);
        let dragged = Rc::clone(&self.title_press);
        let released = Rc::clone(&self.title_press);
        strip
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                if event.click_count == 2 {
                    pressed.set(false);
                    window.titlebar_double_click();
                } else {
                    pressed.set(true);
                }
                cx.stop_propagation();
            })
            .on_mouse_move(move |event, window, _| {
                if event.dragging() && dragged.replace(false) {
                    window.start_window_move();
                }
            })
            .on_mouse_up(MouseButton::Left, move |_, _, _| released.set(false))
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
        let painted = Rc::clone(&self.painted);
        let probe = canvas(
            |_, _, _| {},
            move |bounds, (), _, _| painted.borrow_mut().search = Some(bounds),
        )
        .absolute()
        .inset_0();
        // On macOS the traffic lights sit over the strip above the search.
        let strip = cfg!(target_os = "macos")
            .then(|| self.title_strip(div().flex_none().h(px(HEADER_HEIGHT))));
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .border_r_1()
            .border_color(swatch.line)
            .bg(swatch.surface)
            .children(strip)
            .child(
                div()
                    .flex_none()
                    .px(px(10.0))
                    .pb(px(10.0))
                    .when(!cfg!(target_os = "macos"), |area| area.pt(px(10.0)))
                    .child(
                        Self::text_field(cx)
                            .relative()
                            .child(search)
                            .child(probe),
                    ),
            )
            .child(self.render_nav_area(nav, swatch, cx))
            .child(self.render_footer(swatch, cx))
            .into_any_element()
    }

    /// The section list in a scroll area that takes the sidebar's height
    /// between the search field and the footer, with a scrollbar like the
    /// pane's once the window is too short for every section.
    fn render_nav_area(
        &mut self,
        nav: Tabs,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let painted = Rc::clone(&self.painted);
        let probe = canvas(
            |_, _, _| {},
            move |bounds, (), _, _| painted.borrow_mut().nav = Some(bounds),
        )
        .absolute()
        .inset_0();
        let view = cx.entity();
        ScrollArea::new("settings-sections-scroll", &self.nav_scroll)
            .fill()
            .child(
                div()
                    .key_context("SettingsControl")
                    .px(px(8.0))
                    .pb(px(8.0))
                    .child(div().relative().child(nav).child(probe)),
            )
            .render(
                &mut self.nav_scrollbar,
                swatch.scrollbar,
                &view,
                |view| &mut view.nav_scrollbar,
                cx,
            )
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

    /// The sections as a vertical tab list: one tab stop whose arrows,
    /// as `settings_next` and `settings_previous`, show the section they
    /// reach. A search hides the selection. Each tab's `SettingsGroup`
    /// context nests inside the `SettingsControl` one `render_sidebar` sets
    /// around the list.
    fn render_nav(
        &mut self,
        swatch: Swatch,
        searching: bool,
        cx: &mut Context<'_, Self>,
    ) -> Tabs {
        let mut tabs = Tabs::new("settings-sections", "Sections", swatch)
            .vertical()
            .key_context("SettingsGroup");
        for section in self.visible_sections() {
            let handle = self.section_stop(section, cx);
            // A list counts its entries, as each is something the file sets.
            let count: usize = self.model.sections()[section as usize]
                .groups
                .iter()
                .flat_map(|group| &group.settings)
                .map(|setting| match self.file.get(&setting.path) {
                    Some(toml::Value::Array(entries)) => entries.len(),
                    // Profiles count the values they set, and layouts
                    // count each layout.
                    Some(_) if setting.path == "quake" => self.profile_values(),
                    Some(toml::Value::Table(layouts))
                        if setting.path == "panes.layouts" =>
                    {
                        layouts.len()
                    }
                    Some(_) => 1,
                    None => 0,
                })
                .sum();
            let on = section == self.section && !searching;
            tabs = tabs.tab(section.id(), section.title(), on, &handle);
            if count > 0 {
                // The tab hovers and takes the selection fill, so the count
                // draws no chip fill over them.
                tabs = tabs.trailing(
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
                );
            }
        }
        tabs.on_select(Self::listen(cx, |view, value: SharedString, _, cx| {
            if let Some(section) = SectionId::ALL
                .into_iter()
                .find(|section| section.id() == value.as_ref())
            {
                view.select_section(section, cx);
            }
        }))
    }

    /// The focus handle for the footer's action `key`, after the pane's
    /// controls in the tab order: Tab from the sidebar enters the section,
    /// and Shift-Tab from the section's first control returns to it.
    fn footer_stop(
        &mut self,
        key: &str,
        cx: &mut Context<'_, Self>,
    ) -> FocusHandle {
        let handle = self.action_stop(key.to_owned(), cx);
        if handle.tab_index == 1 {
            return handle;
        }
        let handle = handle.tab_index(1);
        self.actions.insert(key.to_owned(), handle.clone());
        handle
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
            let _ = self.footer_stop(key, cx);
            self.action_button(id.to_owned(), label, key.to_owned(), swatch, cx)
                .variant(Variant::Link)
                .button_role()
        });
        let _ = self.footer_stop("open:footer", cx);
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
            .debug_selector(|| "settings-footer".to_owned())
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(4.0))
            .p(px(10.0))
            .border_t_1()
            .border_color(swatch.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(ui(11.5))
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
        let alert = Alert::new(
            "settings-banner",
            "config.toml has an error",
            swatch,
        )
        .tone(AlertTone::Danger)
        .description(
            "Huterm keeps running with the last valid settings, and Settings is read-only until the file is fixed.",
        )
        .child(
            div()
                .p(px(8.0))
                .rounded(px(5.0))
                .bg(swatch.bg)
                .font_family(mono_font_family())
                .text_size(ui(11.5))
                .text_color(swatch.fg)
                .child(message.trim_end().to_owned()),
        )
        .child(div().flex().gap(px(6.0)).child(open).children(reload));
        Some(div().mb(px(14.0)).child(alert).into_any_element())
    }

    /// The latest warning or failure, spelled out above the section. A
    /// warning starts as one short row whose Details button shows its
    /// messages, so the user can carry on and look later; a failure shows
    /// them at once.
    fn render_notice(
        &mut self,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let notice = self.status.notice()?;
        let open = notice.open;
        let tone = match notice.tone {
            Tone::Warning => AlertTone::Warning,
            _ => AlertTone::Danger,
        };
        let details = notice.collapsible().then(|| {
            self.action_button(
                "settings-notice-details".to_owned(),
                if open { "Hide details" } else { "Details" },
                "notice:details".to_owned(),
                swatch,
                cx,
            )
            .small()
            .expanded(open)
        });
        let reload = (notice.action == Some(Action::Reload)).then(|| {
            self.action_button(
                "settings-notice-reload".to_owned(),
                "Reload",
                "reload:notice".to_owned(),
                swatch,
                cx,
            )
            .small()
        });
        let text = tone.colors(swatch).title;
        let alert = Alert::new("settings-notice", notice.title, swatch)
            .tone(tone)
            .when_some(details, Alert::action)
            .when_some(reload, Alert::action)
            .when(!open, Alert::compact)
            .when(open, |alert| {
                alert.child(
                    div()
                        .debug_selector(|| "settings-notice-messages".into())
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .text_size(ui(12.0))
                        .text_color(text)
                        .children(
                            notice
                                .messages
                                .iter()
                                // A chip's fill on the tone's wash would
                                // cost the text its contrast.
                                .map(|message| {
                                    Prose::new(sentence(message), swatch)
                                        .outlined()
                                }),
                        ),
                )
            });
        Some(div().mb(px(14.0)).child(alert).into_any_element())
    }

    /// A setting's row: a `Field` with the setting's name, a dot when the
    /// file sets it, its descriptions and error, Reset while it is changed,
    /// and its control, then any conflict with the file. The rows that
    /// lead to another section's editor are `Item`s with that button.
    pub(super) fn render_row(
        &mut self,
        setting: &'static Setting,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let path = setting.path.as_str();
        let read_only = self.read_only_reason(setting);
        let foreign = foreign_note(setting);
        let note = apply_note(setting.apply).filter(|_| foreign.is_none());
        let description = foreign.or_else(|| setting.description.clone());
        let changed = self.file.is_set(path);
        let control =
            self.render_control(setting, read_only.clone(), swatch, window, cx);
        if matches!(
            special(path),
            Some(
                Special::Keybindings
                    | Special::GlobalShortcuts
                    | Special::QuakeProfiles
            )
        ) {
            return Item::new(
                id(format!("settings-row-{path}")),
                setting.label.clone(),
                swatch,
            )
            .when_some(description, Item::description)
            .when(changed, |item| item.before_title(changed_dot(swatch)))
            .action(control)
            .into_any_element();
        }
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
        let conflict = self.render_conflict(setting, swatch, cx);
        // A conflict's alert says what the error would, and more. A family
        // that saved but fell back says so where its error would.
        let error = self
            .errors
            .get(path)
            .cloned()
            .or_else(|| Self::fallback_note(setting, cx))
            .filter(|_| conflict.is_none())
            .map(SharedString::from);
        // The theme picker's cards need the row's width.
        let wide = special(path) == Some(Special::ThemePicker);
        let field = Field::new(
            id(format!("settings-row-{path}")),
            setting.label.clone(),
            swatch,
        )
        .when(wide, Field::vertical)
        .label_debug_selector(|| format!("settings-label-{path}"))
        .when(changed, |field| field.before_label(changed_dot(swatch)))
        .when(setting.advanced, |field| {
            field.after_label(badge(
                id(format!("settings-advanced-{path}")),
                "Advanced",
                BadgeTone::Neutral,
                swatch,
            ))
        })
        .when_some(description, Field::description)
        .when_some(note, Field::description)
        .error(error)
        .when_some(reset, Field::action)
        .control(control);
        div()
            .flex()
            .flex_col()
            .debug_selector(move || format!("settings-row-{path}"))
            .child(field)
            .children(conflict)
            .into_any_element()
    }

    /// The warning under a row whose value changed in the file while an
    /// edit to it was waiting, with Use mine and Use file, which an invalid
    /// file cannot take yet.
    pub(super) fn render_conflict(
        &mut self,
        setting: &'static Setting,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let path = setting.path.as_str();
        let message = self.conflicts.get(path)?.message.clone();
        let choices = (!self.locked()).then(|| {
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
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(mine)
                .child(file)
        });
        let alert = Alert::new(
            id(format!("settings-conflict-{path}")),
            message,
            swatch,
        )
        .tone(AlertTone::Warning)
        .children(choices);
        Some(
            div()
                .px(px(14.0))
                .pb(px(10.0))
                .child(alert)
                .into_any_element(),
        )
    }

    /// Check for Updates, which runs the command the app menu has, so the
    /// section that schedules checks can also start one. A build without
    /// an updater disables it with the reason.
    fn render_update_check(
        &mut self,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let refusal = super::super::update_check_refusal(cx);
        let check = self
            .action_button(
                "settings-updates-check".to_owned(),
                "Check for Updates",
                "updates:card".to_owned(),
                swatch,
                cx,
            )
            .when_some(refusal, |button, reason| {
                button.disabled(Some(reason.into()))
            });
        let row = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(14.0))
            .py(px(10.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(ui(12.5))
                    .text_color(swatch.fg)
                    .child("Look for a newer version of Huterm now."),
            )
            .child(check);
        div()
            .mb(px(12.0))
            .child(Card::new("settings-updates", swatch).child(row))
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
        let row = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(14.0))
            .py(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(ui(12.5))
                            .text_color(swatch.fg)
                            .font_family(mono_font_family())
                            .truncate()
                            .child(path.display().to_string()),
                    )
                    .child(
                        div()
                            .text_size(ui(11.5))
                            .text_color(swatch.muted)
                            .child(
                                "Settings changes only the lines it edits, so your comments and formatting stay.",
                            ),
                    ),
            )
            .child(open)
            .child(reveal);
        div()
            .mb(px(12.0))
            .child(Card::new("settings-file", swatch).child(row))
            .into_any_element()
    }

    pub(super) fn render_main(
        &mut self,
        swatch: Swatch,
        query: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let searching = !query.is_empty();
        let mut body = div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(6.0))
            .px(px(24.0))
            .pt(px(8.0))
            .pb(px(24.0));
        if let Some(banner) = self.render_banner(swatch, cx) {
            body = body.child(banner);
        }
        if let Some(notice) = self.render_notice(swatch, cx) {
            body = body.child(notice);
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
                            .text_size(ui(14.0))
                            .text_color(swatch.fg)
                            .my(px(4.0))
                            .child(section.title()),
                    )
                    .children(groups);
            }
            if !matched {
                body = body.child(
                    div()
                        .text_size(ui(13.0))
                        .text_color(swatch.muted)
                        .child("No settings match."),
                );
            }
        } else {
            let section = self.section;
            body = body.children(
                (section == SectionId::General)
                    .then(|| self.render_file_card(swatch, cx)),
            );
            body = body.children(
                (section == SectionId::Updates)
                    .then(|| self.render_update_check(swatch, cx)),
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
        let header = self.render_header(self.title(cx), swatch);
        let view = cx.entity();
        let area = ScrollArea::new("settings-main", &self.scroll)
            .fill()
            .child(body)
            .render(
                &mut self.scrollbar,
                swatch.scrollbar,
                &view,
                |view| &mut view.scrollbar,
                cx,
            );
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(header)
            .child(area)
            .into_any_element()
    }

    /// The pane's header: `title`, level with the traffic lights on macOS,
    /// fixed above the scrolled content and parted from it by a hairline
    /// once the content scrolls under it.
    fn render_header(&self, title: &str, swatch: Swatch) -> AnyElement {
        let scrolled = self.scroll.offset().y < px(0.0);
        let painted = Rc::clone(&self.painted);
        let probe = canvas(
            |_, _, _| {},
            move |bounds, (), _, _| painted.borrow_mut().header = Some(bounds),
        )
        .absolute()
        .inset_0();
        // The hairline overlays the header's bottom edge, so the title stays
        // centred on the traffic lights.
        let hairline = scrolled.then(|| {
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(1.0))
                .bg(swatch.line)
        });
        self.title_strip(
            div()
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .h(px(HEADER_HEIGHT))
                .px(px(24.0))
                .debug_selector(|| "settings-header".to_owned())
                .child(probe)
                .children(hairline)
                .child(
                    div()
                        .id("settings-title")
                        .role(gpui::Role::Heading)
                        .aria_label(SharedString::from(title.to_owned()))
                        .min_w_0()
                        .truncate()
                        .text_size(ui(15.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(swatch.fg)
                        .child(title.to_owned()),
                ),
        )
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
            // The pane layouts and the layout cycle draw a row for each
            // layout.
            let rows: Vec<AnyElement> = settings
                .into_iter()
                .flat_map(|setting| match special(&setting.path) {
                    Some(Special::PaneLayouts) => {
                        self.render_pane_layouts(swatch, window, cx)
                    }
                    Some(Special::LayoutCycle) => {
                        self.render_layout_cycle(setting, swatch, cx)
                    }
                    _ => vec![self.render_row(setting, swatch, window, cx)],
                })
                .collect();
            cards.push(group_card(
                id(format!("settings-group-{}-{}", section.id(), group.title)),
                &group.title,
                rows,
                swatch,
            ));
        }
        cards
    }

    /// The button that shows or hides advanced settings in every section,
    /// under a section that has any available here.
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
        let on = self.show_advanced();
        let read_only = self
            .model
            .setting(super::SHOW_ADVANCED)
            .and_then(|setting| self.read_only_reason(setting));
        Some(
            self.action_button(
                "settings-advanced".to_owned(),
                if on {
                    "Hide advanced settings in every section"
                } else {
                    "Show advanced settings in every section"
                },
                "advanced:pane".to_owned(),
                swatch,
                cx,
            )
            .variant(Variant::Ghost)
            .when_some(read_only, |button, reason| {
                button.disabled(Some(reason))
            })
            .into_any_element(),
        )
    }
}

/// `message` with a capital first letter: messages are written as clauses,
/// such as "cannot register …".
fn sentence(message: &str) -> String {
    let mut chars = message.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A titled card of `rows`, as each group of the pane draws, with the space
/// below it that separates it from the next.
pub(super) fn group_card(
    id: impl Into<ElementId>,
    title: &str,
    rows: Vec<AnyElement>,
    swatch: Swatch,
) -> AnyElement {
    div()
        .mb(px(12.0))
        .child(
            Card::new(id, swatch)
                .title(title.to_uppercase())
                .children(rows),
        )
        .into_any_element()
}

/// The dot before a name the file sets.
pub(super) fn changed_dot(swatch: Swatch) -> Div {
    div()
        .flex_none()
        .size(px(6.0))
        .rounded_full()
        .bg(swatch.accent)
}
