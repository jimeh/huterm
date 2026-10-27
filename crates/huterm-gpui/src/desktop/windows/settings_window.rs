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
use huterm_config::document::{ApplyError, ConfigFile, Patch, Snapshot};
use huterm_config::settings::{
    Apply, Choice, Platform as SettingPlatform, SectionId, Setting,
    SettingKind, SettingsModel,
};
use huterm_protocol::{CommandError, CommandInvocation, ids};
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

/// Closes the Settings window, as Quit does.
pub(super) fn close(cx: &mut App) {
    if let Some(handle) = cx.global_mut::<Desktop>().settings.take() {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

/// A change the file changed under, kept until the user picks theirs or
/// the file's. `patch` is the refused write; a field draft that a reload
/// changed under has none, and saving it again writes the draft.
struct Conflict {
    patch: Option<Patch>,
    message: String,
}

mod controls;
mod render;

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
    /// Reset buttons, which the keyboard reaches too.
    resets: HashMap<&'static str, FocusHandle>,
    /// Focus handles for every other control, by stop.
    stops: HashMap<&'static str, FocusHandle>,
    sections: HashMap<SectionId, FocusHandle>,
    errors: HashMap<&'static str, String>,
    conflicts: HashMap<&'static str, Conflict>,
    writing: bool,
    queued: VecDeque<(&'static Setting, Patch)>,
    /// An open select's menu and the setting it edits.
    menu: Option<(Entity<MenuView>, &'static Setting)>,
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
        let mut view = Self {
            focus: cx.focus_handle(),
            model,
            themes: themes(&path),
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
            resets: HashMap::new(),
            stops: HashMap::new(),
            sections: HashMap::new(),
            errors: HashMap::new(),
            conflicts: HashMap::new(),
            writing: false,
            queued: VecDeque::new(),
            menu: None,
            scroll: ScrollHandle::new(),
        };
        view.load(cx);
        // With watching off nothing reports an edit made elsewhere, so the
        // window looks when it comes forward.
        cx.observe_window_activation(window, |view, window, cx| {
            if window.is_window_active() {
                view.check_disk(cx);
            }
        })
        .detach();
        view
    }

    fn swatch(cx: &App) -> Swatch {
        Swatch::from_theme(&cx.global::<Desktop>().config.theme)
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).text().trim().to_lowercase()
    }

    /// Rereads the file on the background executor.
    fn load(&mut self, cx: &mut Context<'_, Self>) {
        let file = ConfigFile::new(self.path.clone());
        let task = cx.background_spawn(async move { file.load() });
        cx.spawn(async move |view, cx| {
            let loaded = task.await;
            let _ = view.update(cx, |view, cx| view.loaded(loaded, cx));
        })
        .detach();
    }

    fn loaded(
        &mut self,
        loaded: Result<Snapshot, ApplyError>,
        cx: &mut Context<'_, Self>,
    ) {
        match loaded {
            Ok(snapshot) => {
                match FileValues::parse(snapshot.text()) {
                    Ok(file) => self.file = file,
                    Err(message) => self.status.invalid_file(message),
                }
                self.base = Some(snapshot);
                self.sync_fields(cx);
            }
            Err(error) => self.status.failed(error.to_string()),
        }
        cx.notify();
    }

    /// Reports a file that changed since it was read while watching is
    /// off. The controls keep showing what was read, as the running
    /// settings do, until a reload.
    fn check_disk(&mut self, cx: &mut Context<'_, Self>) {
        if cx.global::<Desktop>().config.general.watch_config {
            return;
        }
        let Some(base) = self.base.clone() else {
            return;
        };
        let file = ConfigFile::new(self.path.clone());
        let task = cx.background_spawn(async move { file.load() });
        cx.spawn(async move |view, cx| {
            if let Ok(current) = task.await
                && current.revision() != base.revision()
            {
                let _ = view.update(cx, |view, cx| {
                    view.status.changed_on_disk();
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Follows a reload: the file is reread and every control follows it.
    fn reloaded(
        &mut self,
        outcome: Result<(), String>,
        cx: &mut Context<'_, Self>,
    ) {
        self.status
            .watching(cx.global::<Desktop>().config.general.watch_config);
        match outcome {
            Ok(()) => self.status.reloaded(),
            Err(message) => self.status.invalid_file(message),
        }
        self.load(cx);
    }

    /// Puts each field's text back to what the file and defaults show. A
    /// field holding an unsaved draft keeps it; when the file changed that
    /// setting meanwhile, the row shows a conflict instead.
    fn sync_fields(&mut self, cx: &mut Context<'_, Self>) {
        let paths: Vec<&'static str> = self.fields.keys().copied().collect();
        for path in paths {
            if self.conflicts.contains_key(path)
                || self.errors.contains_key(path)
            {
                continue;
            }
            self.sync_field(path, false, cx);
        }
    }

    /// Shows the file's value in `path`'s field, replacing a draft only when
    /// `discard` is set.
    fn sync_field(
        &mut self,
        path: &'static str,
        discard: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let (Some(field), Some(setting)) =
            (self.fields.get(path).cloned(), self.model.setting(path))
        else {
            return;
        };
        let text = field_text(shown(setting, &self.file, &self.defaults));
        let current = field.read(cx).text().to_owned();
        let draft = self
            .synced
            .get(path)
            .is_some_and(|synced| *synced != current);
        if draft && !discard && current != text {
            if self.synced.get(path) != Some(&text) {
                self.conflicts.insert(
                    path,
                    Conflict {
                        patch: None,
                        message: format!(
                            "config.toml changed this to {} while you were editing",
                            if text.is_empty() { "its default" } else { &text }
                        ),
                    },
                );
            }
            return;
        }
        if current != text {
            field.update(cx, |field, cx| field.set_text(text.clone(), cx));
        }
        self.synced.insert(path, text);
    }

    /// Saves `value` for `setting`, or resets it when `None`.
    fn write(
        &mut self,
        setting: &'static Setting,
        value: Option<Value>,
        cx: &mut Context<'_, Self>,
    ) {
        if self.status.invalid() {
            return;
        }
        // A size's forms are checked by the validator.
        if let Some(value) = &value
            && special(&setting.path) != Some(Special::Size)
            && let Err(message) = values::check(setting, value)
        {
            self.errors.insert(setting.path.as_str(), message);
            cx.notify();
            return;
        }
        self.submit(setting, values::patch(setting, value), cx);
    }

    /// Writes `patch` for `setting`, after any write in flight.
    fn submit(
        &mut self,
        setting: &'static Setting,
        patch: Patch,
        cx: &mut Context<'_, Self>,
    ) {
        if self.status.invalid() {
            return;
        }
        if self.writing {
            self.queued.push_back((setting, patch));
            return;
        }
        self.writing = true;
        self.status.saving();
        let file = ConfigFile::new(self.path.clone());
        let base = self.base.clone();
        let sent = patch.clone();
        let validate = crate::config::validator(self.path.clone());
        let task = cx.background_spawn(async move {
            let base = match base {
                Some(base) => base,
                None => file.load()?,
            };
            file.apply(&base, &[sent], &validate)
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |view, cx| {
                view.written(setting, patch, result, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn written(
        &mut self,
        setting: &'static Setting,
        patch: Patch,
        result: Result<Snapshot, ApplyError>,
        cx: &mut Context<'_, Self>,
    ) {
        self.writing = false;
        let key = setting.path.as_str();
        match result {
            Ok(snapshot) => {
                self.errors.remove(key);
                self.conflicts.remove(key);
                if let Ok(file) = FileValues::parse(snapshot.text()) {
                    self.file = file;
                }
                self.base = Some(snapshot);
                self.status.saved();
                self.sync_fields(cx);
                // Watching may be off, so the save asks for the reload
                // itself; a watcher that also saw it coalesces.
                cx.defer(|cx| {
                    let _ = super::reload(cx);
                });
            }
            Err(ApplyError::Conflict(changes)) => {
                self.conflicts.insert(
                    key,
                    Conflict {
                        patch: Some(patch),
                        message: format!(
                            "config.toml changed this setting: {}",
                            changes.join("; ")
                        ),
                    },
                );
                self.status.failed("config.toml changed on disk");
                self.load(cx);
            }
            Err(ApplyError::InvalidFile(message)) => {
                self.status.invalid_file(message);
            }
            Err(ApplyError::WrittenNotDurable(error)) => {
                self.status.failed(error.to_string());
                self.load(cx);
            }
            Err(error) => {
                let message = error.to_string();
                self.errors.insert(key, message.clone());
                self.status.failed(message);
                self.sync_fields(cx);
            }
        }
        if let Some((setting, patch)) = self.queued.pop_front() {
            self.submit(setting, patch, cx);
        }
        cx.notify();
    }

    /// Keeps the entered value: reapplies it against a fresh read.
    fn keep_mine(&mut self, path: &'static str, cx: &mut Context<'_, Self>) {
        let Some(conflict) = self.conflicts.remove(path) else {
            return;
        };
        let Some(setting) = self.model.setting(path) else {
            return;
        };
        self.base = None;
        match conflict.patch {
            Some(refused) => self.submit(setting, refused, cx),
            None => self.commit_field(setting, cx),
        }
    }

    /// Discards the entered value and shows the file's.
    fn keep_file(&mut self, path: &'static str, cx: &mut Context<'_, Self>) {
        self.conflicts.remove(path);
        self.errors.remove(path);
        self.sync_field(path, true, cx);
        cx.notify();
    }

    fn reset(&mut self, setting: &'static Setting, cx: &mut Context<'_, Self>) {
        if self.file.is_set(&setting.path) {
            self.errors.remove(setting.path.as_str());
            self.write(setting, None, cx);
        }
    }

    /// Commits a number or text field's text.
    fn commit_field(
        &mut self,
        setting: &'static Setting,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(field) = self.fields.get(setting.path.as_str()) else {
            return;
        };
        let text = field.read(cx).text().to_owned();
        // Saving a field again after a conflict keeps the entered value.
        if self.conflicts.remove(setting.path.as_str()).is_some() {
            self.base = None;
        }
        let parsed = match &setting.kind {
            _ if special(&setting.path) == Some(Special::Size) => {
                Ok(size_value(&text))
            }
            SettingKind::Integer { .. } | SettingKind::Number { .. } => {
                parse_number(setting, &text)
            }
            _ if text.trim().is_empty() => Ok(None),
            _ => Ok(Some(Value::String(text.trim().to_owned()))),
        };
        match parsed {
            Ok(value) => {
                self.errors.remove(setting.path.as_str());
                let current = shown(setting, &self.file, &self.defaults);
                if value == current && value.is_some() {
                    cx.notify();
                    return;
                }
                match value {
                    None => self.reset(setting, cx),
                    value => self.write(setting, value, cx),
                }
            }
            Err(message) => {
                self.errors.insert(setting.path.as_str(), message);
                cx.notify();
            }
        }
    }

    /// Saves a field when focus leaves it. Leaving a conflicted field, such
    /// as to press Use file, keeps the conflict for the user to settle.
    fn field_blurred(
        &mut self,
        setting: &'static Setting,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.conflicts.contains_key(setting.path.as_str()) {
            self.commit_field(setting, cx);
        }
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

    fn reset_stop(
        &mut self,
        path: &'static str,
        cx: &mut Context<'_, Self>,
    ) -> FocusHandle {
        self.resets
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
        self.model.setting(path)
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
            self.model.setting(&condition.setting).is_some_and(|other| {
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
    /// neighbour, a number by one unit.
    fn step(
        &mut self,
        forward: bool,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(setting) = self.focused_setting(window, cx) else {
            return;
        };
        let current = shown(setting, &self.file, &self.defaults);
        let next = match (&setting.kind, current) {
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
                Some(Value::Integer(if forward {
                    value + 1
                } else {
                    value - 1
                }))
            }
            (SettingKind::Number { .. }, Some(Value::Float(value))) => {
                Some(Value::Float(if forward {
                    value + 1.0
                } else {
                    value - 1.0
                }))
            }
            _ => None,
        };
        if let Some(value) = next
            && Some(&value)
                != shown(setting, &self.file, &self.defaults).as_ref()
        {
            self.write(setting, Some(value), cx);
        }
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(setting) = self.focused_setting(window, cx) else {
            return;
        };
        match &setting.kind {
            SettingKind::Bool => {
                let on = shown(setting, &self.file, &self.defaults)
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
        let rows = choices
            .iter()
            .map(|choice| {
                MenuRow::Item(MenuItem::new(
                    choice.value.as_str(),
                    option_label(choice),
                ))
            })
            .collect();
        self.show_menu(setting, MenuModel::new(rows), window, cx);
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
        self.show_menu(setting, MenuModel::new(rows), window, cx);
    }

    fn show_menu(
        &mut self,
        setting: &'static Setting,
        model: MenuModel,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let swatch = Self::swatch(cx);
        let menu = cx.new(|cx| MenuView::new(model, swatch, true, cx));
        cx.subscribe_in(&menu, window, move |view, _, event, window, cx| {
            if let MenuEvent::Picked(value) = event {
                view.write(
                    setting,
                    Some(Value::String((*value).to_owned())),
                    cx,
                );
            }
            view.close_menu(window, cx);
        })
        .detach();
        menu.read(cx).focus_handle(cx).focus(window, cx);
        self.menu = Some((menu, setting));
        cx.notify();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some((_, setting)) = self.menu.take() {
            let handle = self.stop(setting.path.as_str(), cx);
            handle.focus(window, cx);
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
        if let Some((menu, _)) = &self.menu
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
                if let Some(path) = self
                    .resets
                    .iter()
                    .find(|(_, handle)| handle.is_focused(window))
                    .map(|(path, _)| *path)
                    && let Some(setting) = self.model.setting(path)
                {
                    self.reset(setting, cx);
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
                if let Some(setting) = self.focused_setting(window, cx) {
                    self.reset(setting, cx);
                }
            }
            ids::CLOSE_WINDOW | ids::CLOSE_TAB | ids::CLOSE_PANE => {
                window.remove_window();
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
                    && let Some(setting) = self.model.setting(path)
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
                if focused.is_none() {
                    search.update(cx, |search, cx| search.set_text("", cx));
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
        let first = self
            .visible_sections()
            .into_iter()
            .flat_map(|section| &self.model.sections()[section as usize].groups)
            .flat_map(|group| &group.settings)
            .find(|setting| {
                self.shows(setting, true) && Self::matches(setting, &query)
            });
        let Some(setting) = first else {
            return;
        };
        let path = setting.path.as_str();
        match self.fields.get(path) {
            Some(field) => field.read(cx).focus_handle(cx).focus(window, cx),
            None => self.stop(path, cx).focus(window, cx),
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
            self.status.invalid(),
            self.writing || !self.queued.is_empty(),
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
        let sidebar = self.render_sidebar(swatch, searching, cx);
        let main = self.render_main(swatch, &query, window, cx);
        let mut context = KeyContext::default();
        context.add("Settings");
        div()
            .key_context(context)
            .track_focus(&self.focus)
            .on_action(cx.listener(
                |view, action: &InvokeWindow, window, cx| {
                    if let Err(error) = view.run(&action.0, window, cx) {
                        view.status.failed(error.to_string());
                        cx.notify();
                    }
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
        self.base.is_some() && !self.writing && self.queued.is_empty()
    }

    pub(super) fn status_text(&self) -> String {
        self.status.line().text
    }

    pub(super) fn read_only(&self) -> bool {
        self.status.invalid()
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
        let setting = self.model.setting(path).expect("a known setting");
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
        let setting = self.model.setting(path).expect("a known setting");
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
        let setting = self.model.setting(path).expect("a known setting");
        self.commit_field(setting, cx);
    }

    /// Moves focus out of the field, as clicking elsewhere does.
    pub(super) fn blur(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        let setting = self.model.setting(path).expect("a known setting");
        self.field_blurred(setting, cx);
    }

    pub(super) fn use_file(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        let setting = self.model.setting(path).expect("a known setting");
        self.keep_file(setting.path.as_str(), cx);
    }

    /// Follows a reload that applied the file.
    pub(super) fn follow_reload(&mut self, cx: &mut Context<'_, Self>) {
        self.reloaded(Ok(()), cx);
    }

    pub(super) fn use_mine(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        let setting = self.model.setting(path).expect("a known setting");
        self.keep_mine(setting.path.as_str(), cx);
    }
}
