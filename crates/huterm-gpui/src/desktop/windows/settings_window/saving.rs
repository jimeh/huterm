//! Reading and writing `config.toml` for the Settings window: loads, the
//! write queue, conflicts, drafts, and closing once writes settle.
//!
//! Reads, parsing, validation, and writes run on the background executor.
//! One write runs at a time. A queued edit keeps the snapshot it was made
//! against, so a later write cannot silently overwrite an edit made
//! elsewhere. It moves to a newer snapshot only when that snapshot still
//! holds the value the edit expected at its path.

use std::path::Path;

use gpui::{AppContext as _, Context, Window};
use huterm_config::document::{ApplyError, ConfigFile, Patch, Snapshot};
use huterm_config::settings::{
    Platform as SettingPlatform, Setting, SettingKind,
};
use toml::Value;

use super::super::super::settings::keybindings::Edit;
use super::super::super::settings::values::{
    self, FileValues, parse_number, patch_at, shown,
};
use super::super::super::settings::{Special, special};
use super::super::Desktop;
use super::{SettingsView, current, field_text, size_value, themes};
use crate::keymap::{Inventory, Platform, inventory};

/// A change the file changed under, kept until the user picks theirs or
/// the file's. `patch` is the refused write; a field draft that a reload
/// changed under has none, and saving it again writes the draft.
pub(super) struct Conflict {
    pub(super) patch: Option<Patch>,
    pub(super) value: Option<Value>,
    pub(super) message: String,
}

/// One change waiting to be written.
pub(super) struct Pending {
    setting: &'static Setting,
    /// The dotted path the patch writes: the setting's, or `theme.name`.
    path: &'static str,
    patch: Patch,
    /// The value it stores; `None` removes the path.
    value: Option<Value>,
    /// The snapshot the user saw when making the change; `None` reads the
    /// file first.
    base: Option<Snapshot>,
    /// The value the path held, or was about to hold, when the change was
    /// made; a newer snapshot that still holds it is a safe base.
    expected: Option<Value>,
    /// The text typed into the setting's field, when a field made it.
    typed: Option<String>,
}

/// A keybinding edit waiting to be written, and the shortcut row that
/// shows its errors.
pub(super) struct KeyChange {
    pub(super) edit: Edit,
    pub(super) row: Option<String>,
}

/// A write waiting behind the one in flight.
pub(super) enum Queued {
    Setting(Pending),
    Keys(KeyChange),
}

/// What a background read found.
struct Loaded {
    snapshot: Snapshot,
    file: Result<FileValues, String>,
    valid: Result<(), String>,
    keys: Option<Inventory>,
}

/// What a background write produced.
struct Written {
    snapshot: Snapshot,
    file: Result<FileValues, String>,
    keys: Option<Inventory>,
}

/// The file's bindings with what became of each, or `None` when they do
/// not compile.
fn listed(text: Option<&str>, path: &Path) -> Option<Inventory> {
    let config =
        crate::config::parse_at(text.unwrap_or_default(), path).ok()?;
    inventory(Platform::current(), &config.keybindings).ok()
}

/// The path a setting's changes write: the theme row edits the theme's
/// name, every other row its own path.
pub(super) fn written_path(setting: &'static Setting) -> &'static str {
    if special(&setting.path) == Some(Special::ThemePicker) {
        "theme.name"
    } else {
        setting.path.as_str()
    }
}

impl SettingsView {
    /// Rereads and validates the file on the background executor. Only the
    /// latest read applies, and only when no write finished since it began.
    /// Creates the file from the default template, as Open Config File
    /// does, then reads it. A file that cannot be created reads as absent.
    pub(super) fn create_and_load(&mut self, cx: &mut Context<'_, Self>) {
        let path = self.path.clone();
        self.loading += 1;
        let task = cx.background_spawn(async move {
            crate::config::create_default(&path).map_err(|error| {
                format!("Cannot create {}: {error}", path.display())
            })
        });
        cx.spawn(async move |view, cx| {
            let created = task.await;
            let _ = view.update(cx, |view, cx| {
                view.loading -= 1;
                if let Err(message) = created {
                    view.status.file_error(message);
                }
                view.load(cx);
            });
        })
        .detach();
    }

    pub(super) fn load(&mut self, cx: &mut Context<'_, Self>) {
        let file = ConfigFile::new(self.path.clone());
        let validate = crate::config::validator(self.path.clone());
        let path = self.path.clone();
        let epoch = self.epoch;
        self.reads += 1;
        self.loading += 1;
        let read = self.reads;
        let task = cx.background_spawn(async move {
            let snapshot = file.load()?;
            let text = snapshot.text();
            Ok::<_, ApplyError>(Loaded {
                file: FileValues::parse(text),
                valid: text.map_or(Ok(()), &validate),
                keys: listed(text, &path),
                snapshot,
            })
        });
        cx.spawn(async move |view, cx| {
            let loaded = task.await;
            let _ = view.update(cx, |view, cx| {
                view.loading -= 1;
                if view.epoch == epoch && view.reads == read {
                    view.loaded(loaded, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn loaded(
        &mut self,
        loaded: Result<Loaded, ApplyError>,
        cx: &mut Context<'_, Self>,
    ) {
        match loaded {
            Ok(Loaded {
                snapshot,
                file,
                valid,
                keys,
            }) => {
                self.keys.inventory = keys;
                match (file, valid) {
                    (Ok(file), Ok(())) => {
                        self.file = file;
                        if let Some(error) = self.reload_error.take() {
                            // The file is fine, so something else refused
                            // the reload, such as a taken global shortcut.
                            self.status.not_applied(error);
                        }
                    }
                    (Ok(file), Err(message)) => {
                        self.file = file;
                        self.status.invalid_file(message);
                    }
                    (Err(message), _) => self.status.invalid_file(message),
                }
                self.base = Some(snapshot);
                self.sync_fields(cx);
                self.sync_key_fields(cx);
            }
            Err(error) => self.status.failed(error.to_string()),
        }
    }

    /// Reads the selectable themes and their previews in the background;
    /// only the latest scan applies.
    pub(super) fn load_themes(&mut self, cx: &mut Context<'_, Self>) {
        let path = self.path.clone();
        self.scans += 1;
        let scan = self.scans;
        let task = cx.background_spawn(async move { themes(&path) });
        cx.spawn(async move |view, cx| {
            let themes = task.await;
            let _ = view.update(cx, |view, cx| {
                if view.scans == scan {
                    view.themes = themes;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Reports a file that changed since it was read while watching is
    /// off. The controls keep showing what was read, as the running
    /// settings do, until a reload.
    pub(super) fn check_disk(&mut self, cx: &mut Context<'_, Self>) {
        if cx.global::<Desktop>().config.general.watch_config {
            return;
        }
        if self.base.is_none() {
            return;
        }
        let epoch = self.epoch;
        let file = ConfigFile::new(self.path.clone());
        let task = cx.background_spawn(async move { file.load() });
        cx.spawn(async move |view, cx| {
            let Ok(current) = task.await else {
                return;
            };
            let _ =
                view.update(cx, |view, cx| {
                    // The window's own writes change the file too: one that
                    // finished or is pending since the read began is not an
                    // edit made elsewhere, and its save reloads anyway.
                    if view.epoch != epoch || view.busy() {
                        return;
                    }
                    if view.base.as_ref().is_some_and(|base| {
                        base.revision() != current.revision()
                    }) {
                        view.status.changed_on_disk();
                        cx.notify();
                    }
                });
        })
        .detach();
    }

    /// Follows a reload: the file and themes are reread, and every control
    /// follows them. A failed reload makes the window read-only only when
    /// the file itself fails validation.
    pub(super) fn reloaded(
        &mut self,
        outcome: Result<(), String>,
        cx: &mut Context<'_, Self>,
    ) {
        self.status
            .watching(cx.global::<Desktop>().config.general.watch_config);
        match outcome {
            Ok(()) => {
                self.reload_error = None;
                self.status.reloaded();
            }
            Err(message) => self.reload_error = Some(message),
        }
        self.load(cx);
        self.load_themes(cx);
    }

    /// The value a control shows: a pending change, else the file's, else
    /// the default.
    pub(super) fn value(&self, setting: &Setting) -> Option<Value> {
        match self.pending.get(setting.path.as_str()) {
            Some(value) => value.clone(),
            None => shown(setting, &self.file, &self.defaults),
        }
    }

    /// Puts each field's text back to what the file and defaults show. A
    /// field holding an unsaved draft keeps it; when the file changed that
    /// setting meanwhile, the row shows a conflict instead.
    pub(super) fn sync_fields(&mut self, cx: &mut Context<'_, Self>) {
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
    pub(super) fn sync_field(
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
                        value: None,
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
    pub(super) fn write(
        &mut self,
        setting: &'static Setting,
        value: Option<Value>,
        cx: &mut Context<'_, Self>,
    ) {
        self.write_typed(setting, value, None, cx);
    }

    fn write_typed(
        &mut self,
        setting: &'static Setting,
        value: Option<Value>,
        typed: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        // A size's forms are checked by the validator.
        if let Some(value) = &value
            && special(&setting.path) != Some(Special::Size)
            && let Err(message) = values::check(setting, value)
        {
            self.errors.insert(setting.path.as_str(), message);
            cx.notify();
            return;
        }
        let patch = values::patch(setting, value.clone());
        self.submit(setting, patch, value, typed, cx);
    }

    /// Queues `patch` for `setting`, after any write in flight. A queued
    /// change to the same setting is replaced, keeping its base.
    pub(super) fn submit(
        &mut self,
        setting: &'static Setting,
        patch: Patch,
        value: Option<Value>,
        typed: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        let key = setting.path.as_str();
        if self.locked() {
            return;
        }
        if let Some(platform) = setting.platform
            && platform != current()
        {
            let name = match platform {
                SettingPlatform::Macos => "macOS",
                SettingPlatform::Linux => "Linux",
            };
            self.errors.insert(key, format!("Applies on {name} only"));
            cx.notify();
            return;
        }
        let target = written_path(setting);
        let expected = match self.pending.get(key) {
            Some(pending) => pending.clone(),
            None => self.file.get(target).cloned(),
        };
        self.pending.insert(key, value.clone());
        // Use mine reads the file afresh for its own change only.
        let base = if self.fresh == Some(key) {
            self.fresh = None;
            None
        } else {
            self.base.clone()
        };
        if let Some(queued) =
            self.queued.iter_mut().find_map(|queued| match queued {
                Queued::Setting(queued) if queued.setting.path == key => {
                    Some(queued)
                }
                _ => None,
            })
        {
            queued.patch = patch;
            queued.value = value;
            queued.typed = typed;
            cx.notify();
            return;
        }
        let change = Pending {
            setting,
            path: target,
            patch,
            value,
            base,
            expected,
            typed,
        };
        if self.writing.is_some() {
            self.queued.push_back(Queued::Setting(change));
        } else {
            self.start(change, cx);
        }
        cx.notify();
    }

    /// Queues a keybinding edit after any write in flight. Its patches are
    /// built when its turn comes, against the snapshot the previous write
    /// left, so successive edits apply in order.
    pub(super) fn submit_keys(
        &mut self,
        edit: Edit,
        row: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        if self.locked() {
            return;
        }
        if let Some(row) = &row {
            self.keys.errors.remove(row);
        }
        let change = KeyChange { edit, row };
        if self.writing.is_some() {
            self.queued.push_back(Queued::Keys(change));
        } else {
            self.start_keys(change, cx);
        }
        cx.notify();
    }

    fn start_keys(&mut self, change: KeyChange, cx: &mut Context<'_, Self>) {
        self.writing = Some("keybinding");
        self.status.saving();
        let file = ConfigFile::new(self.path.clone());
        let validate = crate::config::validator(self.path.clone());
        let path = self.path.clone();
        let base = self.base.clone();
        let edit = change.edit.clone();
        let task = cx.background_spawn(async move {
            let base = match base {
                Some(base) => base,
                None => file.load()?,
            };
            let patches = edit
                .patches(&base)
                .map_err(|message| ApplyError::Conflict(vec![message]))?;
            let snapshot = file.apply(&base, &patches, &validate)?;
            Ok::<_, ApplyError>(Written {
                file: FileValues::parse(snapshot.text()),
                keys: listed(snapshot.text(), &path),
                snapshot,
            })
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view
                .update(cx, |view, cx| view.written_keys(&change, result, cx));
        })
        .detach();
    }

    fn written_keys(
        &mut self,
        change: &KeyChange,
        result: Result<Written, ApplyError>,
        cx: &mut Context<'_, Self>,
    ) {
        self.writing = None;
        self.epoch += 1;
        let report = |view: &mut Self, message: String| {
            if let Some(row) = &change.row {
                view.keys.errors.insert(row.clone(), message.clone());
            }
            view.status.failed(message);
        };
        let failed = match result {
            Ok(written) => {
                if let Ok(file) = written.file {
                    self.file = file;
                }
                self.keys.inventory = written.keys;
                self.base = Some(written.snapshot);
                self.status.saved();
                self.follow_changed_default(
                    &change.edit,
                    change.row.as_deref(),
                );
                self.sync_key_fields(cx);
                cx.defer(|cx| {
                    let _ = super::super::reload(cx);
                });
                false
            }
            // An entry another editor changed: show the file as it is.
            Err(ApplyError::Conflict(changes)) => {
                report(
                    self,
                    format!("config.toml changed: {}", changes.join("; ")),
                );
                self.load(cx);
                true
            }
            Err(ApplyError::InvalidFile(message)) => {
                self.queued.clear();
                self.pending.clear();
                self.status.invalid_file(message);
                self.sync_fields(cx);
                true
            }
            Err(ApplyError::WrittenNotDurable(error)) => {
                self.status.saved_with_warning(error.to_string());
                self.load(cx);
                cx.defer(|cx| {
                    let _ = super::super::reload(cx);
                });
                true
            }
            Err(error) => {
                report(self, error.to_string());
                true
            }
        };
        self.next_write(failed, cx);
    }

    fn start(&mut self, change: Pending, cx: &mut Context<'_, Self>) {
        self.writing = Some(change.setting.path.as_str());
        self.status.saving();
        let file = ConfigFile::new(self.path.clone());
        let validate = crate::config::validator(self.path.clone());
        let config_path = self.path.clone();
        let base = change.base.clone();
        let patch = change.patch.clone();
        let task = cx.background_spawn(async move {
            let base = match base {
                Some(base) => base,
                None => file.load()?,
            };
            let snapshot = file.apply(&base, &[patch], &validate)?;
            Ok::<_, ApplyError>(Written {
                file: FileValues::parse(snapshot.text()),
                keys: listed(snapshot.text(), &config_path),
                snapshot,
            })
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ =
                view.update(cx, |view, cx| view.written(change, result, cx));
        })
        .detach();
    }

    fn written(
        &mut self,
        change: Pending,
        result: Result<Written, ApplyError>,
        cx: &mut Context<'_, Self>,
    ) {
        self.writing = None;
        self.epoch += 1;
        let key = change.setting.path.as_str();
        let reload = |cx: &mut Context<'_, Self>| {
            // Watching may be off, so a save asks for the reload itself; a
            // watcher that also saw it coalesces.
            cx.defer(|cx| {
                let _ = super::super::reload(cx);
            });
        };
        // A newer change to this setting keeps showing until it lands.
        let settle = |view: &mut Self| {
            if view.pending.get(key) == Some(&change.value) {
                view.pending.remove(key);
            }
        };
        let mut failed = false;
        match result {
            Ok(written) => {
                self.errors.remove(key);
                self.conflicts.remove(key);
                settle(self);
                // A queued change whose path still holds what it expected
                // can build on this write: nothing else touched it.
                if let Ok(file) = written.file {
                    for queued in &mut self.queued {
                        if let Queued::Setting(queued) = queued
                            && file.get(queued.path) == queued.expected.as_ref()
                        {
                            queued.base = Some(written.snapshot.clone());
                        }
                    }
                    self.file = file;
                }
                self.keys.inventory = written.keys;
                self.base = Some(written.snapshot);
                self.status.saved();
                let saved = field_text(shown(
                    change.setting,
                    &self.file,
                    &self.defaults,
                ));
                self.adopt_saved_field(key, change.typed.as_deref(), saved, cx);
                self.sync_fields(cx);
                reload(cx);
            }
            Err(ApplyError::Conflict(changes)) => {
                failed = true;
                // The control keeps showing the refused value.
                self.conflicts.insert(
                    key,
                    Conflict {
                        patch: Some(change.patch),
                        value: change.value,
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
                failed = true;
                // Nothing can be written to an invalid file, so every
                // pending change settles unsaved.
                self.queued.clear();
                self.pending.clear();
                self.status.invalid_file(message);
                self.sync_fields(cx);
            }
            Err(ApplyError::WrittenNotDurable(error)) => {
                // The file was replaced; it applies, with the warning. With
                // no snapshot of it, a queued change to the same path reads
                // the file afresh, and a closing window stays to show it.
                settle(self);
                for queued in &mut self.queued {
                    if let Queued::Setting(queued) = queued
                        && queued.path == change.path
                    {
                        queued.base = None;
                    }
                }
                let saved = self.display(change.setting, change.value.clone());
                self.adopt_saved_field(key, change.typed.as_deref(), saved, cx);
                failed = true;
                self.status.saved_with_warning(error.to_string());
                self.load(cx);
                reload(cx);
            }
            Err(error) => {
                failed = true;
                let message = error.to_string();
                settle(self);
                self.errors.insert(key, message.clone());
                self.status.failed(message);
                self.sync_fields(cx);
            }
        }
        self.next_write(failed, cx);
    }

    /// Starts the next queued write, or closes a window waiting for its
    /// writes. A write that failed while closing keeps the window open, so
    /// the user sees it and can recover the entered value.
    fn next_write(&mut self, failed: bool, cx: &mut Context<'_, Self>) {
        if failed {
            self.closing = false;
        }
        match self.queued.pop_front() {
            Some(Queued::Setting(next)) => self.start(next, cx),
            Some(Queued::Keys(next)) => self.start_keys(next, cx),
            None if self.closing => {
                let handle = self.handle;
                cx.defer(move |cx| {
                    let _ = handle
                        .update(cx, |_, window, _| window.remove_window());
                });
            }
            None => {}
        }
        cx.notify();
    }

    /// Brings `key`'s field up to a save of it. A field still showing the
    /// typed text shows the saved value in its display form, such as `15`
    /// for a typed `15.0`. A field typed into since keeps that draft
    /// without a conflict against this window's own save. An untouched
    /// field, such as after Reset, follows the file.
    fn adopt_saved_field(
        &mut self,
        key: &'static str,
        typed: Option<&str>,
        saved: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(field) = self.fields.get(key) else {
            return;
        };
        let current = field.read(cx).text().to_owned();
        if typed == Some(current.as_str()) {
            self.synced.insert(key, saved);
            self.sync_field(key, true, cx);
        } else if self.synced.get(key) != Some(&current) {
            self.synced.insert(key, saved);
        }
    }

    /// How a field shows `value`, or the default for a reset.
    fn display(&self, setting: &Setting, value: Option<Value>) -> String {
        field_text(value.or_else(|| {
            self.defaults
                .get(setting.path.as_str())
                .cloned()
                .or_else(|| setting.default.clone())
        }))
    }

    /// Whether writes are in flight or queued.
    pub(super) fn busy(&self) -> bool {
        self.writing.is_some() || !self.queued.is_empty()
    }

    /// Closes the window once pending writes settle, saving a focused
    /// field's draft first. Returns whether it may close now. A draft that
    /// does not save, or an unsettled conflict, keeps it open.
    pub(super) fn request_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if let Some(setting) = self.focused_field(window, cx) {
            self.field_blurred(setting, cx);
            if self.errors.contains_key(setting.path.as_str()) {
                return false;
            }
        }
        if !self.conflicts.is_empty() {
            return false;
        }
        if self.busy() {
            self.closing = true;
            return false;
        }
        true
    }

    /// Keeps the entered value: reapplies it against a fresh read.
    pub(super) fn keep_mine(
        &mut self,
        path: &'static str,
        cx: &mut Context<'_, Self>,
    ) {
        // An invalid file refuses the write; the conflict waits for a fix.
        if self.locked() {
            return;
        }
        let Some(setting) = self.model.setting(path) else {
            return;
        };
        // A field draft settles through the field, which keeps the conflict
        // until the draft parses.
        let Some(refused) = self
            .conflicts
            .get(path)
            .and_then(|conflict| conflict.patch.clone())
        else {
            if self.conflicts.contains_key(path) {
                self.commit_field(setting, cx);
            }
            return;
        };
        let Some(conflict) = self.conflicts.remove(path) else {
            return;
        };
        self.fresh = Some(path);
        self.submit(setting, refused, conflict.value, None, cx);
        self.clear_unused_fresh(path);
    }

    /// Drops a fresh read that no submitted change used, so a later change
    /// to the setting builds on the window's snapshot and conflicts with an
    /// edit made elsewhere instead of overwriting it.
    fn clear_unused_fresh(&mut self, path: &'static str) {
        if self.fresh == Some(path) {
            self.fresh = None;
        }
    }

    /// Discards the entered value and shows the file's.
    pub(super) fn keep_file(
        &mut self,
        path: &'static str,
        cx: &mut Context<'_, Self>,
    ) {
        if self.locked() {
            return;
        }
        self.conflicts.remove(path);
        self.errors.remove(path);
        self.pending.remove(path);
        if self.conflicts.is_empty() {
            self.status.settled();
        }
        self.sync_field(path, true, cx);
        cx.notify();
    }

    /// Whether Reset applies: the file sets the setting, and its row is not
    /// an editor that later work adds. The theme resets its name only.
    pub(super) fn resettable(&self, setting: &Setting) -> bool {
        match special(&setting.path) {
            Some(
                Special::Keybindings
                | Special::GlobalShortcuts
                | Special::QuakeProfiles,
            ) => false,
            Some(Special::ThemePicker) => self.file.is_set("theme.name"),
            _ => self.file.is_set(&setting.path),
        }
    }

    pub(super) fn reset(
        &mut self,
        setting: &'static Setting,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.resettable(setting) {
            return;
        }
        let path = setting.path.as_str();
        self.errors.remove(path);
        if special(path) == Some(Special::ThemePicker) {
            self.submit(setting, patch_at("theme.name", None), None, None, cx);
            return;
        }
        // Reset discards whatever was typed into the field.
        if let Some(text) = self.synced.get(path).cloned()
            && let Some(field) = self.fields.get(path).cloned()
        {
            field.update(cx, |field, cx| field.set_text(text, cx));
        }
        self.write(setting, None, cx);
    }

    /// Commits a number or text field's text.
    pub(super) fn commit_field(
        &mut self,
        setting: &'static Setting,
        cx: &mut Context<'_, Self>,
    ) {
        let path = setting.path.as_str();
        let Some(field) = self.fields.get(path) else {
            return;
        };
        let text = field.read(cx).text().to_owned();
        // A field still showing what it last synced has nothing to save,
        // even when the file spells the value differently, such as the
        // integer `13` for a number setting.
        if !self.conflicts.contains_key(path)
            && !self.pending.contains_key(path)
            && self.synced.get(path) == Some(&text)
        {
            if self.errors.remove(path).is_some() {
                cx.notify();
            }
            return;
        }
        // A field showing a pending change, such as the default during a
        // reset, has nothing new to save.
        if !self.conflicts.contains_key(path)
            && self.pending.get(path).is_some_and(|value| {
                text == self.display(setting, value.clone())
            })
        {
            return;
        }
        // Saving a field again after a conflict keeps the entered value,
        // against a fresh read; the conflict stays until the text parses.
        let resolving = self.conflicts.contains_key(path);
        let parsed = match &setting.kind {
            _ if special(path) == Some(Special::Size) => Ok(size_value(&text)),
            SettingKind::Integer { .. } | SettingKind::Number { .. } => {
                parse_number(setting, &text)
            }
            _ if text.trim().is_empty() => Ok(None),
            _ => Ok(Some(Value::String(text.trim().to_owned()))),
        };
        match parsed {
            Ok(value) => {
                self.errors.remove(path);
                if resolving {
                    self.conflicts.remove(path);
                }
                if value.is_some() && value == self.value(setting) {
                    // Already saved or saving. A field that matches a
                    // pending change keeps its text until that lands.
                    if !self.pending.contains_key(path) {
                        self.sync_field(path, true, cx);
                    }
                    cx.notify();
                    return;
                }
                match value {
                    None if !self.resettable(setting) => {
                        self.sync_field(path, true, cx);
                    }
                    value => {
                        if resolving {
                            self.fresh = Some(path);
                        }
                        match value {
                            None => self.reset(setting, cx),
                            value => {
                                self.write_typed(
                                    setting,
                                    value,
                                    Some(text),
                                    cx,
                                );
                            }
                        }
                        self.clear_unused_fresh(path);
                    }
                }
            }
            Err(message) => {
                self.errors.insert(path, message);
                cx.notify();
            }
        }
    }

    /// Saves a field when focus leaves it. Leaving a conflicted field, such
    /// as to press Use file, keeps the conflict for the user to settle.
    pub(super) fn field_blurred(
        &mut self,
        setting: &'static Setting,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.conflicts.contains_key(setting.path.as_str()) {
            self.commit_field(setting, cx);
        }
    }

    /// Escape in a field: puts back what it showed before typing, a
    /// pending change's value while one is saving, or, in a conflict, the
    /// file's value, as Use file does.
    pub(super) fn cancel_field(
        &mut self,
        path: &'static str,
        cx: &mut Context<'_, Self>,
    ) {
        if self.conflicts.contains_key(path) {
            self.keep_file(path, cx);
            return;
        }
        self.errors.remove(path);
        let text = match (self.pending.get(path), self.model.setting(path)) {
            // A pending reset shows the default.
            (Some(value), Some(setting)) => {
                Some(self.display(setting, value.clone()))
            }
            _ => self.synced.get(path).cloned(),
        };
        if let (Some(text), Some(field)) =
            (text, self.fields.get(path).cloned())
        {
            field.update(cx, |field, cx| field.set_text(text, cx));
        }
        cx.notify();
    }
}
