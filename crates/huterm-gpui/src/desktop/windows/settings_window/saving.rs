//! Reading and writing `config.toml` for the Settings window: loads, the
//! write queue, conflicts, drafts, and closing once writes settle.
//!
//! Reads, parsing, validation, and writes run on the background executor.
//! One write runs at a time. A queued edit keeps the snapshot it was made
//! against, so a later write cannot silently overwrite an edit made
//! elsewhere. It moves to a newer snapshot only when that snapshot still
//! holds the value the edit expected at its path. Use mine and a profile's
//! or a pane layout's deletion read the file afresh instead: the user chose
//! to replace or remove what is there.
//!
//! A [`Batch`] is one save of several patches, such as a rename: the old
//! key's removal, the new key, and the references rewritten. They go to
//! `ConfigFile::apply` together, which checks each against the file on
//! disk and writes nothing unless all apply.

use std::path::Path;

use gpui::{App, AppContext as _, Context, Window};
use huterm_config::document::{
    ApplyError, ConfigFile, EntryLocator, List, Patch, Snapshot,
};
use huterm_config::settings::{
    Platform as SettingPlatform, Setting, SettingKind,
};
use toml::{Table, Value};

use super::super::super::settings::keybindings::{
    Edit, rename_layout_references, rename_profile_references,
};
use super::super::super::settings::values::{
    self, FileValues, as_number, parse_number, patch_at, shown,
};
use super::super::super::settings::{Special, special};
use super::super::Desktop;
use super::super::model::QuakeRenameId;
use super::{
    SettingsView, current, field_text, layouts, quake, size_value, themes,
};
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
    /// The dotted path the patch writes.
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
    /// Built when the edit was made, from the locators of the snapshot the
    /// user saw. A locator finds its entry by position and content, so a
    /// change made elsewhere to that entry conflicts instead of editing
    /// another one; this window's own writes rebase them
    /// (`rebase_queued_keys`).
    patches: Vec<Patch>,
    /// How many of the leading patches the rest depend on: when a rebase
    /// drops one of them, the whole change goes. A reassignment's removals
    /// of the entries holding a key mean nothing once its own edit is gone.
    required: usize,
    /// The field that made the edit and the text it last saved, put back
    /// when the write fails so the entered text stays a draft to retry.
    pub(super) field: Option<(String, Option<String>)>,
}

/// Several changes written in one save: every patch lands, or none does.
///
/// When its turn comes, the values it moves are read from the file as this
/// window last read or wrote it, so they build on the window's own earlier
/// writes, and they apply against that snapshot, so an edit made elsewhere
/// since conflicts with the whole batch. The entries it rewrites are found
/// in the file as it is when the batch applies, so one added elsewhere
/// since is rewritten too. While it waits, `pending` shows its outcome, as
/// for a setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Batch {
    /// Renames a Quake profile and the shortcuts that name it.
    RenameProfile { from: String, to: String },
    /// Renames a pane layout, the keybindings that name it, and its place
    /// in the layout cycle.
    RenameLayout { from: String, to: String },
    /// Deletes a pane layout, applied to the file as it is then, unless a
    /// keybinding or the cycle there uses it.
    DeleteLayout { name: String },
    /// Sets or removes the theme picker's `theme.*` keys together, such as
    /// a switch from one theme to one for each mode.
    Theme(Vec<(&'static str, Option<String>)>),
}

impl Batch {
    /// The patches built from `current`, the file as it is when the batch
    /// applies: the entries a rename rewrites. `Err` says why `current`
    /// refuses the batch, such as a keybinding naming a deleted layout.
    fn current_patches(
        &self,
        current: &Snapshot,
    ) -> Result<Vec<Patch>, String> {
        match self {
            Self::RenameProfile { from, to } => Ok(rename_profile_references(
                &current.entries(List::Keybinding),
                &current.entries(List::GlobalKeybinding),
                from,
                to,
            )),
            Self::RenameLayout { from, to } => Ok(rename_layout_references(
                &current.entries(List::Keybinding),
                from,
                to,
            )),
            Self::DeleteLayout { name } => {
                match layouts::delete_refusal_in(current, name) {
                    Some(refusal) => Err(refusal),
                    None => Ok(Vec::new()),
                }
            }
            Self::Theme(_) => Ok(Vec::new()),
        }
    }

    /// Whether the batch applies to the file as it is rather than as the
    /// window last saw it: the user confirmed deleting a layout as it is
    /// now, so an edit to its template elsewhere does not conflict.
    fn fresh(&self) -> bool {
        matches!(self, Self::DeleteLayout { .. })
    }
}

/// Why a batch did not save as asked.
enum BatchError {
    /// Applying it failed, or its write was not made durable.
    Apply(ApplyError),
    /// The file as it was when the batch applied refused it, for this
    /// reason; nothing was written.
    Refused(String),
}

/// The values a batch put in `pending` when it was submitted, which go
/// when it settles unless a newer change replaced them.
pub(super) type Outcome = Vec<(&'static str, Option<Value>)>;

/// A write waiting behind the one in flight.
pub(super) enum Queued {
    Setting(Pending),
    Keys(KeyChange),
    Batch(Batch, Outcome),
}

/// What a background read found.
struct Loaded {
    snapshot: Snapshot,
    file: Result<FileValues, String>,
    valid: Result<(), String>,
    keys: Listed,
}

/// What a background write produced.
struct Written {
    snapshot: Snapshot,
    file: Result<FileValues, String>,
    keys: Listed,
}

/// What a read or write found about the file's keybindings.
struct Listed {
    /// The bindings with what became of each, or `None` when they do not
    /// compile.
    inventory: Option<Inventory>,
    /// A locator for each `[[keybinding]]` entry, for edits to address.
    entries: Vec<EntryLocator>,
    /// A locator for each `[[global_keybinding]]` entry.
    globals: Vec<EntryLocator>,
}

/// Parses the file's keybindings, off the GPUI thread.
fn listed(snapshot: &Snapshot, path: &Path) -> Listed {
    let inventory =
        crate::config::parse_at(snapshot.text().unwrap_or_default(), path)
            .ok()
            .and_then(|config| {
                inventory(Platform::current(), &config.keybindings).ok()
            });
    Listed {
        inventory,
        entries: snapshot.entries(List::Keybinding),
        globals: snapshot.entries(List::GlobalKeybinding),
    }
}

/// Whether `patch` writes the dotted path `at` or a table holding it.
fn writes(patch: &Patch, at: &str) -> bool {
    let (Patch::Set { path: written, .. } | Patch::Remove { path: written }) =
        patch
    else {
        return false;
    };
    let mut keys = at.split('.');
    written.iter().all(|key| keys.next() == Some(key.as_str()))
}

/// The patches for the theme picker's `changes`: sets first, so removing
/// the last key the table holds never drops and recreates `[theme]`.
fn theme_patches(changes: &[(&'static str, Option<String>)]) -> Vec<Patch> {
    let mut patches: Vec<(bool, Patch)> = changes
        .iter()
        .map(|(key, value)| {
            (
                value.is_none(),
                patch_at(key, value.clone().map(Value::String)),
            )
        })
        .collect();
    patches.sort_by_key(|(removes, _)| *removes);
    patches.into_iter().map(|(_, patch)| patch).collect()
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
                keys: listed(&snapshot, &path),
                snapshot,
            })
        });
        cx.spawn(async move |view, cx| {
            let loaded = task.await;
            let _ = view.update(cx, |view, cx| {
                view.loading -= 1;
                // A read that finishes while this window's writes are
                // pending would mix its list with theirs, so the file is
                // read again when the queue drains. One that a write
                // finished during may be stale, and a write that failed
                // reads nothing itself, so it is read again too.
                if view.reads == read {
                    if view.busy() {
                        view.reread = true;
                    } else if view.epoch == epoch {
                        view.loaded(loaded, cx);
                    } else {
                        view.load(cx);
                    }
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
                self.status.read();
                if std::mem::take(&mut self.reload_unreported) {
                    if snapshot.text().is_some_and(|text| {
                        self.written.as_deref() == Some(text)
                    }) {
                        self.status.reloaded_unchanged();
                    } else {
                        self.status.reloaded();
                    }
                }
                self.keys.inventory = keys.inventory;
                self.keys.entries = keys.entries;
                self.set_globals(keys.globals);
                match (file, valid) {
                    (Ok(file), Ok(())) => {
                        self.file = file;
                        match self.reload_outcome.take() {
                            // The file is fine, so something else refused
                            // the reload, such as a missing font.
                            Some(Err(error)) => self.status.not_applied(error),
                            Some(Ok(warnings)) if !warnings.is_empty() => {
                                self.status.applied_with_warnings(warnings);
                            }
                            Some(Ok(_)) => self.status.applied(),
                            None => {}
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
                self.forget_gone_layouts(cx);
                self.sync_key_fields(cx);
            }
            Err(error) => {
                self.reload_unreported = false;
                self.status.read_failed(error.to_string());
            }
        }
    }

    /// Records a successful write as the file this window last read and
    /// wrote.
    fn wrote(&mut self, snapshot: Snapshot) {
        self.written = snapshot.text().map(str::to_owned);
        self.base = Some(snapshot);
    }

    /// Reads the selectable themes and their previews in the background;
    /// only the latest scan applies.
    pub(super) fn load_themes(&mut self, cx: &mut Context<'_, Self>) {
        let path = self.path.clone();
        self.scans += 1;
        let scan = self.scans;
        let task = cx.background_spawn(async move { themes::scan(&path) });
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
    ///
    /// The watcher can report this window's own write after the reload its
    /// save asked for, so an automatic reload keeps a save's report when
    /// the reread finds the file as this window last wrote it. Another read
    /// may already have seen another editor's change, so the comparison is
    /// with the write, not with `base`.
    pub(super) fn reloaded(
        &mut self,
        outcome: Result<Vec<String>, String>,
        automatic: bool,
        cx: &mut Context<'_, Self>,
    ) {
        self.status
            .watching(cx.global::<Desktop>().config.general.watch_config);
        if outcome.is_ok() {
            if automatic {
                self.reload_unreported = true;
            } else {
                self.status.reloaded();
            }
        }
        self.reload_outcome = Some(outcome);
        self.load(cx);
        self.load_themes(cx);
        self.scan_fonts(cx);
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
            (self.fields.get(path).cloned(), self.setting_at(path))
        else {
            return;
        };
        let text = self.batch_baseline(setting, path).unwrap_or_else(|| {
            field_text(shown(setting, &self.file, &self.defaults))
        });
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
        self.show_text(path, text.clone(), cx);
        self.synced.insert(path, text);
    }

    /// How `setting`'s field shows what `path` will hold once the batches
    /// waiting to land do, when one writes it, such as a value a rename
    /// copies to the profile's new name. The field syncs against that until
    /// they land, as it showed it.
    fn batch_baseline(&self, setting: &Setting, path: &str) -> Option<String> {
        let queued = self.queued.iter().filter_map(|queued| match queued {
            Queued::Batch(_, outcome) => Some(outcome),
            _ => None,
        });
        let (_, value) = self
            .writing_batch
            .iter()
            .chain(queued)
            .flatten()
            .rfind(|(at, _)| *at == path)?;
        Some(self.display(setting, value.clone()))
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
        let target = setting.path.as_str();
        let expected = match self.pending.get(key) {
            Some(pending) => pending.clone(),
            None => self.file.get(target).cloned(),
        };
        self.pending.insert(key, value.clone());
        // Use mine and a profile's deletion read the file afresh, for their
        // own change only.
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

    /// Queues a keybinding edit after any write in flight; `false` when
    /// the window cannot write. Its patches are built now, against the
    /// snapshot the list came from; writes this window makes before its
    /// turn rebase them, so successive edits to one entry build on each
    /// other.
    pub(super) fn submit_keys(
        &mut self,
        edit: Edit,
        row: Option<String>,
        field: Option<(String, Option<String>)>,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        self.submit_keys_removing(edit, row, field, &[], cx)
    }

    /// [`Self::submit_keys`], removing the user entries `remove` in the
    /// same write.
    pub(super) fn submit_keys_removing(
        &mut self,
        edit: Edit,
        row: Option<String>,
        field: Option<(String, Option<String>)>,
        remove: &[usize],
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if self.locked() {
            return false;
        }
        if self.base.is_none() {
            return false;
        }
        if let Some(row) = &row {
            self.keys.errors.remove(row);
        }
        let removal = Edit::Remove(remove.to_vec());
        let mut required = 0;
        let patches = match edit
            .patches(&self.keys.entries, &self.keys.globals)
            .and_then(|mut patches| {
                // A moved row's removal and re-addition stand or fall
                // together, as do a reassignment's removals with its edit.
                if !remove.is_empty() || matches!(edit, Edit::MoveUser { .. }) {
                    required = patches.len();
                }
                patches.extend(
                    removal.patches(&self.keys.entries, &self.keys.globals)?,
                );
                Ok(patches)
            }) {
            Ok(patches) => patches,
            Err(message) => {
                self.status.failed(message);
                cx.notify();
                return false;
            }
        };
        let change = KeyChange {
            edit,
            row,
            patches,
            required,
            field,
        };
        if self.writing.is_some() {
            self.queued.push_back(Queued::Keys(change));
        } else {
            self.start_keys(change, cx);
        }
        cx.notify();
        true
    }

    fn start_keys(&mut self, change: KeyChange, cx: &mut Context<'_, Self>) {
        self.writing = Some("keybinding");
        self.keys.writing_row.clone_from(&change.row);
        self.status.saving();
        let file = ConfigFile::new(self.path.clone());
        let validate = crate::config::validator(self.path.clone());
        let path = self.path.clone();
        let base = self.base.clone();
        let patches = change.patches.clone();
        let task = cx.background_spawn(async move {
            let base = match base {
                Some(base) => base,
                None => file.load()?,
            };
            let snapshot = file.apply(&base, &patches, &validate)?;
            Ok::<_, ApplyError>(Written {
                file: FileValues::parse(snapshot.text()),
                keys: listed(&snapshot, &path),
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
        self.keys.writing_row = None;
        self.epoch += 1;
        // A field's failed edit stays in the field as a draft to retry;
        // any other shows under its row.
        let report = |view: &mut Self, message: String| {
            match (&change.field, &change.row) {
                (Some((key, saved)), _) => {
                    view.keys.errors.insert(key.clone(), message.clone());
                    match saved {
                        Some(saved) => {
                            view.keys.synced.insert(key.clone(), saved.clone());
                        }
                        None => {
                            view.keys.synced.remove(key);
                        }
                    }
                }
                (None, Some(row)) => {
                    view.keys.errors.insert(row.clone(), message.clone());
                }
                (None, None) => {}
            }
            view.status.failed(message);
        };
        let failed = match result {
            Ok(written) => {
                if let Ok(file) = written.file {
                    self.file = file;
                }
                self.keys.inventory = written.keys.inventory;
                self.keys.entries = written.keys.entries;
                self.set_globals(written.keys.globals);
                self.wrote(written.snapshot);
                self.rebase_queued_keys(&change.patches);
                self.status.saved();
                self.follow_changed_default(
                    &change.edit,
                    change.row.as_deref(),
                    &change.patches,
                );
                self.forget_gone_layouts(cx);
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
                // The field keeps its text as a draft for after the fix, as
                // do fields whose queued edits go unwritten.
                report(self, "config.toml has an error. Not saved.".to_owned());
                self.drop_queue_for_invalid_file(cx);
                self.pending.clear();
                self.status.invalid_file(message);
                self.sync_fields(cx);
                true
            }
            Err(ApplyError::WrittenNotDurable(error)) => {
                // The write landed; queued edits move past it as after any other.
                self.rebase_queued_keys(&change.patches);
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

    /// Queues `batch` after any write in flight, showing its outcome in
    /// `pending` meanwhile; `false` when the window cannot write.
    pub(super) fn submit_batch(
        &mut self,
        batch: Batch,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if self.locked() {
            return false;
        }
        let outcome = self.batch_outcome(&batch);
        for (path, value) in &outcome {
            self.pending.insert(path, value.clone());
        }
        if self.writing.is_some() {
            self.queued.push_back(Queued::Batch(batch, outcome));
        } else {
            self.start_batch(batch, outcome, cx);
        }
        cx.notify();
        true
    }

    /// The pending values `batch` shows until it settles.
    fn batch_outcome(&self, batch: &Batch) -> Outcome {
        match batch {
            Batch::RenameProfile { from, to } => self.rename_outcome(from, to),
            Batch::RenameLayout { from, to } => {
                self.layout_rename_outcome(from, to)
            }
            Batch::DeleteLayout { name } => self.layout_delete_outcome(name),
            Batch::Theme(changes) => changes
                .iter()
                .map(|(key, value)| (*key, value.clone().map(Value::String)))
                .collect(),
        }
    }

    /// Drops what a batch put in `pending`, unless a newer change replaced
    /// it.
    fn settle_batch(&mut self, outcome: &Outcome) {
        for (path, value) in outcome {
            if self.pending.get(path) == Some(value) {
                self.pending.remove(path);
            }
        }
    }

    fn start_batch(
        &mut self,
        batch: Batch,
        outcome: Outcome,
        cx: &mut Context<'_, Self>,
    ) {
        let built = match &batch {
            Batch::RenameProfile { from, to } => self.rename_patches(from, to),
            Batch::RenameLayout { from, to } => {
                self.layout_rename_patches(from, to)
            }
            Batch::DeleteLayout { name } => {
                Ok(vec![patch_at(&layouts::template_path(name), None)])
            }
            Batch::Theme(changes) => Ok(theme_patches(changes)),
        };
        let patches = match built {
            Ok(patches) => patches,
            Err(message) => {
                self.settle_batch(&outcome);
                self.status.failed(message.clone());
                self.batch_refused(&batch, message, cx);
                self.next_write(true, cx);
                return;
            }
        };
        self.writing = Some("batch");
        self.writing_batch = Some(outcome);
        self.status.saving();
        let started = Self::batch_started(&batch, cx);
        #[cfg(test)]
        let gate = self.batch_gate.take();
        let file = ConfigFile::new(self.path.clone());
        let validate = crate::config::validator(self.path.clone());
        let config_path = self.path.clone();
        let base = self.base.clone();
        let rewrites = batch.clone();
        let task = cx.background_spawn(async move {
            let mut patches = patches;
            let current = match file.load() {
                Ok(current) => current,
                Err(error) => return (patches, Err(BatchError::Apply(error))),
            };
            match rewrites.current_patches(&current) {
                Ok(rewritten) => patches.extend(rewritten),
                Err(refusal) => {
                    return (patches, Err(BatchError::Refused(refusal)));
                }
            }
            let base = base.filter(|_| !rewrites.fresh()).unwrap_or(current);
            let written = file
                .apply(&base, &patches, &validate)
                .map(|snapshot| Written {
                    file: FileValues::parse(snapshot.text()),
                    keys: listed(&snapshot, &config_path),
                    snapshot,
                })
                .map_err(BatchError::Apply);
            (patches, written)
        });
        cx.spawn(async move |view, cx| {
            let (patches, result) = task.await;
            #[cfg(test)]
            if let Some(gate) = gate {
                let _ = gate.recv().await;
            }
            // Settled here, even if the window has closed meanwhile.
            let landed = matches!(
                result,
                Ok(_)
                    | Err(BatchError::Apply(ApplyError::WrittenNotDurable(_)))
            );
            cx.update(|cx| Self::batch_settled(started, landed, cx));
            let _ = view.update(cx, |view, cx| {
                view.written_batch(&batch, &patches, result, cx);
            });
        })
        .detach();
    }

    fn written_batch(
        &mut self,
        batch: &Batch,
        patches: &[Patch],
        result: Result<Written, BatchError>,
        cx: &mut Context<'_, Self>,
    ) {
        self.writing = None;
        self.epoch += 1;
        if let Some(outcome) = self.writing_batch.take() {
            self.settle_batch(&outcome);
        }
        let failed = match result {
            Ok(written) => {
                if let Ok(file) = written.file {
                    self.forward_queued(&file, &written.snapshot);
                    self.file = file;
                }
                self.keys.inventory = written.keys.inventory;
                self.keys.entries = written.keys.entries;
                self.set_globals(written.keys.globals);
                self.wrote(written.snapshot);
                self.rebase_queued_keys(patches);
                self.status.saved();
                self.sync_fields(cx);
                self.forget_gone_layouts(cx);
                self.sync_key_fields(cx);
                cx.defer(|cx| {
                    let _ = super::super::reload(cx);
                });
                false
            }
            Err(BatchError::Apply(ApplyError::WrittenNotDurable(error))) => {
                // The write landed; queued edits move past it as after any
                // other. With no snapshot of it, a queued change to a path
                // it wrote reads the file afresh.
                for queued in &mut self.queued {
                    if let Queued::Setting(queued) = queued
                        && patches
                            .iter()
                            .any(|patch| writes(patch, queued.path))
                    {
                        queued.base = None;
                    }
                }
                self.rebase_queued_keys(patches);
                self.status.saved_with_warning(error.to_string());
                self.load(cx);
                cx.defer(|cx| {
                    let _ = super::super::reload(cx);
                });
                true
            }
            Err(BatchError::Apply(ApplyError::InvalidFile(message))) => {
                self.status.invalid_file(message);
                self.batch_refused(
                    batch,
                    "config.toml has an error. Not saved.".to_owned(),
                    cx,
                );
                self.drop_queue_for_invalid_file(cx);
                self.pending.clear();
                self.sync_fields(cx);
                true
            }
            Err(BatchError::Apply(ApplyError::Conflict(changes))) => {
                let message =
                    format!("config.toml changed: {}", changes.join("; "));
                self.status.failed("config.toml changed on disk");
                self.batch_refused(batch, message, cx);
                self.load(cx);
                true
            }
            // The window's own read of what refused it may be stale.
            Err(BatchError::Refused(message)) => {
                self.status.failed(message.clone());
                self.batch_refused(batch, message, cx);
                self.load(cx);
                true
            }
            Err(BatchError::Apply(error)) => {
                let message = error.to_string();
                self.status.failed(message.clone());
                self.batch_refused(batch, message, cx);
                true
            }
        };
        self.next_write(failed, cx);
    }

    /// Follows `batch` outside the window as its write starts, since the
    /// write can replace the file, and a reload read it, before the save
    /// settles here. A renamed profile's open Quake window takes the new
    /// name in the first reload that reads it, so no reload takes it for a
    /// removed profile's window and turns it into an ordinary one. Nothing
    /// outside the window follows a renamed or deleted pane layout.
    fn batch_started(
        batch: &Batch,
        cx: &mut Context<'_, Self>,
    ) -> Option<QuakeRenameId> {
        match batch {
            Batch::RenameProfile { from, to } => Some(
                cx.global_mut::<Desktop>()
                    .windows
                    .quake_rename_started(from, to),
            ),
            Batch::RenameLayout { .. }
            | Batch::DeleteLayout { .. }
            | Batch::Theme(_) => None,
        }
    }

    /// Holds the next batch's save after its write, before it settles,
    /// until the returned sender sends or drops.
    #[cfg(all(test, feature = "test-support"))]
    pub(in crate::desktop::windows) fn hold_batch(
        &mut self,
    ) -> async_channel::Sender<()> {
        let (release, gate) = async_channel::bounded(1);
        self.batch_gate = Some(gate);
        release
    }

    /// Settles what `batch_started` began once the write `landed` in the
    /// file or wrote nothing.
    fn batch_settled(
        started: Option<QuakeRenameId>,
        landed: bool,
        cx: &mut App,
    ) {
        if let Some(started) = started {
            cx.global_mut::<Desktop>()
                .windows
                .quake_rename_settled(started, landed);
        }
    }

    /// Says why `batch` wrote nothing, after the footer has said how the
    /// write failed. Changes made under a renamed profile's new name while
    /// it waited go too: written alone, they would create the profile.
    fn batch_refused(
        &mut self,
        batch: &Batch,
        message: String,
        cx: &mut Context<'_, Self>,
    ) {
        match batch {
            Batch::RenameProfile { from, to } => {
                let table = quake::profile_path(to);
                let message = if self.drop_profile_edits(&table, cx) {
                    format!(
                        "{message} The changes made to {to} meanwhile were \
                         not saved."
                    )
                } else {
                    message
                };
                self.rename_refused(from, to, message, cx);
            }
            Batch::RenameLayout { from, to } => {
                let message = if self.drop_layout_edits(to, cx) {
                    format!(
                        "{message} Changes made meanwhile that name {to} \
                         were not saved."
                    )
                } else {
                    message
                };
                self.layout_rename_refused(from, message, cx);
            }
            Batch::DeleteLayout { name } => {
                self.layout_delete_refused(name, message, cx);
            }
            // The footer said why; the picker shows the file again.
            Batch::Theme(_) => {}
        }
    }

    /// Drops the queued and pending changes to the settings of the profile
    /// at `table`, a profile that will not exist, and puts their fields
    /// back to the file's values, so leaving a field does not submit its
    /// change again. Returns whether a change went.
    fn drop_profile_edits(
        &mut self,
        table: &str,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let prefix = format!("{table}.");
        self.drop_edits(|path| path.starts_with(&prefix), cx)
    }

    /// Drops the queued and pending changes to the settings whose paths
    /// `dropped` matches, and puts their fields back to the file's values,
    /// so leaving a field does not submit its change again. Returns whether
    /// a change went.
    pub(super) fn drop_edits(
        &mut self,
        dropped: impl Fn(&str) -> bool,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let (queued, pending) = (self.queued.len(), self.pending.len());
        self.queued.retain(|queued| {
            !matches!(queued, Queued::Setting(queued) if dropped(queued.path))
        });
        self.pending.retain(|path, _| !dropped(path));
        let went = self.queued.len() != queued || self.pending.len() != pending;
        self.errors.retain(|path, _| !dropped(path));
        self.conflicts.retain(|path, _| !dropped(path));
        let fields: Vec<&'static str> = self
            .fields
            .keys()
            .copied()
            .filter(|path| dropped(path))
            .collect();
        for path in fields {
            self.sync_field(path, true, cx);
        }
        went
    }

    /// Drops the queued deletions of the pane layout `name`, settling what
    /// they show. Returns whether one went.
    pub(super) fn drop_queued_deletes(&mut self, name: &str) -> bool {
        let mut dropped = Vec::new();
        self.queued.retain(|queued| match queued {
            Queued::Batch(Batch::DeleteLayout { name: deleted }, outcome)
                if deleted == name =>
            {
                dropped.push(outcome.clone());
                false
            }
            _ => true,
        });
        for outcome in &dropped {
            self.settle_batch(outcome);
        }
        !dropped.is_empty()
    }

    /// Moves each queued setting change whose path still holds what it
    /// expected onto `snapshot`, which this window just wrote: nothing else
    /// touched that path, so the change can build on the write.
    fn forward_queued(&mut self, file: &FileValues, snapshot: &Snapshot) {
        for queued in &mut self.queued {
            if let Queued::Setting(queued) = queued
                && file.get(queued.path) == queued.expected.as_ref()
            {
                queued.base = Some(snapshot.clone());
            }
        }
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
                keys: listed(&snapshot, &config_path),
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

    /// Whether the file holds `value` for `setting`, or will once its
    /// pending change lands. Picking such a value writes nothing; picking a
    /// default the file leaves unset still writes it.
    pub(super) fn holds(
        &self,
        setting: &'static Setting,
        value: &Value,
    ) -> bool {
        match self.pending.get(setting.path.as_str()) {
            Some(pending) => pending.as_ref() == Some(value),
            None => self.file.get(&setting.path) == Some(value),
        }
    }

    /// Settles a refused creation or deletion of a profile at once: it has
    /// no row to hold a conflict or an error, so the profile bar says why,
    /// and the file is read again to show what it holds. `false` for any
    /// other write.
    fn profile_write_refused(
        &mut self,
        change: &Pending,
        error: &ApplyError,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let key = change.setting.path.as_str();
        if !quake::is_profile_table(key)
            || matches!(error, ApplyError::WrittenNotDurable(_))
        {
            return false;
        }
        if self.pending.get(key) == Some(&change.value) {
            self.pending.remove(key);
        }
        let message = match error {
            ApplyError::Conflict(changes) => {
                format!("config.toml changed: {}", changes.join("; "))
            }
            error => error.to_string(),
        };
        if change.value.is_some() {
            // Edits to the new profile's settings would create it, or write
            // into a profile another editor made, so they go.
            self.drop_profile_edits(key, cx);
        }
        // This change's name comes back before any queued creation's.
        self.profile_failed(key, change.value.is_some(), message.clone(), cx);
        if let ApplyError::InvalidFile(invalid) = error {
            self.drop_queue_for_invalid_file(cx);
            self.pending.clear();
            self.status.invalid_file(invalid.clone());
        } else {
            self.status.failed(message);
            self.load(cx);
        }
        true
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
        if let Err(error) = &result
            && self.profile_write_refused(&change, error, cx)
        {
            self.next_write(true, cx);
            return;
        }
        let mut failed = false;
        match result {
            Ok(written) => {
                self.errors.remove(key);
                self.conflicts.remove(key);
                settle(self);
                // A queued change whose path still holds what it expected
                // can build on this write: nothing else touched it.
                if let Ok(file) = written.file {
                    self.forward_queued(&file, &written.snapshot);
                    self.file = file;
                }
                self.keys.inventory = written.keys.inventory;
                self.keys.entries = written.keys.entries;
                self.set_globals(written.keys.globals);
                self.wrote(written.snapshot);
                self.status.saved();
                let saved = field_text(shown(
                    change.setting,
                    &self.file,
                    &self.defaults,
                ));
                self.adopt_saved_field(key, change.typed.as_deref(), saved, cx);
                self.sync_fields(cx);
                self.forget_gone_layouts(cx);
                reload(cx);
            }
            Err(ApplyError::Conflict(changes)) => {
                failed = true;
                let creating =
                    change.expected.is_none() && change.value.is_some();
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
                self.drop_deletes_of_failed_create(key, creating);
                self.load(cx);
            }
            Err(ApplyError::InvalidFile(message)) => {
                failed = true;
                // Nothing can be written to an invalid file, so every
                // pending change settles unsaved.
                self.drop_queue_for_invalid_file(cx);
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
                self.drop_deletes_of_failed_create(
                    key,
                    change.expected.is_none() && change.value.is_some(),
                );
                self.errors.insert(key, message.clone());
                self.status.failed(message);
                self.sync_fields(cx);
            }
        }
        self.next_write(failed, cx);
    }

    /// Drops every queued write after the file turned out invalid. A
    /// keybinding field whose edit goes unwritten keeps its text as a draft,
    /// marked, for after the fix.
    fn drop_queue_for_invalid_file(&mut self, cx: &mut Context<'_, Self>) {
        for queued in std::mem::take(&mut self.queued) {
            if let Queued::Batch(batch, outcome) = &queued {
                self.settle_batch(outcome);
                self.batch_refused(
                    batch,
                    "config.toml has an error. Not saved.".to_owned(),
                    cx,
                );
                continue;
            }
            // A queued profile creation gives its name back to retry.
            if let Queued::Setting(change) = &queued
                && change.value.is_some()
                && quake::is_profile_table(change.path)
            {
                self.profile_failed(
                    change.path,
                    true,
                    "config.toml has an error. Not saved.".to_owned(),
                    cx,
                );
                continue;
            }
            let Queued::Keys(KeyChange {
                field: Some((key, saved)),
                ..
            }) = queued
            else {
                continue;
            };
            self.keys.errors.insert(
                key.clone(),
                "config.toml has an error. Not saved.".to_owned(),
            );
            match saved {
                Some(saved) => {
                    self.keys.synced.insert(key, saved);
                }
                None => {
                    self.keys.synced.remove(&key);
                }
            }
        }
    }

    /// Carries queued keybinding edits past a write this window just made.
    /// A queued patch for an entry that write changed, matched by position
    /// and content, now finds it by its new content; one for an entry it
    /// removed goes, with its edit when nothing of that is left; and entries
    /// after a removed one move up.
    fn rebase_queued_keys(&mut self, landed: &[Patch]) {
        let mut changed: Vec<(List, usize, Table, Option<Table>)> = Vec::new();
        for patch in landed {
            match patch {
                Patch::SetEntryField {
                    locator,
                    field,
                    value,
                } => {
                    let mut after = locator.content().clone();
                    after.insert(field.clone(), value.clone());
                    changed.push((
                        locator.list(),
                        locator.index(),
                        locator.content().clone(),
                        Some(after),
                    ));
                }
                Patch::ClearEntryField { locator, field } => {
                    let mut after = locator.content().clone();
                    after.remove(field);
                    changed.push((
                        locator.list(),
                        locator.index(),
                        locator.content().clone(),
                        Some(after),
                    ));
                }
                Patch::RemoveEntry { locator } => {
                    changed.push((
                        locator.list(),
                        locator.index(),
                        locator.content().clone(),
                        None,
                    ));
                }
                _ => {}
            }
        }
        if changed.is_empty() {
            return;
        }
        let removed: Vec<(List, usize)> = changed
            .iter()
            .filter(|(_, _, _, after)| after.is_none())
            .map(|(list, index, _, _)| (*list, *index))
            .collect();
        self.queued.retain_mut(|queued| {
            let Queued::Keys(change) = queued else {
                return true;
            };
            let had = change.patches.len();
            let mut position = 0;
            let mut orphaned = false;
            change.patches.retain_mut(|patch| {
                let required = position < change.required;
                position += 1;
                let (Patch::SetEntryField { locator, .. }
                | Patch::ClearEntryField { locator, .. }
                | Patch::RemoveEntry { locator }
                | Patch::UpdateEntry { locator, .. }) = patch
                else {
                    return true;
                };
                let (list, index) = (locator.list(), locator.index());
                let content =
                    match changed.iter().find(|(within, at, before, _)| {
                        *within == list
                            && *at == index
                            && before == locator.content()
                    }) {
                        Some((.., None)) => {
                            orphaned |= required;
                            return false;
                        }
                        Some((.., Some(after))) => after.clone(),
                        None => locator.content().clone(),
                    };
                let moved = index
                    - removed
                        .iter()
                        .filter(|(within, gone)| {
                            *within == list && *gone < index
                        })
                        .count();
                *locator = locator.rebased(moved, content);
                true
            });
            !orphaned && (had == 0 || !change.patches.is_empty())
        });
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
            Some(Queued::Batch(next, outcome)) => {
                self.start_batch(next, outcome, cx);
            }
            None if self.closing => {
                let handle = self.handle;
                cx.defer(move |cx| {
                    let _ = handle
                        .update(cx, |_, window, _| window.remove_window());
                });
            }
            None if std::mem::take(&mut self.reread) => self.load(cx),
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
    pub(super) fn display(
        &self,
        setting: &Setting,
        value: Option<Value>,
    ) -> String {
        field_text(value.or_else(|| self.default_value(setting)))
    }

    /// The value the setting takes when the file leaves it unset, if any.
    pub(super) fn default_value(&self, setting: &Setting) -> Option<Value> {
        self.defaults
            .get(setting.path.as_str())
            .or(setting.default.as_ref())
            .cloned()
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
        // A field's open menu holds focus, and its field's draft waits for
        // the menu to close. Closing it hands focus back to the field, so the
        // draft saves below, and a draft that does not save keeps focus there
        // and refuses every close until it does.
        if matches!(self.menu, Some((_, super::MenuFor::Field(_), _))) {
            self.close_menu(window, cx);
        }
        if !self.settle_menu_left(cx) {
            return false;
        }
        if let Some(setting) = self.focused_field(window, cx) {
            self.field_blurred(setting, cx);
            if self.errors.contains_key(setting.path.as_str()) {
                return false;
            }
        }
        // A keybinding field's draft saves too; one that does not save
        // keeps the window open to show why.
        if let Some((key, _)) = self.focused_key_field(window, cx)
            && key != "search"
        {
            self.commit_key_field(&key, cx);
            if self.keys.errors.contains_key(&key) {
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
        let Some(setting) = self.setting_at(path) else {
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
    /// an editor that later work adds. The theme resets the keys its
    /// picker writes, keeping color overrides. A
    /// pane layout's template has no default: resetting it would delete
    /// the layout without asking, so only Delete removes it.
    pub(super) fn resettable(&self, setting: &Setting) -> bool {
        match special(&setting.path) {
            Some(
                Special::Keybindings
                | Special::GlobalShortcuts
                | Special::QuakeProfiles
                | Special::PaneLayouts
                | Special::LayoutCycle,
            ) => false,
            _ if layouts::is_template(&setting.path) => false,
            Some(Special::ThemePicker) => self.theme_set(),
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
            self.reset_theme(cx);
            return;
        }
        // Reset discards whatever was typed into the field.
        if let Some(text) = self.synced.get(path).cloned() {
            self.show_text(path, text, cx);
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
            // A template that does not parse is never saved.
            _ if layouts::is_template(path) => layouts::template_value(&text),
            SettingKind::Integer { .. } | SettingKind::Number { .. } => {
                parse_number(setting, &text)
            }
            _ if text.trim().is_empty() => Ok(None),
            _ => Ok(Some(Value::String(text.trim().to_owned()))),
        };
        // A number clamped to its range shows the value it saves.
        let text = match &parsed {
            Ok(Some(value))
                if self.numbers.contains_key(path)
                    && text.trim().parse::<f64>().ok() != as_number(value) =>
            {
                let clamped = self.display(setting, Some(value.clone()));
                self.show_text(path, clamped.clone(), cx);
                clamped
            }
            _ => text,
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
        let text = match (self.pending.get(path), self.setting_at(path)) {
            // A pending reset shows the default.
            (Some(value), Some(setting)) => {
                Some(self.display(setting, value.clone()))
            }
            _ => self.synced.get(path).cloned(),
        };
        if let Some(text) = text {
            self.show_text(path, text, cx);
        }
        cx.notify();
    }
}
