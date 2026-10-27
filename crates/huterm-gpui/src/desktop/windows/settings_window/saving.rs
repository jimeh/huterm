//! Reading and writing `config.toml` for the Settings window: loads, the
//! write queue, conflicts, drafts, and closing once writes settle.
//!
//! Reads, parsing, validation, and writes run on the background executor.
//! One write runs at a time. A queued edit keeps the snapshot it was made
//! against, so a later write cannot silently overwrite an edit made
//! elsewhere; it moves to a newer snapshot only past this window's own
//! writes that landed on an unchanged file.

use gpui::{AppContext as _, Context, Window};
use huterm_config::document::{ApplyError, ConfigFile, Patch, Snapshot};
use huterm_config::settings::{
    Platform as SettingPlatform, Setting, SettingKind,
};
use toml::Value;

use super::super::super::settings::values::{
    self, FileValues, parse_number, patch_at, shown,
};
use super::super::super::settings::{Special, special};
use super::super::Desktop;
use super::{SettingsView, current, field_text, size_value, themes};

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
    patch: Patch,
    /// The value it stores; `None` resets.
    value: Option<Value>,
    /// The snapshot the user saw when making the change; `None` reads the
    /// file first.
    base: Option<Snapshot>,
    /// The text typed into the setting's field, when a field made it.
    typed: Option<String>,
}

/// What a background read found.
struct Loaded {
    snapshot: Snapshot,
    file: Result<FileValues, String>,
    valid: Result<(), String>,
}

/// What a background write produced.
struct Written {
    snapshot: Snapshot,
    file: Result<FileValues, String>,
    /// The file still matched the change's base, so the write changed
    /// nothing but this setting.
    clean: bool,
    /// The base the write used.
    base: Snapshot,
}

impl SettingsView {
    /// Rereads and validates the file on the background executor.
    pub(super) fn load(&mut self, cx: &mut Context<'_, Self>) {
        let file = ConfigFile::new(self.path.clone());
        let validate = crate::config::validator(self.path.clone());
        let epoch = self.epoch;
        let task = cx.background_spawn(async move {
            let snapshot = file.load()?;
            let text = snapshot.text();
            Ok::<_, ApplyError>(Loaded {
                file: FileValues::parse(text),
                valid: text.map_or(Ok(()), &validate),
                snapshot,
            })
        });
        cx.spawn(async move |view, cx| {
            let loaded = task.await;
            let _ = view.update(cx, |view, cx| {
                // A write finished since this read started; its snapshot is
                // newer.
                if view.epoch == epoch {
                    view.loaded(loaded, cx);
                }
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
            }) => {
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
            }
            Err(error) => self.status.failed(error.to_string()),
        }
        cx.notify();
    }

    /// Reads the selectable themes and their previews in the background.
    pub(super) fn load_themes(&mut self, cx: &mut Context<'_, Self>) {
        let path = self.path.clone();
        let task = cx.background_spawn(async move { themes(&path) });
        cx.spawn(async move |view, cx| {
            let themes = task.await;
            let _ = view.update(cx, |view, cx| {
                view.themes = themes;
                cx.notify();
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
        if self.status.invalid() {
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
        self.pending.insert(key, value.clone());
        if let Some(queued) = self
            .queued
            .iter_mut()
            .find(|queued| queued.setting.path == key)
        {
            queued.patch = patch;
            queued.value = value;
            queued.typed = typed;
            cx.notify();
            return;
        }
        let change = Pending {
            setting,
            patch,
            value,
            base: self.base.clone(),
            typed,
        };
        if self.writing.is_some() {
            self.queued.push_back(change);
        } else {
            self.start(change, cx);
        }
        cx.notify();
    }

    fn start(&mut self, change: Pending, cx: &mut Context<'_, Self>) {
        self.writing = Some(change.setting.path.as_str());
        self.status.saving();
        let file = ConfigFile::new(self.path.clone());
        let validate = crate::config::validator(self.path.clone());
        let base = change.base.clone();
        let patch = change.patch.clone();
        let task = cx.background_spawn(async move {
            let current = file.load()?;
            let base = base.unwrap_or_else(|| current.clone());
            let clean = current.revision() == base.revision();
            let snapshot = file.apply(&base, &[patch], &validate)?;
            Ok::<_, ApplyError>(Written {
                file: FileValues::parse(snapshot.text()),
                snapshot,
                clean,
                base,
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
        match result {
            Ok(written) => {
                self.errors.remove(key);
                self.conflicts.remove(key);
                if self.pending.get(key) == Some(&change.value) {
                    self.pending.remove(key);
                }
                if let Ok(file) = written.file {
                    self.file = file;
                }
                // Queued changes made on the same view now build on this
                // write, unless the file had also changed elsewhere.
                if written.clean {
                    for queued in &mut self.queued {
                        if queued.base.as_ref().is_some_and(|base| {
                            base.revision() == written.base.revision()
                        }) {
                            queued.base = Some(written.snapshot.clone());
                        }
                    }
                }
                self.base = Some(written.snapshot);
                self.status.saved();
                // The field shows the saved value in its display form, such
                // as `15` for a typed `15.0`.
                let typed_here = change.typed.as_ref().is_some_and(|typed| {
                    self.fields
                        .get(key)
                        .is_some_and(|field| field.read(cx).text() == typed)
                });
                if typed_here {
                    self.sync_field(key, true, cx);
                }
                self.sync_fields(cx);
                reload(cx);
            }
            Err(ApplyError::Conflict(changes)) => {
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
                // Nothing can be written to an invalid file, so every
                // pending change settles unsaved.
                self.queued.clear();
                self.pending.clear();
                self.status.invalid_file(message);
                self.sync_fields(cx);
            }
            Err(ApplyError::WrittenNotDurable(error)) => {
                // The file was replaced; it applies, with the warning.
                self.pending.remove(key);
                self.status.failed(error.to_string());
                self.load(cx);
                reload(cx);
            }
            Err(error) => {
                let message = error.to_string();
                self.pending.remove(key);
                self.errors.insert(key, message.clone());
                self.status.failed(message);
                self.sync_fields(cx);
            }
        }
        if let Some(next) = self.queued.pop_front() {
            self.start(next, cx);
        } else if self.closing {
            let handle = self.handle;
            cx.defer(move |cx| {
                let _ =
                    handle.update(cx, |_, window, _| window.remove_window());
            });
        }
        cx.notify();
    }

    /// Whether writes are in flight or queued.
    pub(super) fn busy(&self) -> bool {
        self.writing.is_some() || !self.queued.is_empty()
    }

    /// Closes the window once pending writes settle, saving a focused
    /// field's draft first. Returns whether it may close now.
    pub(super) fn request_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if let Some(setting) = self.focused_field(window, cx) {
            self.field_blurred(setting, cx);
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
        let Some(conflict) = self.conflicts.remove(path) else {
            return;
        };
        let Some(setting) = self.model.setting(path) else {
            return;
        };
        self.base = None;
        match conflict.patch {
            Some(refused) => {
                self.submit(setting, refused, conflict.value, None, cx);
            }
            None => self.commit_field(setting, cx),
        }
    }

    /// Discards the entered value and shows the file's.
    pub(super) fn keep_file(
        &mut self,
        path: &'static str,
        cx: &mut Context<'_, Self>,
    ) {
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
        // Saving a field again after a conflict keeps the entered value.
        if self.conflicts.remove(path).is_some() {
            self.base = None;
        }
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
                if value.is_some() && value == self.value(setting) {
                    // Nothing changed; show it in its display form.
                    self.sync_field(path, true, cx);
                    cx.notify();
                    return;
                }
                match value {
                    None if !self.resettable(setting) => {
                        self.sync_field(path, true, cx);
                    }
                    None => self.reset(setting, cx),
                    value => self.write_typed(setting, value, Some(text), cx),
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

    /// Escape in a field: puts back what it showed before typing.
    pub(super) fn cancel_field(
        &mut self,
        path: &'static str,
        cx: &mut Context<'_, Self>,
    ) {
        self.errors.remove(path);
        if let (Some(text), Some(field)) = (
            self.synced.get(path).cloned(),
            self.fields.get(path).cloned(),
        ) {
            field.update(cx, |field, cx| field.set_text(text, cx));
        }
        cx.notify();
    }
}
