//! The Settings window: a sidebar of sections with search and the file
//! status, and a pane of grouped rows whose controls save each change to
//! `config.toml` through the patch service.
//!
//! The window has no session, workspace, or terminal and no
//! [`WindowModel`] record. `Desktop::settings` holds its handle, so
//! `open_settings` focuses it instead of opening a second one. Writes run on
//! the background executor one at a time; a change made during one waits
//! behind it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::OnceLock;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Hsla, KeyContext, Render,
    ScrollHandle, SharedString, Stateful, Window, WindowBounds, WindowKind,
    WindowOptions, div, point, prelude::*, px, size,
};
use huterm_config::document::Snapshot;
use huterm_config::settings::{
    Apply, Choice, Platform as SettingPlatform, SectionId, Setting,
    SettingKind, SettingsModel,
};
use huterm_protocol::{ArgumentKind, CommandError, CommandInvocation, ids};
use toml::Value;

use super::super::menu::{
    Menu as MenuView, MenuEvent, MenuItem, MenuModel, MenuRow,
};
use super::super::overlay::Swatch;
use super::super::settings::status::{FileStatus, Tone};
use super::super::settings::values::{
    self, Defaults, FileValues, number, parse_number, shown,
};
use super::super::settings::{Special, file_only, special};
use super::Desktop;
use crate::APP_ID;
use crate::commands::{InvokePalette, InvokeWindow};
use crate::config::Theme;
use crate::ui::text_field::TextField;

const SIDEBAR_WIDTH: f32 = 200.0;
const WINDOW_SIZE: (f32, f32) = (860.0, 620.0);
const MIN_SIZE: (f32, f32) = (640.0, 420.0);
const SEGMENTED_MAX: usize = 3;

/// Opens the Settings window, or brings the open one forward.
pub(super) fn open(cx: &mut App) {
    if let Some(handle) = cx.global::<Desktop>().settings
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        cx.activate(true);
        return;
    }
    let display_id = cx.active_window().and_then(|window| {
        window
            .update(cx, |_, window, cx| window.display(cx))
            .ok()
            .flatten()
    });
    let bounds = gpui::Bounds::centered(
        display_id.map(|display| display.id()),
        size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)),
        cx,
    );
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Settings".into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
            window_decorations: Some(gpui::WindowDecorations::Server),
            kind: WindowKind::Normal,
            app_id: Some(APP_ID.into()),
            focus: true,
            show: true,
            ..WindowOptions::default()
        },
        |window, cx| {
            let view = cx.new(|cx| SettingsView::new(window, cx));
            view.read(cx).focus.clone().focus(window, cx);
            view
        },
    );
    match opened {
        Ok(handle) => {
            cx.global_mut::<Desktop>().settings = Some(handle);
            cx.activate(true);
        }
        Err(error) => {
            super::show_active_window_failure(
                cx,
                &format!("Cannot open Settings: {error}"),
            );
        }
    }
}

/// Tells the Settings window that a reload finished, so it rereads the
/// file and reports the outcome.
pub(super) fn reloaded(cx: &mut App, outcome: Result<(), String>) {
    let Some(handle) = cx.global::<Desktop>().settings else {
        return;
    };
    let _ = handle.update(cx, |view, _, cx| {
        view.reloaded(outcome, cx);
    });
}

/// Shows a failure in the Settings window's footer when it is the active
/// window, as a terminal window shows a command failure. Returns whether
/// it did.
pub(super) fn show_failure(cx: &mut App, message: &str) -> bool {
    let Some(handle) = cx.global::<Desktop>().settings else {
        return false;
    };
    if cx.active_window() != Some(handle.into()) {
        return false;
    }
    handle
        .update(cx, |view, _, cx| {
            view.status.file_error(message);
            cx.notify();
        })
        .is_ok()
}

/// Closes the Settings window, as Quit does.
pub(super) fn close(cx: &mut App) {
    if let Some(handle) = cx.global_mut::<Desktop>().settings.take() {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

mod controls;
mod keybindings;
mod quake;
mod render;
mod saving;

#[cfg(all(test, feature = "test-support"))]
pub(super) use keybindings::CHORD_TIMEOUT;
use keybindings::Keys;
use saving::{Conflict, Queued};

/// What an open select's menu sets: a setting, or one argument of a
/// shortcut row.
pub(super) enum MenuFor {
    Setting(&'static Setting),
    Argument {
        row: String,
        name: &'static str,
        kind: ArgumentKind,
    },
    /// The Quake windows section's profile dropdown.
    Profile,
    /// A global shortcut's `command` or `profile`.
    Global {
        entry: usize,
        field: &'static str,
    },
}

impl MenuFor {
    /// The key of the control the menu opens under.
    fn key(&self) -> String {
        match self {
            Self::Setting(setting) => setting.path.clone(),
            Self::Argument { row, name, .. } => format!("karg:{row}:{name}"),
            Self::Profile => "qprof".to_owned(),
            Self::Global {
                entry,
                field: "command",
            } => format!("gcmd:{entry}"),
            Self::Global { entry, .. } => format!("gprofile:{entry}"),
        }
    }
}

pub(super) struct SettingsView {
    focus: FocusHandle,
    model: &'static SettingsModel,
    path: PathBuf,
    /// The snapshot the next write rebases from; `None` until the first
    /// load finishes.
    base: Option<Snapshot>,
    file: FileValues,
    defaults: Defaults,
    status: FileStatus,
    section: SectionId,
    advanced: HashSet<SectionId>,
    search: Entity<TextField>,
    /// Number and text fields, by setting path.
    fields: HashMap<&'static str, Entity<TextField>>,
    /// The text each field last showed from the file; a field whose text
    /// differs holds an unsaved draft that a reload must not replace.
    synced: HashMap<&'static str, String>,
    /// Focus handles for every other control, by stop.
    stops: HashMap<&'static str, FocusHandle>,
    sections: HashMap<SectionId, FocusHandle>,
    /// Focus handles for buttons and cards that act rather than hold a
    /// value, by action key such as `theme:nord`; see `act`.
    actions: HashMap<String, FocusHandle>,
    errors: HashMap<&'static str, String>,
    conflicts: HashMap<&'static str, Conflict>,
    /// The setting whose write is in flight.
    writing: Option<&'static str>,
    queued: VecDeque<Queued>,
    /// The latest value requested for each setting whose write has not
    /// settled; controls show it, and steps build on it.
    pending: HashMap<&'static str, Option<Value>>,
    /// Incremented by every finished write; a load started before one is
    /// stale.
    epoch: u64,
    /// Reads and theme scans started; only the latest of each applies.
    reads: u64,
    scans: u64,
    /// Reads still running.
    loading: u32,
    /// A read finished while writes were pending and was dropped; the file
    /// is read again when the queue drains.
    reread: bool,
    /// A setting whose next change reads the file afresh, after Use mine.
    fresh: Option<&'static str>,
    /// The last reload's error when the file itself validates, so the
    /// footer can say the settings were not applied.
    reload_error: Option<String>,
    /// Close was asked for while writes were pending.
    closing: bool,
    handle: gpui::AnyWindowHandle,
    /// An open select's menu, what it sets, and the control that opened
    /// it, which takes focus back.
    menu: Option<(Entity<MenuView>, MenuFor, FocusHandle)>,
    /// The Keybindings section.
    keys: Keys,
    /// The Quake windows section.
    quake: quake::Quake,
    /// The action and keybinding field keys the last frame drew, so focus
    /// left on a control that went away can move on.
    drawn: HashSet<String>,
    /// What had focus at the last render, to tell a control that left the
    /// tree from one that just took focus and has yet to draw.
    last_focus: Option<FocusHandle>,
    /// Takes key-downs while a shortcut records.
    _recorder: gpui::Subscription,
    scroll: ScrollHandle,
    /// Every selectable theme with its colors, read when the window opens.
    themes: Vec<(String, Option<Theme>)>,
}

impl SettingsView {
    fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let model = SettingsModel::load()
            .expect("tests keep the committed schema valid");
        let desktop = cx.global::<Desktop>();
        let path = desktop.config_path.clone();
        let watching = desktop.config.general.watch_config;
        let swatch = Swatch::from_theme(&desktop.config.theme);
        let search =
            cx.new(|cx| TextField::new("Search settings", swatch.fg, cx));
        let _ = search.read(cx).focus_handle(cx).tab_stop(true);
        cx.subscribe(
            &search,
            |view, _, _: &crate::ui::text_field::Changed, cx| {
                view.scroll.set_offset(point(px(0.0), px(0.0)));
                cx.notify();
            },
        )
        .detach();
        let keys = Keys::new(swatch, cx);
        // While a shortcut records, its keys reach no binding, menu, or
        // field: the interceptor runs before GPUI matches the keymap.
        let weak = cx.entity().downgrade();
        let handle = window.window_handle();
        let recorder = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != handle {
                return;
            }
            let Some(view) = weak.upgrade() else {
                return;
            };
            if view.update(cx, |view, cx| view.record(&event.keystroke, cx)) {
                cx.stop_propagation();
            }
        });
        let mut view = Self {
            focus: cx.focus_handle(),
            keys,
            quake: quake::Quake::new(),
            drawn: HashSet::new(),
            last_focus: None,
            _recorder: recorder,
            model,
            themes: Vec::new(),
            path,
            base: None,
            file: FileValues::default(),
            defaults: Defaults::current(),
            status: FileStatus::new(watching),
            section: SectionId::General,
            advanced: HashSet::new(),
            search,
            fields: HashMap::new(),
            synced: HashMap::new(),
            stops: HashMap::new(),
            sections: HashMap::new(),
            actions: HashMap::new(),
            errors: HashMap::new(),
            conflicts: HashMap::new(),
            writing: None,
            queued: VecDeque::new(),
            pending: HashMap::new(),
            epoch: 0,
            reads: 0,
            scans: 0,
            loading: 0,
            reread: false,
            fresh: None,
            reload_error: None,
            closing: false,
            handle: window.window_handle(),
            menu: None,
            scroll: ScrollHandle::new(),
        };
        view.create_and_load(cx);
        view.load_themes(cx);
        // Closing waits for pending writes, so none is lost.
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |view, cx| view.request_close(window, cx))
                .unwrap_or(true)
        });
        // With watching off nothing reports an edit made elsewhere, so the
        // window looks when it comes forward.
        cx.observe_window_activation(window, |view, window, cx| {
            if window.is_window_active() {
                view.check_disk(cx);
            } else {
                view.cancel_recording(cx);
            }
        })
        .detach();
        view
    }

    /// Whether controls are read-only: the file is invalid, or the first
    /// read has not finished, so an edit cannot race the default file's
    /// creation or build on a file the window has not seen.
    fn locked(&self) -> bool {
        self.status.invalid() || self.base.is_none()
    }

    fn swatch(cx: &App) -> Swatch {
        Swatch::from_theme(&cx.global::<Desktop>().config.theme)
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).text().trim().to_lowercase()
    }

    fn field(
        &mut self,
        setting: &'static Setting,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<TextField> {
        if let Some(field) = self.fields.get(setting.path.as_str()) {
            return field.clone();
        }
        let swatch = Self::swatch(cx);
        let text = field_text(shown(setting, &self.file, &self.defaults));
        let placeholder = if setting.default.is_none()
            && self.defaults.get(&setting.path).is_none()
        {
            "Default"
        } else {
            ""
        };
        let field = cx.new(|cx| {
            let mut field = TextField::new(placeholder, swatch.fg, cx);
            field.set_text(text, cx);
            field
        });
        let handle = field.read(cx).focus_handle(cx).tab_stop(true);
        cx.on_blur(&handle, window, move |view, _, cx| {
            view.field_blurred(setting, cx);
        })
        .detach();
        self.fields.insert(setting.path.as_str(), field.clone());
        self.synced
            .insert(setting.path.as_str(), text_for(field.read(cx)));
        field
    }

    fn stop(
        &mut self,
        path: &'static str,
        cx: &mut Context<'_, Self>,
    ) -> FocusHandle {
        self.stops
            .entry(path)
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    fn section_stop(
        &mut self,
        section: SectionId,
        cx: &mut Context<'_, Self>,
    ) -> FocusHandle {
        self.sections
            .entry(section)
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    /// The setting whose control has focus, if any.
    fn focused_setting(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<&'static Setting> {
        let path = self
            .stops
            .iter()
            .find(|(_, handle)| handle.is_focused(window))
            .map(|(path, _)| *path)
            .or_else(|| {
                self.fields
                    .iter()
                    .find(|(_, field)| {
                        field.read(cx).focus_handle(cx).is_focused(window)
                    })
                    .map(|(path, _)| *path)
            })?;
        self.setting_at(path)
    }

    /// The setting at `path`: one of the model's, or a quake profile's.
    fn setting_at(&self, path: &str) -> Option<&'static Setting> {
        self.model
            .setting(path)
            .or_else(|| quake::profile_setting(self.model, path))
    }

    /// The setting whose number or text field has focus.
    fn focused_field(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<&'static Setting> {
        let path = self
            .fields
            .iter()
            .find(|(_, field)| {
                field.read(cx).focus_handle(cx).is_focused(window)
            })
            .map(|(path, _)| *path)?;
        self.setting_at(path)
    }

    /// The focus handle for an action key, created on first use.
    fn action_stop(
        &mut self,
        key: String,
        cx: &mut Context<'_, Self>,
    ) -> FocusHandle {
        // Controls ask for their stop while they draw, which marks them
        // drawn for focus recovery.
        self.drawn.insert(key.clone());
        self.actions
            .entry(key)
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    /// The action key whose control has focus.
    fn focused_action(&self, window: &Window) -> Option<String> {
        self.actions
            .iter()
            .find(|(_, handle)| handle.is_focused(window))
            .map(|(key, _)| key.clone())
    }

    /// Runs the control an action key names, as a click on it does.
    fn act(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let (kind, argument) = key.split_once(':').unwrap_or((key, ""));
        if self.act_keys(kind, argument, window, cx)
            || self.act_quake(kind, argument, window, cx)
        {
            cx.notify();
            return;
        }
        let model = self.model;
        match kind {
            "theme" => {
                if let Some(theme) = model.setting("theme")
                    && self.selected_theme() != argument
                {
                    let name = Value::String(argument.to_owned());
                    self.submit(
                        theme,
                        values::patch_at("theme.name", Some(name.clone())),
                        Some(name),
                        None,
                        cx,
                    );
                }
            }
            "chip" => {
                if let Some((path, modifier)) = argument.split_once(':')
                    && let Some(setting) = model.setting(path)
                    && let Some(next) =
                        toggled_modifiers(self.value(setting), modifier)
                {
                    self.write(setting, Some(Value::String(next)), cx);
                }
            }
            "advanced" => {
                if let Some(section) = SectionId::ALL
                    .into_iter()
                    .find(|section| section.id() == argument)
                    && !self.advanced.remove(&section)
                {
                    self.advanced.insert(section);
                }
            }
            "fonts" => {
                if let Some(setting) = model.setting(argument) {
                    self.open_fonts(setting, window, cx);
                }
            }
            "mine" => {
                if let Some(setting) = self.setting_at(argument) {
                    self.keep_mine(setting.path.as_str(), cx);
                }
            }
            "file" => {
                if let Some(setting) = self.setting_at(argument) {
                    self.keep_file(setting.path.as_str(), cx);
                }
            }
            "reset" => {
                if let Some(setting) = self.setting_at(argument) {
                    self.reset(setting, cx);
                }
            }
            "reload" => {
                cx.defer(|cx| {
                    let _ = super::reload(cx);
                });
            }
            "open" => cx.defer(super::open_config_file),
            "reveal" => cx.reveal_path(&self.path),
            _ => {}
        }
        cx.notify();
    }

    /// The theme the picker marks: a pending choice, else the file's.
    fn selected_theme(&self) -> String {
        let name = match self.pending.get("theme") {
            Some(pending) => pending.as_ref(),
            None => self.file.get("theme.name"),
        };
        name.and_then(Value::as_str)
            .unwrap_or("huterm-dark")
            .to_owned()
    }

    /// The first control a setting's row renders, for search to focus.
    fn focus_target(
        &mut self,
        setting: &'static Setting,
        cx: &mut Context<'_, Self>,
    ) -> Option<FocusHandle> {
        let path = setting.path.as_str();
        if let Some(field) = self.fields.get(path) {
            return Some(field.read(cx).focus_handle(cx));
        }
        let key = match special(path) {
            Some(Special::ThemePicker) => {
                // Before the scan lists it, or for a missing theme, the
                // first card takes focus.
                let selected = self.selected_theme();
                let name =
                    if self.themes.iter().any(|(name, _)| *name == selected) {
                        selected
                    } else {
                        self.themes.first().map(|(name, _)| name.clone())?
                    };
                format!("theme:{name}")
            }
            Some(Special::Modifiers) => format!(
                "chip:{path}:{}",
                if cfg!(target_os = "macos") {
                    "cmd"
                } else {
                    "ctrl"
                }
            ),
            Some(Special::Keybindings) => format!("open:{path}"),
            Some(Special::GlobalShortcuts | Special::QuakeProfiles) => {
                "qgo".to_owned()
            }
            // Fields exist once their row renders.
            Some(Special::FontPicker | Special::Size) => return None,
            None if matches!(
                setting.kind,
                SettingKind::Integer { .. }
                    | SettingKind::Number { .. }
                    | SettingKind::Text { .. }
            ) =>
            {
                return None;
            }
            None => return Some(self.stop(path, cx)),
        };
        Some(self.action_stop(key, cx))
    }

    fn visible_sections(&self) -> Vec<SectionId> {
        SectionId::ALL
            .into_iter()
            .filter(|section| {
                self.model.sections()[*section as usize]
                    .groups
                    .iter()
                    .flat_map(|group| &group.settings)
                    .any(|setting| self.available(setting))
            })
            .collect()
    }

    /// Whether a setting applies here, or the file sets it anyway.
    fn available(&self, setting: &Setting) -> bool {
        !file_only(&setting.path)
            && (setting
                .platform
                .is_none_or(|platform| platform == current())
                || self.file.is_set(&setting.path))
    }

    /// Whether a row shows: available, its condition holds, and advanced
    /// settings are on for its section or a search is running.
    fn shows(&self, setting: &Setting, searching: bool) -> bool {
        if !self.available(setting) {
            return false;
        }
        if setting.advanced
            && !searching
            && !self.advanced.contains(&setting.section)
        {
            return false;
        }
        setting.visible_when.as_ref().is_none_or(|condition| {
            self.setting_at(&condition.setting).is_some_and(|other| {
                shown(other, &self.file, &self.defaults).as_ref()
                    == Some(&condition.equals)
            })
        })
    }

    fn matches(setting: &Setting, query: &str) -> bool {
        query.is_empty()
            || setting.label.to_lowercase().contains(query)
            || setting.path.to_lowercase().contains(query)
            || setting
                .description
                .as_deref()
                .is_some_and(|text| text.to_lowercase().contains(query))
    }

    fn select_section(
        &mut self,
        section: SectionId,
        cx: &mut Context<'_, Self>,
    ) {
        self.section = section;
        self.search.update(cx, |search, cx| search.set_text("", cx));
        self.scroll.set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    fn step_section(&mut self, forward: bool, cx: &mut Context<'_, Self>) {
        let sections = self.visible_sections();
        let Some(index) =
            sections.iter().position(|section| *section == self.section)
        else {
            return;
        };
        let next = if forward {
            (index + 1) % sections.len()
        } else {
            (index + sections.len() - 1) % sections.len()
        };
        if let Some(section) = sections.get(next) {
            self.select_section(*section, cx);
        }
    }

    /// Steps the focused control: a switch on or off, a choice to its
    /// neighbour, a number by one unit. A number field steps from its
    /// typed value.
    fn step(
        &mut self,
        forward: bool,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(setting) = self.focused_setting(window, cx) else {
            return;
        };
        self.step_setting(setting, forward, cx);
    }

    fn step_setting(
        &mut self,
        setting: &'static Setting,
        forward: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let field = self.fields.get(setting.path.as_str()).cloned();
        let typed = field
            .as_ref()
            .map(|field| field.read(cx).text().to_owned())
            .and_then(|text| parse_number(setting, &text).ok().flatten());
        let current = typed.or_else(|| self.value(setting));
        let Some(value) = stepped(setting, current.clone(), forward) else {
            return;
        };
        if Some(&value) == current.as_ref() {
            return;
        }
        match field {
            Some(field) => {
                field.update(cx, |field, cx| {
                    field.set_text(field_text(Some(value.clone())), cx);
                });
                self.commit_field(setting, cx);
            }
            None => self.write(setting, Some(value), cx),
        }
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(setting) = self.focused_setting(window, cx) else {
            return;
        };
        match &setting.kind {
            SettingKind::Bool => {
                let on = self
                    .value(setting)
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                self.write(setting, Some(Value::Boolean(!on)), cx);
            }
            SettingKind::Choice(_) => self.open_select(setting, window, cx),
            _ => {}
        }
    }

    fn open_select(
        &mut self,
        setting: &'static Setting,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let SettingKind::Choice(choices) = &setting.kind else {
            return;
        };
        let mut rows: Vec<MenuRow> = choices
            .iter()
            .map(|choice| {
                MenuRow::Item(MenuItem::new(
                    choice.value.as_str(),
                    option_label(choice),
                ))
            })
            .collect();
        // A value the menu does not offer stays one to pick.
        if let Some(Value::String(value)) = self.value(setting)
            && !choices.iter().any(|choice| choice.value == value)
        {
            let value = keybindings::interned(&value);
            rows.push(MenuRow::Item(MenuItem::new(value, value)));
        }
        let opener = self.stop(setting.path.as_str(), cx);
        self.show_menu(
            MenuFor::Setting(setting),
            MenuModel::new(rows),
            opener,
            window,
            cx,
        );
    }

    /// Opens a menu of the installed font families for `setting`.
    fn open_fonts(
        &mut self,
        setting: &'static Setting,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let rows = installed_fonts(cx)
            .iter()
            .map(|name| MenuRow::Item(MenuItem::new(name, *name)))
            .collect();
        let opener = self.action_stop(format!("fonts:{}", setting.path), cx);
        self.show_menu(
            MenuFor::Setting(setting),
            MenuModel::new(rows),
            opener,
            window,
            cx,
        );
    }

    fn show_menu(
        &mut self,
        target: MenuFor,
        model: MenuModel,
        opener: FocusHandle,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let swatch = Self::swatch(cx);
        let menu = cx.new(|cx| MenuView::new(model, swatch, true, cx));
        cx.subscribe_in(&menu, window, move |view, _, event, window, cx| {
            if let MenuEvent::Picked(value) = event {
                match view.menu.as_ref().map(|(_, target, _)| target) {
                    Some(MenuFor::Setting(setting)) => {
                        let setting = *setting;
                        view.write(
                            setting,
                            Some(Value::String((*value).to_owned())),
                            cx,
                        );
                    }
                    Some(MenuFor::Argument { row, name, kind }) => {
                        let (row, name, kind) = (row.clone(), *name, *kind);
                        view.pick_argument(&row, name, kind, value, cx);
                    }
                    Some(MenuFor::Profile) => view.pick_profile(value),
                    Some(MenuFor::Global { entry, field }) => {
                        let (entry, field) = (*entry, *field);
                        view.pick_global(entry, field, value, cx);
                    }
                    None => {}
                }
            }
            view.close_menu(window, cx);
        })
        .detach();
        menu.read(cx).focus_handle(cx).focus(window, cx);
        self.menu = Some((menu, target, opener));
        cx.notify();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some((_, _, opener)) = self.menu.take() {
            opener.focus(window, cx);
            cx.notify();
        }
    }

    /// Runs one of the window's commands.
    fn run(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<(), CommandError> {
        if let Some((menu, _, _)) = &self.menu
            && super::is_menu_command(invocation.id)
        {
            let menu = menu.clone();
            menu.update(cx, |menu, cx| match invocation.id {
                ids::MENU_SELECT_NEXT => menu.select_next(cx),
                ids::MENU_SELECT_PREVIOUS => menu.select_previous(cx),
                ids::MENU_SELECT_FIRST => menu.select_first(cx),
                ids::MENU_SELECT_LAST => menu.select_last(cx),
                ids::MENU_CONFIRM => menu.confirm(cx),
                _ => MenuView::dismiss(cx),
            });
            return Ok(());
        }
        match invocation.id {
            // In Keybindings, search finds commands in the section.
            ids::SETTINGS_SEARCH
                if self.section == SectionId::Keybindings
                    && self.query(cx).is_empty() =>
            {
                let handle = self.keys.search.read(cx).focus_handle(cx);
                handle.focus(window, cx);
            }
            ids::SETTINGS_SEARCH => {
                let handle = self.search.read(cx).focus_handle(cx);
                handle.focus(window, cx);
            }
            // The window's tabs are its sections.
            ids::NEXT_TAB => self.step_section(true, cx),
            ids::PREVIOUS_TAB => self.step_section(false, cx),
            ids::SETTINGS_FOCUS_NEXT => window.focus_next(cx),
            ids::SETTINGS_FOCUS_PREVIOUS => window.focus_prev(cx),
            ids::SETTINGS_TOGGLE => {
                if let Some(key) = self.focused_action(window) {
                    self.act(&key, window, cx);
                } else if let Some(section) = self
                    .sections
                    .iter()
                    .find(|(_, handle)| handle.is_focused(window))
                    .map(|(section, _)| *section)
                {
                    self.select_section(section, cx);
                } else {
                    self.toggle(window, cx);
                }
            }
            ids::SETTINGS_INCREASE => self.step(true, window, cx),
            ids::SETTINGS_DECREASE => self.step(false, window, cx),
            ids::SETTINGS_RESET => {
                if let Some(command) = self
                    .focused_action(window)
                    .as_deref()
                    .and_then(|key| key.strip_prefix("kcmd:"))
                    .and_then(huterm_protocol::lookup)
                {
                    self.restore_defaults(command.id, cx);
                } else if let Some(setting) = self.focused_setting(window, cx) {
                    self.reset(setting, cx);
                }
            }
            ids::CLOSE_WINDOW | ids::CLOSE_TAB | ids::CLOSE_PANE => {
                if self.request_close(window, cx) {
                    window.remove_window();
                }
            }
            ids::MINIMIZE => window.minimize_window(),
            ids::ZOOM => window.zoom_window(),
            ids::TOGGLE_FULLSCREEN | ids::TOGGLE_NATIVE_FULLSCREEN => {
                window.toggle_fullscreen();
            }
            other => {
                return Err(CommandError::Unavailable(format!(
                    "`{other}` is not available in Settings"
                )));
            }
        }
        cx.notify();
        Ok(())
    }

    /// Text editing reaches the focused field; Enter commits it, Tab moves
    /// on, and Escape clears the search.
    fn run_palette(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some((key, field)) = self.focused_key_field(window, cx) {
            self.run_key_field(&key, &field, invocation, window, cx);
            cx.notify();
            return;
        }
        if let Some(field) = self.focused_name_field(window, cx) {
            match invocation.id {
                ids::PALETTE_CONFIRM => self.create_profile(window, cx),
                ids::PALETTE_BACK => self.act("qcancel", window, cx),
                ids::PALETTE_EXPAND => window.focus_next(cx),
                ids::PALETTE_PREVIOUS_SLOT => window.focus_prev(cx),
                _ => {
                    let _ = field.update(cx, |field, cx| {
                        field.run(invocation, window, cx)
                    });
                }
            }
            cx.notify();
            return;
        }
        let search = self.search.clone();
        let focused = self
            .fields
            .iter()
            .find(|(_, field)| {
                field.read(cx).focus_handle(cx).is_focused(window)
            })
            .map(|(path, field)| (*path, field.clone()));
        let field = match &focused {
            Some((_, field)) => field.clone(),
            None if search.read(cx).focus_handle(cx).is_focused(window) => {
                search.clone()
            }
            None => return,
        };
        match invocation.id {
            ids::PALETTE_CONFIRM => {
                if let Some((path, _)) = focused
                    && let Some(setting) = self.setting_at(path)
                {
                    self.commit_field(setting, cx);
                } else {
                    self.focus_first_match(window, cx);
                }
            }
            // Up and Down step a number field's value.
            ids::PALETTE_SELECT_PREVIOUS | ids::PALETTE_SELECT_NEXT
                if focused.is_some() =>
            {
                self.step(
                    invocation.id == ids::PALETTE_SELECT_PREVIOUS,
                    window,
                    cx,
                );
            }
            ids::PALETTE_EXPAND => window.focus_next(cx),
            ids::PALETTE_PREVIOUS_SLOT => window.focus_prev(cx),
            ids::PALETTE_BACK => {
                match focused {
                    // Escape in a field cancels its typing.
                    Some((path, _)) => self.cancel_field(path, cx),
                    None => {
                        search.update(cx, |search, cx| search.set_text("", cx));
                    }
                }
                self.focus.focus(window, cx);
            }
            _ => {
                let _ = field
                    .update(cx, |field, cx| field.run(invocation, window, cx));
            }
        }
        cx.notify();
    }
}

impl SettingsView {
    /// Text editing in a keybinding field: completion takes Up, Down,
    /// Enter, Tab, and Escape while its list is open; otherwise Enter saves
    /// and Escape puts the saved text back.
    fn run_key_field(
        &mut self,
        key: &str,
        field: &Entity<TextField>,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let completing = self
            .keys
            .completion
            .as_ref()
            .is_some_and(|(open, _)| open == key);
        match invocation.id {
            ids::PALETTE_SELECT_NEXT | ids::PALETTE_SELECT_PREVIOUS
                if completing =>
            {
                self.step_completion(
                    key,
                    invocation.id == ids::PALETTE_SELECT_NEXT,
                    cx,
                );
            }
            // Up and Down step a number argument.
            ids::PALETTE_SELECT_NEXT | ids::PALETTE_SELECT_PREVIOUS
                if key.starts_with("arg:") =>
            {
                self.step_argument(
                    key,
                    invocation.id == ids::PALETTE_SELECT_PREVIOUS,
                    cx,
                );
            }
            ids::PALETTE_CONFIRM
                if completing && self.accept_completion(key, None, cx) => {}
            ids::PALETTE_CONFIRM if key == "search" => {
                self.focus_first_command(window, cx);
            }
            ids::PALETTE_CONFIRM => self.commit_key_field(key, cx),
            // Tab and Escape close an open suggestion list first, keeping
            // the text; a suggestion takes Return or a click.
            ids::PALETTE_EXPAND | ids::PALETTE_BACK if completing => {
                self.keys.completion = None;
            }
            ids::PALETTE_EXPAND => window.focus_next(cx),
            ids::PALETTE_PREVIOUS_SLOT => window.focus_prev(cx),
            ids::PALETTE_BACK if key == "search" => {
                field.update(cx, |field, cx| field.set_text("", cx));
            }
            // Escape puts the saved text back and stays in the field.
            ids::PALETTE_BACK => self.cancel_key_field(key, cx),
            _ => {
                let _ = field
                    .update(cx, |field, cx| field.run(invocation, window, cx));
            }
        }
    }

    /// Moves focus from the search field to the first matching control.
    fn focus_first_match(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let query = self.query(cx);
        if query.is_empty() {
            return;
        }
        // Results follow section order; Keybindings lists commands.
        for section in self.visible_sections() {
            let target = if section == SectionId::Keybindings {
                self.matching_commands(&query).first().map(|spec| {
                    self.action_stop(format!("kgo:{}", spec.id.as_str()), cx)
                })
            } else {
                let first = self.model.sections()[section as usize]
                    .groups
                    .iter()
                    .flat_map(|group| &group.settings)
                    .find(|setting| {
                        self.shows(setting, true)
                            && Self::matches(setting, &query)
                    });
                match first {
                    Some(setting) => self.focus_target(setting, cx),
                    None => continue,
                }
            };
            if let Some(target) = target {
                target.focus(window, cx);
                return;
            }
        }
    }

    /// Smoke state: the section, the search, what has focus, and the file
    /// status, as `settings.` fields on one line.
    pub(super) fn smoke_state(&self, window: &Window, cx: &App) -> String {
        let focus = if self.menu.is_some() {
            "menu".to_owned()
        } else if self.search.read(cx).focus_handle(cx).is_focused(window) {
            "search".to_owned()
        } else if let Some(section) = self
            .sections
            .iter()
            .find(|(_, handle)| handle.is_focused(window))
            .map(|(section, _)| *section)
        {
            format!("section:{}", section.id())
        } else if let Some(key) = self.focused_action(window) {
            format!("action:{key}")
        } else if let Some((key, _)) = self.focused_key_field(window, cx) {
            format!("key:{key}")
        } else if let Some(setting) = self.focused_setting(window, cx) {
            setting.path.clone()
        } else {
            "none".to_owned()
        };
        format!(
            "settings.open=true settings.active={} settings.section={} settings.query={:?} settings.focus={focus} settings.status={:?} settings.read_only={} settings.writing={}",
            window.is_window_active(),
            self.section.id(),
            self.query(cx),
            self.status.line().text,
            self.locked(),
            self.busy(),
        )
    }
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Every selectable theme with its preview colors.
fn themes(config: &std::path::Path) -> Vec<(String, Option<Theme>)> {
    let directory = config
        .parent()
        .map_or_else(PathBuf::new, |parent| parent.join("themes"));
    crate::themes::names(Some(&directory))
        .into_iter()
        .map(|name| {
            let preview = crate::themes::preview(&name, &directory);
            (name, preview)
        })
        .collect()
}

/// The installed font families, read once. The menu names its items with
/// static strings.
fn installed_fonts(cx: &App) -> &'static [&'static str] {
    static FONTS: OnceLock<Vec<&'static str>> = OnceLock::new();
    FONTS.get_or_init(|| {
        cx.text_system()
            .all_font_names()
            .into_iter()
            .filter(|name| !name.starts_with('.'))
            .map(|name| &*Box::leak(name.into_boxed_str()))
            .collect()
    })
}

/// A size field's value: a byte count when it is only digits, else the
/// text, which the validator checks; empty resets.
fn size_value(text: &str) -> Option<Value> {
    let text = text.trim();
    if text.is_empty() {
        None
    } else if let Ok(bytes) = text.parse::<i64>() {
        Some(Value::Integer(bytes))
    } else {
        Some(Value::String(text.to_owned()))
    }
}

/// The value one step from `current`: a switch on or off, a choice's
/// neighbour, or a number one unit up or down. A `Number` written in
/// integer form steps as a float.
fn stepped(
    setting: &Setting,
    current: Option<Value>,
    forward: bool,
) -> Option<Value> {
    let delta: i64 = if forward { 1 } else { -1 };
    match (&setting.kind, current) {
        (SettingKind::Bool, _) => Some(Value::Boolean(forward)),
        (SettingKind::Choice(choices), current) => {
            let index = choices.iter().position(|choice| {
                current.as_ref().and_then(Value::as_str)
                    == Some(choice.value.as_str())
            });
            let next = match (index, forward) {
                (Some(index), true) => (index + 1).min(choices.len() - 1),
                (Some(index), false) => index.saturating_sub(1),
                (None, _) => 0,
            };
            choices
                .get(next)
                .map(|choice| Value::String(choice.value.clone()))
        }
        (SettingKind::Integer { .. }, Some(Value::Integer(value))) => {
            Some(Value::Integer(value + delta))
        }
        (SettingKind::Number { .. }, Some(Value::Float(value))) => {
            Some(Value::Float(value + if forward { 1.0 } else { -1.0 }))
        }
        (SettingKind::Number { .. }, Some(Value::Integer(value))) => {
            Some(Value::Float(number_from(value + delta)))
        }
        _ => None,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "setting values are far below 2^52"
)]
fn number_from(value: i64) -> f64 {
    value as f64
}

/// Link modifiers with `modifier` switched, or `None` when that would leave
/// none.
fn toggled_modifiers(current: Option<Value>, modifier: &str) -> Option<String> {
    let current = match current {
        Some(Value::String(text)) => text,
        _ if cfg!(target_os = "macos") => "cmd".to_owned(),
        _ => "ctrl".to_owned(),
    };
    let held: Vec<&str> = current.split('-').collect();
    let on = held.contains(&modifier);
    let next: Vec<&str> = ["cmd", "ctrl", "alt", "shift"]
        .into_iter()
        .filter(|token| {
            if *token == modifier {
                !on
            } else {
                held.contains(token)
            }
        })
        .collect();
    (!next.is_empty()).then(|| next.join("-"))
}

fn current() -> SettingPlatform {
    if cfg!(target_os = "macos") {
        SettingPlatform::Macos
    } else {
        SettingPlatform::Linux
    }
}

fn text_for(field: &TextField) -> String {
    field.text().to_owned()
}

fn field_text(value: Option<Value>) -> String {
    match value {
        Some(Value::Float(value)) => number(value),
        Some(Value::Integer(value)) => value.to_string(),
        Some(Value::String(value)) => value,
        _ => String::new(),
    }
}

fn option_label(choice: &Choice) -> String {
    choice.label.clone().unwrap_or_else(|| choice.value.clone())
}

fn apply_note(apply: Apply) -> Option<&'static str> {
    match apply {
        Apply::Live => None,
        Apply::NewTabsAndPanes => Some("Applies to new tabs and panes"),
        Apply::NewTerminals => Some("Applies to new terminals"),
        Apply::LaterEvents => Some("Applies from the next time it is used"),
        Apply::NextPaletteOpen => {
            Some("Applies the next time the palette opens")
        }
    }
}

fn tone_color(tone: Tone, swatch: Swatch) -> Hsla {
    match tone {
        Tone::Ok => swatch.accent,
        Tone::Idle => swatch.dim,
        Tone::Warning => swatch.warning,
        Tone::Error => swatch.danger,
    }
}

fn button(id: impl Into<SharedString>, swatch: Swatch) -> Stateful<gpui::Div> {
    div()
        .id(gpui::ElementId::Name(id.into()))
        .flex()
        .flex_none()
        .items_center()
        .h(px(24.0))
        .px(px(10.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(swatch.line_strong)
        .text_size(px(12.0))
        .text_color(swatch.fg)
        .hover(|style| style.bg(swatch.hover))
        .active(|style| style.bg(swatch.pressed()))
}

fn link(id: impl Into<SharedString>, swatch: Swatch) -> Stateful<gpui::Div> {
    div()
        .id(gpui::ElementId::Name(id.into()))
        .flex_none()
        .text_size(px(11.5))
        .text_color(swatch.accent)
        .hover(gpui::Styled::underline)
}

fn modifier_label(modifier: &str) -> &'static str {
    match (modifier, cfg!(target_os = "macos")) {
        ("cmd", true) => "⌘ Cmd",
        ("alt", true) => "⌥ Option",
        ("shift", true) => "⇧ Shift",
        ("ctrl", _) => "Ctrl",
        ("alt", false) => "Alt",
        ("shift", false) => "Shift",
        _ => "Cmd",
    }
}

impl Render for SettingsView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let swatch = Self::swatch(cx);
        let query = self.query(cx);
        let searching = !query.is_empty();
        self.drawn.clear();
        let sidebar = self.render_sidebar(swatch, searching, cx);
        let main = self.render_main(swatch, &query, window, cx);
        // A followed control, or focus stranded on a control this frame did
        // not draw, moves once the frame is done; a stranded focus would
        // leave no binding able to match.
        // Focus held across a frame by something the last frame did not
        // draw inside this view, such as a field whose row went away.
        let focused = window.focused(cx);
        let orphaned = focused.is_some()
            && focused == self.last_focus
            && !self.focus.contains_focused(window, cx);
        self.last_focus = focused;
        let stranded = self
            .focused_action(window)
            .or_else(|| self.focused_key_field(window, cx).map(|(key, _)| key))
            .is_some_and(|key| !self.drawn.contains(&key))
            || std::mem::take(&mut self.keys.stranded)
            || orphaned;
        if self.keys.follow.is_some() || stranded {
            cx.defer_in(window, move |view, window, cx| {
                if view.keys.follow.is_some() {
                    view.focus_follow(window, cx);
                } else if stranded {
                    view.focus.focus(window, cx);
                }
                cx.notify();
            });
        }
        let mut context = KeyContext::default();
        context.add("Settings");
        div()
            .key_context(context)
            .track_focus(&self.focus)
            .on_action(cx.listener(
                |view, action: &InvokeWindow, window, cx| {
                    // A command for terminal windows has nothing to act on
                    // here; it is not a failed save.
                    let _ = view.run(&action.0, window, cx);
                },
            ))
            .on_action(cx.listener(
                |view, action: &InvokePalette, window, cx| {
                    cx.stop_propagation();
                    view.run_palette(&action.0, window, cx);
                },
            ))
            .flex()
            .size_full()
            .bg(swatch.bg)
            .text_color(swatch.fg)
            .child(sidebar)
            .child(main)
    }
}

/// What desktop tests drive and observe in the window.
#[cfg(all(test, feature = "test-support"))]
impl SettingsView {
    pub(super) fn command(
        &mut self,
        id: huterm_protocol::CommandId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<(), CommandError> {
        self.run(&CommandInvocation::new(id, Vec::new()), window, cx)
    }

    /// Whether the first read finished and no write is in flight or queued.
    pub(super) fn settled(&self) -> bool {
        self.base.is_some() && !self.busy() && self.loading == 0
    }

    pub(super) fn selected(&self) -> String {
        self.selected_theme()
    }

    /// What has keyboard focus, as smoke state names it.
    pub(super) fn focus_name(&self, window: &Window, cx: &App) -> String {
        self.smoke_state(window, cx)
            .split(' ')
            .find_map(|field| field.strip_prefix("settings.focus="))
            .unwrap_or_default()
            .to_owned()
    }

    /// Gives the setting's field keyboard focus.
    pub(super) fn focus_field(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let field = self.fields.get(path).expect("a rendered field").clone();
        field.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// Dismisses an open menu, as Escape in it does.
    pub(super) fn dismiss_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.close_menu(window, cx);
    }

    /// Steps the setting as an arrow key on its control does.
    pub(super) fn step_once(
        &mut self,
        path: &str,
        forward: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let setting = self.setting_at(path).expect("a known setting");
        self.step_setting(setting, forward, cx);
    }

    /// Runs the control an action key names, as Space on it does.
    pub(super) fn press(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.action_stop(key.to_owned(), cx).focus(window, cx);
        self.command(ids::SETTINGS_TOGGLE, window, cx).unwrap();
    }

    /// Escape in the setting's field.
    pub(super) fn escape(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        let setting = self.setting_at(path).expect("a known setting");
        self.cancel_field(setting.path.as_str(), cx);
    }

    /// Follows a reload that failed with `message`.
    pub(super) fn follow_failed_reload(
        &mut self,
        message: &str,
        cx: &mut Context<'_, Self>,
    ) {
        self.reloaded(Err(message.to_owned()), cx);
    }

    pub(super) fn status_text(&self) -> String {
        self.status.line().text
    }

    pub(super) fn read_only(&self) -> bool {
        self.locked()
    }

    pub(super) fn conflicted(&self, path: &str) -> bool {
        self.conflicts.contains_key(path)
    }

    /// Saves `value` for the setting at `path`, or resets it when `None`.
    pub(super) fn set(
        &mut self,
        path: &str,
        value: Option<Value>,
        cx: &mut Context<'_, Self>,
    ) {
        let setting = self.setting_at(path).expect("a known setting");
        match value {
            Some(value) => self.write(setting, Some(value), cx),
            None => self.reset(setting, cx),
        }
    }

    /// Types `text` into the setting's field without saving it.
    pub(super) fn type_into(
        &mut self,
        path: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let setting = self.setting_at(path).expect("a known setting");
        let field = self.field(setting, window, cx);
        field.update(cx, |field, cx| field.set_text(text, cx));
    }

    pub(super) fn field_value(&self, path: &str, cx: &App) -> String {
        self.fields
            .get(path)
            .map(|field| field.read(cx).text().to_owned())
            .unwrap_or_default()
    }

    /// Saves the field as Enter does.
    pub(super) fn enter(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        let setting = self.setting_at(path).expect("a known setting");
        self.commit_field(setting, cx);
    }

    /// Moves focus out of the field, as clicking elsewhere does.
    pub(super) fn blur(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        let setting = self.setting_at(path).expect("a known setting");
        self.field_blurred(setting, cx);
    }

    pub(super) fn use_file(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        let setting = self.setting_at(path).expect("a known setting");
        self.keep_file(setting.path.as_str(), cx);
    }

    /// Follows a reload that applied the file.
    pub(super) fn follow_reload(&mut self, cx: &mut Context<'_, Self>) {
        self.reloaded(Ok(()), cx);
    }

    /// Looks for an edit made elsewhere, as coming forward does.
    pub(super) fn look_at_disk(&mut self, cx: &mut Context<'_, Self>) {
        self.check_disk(cx);
    }

    /// The row showing `command`'s binding on `key`.
    pub(super) fn key_row(
        &self,
        command: huterm_protocol::CommandId,
        key: &str,
    ) -> Option<String> {
        self.keys.row_named(command, key)
    }

    /// Types into a keybinding field, creating it as its row would.
    pub(super) fn type_key(
        &mut self,
        key: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let field = self.key_field_for(key, window, cx);
        field.update(cx, |field, cx| field.set_text(text, cx));
    }

    pub(super) fn enter_key(&mut self, key: &str, cx: &mut Context<'_, Self>) {
        self.commit_key_field(key, cx);
    }

    pub(super) fn key_error(&self, row: &str) -> Option<String> {
        self.keys.row_error(row)
    }

    /// Focuses a keybinding field, creating it as its row would.
    pub(super) fn focus_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let field = self.key_field_for(key, window, cx);
        field.read(cx).focus_handle(cx).focus(window, cx);
        // The test window does not deliver the focus event that marks it.
        self.keys.editing = Some(key.to_owned());
    }

    /// Leaves a keybinding field, as moving focus away does.
    pub(super) fn blur_key(&mut self, key: &str, cx: &mut Context<'_, Self>) {
        self.keys.editing = None;
        self.commit_key_field(key, cx);
    }

    pub(super) fn cancel_key(&mut self, key: &str, cx: &mut Context<'_, Self>) {
        self.cancel_key_field(key, cx);
    }

    pub(super) fn step_key(
        &mut self,
        key: &str,
        up: bool,
        cx: &mut Context<'_, Self>,
    ) {
        self.step_argument(key, up, cx);
    }

    pub(super) fn pick_key_argument(
        &mut self,
        row: &str,
        name: &'static str,
        kind: ArgumentKind,
        value: &str,
        cx: &mut Context<'_, Self>,
    ) {
        self.pick_argument(row, name, kind, value, cx);
    }

    pub(super) fn completion_open(&self) -> bool {
        self.keys.completion.is_some()
    }

    pub(super) fn key_text(&self, key: &str, cx: &App) -> String {
        self.keys
            .fields
            .get(key)
            .map(|field| field.read(cx).text().to_owned())
            .unwrap_or_default()
    }

    /// The strokes recorded so far, while recording.
    pub(super) fn recording(&self) -> Option<Vec<String>> {
        self.keys
            .recording
            .as_ref()
            .map(|recording| recording.strokes.clone())
    }

    pub(super) fn use_mine(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        let setting = self.setting_at(path).expect("a known setting");
        self.keep_mine(setting.path.as_str(), cx);
    }
}
