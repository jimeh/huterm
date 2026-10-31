//! The Quake windows section: a profile dropdown and the chosen profile's
//! card, new, renamed, and deleted profiles, and global shortcuts.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use gpui::{
    AnyElement, App, Context, Entity, Focusable, IntoElement, ParentElement,
    SharedString, Styled, Window, div, prelude::*, px,
};
use huterm_config::document::{EntryLocator, Patch};
use huterm_config::settings::{Choice, Setting, SettingKind, SettingsModel};
use huterm_protocol::{ids, lookup};
use toml::{Table, Value};

use super::super::super::key_hint::KeyHint;
use super::super::super::overlay::Swatch;
use super::super::super::settings::keybindings::{Edit, names_profile};
use super::super::Desktop;
use super::controls::Opener;
use super::dialog::Prompt;
use super::keybindings::{CAP_WIDTH, global_tag, small};
use super::render::group_card;
use super::saving::Batch;
use super::{MenuFor, SettingsView};
use crate::keymap::Platform;
use huterm_ui::components::alert_dialog::AlertDialog;
use huterm_ui::components::button::Variant;
use huterm_ui::components::dialog::{Dialog, Dismissal};
use huterm_ui::components::empty::Empty;
use huterm_ui::components::field::Field;
use huterm_ui::components::input::{Chrome, Input};
use huterm_ui::components::input_group::InputGroup;
use huterm_ui::components::key_recorder::KeyRecorderButton;
use huterm_ui::components::switch::Switch;
use huterm_ui::listbox::{
    Item as MenuItem, Model as MenuModel, Row as MenuRow,
};

const PREFIX: &str = "quake.profiles.";
const DEFAULT: &str = "default";
/// The commands a global shortcut may run, in menu order.
pub(super) const COMMANDS: [huterm_protocol::CommandId; 3] =
    [ids::TOGGLE_QUAKE, ids::SHOW_QUAKE, ids::HIDE_QUAKE];

/// The section's state, held by the view.
pub(super) struct Quake {
    /// The profile the card edits.
    pub(super) profile: String,
    /// The text the New profile or Rename profile dialog's name was refused
    /// for, and why; the message shows while the field still holds it.
    pub(super) name_error: Option<(String, String)>,
    /// Why the write of a confirmed deletion was refused.
    pub(super) delete_error: Option<String>,
}

impl Quake {
    pub(super) fn new() -> Self {
        Self {
            profile: DEFAULT.to_owned(),
            name_error: None,
            delete_error: None,
        }
    }
}

/// Why a typed name cannot name a new or renamed profile.
struct NameProblem {
    message: String,
    /// Whether the dialog shows it before the user confirms; a blank or
    /// unchanged name only disables the primary button.
    shown: bool,
}

/// Whether Settings can edit a profile named `name`: a bare TOML key, so
/// its dotted paths split back into the same keys.
pub(super) fn bare(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The dotted path of the profile named `name`'s table.
pub(super) fn profile_path(name: &str) -> String {
    format!("{PREFIX}{name}")
}

/// Whether `path` names a whole profile, `quake.profiles.<name>`, as its
/// creation and deletion write.
pub(super) fn is_profile_table(path: &str) -> bool {
    path.strip_prefix(PREFIX).is_some_and(bare)
}

/// The setting at `path` within a profile: `quake.profiles.<name>` for the
/// profile's table, or `quake.profiles.<name>.<field>` for one of its
/// settings, made from the model's template on first use. Controls hold
/// `&'static Setting`, so each is made once and kept.
pub(super) fn profile_setting(
    model: &'static SettingsModel,
    path: &str,
) -> Option<&'static Setting> {
    static INTERNED: OnceLock<Mutex<HashMap<String, &'static Setting>>> =
        OnceLock::new();
    let rest = path.strip_prefix(PREFIX)?;
    let (name, field) = match rest.split_once('.') {
        Some((name, field)) => (name, Some(field)),
        None => (rest, None),
    };
    if !bare(name) {
        return None;
    }
    let mut interned = INTERNED
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(setting) = interned.get(path) {
        return Some(setting);
    }
    let table = model.setting("quake")?;
    let mut setting = match field {
        None => Setting {
            path: path.to_owned(),
            label: name.to_owned(),
            description: None,
            kind: SettingKind::Compound,
            default: None,
            unit: None,
            visible_when: Vec::new(),
            ..table.clone()
        },
        Some(field) => {
            let mut setting = model
                .profile_settings()
                .iter()
                .find(|setting| setting.path == field)?
                .clone();
            path.clone_into(&mut setting.path);
            for condition in &mut setting.visible_when {
                condition.setting =
                    format!("{PREFIX}{name}.{}", condition.setting);
            }
            // The schema takes any `id:` identifier; the menu offers the
            // three named displays, and a file's identifier shows as itself.
            if field == "display" {
                setting.kind = SettingKind::Choice(
                    [
                        ("active", "Active display"),
                        ("pointer", "Display under the pointer"),
                        ("primary", "Primary display"),
                    ]
                    .into_iter()
                    .map(|(value, label)| Choice {
                        value: value.to_owned(),
                        label: Some(label.to_owned()),
                        description: None,
                    })
                    .collect(),
                );
            }
            setting
        }
    };
    setting.order = 0;
    let setting: &'static Setting = Box::leak(Box::new(setting));
    interned.insert(path.to_owned(), setting);
    Some(setting)
}

/// One `[[global_keybinding]]` entry, as the section lists it.
pub(super) struct Global {
    pub(super) key: String,
    pub(super) command: String,
    /// `None` runs the default profile.
    pub(super) profile: Option<String>,
    /// Whether `disabled = true` keeps it from registering.
    pub(super) disabled: bool,
}

impl Global {
    fn of(content: &Table) -> Self {
        let text = |value: Option<&Value>| {
            value.and_then(Value::as_str).unwrap_or_default().to_owned()
        };
        Self {
            key: text(content.get("key")),
            command: text(content.get("command")),
            profile: content
                .get("args")
                .and_then(|args| args.get("profile"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            disabled: content.get("disabled").and_then(Value::as_bool)
                == Some(true),
        }
    }
}

impl SettingsView {
    /// The file's profiles, with `default` first, counting creations and
    /// deletions still being written.
    pub(super) fn profiles(&self) -> Vec<String> {
        let mut names: Vec<String> = match self.file.get("quake.profiles") {
            Some(Value::Table(profiles)) => profiles.keys().cloned().collect(),
            _ => Vec::new(),
        };
        for (path, value) in &self.pending {
            let Some(name) = path.strip_prefix(PREFIX) else {
                continue;
            };
            if name.contains('.') {
                continue;
            }
            match value {
                Some(_) if !names.iter().any(|known| known == name) => {
                    names.push(name.to_owned());
                }
                None => names.retain(|known| known != name),
                Some(_) => {}
            }
        }
        names.retain(|name| name != DEFAULT);
        names.sort();
        names.insert(0, DEFAULT.to_owned());
        names
    }

    /// The profile the card shows: the chosen one while it exists, else
    /// `default`.
    pub(super) fn shown_profile(&self) -> String {
        let profile = &self.quake.profile;
        if self.profiles().iter().any(|name| name == profile) {
            profile.clone()
        } else {
            DEFAULT.to_owned()
        }
    }

    pub(super) fn globals(&self) -> Vec<Global> {
        self.keys
            .globals
            .iter()
            .map(|locator| Global::of(locator.content()))
            .collect()
    }

    /// Takes a new list of global shortcuts. Messages under their rows
    /// describe the list they were made against, so they go once it
    /// changes, such as after another editor's change.
    pub(super) fn set_globals(&mut self, globals: Vec<EntryLocator>) {
        if self.keys.globals != globals {
            // An open editor names its shortcut by position, which may now
            // be another shortcut's.
            let moved = |entry: usize| {
                self.keys.globals.get(entry).map(EntryLocator::content)
                    != globals.get(entry).map(EntryLocator::content)
            };
            if self
                .recorder_key()
                .and_then(|key| key.strip_prefix("grec:")?.parse().ok())
                .is_some_and(moved)
            {
                self.keys.recorder_stale = true;
            }
            if let Some((_, MenuFor::Global { entry, .. }, _)) = &self.menu
                && moved(*entry)
            {
                // Focus held by the menu goes to the window.
                let _ = self.drop_menu();
                self.keys.stranded = true;
            }
            self.keys.errors.retain(|row, _| {
                row != "gadd"
                    && !row.starts_with("gadd:")
                    && row
                        .strip_prefix('g')
                        .is_none_or(|rest| rest.parse::<usize>().is_err())
            });
        }
        self.keys.globals = globals;
    }

    /// The number of values the file sets in its profiles, for the
    /// sidebar.
    pub(super) fn profile_values(&self) -> usize {
        match self.file.get("quake.profiles") {
            Some(Value::Table(profiles)) => profiles
                .values()
                .map(|profile| profile.as_table().map_or(1, Table::len))
                .sum(),
            _ => 0,
        }
    }

    /// Runs a Quake windows action key; `false` when `kind` is not one.
    pub(super) fn act_quake(
        &mut self,
        kind: &str,
        argument: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        match kind {
            "qgo" => {
                self.select_section(
                    huterm_config::settings::SectionId::Quake,
                    cx,
                );
                self.search.update(cx, |search, cx| search.set_text("", cx));
                self.action_stop("qprof".to_owned(), cx).focus(window, cx);
            }
            "qprof" => self.open_profiles(window, cx),
            // Nothing that writes opens while the file is read-only.
            "qnew" | "qren" | "qdel" if self.locked() => {}
            "qnew" => {
                self.quake.name_error = None;
                let field = Self::name_field("", cx);
                let new = self.action_stop("qnew".to_owned(), cx);
                self.open_field_dialog(Prompt::NewProfile, field, &new);
            }
            "qcreate" => self.create_profile(window, cx),
            "qcancel" => {
                self.quake.name_error = None;
                self.close_dialog(window, cx);
            }
            "qren" => {
                let profile = self.shown_profile();
                if profile != DEFAULT && bare(&profile) {
                    self.quake.name_error = None;
                    let field = Self::name_field(&profile, cx);
                    field.update(cx, Input::select_all);
                    let rename = self.action_stop("qren".to_owned(), cx);
                    self.open_field_dialog(
                        Prompt::RenameProfile(profile),
                        field,
                        &rename,
                    );
                }
            }
            "qrename" => self.rename_profile(window, cx),
            "qrenno" => {
                self.quake.name_error = None;
                self.close_dialog(window, cx);
                // A profile removed meanwhile took its Rename profile with
                // it.
                if self.shown_profile() == DEFAULT {
                    self.action_stop("qprof".to_owned(), cx).focus(window, cx);
                }
            }
            "qdel" => {
                let profile = self.shown_profile();
                let prompt = match self.delete_refusal(&profile) {
                    Some(message) => Prompt::DeleteRefused { profile, message },
                    None => Prompt::DeleteProfile(profile),
                };
                self.quake.delete_error = None;
                let delete = self.action_stop("qdel".to_owned(), cx);
                self.open_dialog(prompt, &delete);
            }
            "qdelno" => {
                self.close_dialog(window, cx);
                // A profile removed meanwhile took its Delete profile with
                // it.
                if self.shown_profile() == DEFAULT {
                    self.action_stop("qprof".to_owned(), cx).focus(window, cx);
                }
            }
            "qdelyes" => self.delete_profile(window, cx),
            "grec" if argument.parse::<usize>().is_ok() => {
                self.start_recording(&format!("grec:{argument}"), window, cx);
            }
            "gadd" => self.start_recording(&add_key(argument), window, cx),
            "gcmd" | "gprofile" => {
                if let Ok(entry) = argument.parse() {
                    self.open_global_menu(entry, kind == "gcmd", window, cx);
                }
            }
            "gon" => {
                if let Ok(entry) = argument.parse() {
                    self.switch_global(entry, cx);
                }
            }
            "gdel" => {
                if let Ok(entry) = argument.parse::<usize>() {
                    // Keybindings lists the shortcut under its command.
                    let next = match self.globals().get(entry) {
                        Some(global)
                            if self.section
                                == huterm_config::settings::SectionId::Keybindings =>
                        {
                            format!("kcmd:{}", global.command)
                        }
                        _ => "gadd".to_owned(),
                    };
                    self.keys.follow = Some((next, format!("gdel:{entry}")));
                    self.submit_keys(
                        Edit::RemoveGlobal(entry),
                        Some(format!("g{entry}")),
                        None,
                        cx,
                    );
                }
            }
            _ => return false,
        }
        true
    }

    fn open_profiles(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let rows = self
            .profiles()
            .into_iter()
            .map(|name| {
                let name = super::keybindings::interned(&name);
                MenuRow::Item(MenuItem::new(name, name))
            })
            .collect();
        let opener = self.action_stop("qprof".to_owned(), cx);
        self.show_menu(
            MenuFor::Profile,
            MenuModel::new(rows),
            opener,
            window,
            cx,
        );
    }

    /// The menu item a global shortcut's `command` or `profile` matches.
    pub(super) fn global_choice(
        &self,
        entry: usize,
        field: &str,
    ) -> Option<String> {
        let global = self.globals().into_iter().nth(entry)?;
        Some(if field == "command" {
            global.command
        } else {
            global.profile.unwrap_or_else(|| DEFAULT.to_owned())
        })
    }

    /// Shows why a profile's creation or deletion was refused. A refused
    /// creation reopens the New profile dialog with its name; while another
    /// dialog is open, the footer alone says why.
    pub(super) fn profile_failed(
        &mut self,
        path: &str,
        created: bool,
        message: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(name) = path.strip_prefix(PREFIX) else {
            return;
        };
        if !created {
            self.pick_profile(name);
            self.quake.delete_error = Some(message);
        } else if self.dialog.prompt().is_none() {
            let field = Self::name_field(name, cx);
            self.quake.name_error = Some((name.to_owned(), message));
            let new = self.action_stop("qnew".to_owned(), cx);
            self.open_field_dialog(Prompt::NewProfile, field, &new);
        } else if self.dialog.prompt() == Some(&Prompt::NewProfile)
            && let Some(field) = self.dialog.field().cloned()
        {
            let text = field.read(cx).text().to_owned();
            let typed = text.trim();
            if !typed.is_empty() && typed != name {
                // A name typed for another profile since stays in the field.
                self.quake.name_error =
                    Some((text, format!("{name}: {message}")));
            } else {
                // The open dialog keeps its focus and takes the name back.
                field.update(cx, |field, cx| field.set_text(name, cx));
                self.quake.name_error = Some((name.to_owned(), message));
            }
        }
        cx.notify();
    }

    /// Shows why a rename of `from` to `to` wrote nothing, in an alert over
    /// `from`'s card. While another dialog is open the footer says why
    /// instead, unless the file's error already fills it, and the card
    /// behind that dialog stays as it is unless it shows `to`, which will
    /// not exist.
    pub(super) fn rename_refused(
        &mut self,
        from: &str,
        to: &str,
        message: String,
        cx: &mut Context<'_, Self>,
    ) {
        if self.dialog.prompt().is_none() {
            self.pick_profile(from);
            let rename = self.action_stop("qren".to_owned(), cx);
            self.open_dialog(
                Prompt::RenameRefused {
                    profile: from.to_owned(),
                    message,
                },
                &rename,
            );
        } else {
            if self.quake.profile == to {
                self.pick_profile(from);
            }
            if !self.status.invalid() {
                self.status.failed(message);
            }
        }
        cx.notify();
    }

    /// Shows the profile picked from the dropdown.
    pub(in crate::desktop::windows) fn pick_profile(&mut self, name: &str) {
        name.clone_into(&mut self.quake.profile);
        self.quake.delete_error = None;
    }

    /// A name field holding `text`, for the New profile and Rename profile
    /// dialogs.
    fn name_field(text: &str, cx: &mut Context<'_, Self>) -> Entity<Input> {
        let swatch = Self::swatch(cx);
        cx.new(|cx| {
            let mut field =
                Input::new("quake-profile-name", "Profile name", swatch, cx)
                    .with_placeholder("Profile name")
                    .with_chrome(Chrome::Grouped);
            field.set_text(text, cx);
            field
        })
    }

    /// Why `name` cannot name a new profile, or `from`'s new name when
    /// renaming.
    fn name_check(
        &self,
        name: &str,
        from: Option<&str>,
        cx: &App,
    ) -> Result<(), NameProblem> {
        let (message, shown) = if self.locked() {
            ("config.toml has an error. Not saved.".to_owned(), false)
        } else if name.is_empty() {
            ("Type a name for the profile.".to_owned(), false)
        } else if from == Some(name) {
            ("Type a new name for the profile.".to_owned(), false)
        } else if !bare(name) {
            (
                "Use letters, digits, - and _ only; other names need \
                 quoting, so add them in config.toml."
                    .to_owned(),
                true,
            )
        } else if self.profiles().iter().any(|known| known == name) {
            (format!("A profile named {name} exists."), true)
        } else if from.is_some()
            && cx.global::<Desktop>().windows.quake_name_held(name)
        {
            // A removed profile's window keeps its name until it becomes an
            // ordinary window, and the renamed profile's window would turn
            // ordinary instead of taking the name.
            (
                format!(
                    "A window for {name} is still closing; try again in a \
                     moment."
                ),
                true,
            )
        } else {
            return Ok(());
        };
        Err(NameProblem { message, shown })
    }

    /// The message under the open dialog's name field holding `text`: why
    /// it was refused, or else what is wrong with it as typed.
    fn name_message(
        &self,
        text: &str,
        check: &Result<(), NameProblem>,
    ) -> Option<String> {
        self.quake
            .name_error
            .as_ref()
            .filter(|(refused, _)| refused == text)
            .map(|(_, message)| message.clone())
            .or_else(|| {
                check
                    .as_ref()
                    .err()
                    .filter(|problem| problem.shown)
                    .map(|problem| problem.message.clone())
            })
    }

    /// The open name dialog's typed name, or why it cannot be used, which
    /// then shows under the field.
    fn checked_name(&mut self, from: Option<&str>, cx: &App) -> Option<String> {
        let text = self.dialog.field()?.read(cx).text().to_owned();
        let name = text.trim().to_owned();
        match self.name_check(&name, from, cx) {
            Ok(()) => Some(name),
            Err(problem) => {
                self.quake.name_error = Some((text, problem.message));
                None
            }
        }
    }

    /// Writes an empty table for the New profile dialog's name and shows
    /// it.
    fn create_profile(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.dialog.prompt() != Some(&Prompt::NewProfile) {
            return;
        }
        let Some(name) = self.checked_name(None, cx) else {
            return;
        };
        let Some(setting) =
            profile_setting(self.model, &format!("{PREFIX}{name}"))
        else {
            return;
        };
        let table = Value::Table(Table::new());
        self.submit(
            setting,
            Patch::Set {
                path: vec!["quake".into(), "profiles".into(), name.clone()],
                value: table.clone(),
            },
            Some(table),
            None,
            cx,
        );
        self.pick_profile(&name);
        self.quake.name_error = None;
        self.close_dialog(window, cx);
        self.action_stop("qprof".to_owned(), cx).focus(window, cx);
    }

    /// Renames the profile the open Rename profile dialog names to its
    /// typed name, in one save with the shortcuts that name it, and keeps
    /// showing it under the new name.
    fn rename_profile(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(Prompt::RenameProfile(from)) = self.dialog.prompt().cloned()
        else {
            return;
        };
        let Some(to) = self.checked_name(Some(&from), cx) else {
            return;
        };
        if !self.submit_batch(
            Batch::RenameProfile {
                from,
                to: to.clone(),
            },
            cx,
        ) {
            return;
        }
        self.pick_profile(&to);
        self.quake.name_error = None;
        // Focus returns to Rename profile, now the renamed profile's.
        self.close_dialog(window, cx);
    }

    /// What a rename shows until it lands: `from` gone, and `to` there
    /// with each value `from` sets or is saving. An edit to `to` meanwhile
    /// then starts from the value the rename writes, and builds on it.
    pub(super) fn rename_outcome(
        &self,
        from: &str,
        to: &str,
    ) -> Vec<(&'static str, Option<Value>)> {
        let mut outcome: Vec<(&'static str, Option<Value>)> =
            [(from, None), (to, Some(Value::Table(Table::new())))]
                .into_iter()
                .filter_map(|(name, value)| {
                    let setting =
                        profile_setting(self.model, &profile_path(name))?;
                    Some((setting.path.as_str(), value))
                })
                .collect();
        for template in self.model.profile_settings() {
            let field = |name: &str| {
                profile_setting(
                    self.model,
                    &format!("{}.{}", profile_path(name), template.path),
                )
            };
            let (Some(source), Some(target)) = (field(from), field(to)) else {
                continue;
            };
            let value = match self.pending.get(source.path.as_str()) {
                Some(pending) => pending.clone(),
                None => self.file.get(&source.path).cloned(),
            };
            if value.is_some() {
                outcome.push((target.path.as_str(), value));
            }
        }
        outcome
    }

    /// A rename's patches for the profile's table, built from the file as
    /// the window last saw it: `to` set to `from`'s table and `from`
    /// removed. The shortcuts that name `from` are found when the batch
    /// applies (`Batch::current_patches`).
    pub(super) fn rename_patches(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<Patch>, String> {
        let Some(Value::Table(table)) =
            self.file.get(&format!("{PREFIX}{from}"))
        else {
            return Err(format!(
                "config.toml no longer has a profile named {from}."
            ));
        };
        if self.file.get(&format!("{PREFIX}{to}")).is_some() {
            return Err(format!("A profile named {to} exists."));
        }
        let path = |name: &str| {
            vec!["quake".to_owned(), "profiles".to_owned(), name.to_owned()]
        };
        Ok(vec![
            Patch::Set {
                path: path(to),
                value: Value::Table(table.clone()),
            },
            Patch::Remove { path: path(from) },
        ])
    }

    /// Why `profile` cannot be deleted now, if it cannot.
    fn delete_refusal(&self, profile: &str) -> Option<String> {
        if self.locked() {
            return Some("config.toml has an error. Not saved.".to_owned());
        }
        // The validator refuses a global shortcut for a missing profile.
        let global = self
            .globals()
            .into_iter()
            .find(|global| global.profile.as_deref() == Some(profile))?;
        let key = KeyHint::parse(&global.key, Platform::current()).text();
        Some(format!(
            "The global shortcut {key} uses this profile. Remove it, or give \
             it another profile, first."
        ))
    }

    /// Deletes the profile the open alert asks about, or shows why it
    /// cannot in the alert's place.
    fn delete_profile(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(Prompt::DeleteProfile(name)) = self.dialog.prompt().cloned()
        else {
            return;
        };
        if let Some(message) = self.delete_refusal(&name) {
            let delete = self.action_stop("qdel".to_owned(), cx);
            self.open_dialog(
                Prompt::DeleteRefused {
                    profile: name,
                    message,
                },
                &delete,
            );
            return;
        }
        self.close_dialog(window, cx);
        let setting = (name != DEFAULT && self.profiles().contains(&name))
            .then(|| profile_setting(self.model, &format!("{PREFIX}{name}")))
            .flatten();
        if let Some(setting) = setting {
            // The user confirmed deleting the profile as it is now, so the
            // write reads the file afresh rather than conflicting with an
            // edit to it.
            self.fresh = Some(setting.path.as_str());
            self.submit(
                setting,
                huterm_config::document::Patch::Remove {
                    path: vec!["quake".into(), "profiles".into(), name],
                },
                None,
                None,
                cx,
            );
            self.pick_profile(DEFAULT);
        }
        // Delete profile went with the profile.
        self.action_stop("qprof".to_owned(), cx).focus(window, cx);
    }

    fn open_global_menu(
        &mut self,
        entry: usize,
        command: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.keys_read_only() {
            return;
        }
        let rows = if command {
            COMMANDS
                .iter()
                .filter_map(|id| lookup(id.as_str()))
                .map(|spec| {
                    MenuRow::Item(MenuItem::new(spec.id.as_str(), spec.title))
                })
                .collect()
        } else {
            self.profiles()
                .into_iter()
                .map(|name| {
                    let name = super::keybindings::interned(&name);
                    MenuRow::Item(MenuItem::new(name, name))
                })
                .collect()
        };
        let field = if command { "command" } else { "profile" };
        let key = if command {
            format!("gcmd:{entry}")
        } else {
            format!("gprofile:{entry}")
        };
        let opener = self.action_stop(key, cx);
        self.show_menu(
            MenuFor::Global { entry, field },
            MenuModel::new(rows),
            opener,
            window,
            cx,
        );
    }

    /// Writes a command or profile picked for a global shortcut.
    pub(in crate::desktop::windows) fn pick_global(
        &mut self,
        entry: usize,
        field: &'static str,
        picked: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(global) = self.globals().into_iter().nth(entry) else {
            return;
        };
        let (current, value) = if field == "command" {
            (Some(global.command), Some(picked.to_owned()))
        } else {
            // An unset profile is the `default` profile.
            (
                global.profile,
                (picked != DEFAULT).then(|| picked.to_owned()),
            )
        };
        if current == value
            || (field == "profile" && current.is_none() && value.is_none())
        {
            return;
        }
        self.submit_keys(
            Edit::ChangeGlobal {
                entry,
                field,
                value: value.map(Value::String),
            },
            Some(format!("g{entry}")),
            None,
            cx,
        );
    }

    /// Switches a global shortcut on or off. Switching on clears its
    /// `disabled` rather than writing `false`, as a binding's switch does.
    fn switch_global(&mut self, entry: usize, cx: &mut Context<'_, Self>) {
        let Some(global) = self.globals().into_iter().nth(entry) else {
            return;
        };
        self.submit_keys(
            Edit::ChangeGlobal {
                entry,
                field: "disabled",
                value: (!global.disabled).then_some(Value::Boolean(true)),
            },
            Some(format!("g{entry}")),
            None,
            cx,
        );
    }

    /// Refuses a recorded global shortcut that needs a modifier, under
    /// `row`; true when the key may register.
    fn global_key_checked(&mut self, key: &str, row: String) -> bool {
        // A global shortcut works in every application, so a key without
        // a modifier would stop reaching them.
        if global_key_allowed(key) {
            return true;
        }
        self.keys.errors.insert(
            row,
            if cfg!(target_os = "macos") {
                "A global shortcut needs Control, Option, or Command, or a \
                 function key, since it works in every app. Not saved."
            } else {
                "A global shortcut needs Ctrl, Alt, or Super, or a function \
                 key, since it works in every app. Not saved."
            }
            .to_owned(),
        );
        false
    }

    /// Adds a recorded global shortcut: in the Quake windows section, one
    /// that toggles the profile it shows; in Keybindings, one that runs
    /// `command` on the default profile.
    pub(super) fn add_global(
        &mut self,
        command: Option<huterm_protocol::CommandId>,
        key: String,
        cx: &mut Context<'_, Self>,
    ) {
        let row = add_key(command.map_or("", |command| command.as_str()));
        if !self.global_key_checked(&key, row.clone()) {
            return;
        }
        let (command, profile) = command.map_or_else(
            || {
                let profile = self.shown_profile();
                (ids::TOGGLE_QUAKE, (profile != DEFAULT).then_some(profile))
            },
            |command| (command, None),
        );
        if let Some(spec) = lookup(command.as_str()) {
            self.keys.open.insert(spec.id);
        }
        self.submit_keys(
            Edit::AddGlobal {
                key,
                command,
                profile,
            },
            Some(row),
            None,
            cx,
        );
    }

    /// Writes a recorded key for a global shortcut.
    pub(super) fn finish_global(
        &mut self,
        entry: usize,
        key: String,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.global_key_checked(&key, format!("g{entry}")) {
            return;
        }
        if self
            .globals()
            .get(entry)
            .is_none_or(|global| global.key != key)
        {
            self.submit_keys(
                Edit::ChangeGlobal {
                    entry,
                    field: "key",
                    value: Some(Value::String(key)),
                },
                Some(format!("g{entry}")),
                None,
                cx,
            );
        }
    }

    /// The section: the profile dropdown and card, then global shortcuts.
    pub(super) fn render_quake(
        &mut self,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Vec<AnyElement> {
        vec![
            self.render_profile_bar(swatch, window, cx),
            self.render_profile(swatch, window, cx),
            self.render_globals(swatch, cx),
        ]
    }

    fn render_profile_bar(
        &mut self,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let reason = self.lock_reason();
        let profile = self.shown_profile();
        // Choosing which profile to show writes nothing, so it stays
        // available while the file is read-only.
        let dropdown = self.select(
            "settings-qprof".to_owned(),
            "qprof",
            "Profile".to_owned(),
            profile,
            Opener::Action("qprof".to_owned()),
            false,
            px(150.0),
            swatch,
            cx,
        );
        let new = self
            .action_button(
                "settings-qnew".to_owned(),
                "New profile",
                "qnew".to_owned(),
                swatch,
                cx,
            )
            .when_some(reason, |button, reason| button.disabled(Some(reason)));
        let _ = window;
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .mb(px(12.0))
            .child(small("Profile".to_owned(), swatch.muted))
            .child(dropdown)
            .child(new)
            .into_any_element()
    }

    fn render_profile(
        &mut self,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let profile = self.shown_profile();
        let title = format!("Profile \u{201c}{profile}\u{201d}");
        let mut rows = Vec::new();
        if bare(&profile) {
            let fields: Vec<&'static Setting> = self
                .model
                .profile_settings()
                .iter()
                .filter_map(|template| {
                    profile_setting(
                        self.model,
                        &format!("{PREFIX}{profile}.{}", template.path),
                    )
                })
                .filter(|setting| self.shows(setting, false))
                .collect();
            for setting in fields {
                rows.push(self.render_row(setting, swatch, window, cx));
            }
        } else {
            rows.push(
                div()
                    .px(px(14.0))
                    .py(px(10.0))
                    .child(small(
                        "This profile's name needs quoting in TOML, so \
                         Settings leaves it to config.toml."
                            .to_owned(),
                        swatch.muted,
                    ))
                    .into_any_element(),
            );
        }
        if profile != DEFAULT && bare(&profile) {
            rows.push(self.render_profile_actions(swatch, cx));
        }
        group_card("settings-qprofile", &title, rows, swatch)
    }

    /// A named profile's Rename profile and Delete profile.
    fn render_profile_actions(
        &mut self,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let reason = self.lock_reason();
        let rename = self
            .action_button(
                "settings-qren".to_owned(),
                "Rename profile",
                "qren".to_owned(),
                swatch,
                cx,
            )
            .when_some(reason.clone(), |button, reason| {
                button.disabled(Some(reason))
            });
        let delete = self
            .action_button(
                "settings-qdel".to_owned(),
                "Delete profile",
                "qdel".to_owned(),
                swatch,
                cx,
            )
            .when_some(reason, |button, reason| button.disabled(Some(reason)));
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(14.0))
            .py(px(10.0))
            .child(div().flex().gap(px(8.0)).child(rename).child(delete))
            .child(small(
                "An open window of a deleted profile becomes an ordinary \
                 window and keeps its tabs."
                    .to_owned(),
                swatch.muted,
            ))
            .children(
                self.quake
                    .delete_error
                    .clone()
                    .map(|error| small(error, swatch.danger_text())),
            )
            .into_any_element()
    }

    /// The alert Delete profile opens: whether to delete the profile, or
    /// why it cannot be. Cancel, or OK on a refusal, returns focus to Delete
    /// profile. A profile removed while the alert is open, such as by
    /// another editor, closes it.
    pub(super) fn render_delete_alert(
        &mut self,
        prompt: &Prompt,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let (Prompt::DeleteProfile(profile)
        | Prompt::DeleteRefused { profile, .. }) = prompt
        else {
            return None;
        };
        if !self.profiles().iter().any(|name| name == profile) {
            cx.defer_in(window, |view, window, cx| {
                view.dismiss_dialog(Dismissal::OwnerChanged, window, cx);
            });
            return None;
        }
        let alert = match prompt {
            Prompt::DeleteProfile(_) => {
                let (cancel, cancel_focus) = self.dialog_button(
                    "settings-qdelno",
                    "Cancel",
                    "qdelno",
                    swatch,
                    cx,
                );
                let (delete, delete_focus) = self.dialog_button(
                    "settings-qdelyes",
                    "Delete",
                    "qdelyes",
                    swatch,
                    cx,
                );
                let mut description = "Its settings and comments are removed \
                                       from config.toml."
                    .to_owned();
                if let Some(note) = self.keybinding_note(profile) {
                    description.push(' ');
                    description.push_str(&note);
                }
                AlertDialog::new(
                    "settings-delete-profile",
                    format!("Delete the \u{201c}{profile}\u{201d} profile?"),
                    self.dialog.modal(),
                    swatch,
                )
                .description(description)
                .cancel(cancel, &cancel_focus)
                .action(delete.variant(Variant::Destructive), &delete_focus)
            }
            Prompt::DeleteRefused { message, .. } => {
                let (ok, ok_focus) = self.dialog_button(
                    "settings-qdelno",
                    "OK",
                    "qdelno",
                    swatch,
                    cx,
                );
                AlertDialog::new(
                    "settings-delete-profile",
                    format!(
                        "The \u{201c}{profile}\u{201d} profile cannot be \
                         deleted"
                    ),
                    self.dialog.modal(),
                    swatch,
                )
                .description(message.clone())
                .cancel(ok.variant(Variant::Primary), &ok_focus)
            }
            Prompt::NewProfile
            | Prompt::RenameProfile(_)
            | Prompt::RenameRefused { .. }
            | Prompt::NewLayout
            | Prompt::RenameLayout(_)
            | Prompt::LayoutRenameRefused { .. }
            | Prompt::DeleteLayout(_)
            | Prompt::LayoutDeleteRefused { .. }
            | Prompt::CycleRefused(_)
            | Prompt::Reassign { .. } => return None,
        };
        Some(alert.into_any_element())
    }

    /// What deleting `profile` does to the keybindings that run it, which
    /// the file still accepts once it is gone: each fails when pressed.
    fn keybinding_note(&self, profile: &str) -> Option<String> {
        let keys: Vec<String> = self
            .keys
            .entries
            .iter()
            .map(EntryLocator::content)
            .filter(|entry| {
                names_profile(entry, profile)
                    && entry.get("disabled").and_then(Value::as_bool)
                        != Some(true)
            })
            .map(|entry| {
                let key = entry.get("key").and_then(Value::as_str);
                KeyHint::parse(key.unwrap_or_default(), Platform::current())
                    .text()
            })
            .collect();
        match keys.as_slice() {
            [] => None,
            [key] => Some(format!(
                "The keybinding {key} runs this profile and will fail."
            )),
            keys => Some(format!(
                "The keybindings {} run this profile and will fail.",
                keys.join(", ")
            )),
        }
    }

    /// The New profile or Rename profile dialog: a name field with its
    /// message, Cancel, and Create or Rename, which stays disabled while
    /// the name cannot be used. A profile removed while its Rename profile
    /// dialog is open, such as by another editor, closes the dialog.
    pub(super) fn render_name_dialog(
        &mut self,
        prompt: &Prompt,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let field = self.dialog.field()?.clone();
        let from = match prompt {
            Prompt::RenameProfile(from) => Some(from.as_str()),
            _ => None,
        };
        if let Some(from) = from
            && !self.profiles().iter().any(|name| name == from)
        {
            cx.defer_in(window, |view, window, cx| {
                view.dismiss_dialog(Dismissal::OwnerChanged, window, cx);
            });
            return None;
        }
        let text = field.read(cx).text().to_owned();
        let check = self.name_check(text.trim(), from, cx);
        let message = self.name_message(&text, &check);
        field.update(cx, |field, cx| field.set_invalid(message.is_some(), cx));
        let refusal: Option<SharedString> = self
            .lock_reason()
            .or_else(|| check.err().map(|problem| problem.message.into()));
        let (id, title, cancel, confirm, label) = match from {
            None => (
                "settings-new-profile",
                "New profile".to_owned(),
                "qcancel",
                "qcreate",
                "Create",
            ),
            Some(from) => (
                "settings-rename-profile",
                format!("Rename the \u{201c}{from}\u{201d} profile"),
                "qrenno",
                "qrename",
                "Rename",
            ),
        };
        let (cancel, cancel_focus) = self.dialog_button(
            if from.is_none() {
                "settings-qcancel"
            } else {
                "settings-qrenno"
            },
            "Cancel",
            cancel,
            swatch,
            cx,
        );
        let (primary, primary_focus) = self.dialog_button(
            if from.is_none() {
                "settings-qcreate"
            } else {
                "settings-qrename"
            },
            label,
            confirm,
            swatch,
            cx,
        );
        let primary = primary
            .variant(Variant::Primary)
            .when_some(refusal, |button, reason| button.disabled(Some(reason)));
        let handle = field.read(cx).focus_handle(cx);
        let name = Field::new("settings-qname", "Name", swatch)
            .vertical()
            .error(message.map(SharedString::from))
            .control(Self::text_field(cx).w_full().child(InputGroup::new(
                "settings-qname-group",
                &field,
                swatch,
            )))
            .p_0();
        Some(
            Dialog::new(id, title, self.dialog.modal(), swatch)
                .when(from.is_some(), |dialog| {
                    dialog.description(
                        "Shortcuts that use it follow the new name. Comments \
                         in its table are removed from config.toml.",
                    )
                })
                .focusable(&handle)
                .initial_focus(&handle)
                .child(name)
                .button(cancel, &cancel_focus)
                .button(primary, &primary_focus)
                .into_any_element(),
        )
    }

    /// The alert that says why a rename wrote nothing, with OK, which
    /// returns focus to Rename profile.
    pub(super) fn render_rename_refusal(
        &mut self,
        profile: &str,
        message: String,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let (ok, ok_focus) =
            self.dialog_button("settings-qrenno", "OK", "qrenno", swatch, cx);
        AlertDialog::new(
            "settings-rename-refused",
            format!("The \u{201c}{profile}\u{201d} profile was not renamed"),
            self.dialog.modal(),
            swatch,
        )
        .description(message)
        .cancel(ok.variant(Variant::Primary), &ok_focus)
        .into_any_element()
    }

    fn render_globals(
        &mut self,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let read_only = self.keys_read_only();
        let globals = self.globals();
        let mut rows = Vec::new();
        if globals.is_empty() {
            rows.push(
                Empty::new("settings-gempty", "No global shortcuts", swatch)
                    .description(
                        "A global shortcut shows or hides a quake window \
                         while another application has focus.",
                    )
                    .into_any_element(),
            );
        }
        for (index, global) in globals.iter().enumerate() {
            rows.push(self.render_global(index, global, false, swatch, cx));
        }
        let recorder = self.recorder("gadd", cx);
        let add = KeyRecorderButton::new(
            "settings-gadd",
            "Add global shortcut",
            &recorder,
            swatch,
        )
        .placeholder("Add global shortcut")
        .key_context("SettingsControl")
        .px(px(10.0))
        .when(read_only, |add| add.disabled(self.keys_lock_reason()));
        rows.push(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .px(px(14.0))
                .py(px(10.0))
                .child(div().flex().child(add))
                .children(self.recording_hint("gadd", swatch, cx))
                .children(
                    self.keys
                        .errors
                        .get("gadd")
                        .cloned()
                        .map(|error| small(error, swatch.danger_text())),
                )
                .when(wayland(), |block| {
                    block.child(small(
                        "Global shortcuts need macOS or a native X11 \
                         session; this Wayland session does not support \
                         them."
                            .to_owned(),
                        swatch.muted,
                    ))
                })
                .into_any_element(),
        );
        group_card("settings-globals", "Global shortcuts", rows, swatch)
    }

    /// One global shortcut's row: its key, command, profile, switch, and
    /// remove button. Under its command in Keybindings, it drops the
    /// command select for a Global tag.
    pub(super) fn render_global(
        &mut self,
        index: usize,
        global: &Global,
        under_command: bool,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let read_only = self.keys_read_only();
        let record_key = format!("grec:{index}");
        let recorder = self.recorder(&record_key, cx);
        let cap = KeyRecorderButton::new(
            gpui::ElementId::Name(format!("settings-grec-{index}").into()),
            "Global shortcut",
            &recorder,
            swatch,
        )
        .shortcut(Some(global.key.clone()))
        .key_context("SettingsControl")
        .w(px(CAP_WIDTH))
        .border_color(swatch.accent.opacity(0.5))
        .when(read_only, |cap| cap.disabled(self.keys_lock_reason()));
        // Under its command, a Global tag stands where a shortcut's tag
        // does.
        let command = (!under_command).then(|| {
            let command = lookup(&global.command).map_or_else(
                || global.command.clone(),
                |spec| spec.title.to_owned(),
            );
            self.global_select(
                &format!("gcmd:{index}"),
                command,
                read_only,
                swatch,
                cx,
            )
        });
        let tag = under_command.then(|| global_tag(index, swatch));
        let profile = self.global_select(
            &format!("gprofile:{index}"),
            global.profile.clone().unwrap_or_else(|| DEFAULT.to_owned()),
            read_only,
            swatch,
            cx,
        );
        let switch_key = format!("gon:{index}");
        let handle = self.action_stop(switch_key.clone(), cx);
        let focus = handle.clone();
        let toggle = Switch::new(
            gpui::ElementId::Name(format!("settings-gon-{index}").into()),
            format!(
                "Global shortcut {}",
                KeyHint::parse(&global.key, Platform::current()).text()
            ),
            !global.disabled,
            &handle,
            swatch,
        )
        .key_context("SettingsControl")
        .when(read_only, |switch| switch.disabled(self.keys_lock_reason()))
        .on_toggle(Self::listen(cx, move |view, _, window, cx| {
            focus.focus(window, cx);
            view.act(&switch_key, window, cx);
        }));
        let error = self.keys.errors.get(&format!("g{index}")).cloned();
        // Under its command, the row lines up with the shortcuts above it.
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .when(!under_command, |row| row.px(px(14.0)).py(px(8.0)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(if under_command { 6.0 } else { 8.0 }))
                    .child(cap)
                    .children(command)
                    .child(profile)
                    .child(div().flex_1())
                    .children(tag)
                    .child(toggle)
                    // A read-only row draws no remove button.
                    .when(!read_only, |row| {
                        row.child(self.remove_button(
                            format!("settings-gdel-{index}"),
                            format!("gdel:{index}"),
                            &global.key,
                            swatch,
                            cx,
                        ))
                    }),
            )
            .children(self.recording_hint(&record_key, swatch, cx))
            .children(error.map(|error| small(error, swatch.danger_text())))
            .into_any_element()
    }

    fn global_select(
        &mut self,
        key: &str,
        label: String,
        read_only: bool,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let name = if key.starts_with("gcmd:") {
            "Command"
        } else {
            "Profile"
        };
        self.select(
            format!("settings-{key}"),
            key,
            name.to_owned(),
            label,
            Opener::Action(key.to_owned()),
            read_only,
            px(150.0),
            swatch,
            cx,
        )
    }
}

/// The action key of a recorder that adds a global shortcut: the Quake
/// windows section's, or the one under `command` in Keybindings.
pub(super) fn add_key(command: &str) -> String {
    if command.is_empty() {
        "gadd".to_owned()
    } else {
        format!("gadd:{command}")
    }
}

/// Whether this session cannot register global shortcuts.
pub(super) fn wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// What desktop tests drive and observe in the section.
#[cfg(all(test, feature = "test-support"))]
impl SettingsView {
    /// Opens the New profile dialog, unless it is open, and types `name`
    /// into its field.
    pub(in crate::desktop::windows) fn type_profile_name(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.dialog.prompt() != Some(&Prompt::NewProfile) {
            self.act("qnew", window, cx);
        }
        self.type_dialog_name(name, cx);
    }

    /// Replaces the open dialog's name with `name`, as typing does.
    pub(in crate::desktop::windows) fn type_dialog_name(
        &mut self,
        name: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(field) = self.dialog.field() {
            field.update(cx, |field, cx| field.set_text(name, cx));
        }
    }

    /// Runs a dialog button's action key, as a click on it does.
    pub(in crate::desktop::windows) fn answer(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.act(key, window, cx);
    }

    pub(in crate::desktop::windows) fn shown_profile_name(&self) -> String {
        self.shown_profile()
    }

    pub(in crate::desktop::windows) fn profile_names(&self) -> Vec<String> {
        self.profiles()
    }

    /// The setting for a profile's field, when Settings edits it.
    pub(in crate::desktop::windows) fn profile_field(
        &self,
        profile: &str,
        field: &str,
    ) -> Option<&'static Setting> {
        profile_setting(self.model, &format!("{PREFIX}{profile}.{field}"))
    }

    /// The message under the open dialog's name field, or Delete
    /// profile's.
    pub(in crate::desktop::windows) fn quake_message(
        &self,
        cx: &App,
    ) -> Option<String> {
        let from = match self.dialog.prompt() {
            Some(Prompt::RenameProfile(from)) => Some(from.as_str()),
            _ => None,
        };
        self.dialog
            .field()
            .and_then(|field| {
                let text = field.read(cx).text();
                self.name_message(text, &self.name_check(text.trim(), from, cx))
            })
            .or_else(|| self.quake.delete_error.clone())
    }

    /// Whether the New profile dialog or the delete alert shows.
    pub(in crate::desktop::windows) fn quake_prompts(&self) -> (bool, bool) {
        (
            self.dialog.prompt() == Some(&Prompt::NewProfile),
            matches!(self.dialog.prompt(), Some(Prompt::DeleteProfile(_))),
        )
    }

    /// The open dialog's name field's text.
    pub(in crate::desktop::windows) fn profile_name_text(
        &self,
        cx: &App,
    ) -> String {
        self.dialog
            .field()
            .map(|field| field.read(cx).text().to_owned())
            .unwrap_or_default()
    }

    /// Whether a global shortcut's menu is open.
    pub(in crate::desktop::windows) fn global_menu_open(&self) -> bool {
        matches!(self.menu, Some((_, MenuFor::Global { .. }, _)))
    }

    /// Opens a select's menu, as Space on it does.
    pub(in crate::desktop::windows) fn open_select_for(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let setting = self.setting_at(path).expect("a known setting");
        self.open_select(setting, window, cx);
    }

    /// The message under a setting's row.
    pub(in crate::desktop::windows) fn setting_error(
        &self,
        path: &str,
    ) -> Option<String> {
        self.errors.get(path).cloned()
    }

    /// The message under a global shortcut row, `g<n>`, or Add.
    pub(in crate::desktop::windows) fn global_error(
        &self,
        row: &str,
    ) -> Option<String> {
        self.keys.errors.get(row).cloned()
    }
}

/// Whether a recorded key may be a global shortcut: one with Ctrl, Alt, or
/// Cmd, or a function key the platform registers (F1-F20 on macOS, F1-F24
/// on X11, as `quake::hotkeys` accepts).
fn global_key_allowed(key: &str) -> bool {
    let Ok(stroke) = gpui::Keystroke::parse_with_os(
        key,
        crate::keymap::Platform::current().os(),
    ) else {
        return false;
    };
    let modifiers = stroke.modifiers;
    let maximum_function_key = if cfg!(target_os = "macos") { 20 } else { 24 };
    modifiers.control
        || modifiers.alt
        || modifiers.platform
        || stroke
            .key
            .strip_prefix('f')
            .and_then(|number| number.parse::<u8>().ok())
            .is_some_and(|number| (1..=maximum_function_key).contains(&number))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_keys_need_a_modifier_or_a_registrable_function_key() {
        for key in ["ctrl-alt-shift-y", "alt-space", "cmd-f13", "f1", "f20"] {
            assert!(global_key_allowed(key), "{key}");
        }
        for key in ["a", "shift-a", "space", "f0", "f25", "f99"] {
            assert!(!global_key_allowed(key), "{key}");
        }
    }
}
