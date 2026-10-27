//! The Quake windows section: a profile dropdown and the chosen profile's
//! card, new and deleted profiles, and global shortcuts.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use gpui::{
    AnyElement, App, Context, Entity, Focusable, InteractiveElement,
    IntoElement, ParentElement, Styled, Window, div, prelude::*, px,
};
use huterm_config::document::EntryLocator;
use huterm_config::settings::{Choice, Setting, SettingKind, SettingsModel};
use huterm_protocol::{ids, lookup};
use toml::{Table, Value};

use super::super::super::key_hint::KeyHint;
use super::super::super::menu::{MenuItem, MenuModel, MenuRow};
use super::super::super::overlay::Swatch;
use super::super::super::settings::keybindings::Edit;
use super::controls::{caret, select_trigger};
use super::keybindings::{
    CAP_WIDTH, Target, card, group_block, remove_button, small,
};
use super::{MenuFor, SettingsView, button};
use crate::keymap::Platform;
use crate::ui::text_field::TextField;

const PREFIX: &str = "quake.profiles.";
const DEFAULT: &str = "default";
/// The commands a global shortcut may run, in menu order.
const COMMANDS: [huterm_protocol::CommandId; 3] =
    [ids::TOGGLE_QUAKE, ids::SHOW_QUAKE, ids::HIDE_QUAKE];

/// The section's state, held by the view.
pub(super) struct Quake {
    /// The profile the card edits.
    pub(super) profile: String,
    /// The New profile name field, while it is open.
    pub(super) naming: Option<Entity<TextField>>,
    pub(super) name_error: Option<String>,
    /// Delete profile is asking for confirmation.
    pub(super) deleting: bool,
    pub(super) delete_error: Option<String>,
}

impl Quake {
    pub(super) fn new() -> Self {
        Self {
            profile: DEFAULT.to_owned(),
            naming: None,
            name_error: None,
            deleting: false,
            delete_error: None,
        }
    }
}

/// Whether Settings can edit a profile named `name`: a bare TOML key, so
/// its dotted paths split back into the same keys.
pub(super) fn bare(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
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
            visible_when: None,
            ..table.clone()
        },
        Some(field) => {
            let mut setting = model
                .profile_settings()
                .iter()
                .find(|setting| setting.path == field)?
                .clone();
            path.clone_into(&mut setting.path);
            if let Some(condition) = &mut setting.visible_when {
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
struct Global {
    key: String,
    command: String,
    /// `None` runs the default profile.
    profile: Option<String>,
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

    fn globals(&self) -> Vec<Global> {
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
            if let Some(Target::Global(entry)) = self
                .keys
                .recording
                .as_ref()
                .map(|recording| recording.target.clone())
                && moved(entry)
            {
                self.keys.recording = None;
            }
            if let Some((_, MenuFor::Global { entry, .. }, _)) = &self.menu
                && moved(*entry)
            {
                // Focus held by the menu goes to the window.
                self.menu = None;
                self.keys.stranded = true;
            }
            self.keys.errors.retain(|row, _| {
                row != "gadd"
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
            "qnew" => self.start_naming(window, cx),
            "qcreate" => self.create_profile(window, cx),
            "qcancel" => self.stop_naming(window, cx),
            "qdel" if self.locked() => {}
            "qdel" => {
                self.quake.deleting = true;
                self.quake.delete_error = None;
                self.action_stop("qdelno".to_owned(), cx).focus(window, cx);
            }
            "qdelno" => {
                self.quake.deleting = false;
                self.action_stop("qdel".to_owned(), cx).focus(window, cx);
            }
            "qdelyes" => self.delete_profile(window, cx),
            "grec" => {
                if let Ok(entry) = argument.parse() {
                    let handle = self.action_stop(format!("grec:{entry}"), cx);
                    self.start_recording(
                        Target::Global(entry),
                        &handle,
                        window,
                        cx,
                    );
                }
            }
            "gadd" => {
                let handle = self.action_stop("gadd".to_owned(), cx);
                self.start_recording(Target::AddGlobal, &handle, window, cx);
            }
            "gcmd" | "gprofile" => {
                if let Ok(entry) = argument.parse() {
                    self.open_global_menu(entry, kind == "gcmd", window, cx);
                }
            }
            "gdel" => {
                if let Ok(entry) = argument.parse() {
                    self.keys.follow =
                        Some(("gadd".to_owned(), format!("gdel:{entry}")));
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

    /// Escape closes the delete confirmation, as Cancel does. Returns
    /// whether it took the key.
    pub(super) fn escape_delete(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let on_prompt = self
            .focused_action(window)
            .is_some_and(|key| key == "qdelyes" || key == "qdelno");
        if !self.quake.deleting
            || !on_prompt
            || keystroke.key != "escape"
            || keystroke.modifiers.modified()
        {
            return false;
        }
        self.act("qdelno", window, cx);
        true
    }

    /// Shows why a profile's creation or deletion was refused. A refused
    /// creation reopens the name field with its name.
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
        if created {
            // A name typed for another profile since stays in the field.
            let typing = self.quake.naming.as_ref().is_some_and(|field| {
                let text = field.read(cx).text().trim();
                !text.is_empty() && text != name
            });
            if typing {
                self.quake.name_error = Some(format!("{name}: {message}"));
            } else {
                let swatch = Self::swatch(cx);
                let field = cx.new(|cx| {
                    let mut field =
                        TextField::new("Profile name", swatch.fg, cx);
                    field.set_text(name, cx);
                    field
                });
                let _ = field.read(cx).focus_handle(cx).tab_stop(true);
                self.quake.naming = Some(field);
                self.quake.name_error = Some(message);
            }
        } else {
            self.pick_profile(name);
            self.quake.delete_error = Some(message);
        }
        cx.notify();
    }

    /// Shows the profile picked from the dropdown.
    pub(in crate::desktop::windows) fn pick_profile(&mut self, name: &str) {
        name.clone_into(&mut self.quake.profile);
        self.quake.deleting = false;
        self.quake.delete_error = None;
    }

    fn start_naming(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.locked() {
            return;
        }
        let swatch = Self::swatch(cx);
        let field = self.quake.naming.get_or_insert_with(|| {
            cx.new(|cx| TextField::new("Profile name", swatch.fg, cx))
        });
        let handle = field.read(cx).focus_handle(cx).tab_stop(true);
        handle.focus(window, cx);
        self.quake.name_error = None;
        self.quake.deleting = false;
    }

    fn stop_naming(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.quake.naming = None;
        self.quake.name_error = None;
        self.action_stop("qnew".to_owned(), cx).focus(window, cx);
    }

    /// Writes an empty table for the typed name and shows it.
    pub(super) fn create_profile(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(field) = &self.quake.naming else {
            return;
        };
        let name = field.read(cx).text().trim().to_owned();
        let error = if self.locked() {
            Some("config.toml has an error. Not saved.".to_owned())
        } else if name.is_empty() {
            Some("Type a name for the profile.".to_owned())
        } else if !bare(&name) {
            Some(
                "Use letters, digits, - and _ only; other names need \
                 quoting, so add them in config.toml."
                    .to_owned(),
            )
        } else if self.profiles().contains(&name) {
            Some(format!("A profile named {name} exists."))
        } else {
            None
        };
        if let Some(error) = error {
            self.quake.name_error = Some(error);
            return;
        }
        let Some(setting) =
            profile_setting(self.model, &format!("{PREFIX}{name}"))
        else {
            return;
        };
        let table = Value::Table(Table::new());
        self.submit(
            setting,
            huterm_config::document::Patch::Set {
                path: vec!["quake".into(), "profiles".into(), name.clone()],
                value: table.clone(),
            },
            Some(table),
            None,
            cx,
        );
        self.pick_profile(&name);
        self.quake.naming = None;
        self.quake.name_error = None;
        self.action_stop("qprof".to_owned(), cx).focus(window, cx);
    }

    fn delete_profile(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let name = self.shown_profile();
        self.quake.deleting = false;
        if name == DEFAULT {
            return;
        }
        if self.locked() {
            self.quake.delete_error =
                Some("config.toml has an error. Not saved.".to_owned());
            self.action_stop("qdel".to_owned(), cx).focus(window, cx);
            return;
        }
        // The validator refuses a global shortcut for a missing profile.
        if let Some(global) = self
            .globals()
            .into_iter()
            .find(|global| global.profile.as_deref() == Some(name.as_str()))
        {
            let key = KeyHint::parse(&global.key, Platform::current()).text();
            self.quake.delete_error = Some(format!(
                "The global shortcut {key} uses this profile. Remove it, or \
                 give it another profile, first."
            ));
            self.action_stop("qdel".to_owned(), cx).focus(window, cx);
            return;
        }
        let Some(setting) =
            profile_setting(self.model, &format!("{PREFIX}{name}"))
        else {
            return;
        };
        // The user confirmed deleting the profile as it is now, so the write
        // reads the file afresh rather than conflicting with an edit to it.
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

    /// Writes a recorded key for a global shortcut, or adds one.
    pub(super) fn finish_global(
        &mut self,
        target: Option<usize>,
        key: String,
        cx: &mut Context<'_, Self>,
    ) {
        // A global shortcut works in every application, so a key without
        // a modifier would stop reaching them.
        if !global_key_allowed(&key) {
            let row = target
                .map_or_else(|| "gadd".to_owned(), |entry| format!("g{entry}"));
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
            return;
        }
        let Some(entry) = target else {
            let profile = self.shown_profile();
            self.submit_keys(
                Edit::AddGlobal {
                    key,
                    profile: (profile != DEFAULT).then_some(profile),
                },
                Some("gadd".to_owned()),
                None,
                cx,
            );
            return;
        };
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
        let read_only = self.locked();
        let profile = self.shown_profile();
        let trigger = select_trigger("settings-qprof".to_owned(), swatch)
            .child(div().flex_1().child(profile))
            .child(caret(swatch));
        let trigger = self.actionable(trigger, "qprof".to_owned(), swatch, cx);
        let dropdown = self.with_menu("qprof", trigger.into_any_element());
        let mut bar = div().flex().flex_col().gap(px(6.0)).mb(px(12.0)).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(small("Profile".to_owned(), swatch.muted))
                .child(dropdown)
                .when(self.quake.naming.is_none(), |bar| {
                    let new = button("settings-qnew", swatch)
                        .when(read_only, |button| button.opacity(0.5))
                        .child("New profile");
                    let new =
                        self.actionable(new, "qnew".to_owned(), swatch, cx);
                    let _ = self
                        .action_stop("qnew".to_owned(), cx)
                        .tab_stop(!read_only);
                    bar.child(new)
                }),
        );
        if let Some(field) = self.quake.naming.clone() {
            self.drawn.insert("qname".to_owned());
            let create = button("settings-qcreate", swatch).child("Create");
            let create =
                self.actionable(create, "qcreate".to_owned(), swatch, cx);
            let cancel = button("settings-qcancel", swatch).child("Cancel");
            let cancel =
                self.actionable(cancel, "qcancel".to_owned(), swatch, cx);
            bar = bar
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .key_context("Palette")
                                .w(px(220.0))
                                .h(px(24.0))
                                .px(px(8.0))
                                .flex()
                                .items_center()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(
                                    if self.quake.name_error.is_some() {
                                        swatch.danger
                                    } else {
                                        swatch.line_strong
                                    },
                                )
                                .bg(swatch.bg)
                                .text_size(px(12.0))
                                .child(field),
                        )
                        .child(create)
                        .child(cancel),
                )
                .children(
                    self.quake
                        .name_error
                        .clone()
                        .map(|error| small(error, swatch.danger)),
                );
        }
        let _ = window;
        bar.into_any_element()
    }

    fn render_profile(
        &mut self,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let profile = self.shown_profile();
        let title = format!("Profile \u{201c}{profile}\u{201d}");
        let mut body = card(swatch);
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
            for (index, setting) in fields.into_iter().enumerate() {
                let row = self.render_row(setting, swatch, window, cx);
                body = body.child(
                    div()
                        .when(index > 0, |row| {
                            row.border_t_1().border_color(swatch.line)
                        })
                        .child(row),
                );
            }
        } else {
            body = body.child(
                div().px(px(14.0)).py(px(10.0)).child(small(
                    "This profile's name needs quoting in TOML, so Settings \
                     leaves it to config.toml."
                        .to_owned(),
                    swatch.muted,
                )),
            );
        }
        if profile != DEFAULT && bare(&profile) {
            body = body.child(self.render_delete(&profile, swatch, cx));
        }
        group_block(&title, body, swatch)
    }

    fn render_delete(
        &mut self,
        profile: &str,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let read_only = self.locked();
        let mut row = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(14.0))
            .py(px(10.0))
            .border_t_1()
            .border_color(swatch.line);
        if self.quake.deleting {
            let yes = button("settings-qdelyes", swatch)
                .text_color(swatch.danger)
                .child("Delete");
            let yes = self.actionable(yes, "qdelyes".to_owned(), swatch, cx);
            let no = button("settings-qdelno", swatch).child("Cancel");
            let no = self.actionable(no, "qdelno".to_owned(), swatch, cx);
            row = row
                .child(small(
                    format!(
                        "Delete profile {profile}? Its settings and comments \
                         are removed from config.toml."
                    ),
                    swatch.fg,
                ))
                .child(div().flex().gap(px(8.0)).child(yes).child(no));
        } else {
            let delete = button("settings-qdel", swatch)
                .when(read_only, |button| button.opacity(0.5))
                .child("Delete profile");
            let delete = self.actionable(delete, "qdel".to_owned(), swatch, cx);
            let _ =
                self.action_stop("qdel".to_owned(), cx).tab_stop(!read_only);
            row = row.child(div().flex().child(delete)).child(small(
                "An open window of a deleted profile becomes an ordinary \
                     window and keeps its tabs."
                    .to_owned(),
                swatch.muted,
            ));
        }
        row.children(
            self.quake
                .delete_error
                .clone()
                .map(|error| small(error, swatch.danger)),
        )
        .into_any_element()
    }

    fn render_globals(
        &mut self,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let read_only = self.keys_read_only();
        let globals = self.globals();
        let mut body = card(swatch);
        if globals.is_empty() {
            body = body.child(
                div().px(px(14.0)).py(px(10.0)).child(small(
                    "No global shortcuts are set. A global shortcut shows or \
                 hides a quake window while another application has focus."
                        .to_owned(),
                    swatch.muted,
                )),
            );
        }
        for (index, global) in globals.iter().enumerate() {
            let row = self.render_global(index, global, read_only, swatch, cx);
            body = body.child(
                div()
                    .when(index > 0, |row| {
                        row.border_t_1().border_color(swatch.line)
                    })
                    .child(row),
            );
        }
        let recording_add = self
            .keys
            .recording
            .as_ref()
            .is_some_and(|recording| recording.target == Target::AddGlobal);
        let add = button("settings-gadd", swatch)
            .when(read_only, |button| button.opacity(0.5))
            .when(recording_add, |button| button.border_color(swatch.accent))
            .child(if recording_add {
                self.recording_content(swatch)
            } else {
                div().child("Add global shortcut").into_any_element()
            });
        let add = self.actionable(add, "gadd".to_owned(), swatch, cx);
        let _ = self.action_stop("gadd".to_owned(), cx).tab_stop(!read_only);
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .px(px(14.0))
                .py(px(10.0))
                .border_t_1()
                .border_color(swatch.line)
                .child(div().flex().child(add))
                .when(recording_add, |block| {
                    block.child(self.recording_hint(swatch))
                })
                .children(
                    self.keys
                        .errors
                        .get("gadd")
                        .cloned()
                        .map(|error| small(error, swatch.danger)),
                )
                .when(wayland(), |block| {
                    block.child(small(
                        "Global shortcuts need macOS or a native X11 \
                         session; this Wayland session does not support \
                         them."
                            .to_owned(),
                        swatch.muted,
                    ))
                }),
        );
        group_block("Global shortcuts", body, swatch)
    }

    fn render_global(
        &mut self,
        index: usize,
        global: &Global,
        read_only: bool,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let recording =
            self.keys.recording.as_ref().is_some_and(|recording| {
                recording.target == Target::Global(index)
            });
        let content = if recording {
            self.recording_content(swatch)
        } else {
            KeyHint::parse(&global.key, Platform::current())
                .element(px(12.0), swatch.fg)
                .into_any_element()
        };
        let cap = div()
            .id(gpui::ElementId::Name(
                format!("settings-grec-{index}").into(),
            ))
            .flex()
            .flex_none()
            .items_center()
            .w(px(CAP_WIDTH))
            .overflow_hidden()
            .h(px(24.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(if recording {
                swatch.accent
            } else {
                swatch.accent.opacity(0.5)
            })
            .when(read_only, |cap| cap.opacity(0.6))
            .child(content);
        let cap = self.actionable(cap, format!("grec:{index}"), swatch, cx);
        let _ = self
            .action_stop(format!("grec:{index}"), cx)
            .tab_stop(!read_only);
        let command = lookup(&global.command).map_or_else(
            || global.command.clone(),
            |spec| spec.title.to_owned(),
        );
        let command = self.global_select(
            &format!("gcmd:{index}"),
            command,
            read_only,
            swatch,
            cx,
        );
        let profile = self.global_select(
            &format!("gprofile:{index}"),
            global.profile.clone().unwrap_or_else(|| DEFAULT.to_owned()),
            read_only,
            swatch,
            cx,
        );
        let remove = remove_button(format!("settings-gdel-{index}"), swatch);
        let remove =
            self.actionable(remove, format!("gdel:{index}"), swatch, cx);
        let _ = self
            .action_stop(format!("gdel:{index}"), cx)
            .tab_stop(!read_only);
        let error = self.keys.errors.get(&format!("g{index}")).cloned();
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .px(px(14.0))
            .py(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(cap)
                    .child(command)
                    .child(profile)
                    .child(div().flex_1())
                    .when(!read_only, |row| row.child(remove)),
            )
            .when(recording, |row| row.child(self.recording_hint(swatch)))
            .children(error.map(|error| small(error, swatch.danger)))
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
        let trigger = select_trigger(format!("settings-{key}"), swatch)
            .when(read_only, |control| control.opacity(0.5))
            .child(div().flex_1().child(label))
            .child(caret(swatch));
        let trigger = self.actionable(trigger, key.to_owned(), swatch, cx);
        let _ = self.action_stop(key.to_owned(), cx).tab_stop(!read_only);
        self.with_menu(key, trigger.into_any_element())
    }

    /// The name field, when it has focus.
    pub(super) fn focused_name_field(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<Entity<TextField>> {
        self.quake
            .naming
            .clone()
            .filter(|field| field.read(cx).focus_handle(cx).is_focused(window))
    }
}

/// Whether this session cannot register global shortcuts.
fn wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// What desktop tests drive and observe in the section.
#[cfg(all(test, feature = "test-support"))]
impl SettingsView {
    /// Opens the New profile field and types `name` into it.
    pub(in crate::desktop::windows) fn type_profile_name(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.quake.naming.is_none() {
            self.act("qnew", window, cx);
        }
        if let Some(field) = &self.quake.naming {
            field.update(cx, |field, cx| field.set_text(name, cx));
        }
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

    /// The name field's or Delete profile's message.
    pub(in crate::desktop::windows) fn quake_message(&self) -> Option<String> {
        self.quake
            .name_error
            .clone()
            .or_else(|| self.quake.delete_error.clone())
    }

    /// Whether the New profile field or the delete confirmation shows.
    pub(in crate::desktop::windows) fn quake_prompts(&self) -> (bool, bool) {
        (self.quake.naming.is_some(), self.quake.deleting)
    }

    /// The New profile field's text.
    pub(in crate::desktop::windows) fn profile_name_text(
        &self,
        cx: &App,
    ) -> String {
        self.quake
            .naming
            .as_ref()
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
/// Cmd, or a function key.
fn global_key_allowed(key: &str) -> bool {
    let Ok(stroke) = gpui::Keystroke::parse(key) else {
        return false;
    };
    let modifiers = stroke.modifiers;
    modifiers.control
        || modifiers.alt
        || modifiers.platform
        || stroke
            .key
            .strip_prefix('f')
            .is_some_and(|number| number.parse::<u8>().is_ok())
}
