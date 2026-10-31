//! The Appearance theme picker: one theme, or one for each of the system's
//! light and dark modes (`[theme] light` and `dark`).
//!
//! A mode switch chooses between them. With a theme for each mode, two
//! slot cards pick which mode's theme the grid sets; the one the system
//! uses now says so. The grid groups light and dark themes under headings,
//! but any theme can go in either slot. Every change writes the `theme.*`
//! keys it moves in one save (`Batch::Theme`).

use gpui::{
    AnyElement, Context, Div, FontWeight, SharedString, Window, div,
    prelude::*, px, svg,
};
use huterm_ui::components::badge::{Tone as BadgeTone, badge};
use huterm_ui::components::radio_group::RadioGroup;
use huterm_ui::components::toggle_group::ToggleGroup;
use huterm_ui::typography::ui;
use toml::Value;

use super::super::super::overlay::{Swatch, mono_font_family, theme_is_dark};
use super::super::Desktop;
use super::saving::Batch;
use super::{MenuFor, MenuItem, MenuModel, MenuRow, SettingsView};
use crate::assets::Icon;
use crate::config::Theme;
use crate::renderer::rgb_color as color;

/// A theme card's width.
const CARD_WIDTH: f32 = 128.0;

/// The keys the picker writes.
pub(super) const NAME: &str = "theme.name";
pub(super) const LIGHT: &str = "theme.light";
pub(super) const DARK: &str = "theme.dark";

/// The mode switch's action key: Space and Return toggle it, and the
/// arrows step to One theme or Match system.
pub(super) const MODE_KEY: &str = "theme-mode";

/// The light and dark mode cards' action keys, in order.
pub(super) const SLOT_KEYS: [&str; 2] = ["theme-slot:light", "theme-slot:dark"];

/// Which of the system's modes a theme is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Slot {
    Light,
    Dark,
}

impl Slot {
    fn key(self) -> &'static str {
        match self {
            Self::Light => LIGHT,
            Self::Dark => DARK,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Light => "Light mode",
            Self::Dark => "Dark mode",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Light => Icon::Sun,
            Self::Dark => Icon::Moon,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub(super) fn from_id(id: &str) -> Option<Self> {
        match id {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    /// The slot the system's mode uses now, as the desktop last followed
    /// it, so what Settings shows and what it writes always agree.
    fn current(cx: &gpui::App) -> Self {
        if cx.global::<Desktop>().system_mode.is_dark() {
            Self::Dark
        } else {
            Self::Light
        }
    }
}

impl SettingsView {
    /// A `theme.*` key as it will be once pending saves land.
    fn theme_entry(&self, key: &'static str) -> Option<String> {
        match self.pending.get(key) {
            Some(pending) => pending.as_ref(),
            None => self.file.get(key),
        }
        .and_then(Value::as_str)
        .map(str::to_owned)
    }

    /// Whether the theme follows the system's mode: `light` or `dark` set.
    pub(super) fn theme_auto(&self) -> bool {
        self.theme_entry(LIGHT).is_some() || self.theme_entry(DARK).is_some()
    }

    /// The theme `name` selects, else the default.
    fn theme_name(&self) -> String {
        self.theme_entry(NAME)
            .unwrap_or_else(|| crate::themes::DEFAULT.to_owned())
    }

    /// The theme for `slot`, which falls back to `name`.
    fn slot_theme(&self, slot: Slot) -> String {
        self.theme_entry(slot.key())
            .unwrap_or_else(|| self.theme_name())
    }

    /// The theme in use for the system's mode now, as the file chooses it.
    pub(super) fn chosen_theme(&self, cx: &gpui::App) -> String {
        if self.theme_auto() {
            self.slot_theme(Slot::current(cx))
        } else {
            self.theme_name()
        }
    }

    /// The theme in use now: the one the desktop applied for the system's
    /// mode, which a file that fails to load, a save not yet reloaded, or
    /// an edit made while watching is off has not replaced.
    pub(super) fn active_theme(cx: &gpui::App) -> String {
        let desktop = cx.global::<Desktop>();
        let dark = desktop.system_mode.is_dark();
        desktop.config.themes.name(dark).to_owned()
    }

    /// The slot the grid sets: the one chosen, else the system's now.
    fn target_slot(&self, cx: &gpui::App) -> Slot {
        self.theme_slot.unwrap_or_else(|| Slot::current(cx))
    }

    /// The action key of the mode card the grid sets.
    pub(super) fn selected_slot(&self, cx: &gpui::App) -> &'static str {
        match self.target_slot(cx) {
            Slot::Light => SLOT_KEYS[0],
            Slot::Dark => SLOT_KEYS[1],
        }
    }

    /// The theme the grid marks: the target slot's, or the one theme.
    pub(super) fn selected_theme(&self, cx: &gpui::App) -> String {
        if self.theme_auto() {
            self.slot_theme(self.target_slot(cx))
        } else {
            self.theme_name()
        }
    }

    /// Writes the `theme.*` keys in `changes` that differ from what they
    /// hold, in one save.
    fn write_theme(
        &mut self,
        changes: &[(&'static str, Option<String>)],
        cx: &mut Context<'_, Self>,
    ) {
        let changes: Vec<_> = changes
            .iter()
            .filter(|(key, value)| self.theme_entry(key) != *value)
            .cloned()
            .collect();
        if !changes.is_empty() {
            self.submit_batch(Batch::Theme(changes), cx);
        }
    }

    /// Chooses `name` for the target slot, or as the one theme.
    pub(super) fn choose_theme(
        &mut self,
        name: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let key = if self.theme_auto() {
            self.target_slot(cx).key()
        } else {
            NAME
        };
        self.write_theme(&[(key, Some(name.to_owned()))], cx);
    }

    /// Switches between one theme and one for each mode. Following the
    /// system starts both modes on the current theme; one theme keeps the
    /// one in use now.
    pub(super) fn set_theme_auto(
        &mut self,
        auto: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if auto == self.theme_auto() {
            return;
        }
        self.theme_slot = None;
        let changes = if auto {
            let current = self.theme_name();
            [
                (LIGHT, Some(current.clone())),
                (DARK, Some(current)),
                (NAME, None),
            ]
        } else {
            [
                (NAME, Some(self.chosen_theme(cx))),
                (LIGHT, None),
                (DARK, None),
            ]
        };
        self.write_theme(&changes, cx);
    }

    /// Resets the picker: no name and no mode themes, so the default.
    pub(super) fn reset_theme(&mut self, cx: &mut Context<'_, Self>) {
        self.theme_slot = None;
        self.write_theme(&[(NAME, None), (LIGHT, None), (DARK, None)], cx);
    }

    /// Whether the file sets any key the picker writes.
    pub(super) fn theme_set(&self) -> bool {
        [NAME, LIGHT, DARK].iter().any(|key| self.file.is_set(key))
    }

    /// The Theme mode row's control: a segmented One theme / Match system
    /// switch at the row's end, as Settings' other segmented rows have.
    /// `read_only` says why it cannot change, when it cannot.
    pub(super) fn render_theme_mode(
        &mut self,
        read_only: Option<SharedString>,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let auto = self.theme_auto();
        let handle = self.action_stop(MODE_KEY.to_owned(), cx);
        let focus = handle.clone();
        let group = ToggleGroup::single(
            "settings-theme-mode",
            "Theme mode",
            &handle,
            swatch,
        )
        .option("single", "One theme", !auto)
        .option("auto", "Match system", auto)
        .key_context("SettingsControl")
        .when_some(read_only, |group, reason| group.disabled(Some(reason)))
        .on_select(Self::listen(
            cx,
            move |view, picked: SharedString, window, cx| {
                focus.focus(window, cx);
                view.set_theme_auto(picked.as_ref() == "auto", cx);
            },
        ));
        // The menu Space and Return open anchors to the switch.
        let control = self.with_menu(MODE_KEY, group.into_any_element());
        div()
            .flex_none()
            .debug_selector(|| "settings-theme-mode".to_owned())
            .child(control)
            .into_any_element()
    }

    /// Opens the Theme mode options under the switch, as Space and Return
    /// open a segmented row's choices.
    pub(super) fn open_theme_mode(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.locked() {
            return;
        }
        let rows = [("single", "One theme"), ("auto", "Match system")]
            .into_iter()
            .map(|(value, label)| MenuRow::Item(MenuItem::new(value, label)))
            .collect();
        let opener = self.action_stop(MODE_KEY.to_owned(), cx);
        self.show_menu(
            MenuFor::ThemeMode,
            MenuModel::new(rows),
            opener,
            window,
            cx,
        );
    }

    /// What the Theme mode row holds under its line: the slot cards while
    /// the theme follows the system, a note on the color overrides the
    /// file sets on top, and the grid. `read_only` says why nothing can
    /// change, when it cannot.
    pub(super) fn render_theme_body(
        &mut self,
        read_only: Option<SharedString>,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let locked = read_only.is_some();
        let grid = self.render_theme_grid(read_only.as_ref(), swatch, cx);
        let slots = self
            .theme_auto()
            .then(|| self.render_slots(read_only, swatch, cx));
        let overrides: Vec<String> = self
            .file
            .get("theme")
            .and_then(Value::as_table)
            .map(|table| {
                table
                    .keys()
                    .filter(|key| {
                        !matches!(key.as_str(), "name" | "light" | "dark")
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        // A hairline sets the picker under the row's line, as a card's
        // rows are set apart.
        div()
            .key_context("SettingsControl")
            .flex()
            .flex_col()
            .gap(px(10.0))
            .px(px(14.0))
            .pt(px(12.0))
            .pb(px(14.0))
            .border_t_1()
            .border_color(swatch.line)
            .children(slots)
            .when(!overrides.is_empty(), |block| {
                block.child(
                    div()
                        .text_size(ui(11.5))
                        .text_color(swatch.warning_text())
                        .when(locked, |note| note.opacity(0.5))
                        .child(format!(
                            "Your [theme] sets {}, which applies on top of any theme.",
                            overrides.join(", ")
                        )),
                )
            })
            .child(grid)
            .into_any_element()
    }

    /// The light and dark mode cards, a radio group choosing the slot the
    /// grid sets, then a line naming it.
    fn render_slots(
        &mut self,
        read_only: Option<SharedString>,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let target = self.target_slot(cx);
        let current = Slot::current(cx);
        let active = Self::active_theme(cx);
        let mut cards =
            RadioGroup::new("settings-theme-slots", "Theme to choose", swatch)
                .card(px(CARD_WIDTH * 2.0))
                .stretch()
                .key_context("SettingsGroup");
        let system = if cfg!(target_os = "macos") {
            "macOS"
        } else {
            "The system"
        };
        for slot in [Slot::Light, Slot::Dark] {
            let name = self.slot_theme(slot);
            let handle =
                self.action_stop(format!("theme-slot:{}", slot.id()), cx);
            let preview = self.preview(&name);
            let header = div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .px(px(2.0))
                .text_size(ui(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(swatch.fg)
                .child(icon(slot.icon(), swatch.fg))
                .child(slot.label())
                .child(div().flex_1())
                .when(slot == current && name == active, |header| {
                    header.child(badge(
                        format!("settings-theme-slot-active-{}", slot.id()),
                        "Active",
                        BadgeTone::Accent,
                        swatch,
                    ))
                });
            cards = cards
                .option(slot.id(), name.clone(), slot == target, &handle)
                .accessible_name(if slot == current {
                    // "macOS" keeps its case; "The system" continues the
                    // sentence.
                    let system = system.replacen("The", "the", 1);
                    format!(
                        "{} theme: {name}, {system} is {}",
                        slot.label(),
                        slot.id()
                    )
                } else {
                    format!("{} theme: {name}", slot.label())
                })
                .content(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(header)
                        .child(preview_block(preview.as_ref(), 60.0)),
                );
            // The card for the system's mode says which mode that is.
            if slot == current {
                cards = cards.trailing(
                    div()
                        .text_color(swatch.muted)
                        .child(format!("{system} is {}", slot.id())),
                );
            }
        }
        let cards = cards
            .when_some(read_only, |cards, reason| cards.disabled(Some(reason)))
            .on_select(Self::listen(
                cx,
                move |view, slot: SharedString, window, cx| {
                    let key = format!("theme-slot:{slot}");
                    if let Some(handle) = view.actions.get(&key) {
                        handle.clone().focus(window, cx);
                    }
                    view.act(&key, window, cx);
                },
            ));
        let note = div()
            .flex()
            .text_size(ui(11.5))
            .text_color(swatch.muted)
            .child("Choosing a theme below sets the\u{a0}")
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(swatch.fg)
                    .child(target.label()),
            )
            .child("\u{a0}theme.");
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(cards)
            .child(note)
            .into_any_element()
    }

    /// The theme cards under Light and Dark headings: a radio group each,
    /// whose cards' `SettingsGroup` context binds the arrows to
    /// `settings_next` and `settings_previous`, which move across both
    /// groups in order and choose the theme they reach.
    fn render_theme_grid(
        &mut self,
        read_only: Option<&SharedString>,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let auto = self.theme_auto();
        let selected = self.selected_theme(cx);
        let active = Self::active_theme(cx);
        let light = auto.then(|| self.slot_theme(Slot::Light));
        let dark = auto.then(|| self.slot_theme(Slot::Dark));
        let themes = self.themes.clone();
        let mut block = div().flex().flex_col().gap(px(6.0));
        for (heading, dark_group) in [("Light", false), ("Dark", true)] {
            let group: Vec<_> = themes
                .iter()
                .filter(|card| card.dark == dark_group)
                .collect();
            if group.is_empty() {
                continue;
            }
            let mut cards = RadioGroup::new(
                format!("settings-themes-{}", heading.to_lowercase()),
                format!("{heading} themes"),
                swatch,
            )
            .card(px(CARD_WIDTH))
            .key_context("SettingsGroup");
            let count = group.len();
            for ThemeCard { name, preview, .. } in group {
                let handle = self.action_stop(format!("theme:{name}"), cx);
                let marks = Marks {
                    active: *name == active,
                    light: light.as_deref() == Some(name.as_str()),
                    dark: dark.as_deref() == Some(name.as_str()),
                };
                cards = cards
                    .option(
                        name.clone(),
                        name.clone(),
                        *name == selected,
                        &handle,
                    )
                    .content(preview_block(preview.as_ref(), 52.0))
                    .when(marks.any(), |cards| {
                        cards.footer(card_marks(name, marks, swatch))
                    });
            }
            let cards = cards
                .when_some(read_only.cloned(), |cards, reason| {
                    cards.disabled(Some(reason))
                })
                .on_select(Self::listen(
                    cx,
                    move |view, name: SharedString, window, cx| {
                        let key = format!("theme:{name}");
                        if let Some(handle) = view.actions.get(&key) {
                            handle.clone().focus(window, cx);
                        }
                        view.act(&key, window, cx);
                    },
                ));
            block = block
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .mt(px(4.0))
                        .text_size(ui(11.0))
                        .text_color(swatch.muted)
                        .child(format!("{heading} · {count}"))
                        .child(div().flex_1().h(px(1.0)).bg(swatch.line)),
                )
                .child(cards);
        }
        block.into_any_element()
    }

    /// A theme's colors from the scan, if it resolved.
    fn preview(&self, name: &str) -> Option<Theme> {
        self.themes
            .iter()
            .find(|card| card.name == name)
            .and_then(|card| card.preview.clone())
    }

    /// Every theme card's name in the order the grid shows them: light
    /// themes, then dark ones.
    pub(super) fn ordered_theme_names(&self) -> Vec<String> {
        self.themes.iter().map(|card| card.name.clone()).collect()
    }
}

/// A theme the picker offers, with its colors when it resolves.
#[derive(Clone)]
pub(super) struct ThemeCard {
    pub(super) name: String,
    pub(super) preview: Option<Theme>,
    /// Whether it reads as dark, which groups it. One that did not resolve
    /// counts as dark, as the colors it would start from are.
    pub(super) dark: bool,
}

/// The themes `[theme] name` can select, with the themes directory next
/// to `config`, in the order the grid shows them: light, then dark, each
/// sorted by name. Reads theme files, so it runs off the UI thread.
pub(super) fn scan(config: &std::path::Path) -> Vec<ThemeCard> {
    let directory = config
        .parent()
        .map_or_else(std::path::PathBuf::new, |parent| parent.join("themes"));
    let mut cards: Vec<ThemeCard> = crate::themes::names(Some(&directory))
        .into_iter()
        .map(|name| {
            let preview = crate::themes::preview(&name, &directory);
            let dark = preview.as_ref().is_none_or(theme_is_dark);
            ThemeCard {
                name,
                preview,
                dark,
            }
        })
        .collect();
    // Stable, so each group keeps the names' order.
    cards.sort_by_key(|card| card.dark);
    cards
}

/// What a theme card marks besides being chosen.
#[derive(Clone, Copy)]
struct Marks {
    /// The theme in use now.
    active: bool,
    /// The light or dark mode theme, while the theme follows the system.
    light: bool,
    dark: bool,
}

impl Marks {
    fn any(self) -> bool {
        self.active || self.light || self.dark
    }
}

fn icon(icon: Icon, color: gpui::Hsla) -> gpui::Svg {
    svg()
        .path(icon.asset_path())
        .size(px(12.0))
        .flex_none()
        .text_color(color)
}

/// A card's marks under its name: the light and dark mode themes, while
/// the theme follows the system, and the theme in use. They take a line of
/// their own so the name keeps the card's width.
fn card_marks(name: &str, marks: Marks, swatch: Swatch) -> Div {
    let mut row = div().flex().items_center().gap(px(3.0));
    for (on, slot) in [(marks.light, Slot::Light), (marks.dark, Slot::Dark)] {
        if on {
            row = row.child(
                div()
                    .p(px(2.0))
                    .rounded(px(4.0))
                    .bg(swatch.chip)
                    .child(icon(slot.icon(), swatch.fg).size(px(10.0))),
            );
        }
    }
    if marks.active {
        row = row.child(badge(
            SharedString::from(format!("settings-theme-active-{name}")),
            "Active",
            BadgeTone::Accent,
            swatch,
        ));
    }
    row
}

/// A theme's preview: a prompt and a build's output in its colors over its
/// background, with its six ANSI accents.
fn preview_block(theme: Option<&Theme>, height: f32) -> Div {
    // Sized with the interface text it holds.
    let block = div()
        .h(ui(height))
        .rounded(px(5.0))
        .px(px(7.0))
        .py(px(6.0))
        .overflow_hidden();
    let Some(theme) = theme else {
        return block;
    };
    let text =
        |rgb, label: &'static str| div().text_color(color(rgb)).child(label);
    let dots = (1..=6).fold(div().flex().gap(px(2.0)), |dots, index| {
        dots.child(
            div()
                .size(px(5.0))
                .rounded(px(1.5))
                .bg(color(theme.ansi[index])),
        )
    });
    block
        .bg(color(theme.background))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .font_family(mono_font_family())
        .text_size(ui(9.5))
        .line_height(ui(12.0))
        .child(
            div()
                .flex()
                .gap(px(5.0))
                .child(text(theme.ansi[2], "~/huterm"))
                .child(text(theme.ansi[4], "main")),
        )
        .child(
            div()
                .flex()
                .gap(px(5.0))
                .child(text(theme.ansi[5], "$"))
                .child(text(theme.foreground, "cargo test")),
        )
        // The words give way before the accents, which always show whole.
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .gap(px(5.0))
                        .child(text(theme.ansi[1], "error"))
                        .child(text(theme.ansi[3], "warn")),
                )
                .child(dots.flex_none()),
        )
}
