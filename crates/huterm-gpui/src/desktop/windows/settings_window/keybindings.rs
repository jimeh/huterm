//! The Keybindings section: command rows that expand to their shortcuts,
//! each with a key recorder, a `when` field, argument controls, and an
//! enable switch. Every change becomes a keybinding [`Edit`] on the
//! window's write queue.
//!
//! Rows are named by where their binding comes from: `d<n>` for the nth
//! platform default and `u<n>` for user entry `n`, so a row keeps its name
//! across reads while the file keeps its entries.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use gpui::{
    AnyElement, App, Context, ElementId, Entity, EntityId, Focusable,
    KeystrokeEvent, SharedString, Window, div, prelude::*, px,
};
use huterm_config::document::Patch;
use huterm_protocol::{
    ArgumentKind, ArgumentSpec, CommandId, CommandSpec, NamedLayout,
    Requirement, SIDES, catalog, lookup,
};
use toml::Value;

use super::super::super::key_hint::KeyHint;
use super::super::super::overlay::{Swatch, mono_font_family, tokens};
use super::super::super::settings::keybindings::{
    Binding, CONTEXTS, Context as WhenContext, Edit, Field, GROUPS, OVERLAYS,
    check_condition, completions, notes,
};
use super::super::Desktop;
use super::controls::Opener;
use super::dialog::Prompt;
use super::quake::{self, Global};
use super::render::{changed_dot, group_card};
use super::saving::Queued;
use super::{MenuFor, SettingsView};
use crate::assets::Icon;
use crate::keymap::{
    BindingState, Bound, Inventory, ListedBinding, Origin, Platform,
    same_arguments, same_keys, would_replace,
};
use huterm_ui::components::alert_dialog::AlertDialog;
use huterm_ui::components::badge::{Tone as BadgeTone, badge};
use huterm_ui::components::button::{Button, Variant};
use huterm_ui::components::card;
use huterm_ui::components::collapsible::{Collapsible, chevron};
use huterm_ui::components::combobox::{Combobox, ComboboxEvent};
use huterm_ui::components::input::{Changed, Chrome, Input};
use huterm_ui::components::item::Item;
use huterm_ui::components::key_recorder::{
    KeyRecorder, KeyRecorderButton, KeyRecorderEvent,
};
use huterm_ui::components::switch::Switch;
use huterm_ui::components::toggle_group::ToggleGroup;
use huterm_ui::listbox::{
    Item as MenuItem, Model as MenuModel, Row as MenuRow,
};
use huterm_ui::option_list::ListOption;
use huterm_ui::tokens::size;
use huterm_ui::typography::ui;

/// The key cap's width, and the tag's, so rows line up.
pub(super) const CAP_WIDTH: f32 = 96.0;
const TAG_WIDTH: f32 = 54.0;

/// The menu item that leaves an argument unset.
const UNSET: &str = "\u{0}unset";

fn id(name: String) -> ElementId {
    ElementId::Name(name.into())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Filter {
    #[default]
    All,
    Customized,
    Conditional,
    Disabled,
}

impl Filter {
    const ALL: [(Self, &'static str, &'static str); 4] = [
        (Self::All, "all", "All"),
        (Self::Customized, "custom", "Customized"),
        (Self::Conditional, "when", "Conditional"),
        (Self::Disabled, "off", "Disabled"),
    ];
}

/// What a recording sets: a row's key, or a new binding for a command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Target {
    Row(String),
    Add(CommandId),
    /// A global shortcut's key, by entry.
    Global(usize),
    /// A new global shortcut: the Quake windows section's, which toggles
    /// the profile it shows, or one for a quake command in Keybindings.
    AddGlobal(Option<CommandId>),
}

impl Target {
    /// The target of the recorder whose action key is `key`.
    fn of(key: &str) -> Option<Self> {
        let (kind, argument) = key.split_once(':').unwrap_or((key, ""));
        match kind {
            "krec" => Some(Self::Row(argument.to_owned())),
            "kadd" => lookup(argument).map(|spec| Self::Add(spec.id)),
            "grec" => argument.parse().ok().map(Self::Global),
            "gadd" if argument.is_empty() => Some(Self::AddGlobal(None)),
            "gadd" => {
                lookup(argument).map(|spec| Self::AddGlobal(Some(spec.id)))
            }
            _ => None,
        }
    }
}

/// What a recorded shortcut takes its key from, as the reassign prompt
/// asked about it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Taking {
    /// The titles of the commands that lose the key.
    pub(super) from: Vec<String>,
    /// Every user entry for another command with the same keys and
    /// predicate, whatever became of it, which Reassign removes so the
    /// file keeps no entry that only loses to the recorded one.
    entries: Vec<usize>,
    /// Something after the recorded user row could take the keys back
    /// from it: an entry with the same keys and predicate, which replaces
    /// or disables it, or an `unbind` of the keys. The recording then
    /// moves the row to the end of the file, after all of them, so the
    /// recorded binding is the one that holds the keys.
    last: bool,
    /// The recorded row's binding, which Reassign checks is still where
    /// the prompt found it.
    row: Option<Identity>,
}

/// The section's state, held by the view.
pub(super) struct Keys {
    /// The file's bindings and what became of each; `None` until a read
    /// finds bindings that compile.
    pub(super) inventory: Option<Inventory>,
    pub(super) open: HashSet<CommandId>,
    pub(super) filter: Filter,
    pub(super) search: Entity<Input>,
    /// `when` and argument fields, keyed `when:<row>` or
    /// `arg:<row>:<name>`.
    pub(super) fields: HashMap<String, Entity<Input>>,
    /// Each field's text from the file; a focused field that differs
    /// holds a draft.
    pub(super) synced: HashMap<String, String>,
    /// Errors by row.
    pub(super) errors: HashMap<String, String>,
    /// Each `when` field's context suggestions, by the field's entity,
    /// which keeps them as the field moves between rows.
    pub(super) completions: HashMap<EntityId, Entity<Combobox>>,
    /// Key recorders by action key: `krec:<row>`, `kadd:<command>`,
    /// `grec:<entry>`, `gadd`, and `gadd:<command>`.
    pub(super) recorders: HashMap<String, Entity<KeyRecorder>>,
    /// The recorder that started last, which the window's interceptor
    /// feeds, even after it stops, so it can swallow what follows.
    pub(super) recorder: Option<Entity<KeyRecorder>>,
    /// That recorder records a global shortcut whose entry now holds
    /// another one; the next frame or keystroke cancels it.
    pub(super) recorder_stale: bool,
    /// The field being edited, whose draft a read must not replace.
    pub(super) editing: Option<String>,
    /// A control to focus once it draws, and the control focus must still
    /// be on for the move to happen: the user may have moved on.
    pub(super) follow: Option<(String, String)>,
    /// A focused field went away with its row; focus moves to the window.
    pub(super) stranded: bool,
    /// Each row's binding at the last sync, to follow a field's shortcut
    /// when another editor shifts or edits the entries.
    pub(super) identities: HashMap<String, Identity>,
    /// A locator for each `[[keybinding]]` entry of the snapshot the list
    /// came from, parsed off the GPUI thread with it.
    pub(super) entries: Vec<huterm_config::document::EntryLocator>,
    /// A locator for each `[[global_keybinding]]` entry, read with them.
    pub(super) globals: Vec<huterm_config::document::EntryLocator>,
    /// The row whose change is being written.
    pub(super) writing_row: Option<String>,
}

impl Keys {
    /// The key a field is held under.
    pub(super) fn key_of(&self, id: gpui::EntityId) -> Option<String> {
        self.fields
            .iter()
            .find(|(_, field)| field.entity_id() == id)
            .map(|(key, _)| key.clone())
    }

    /// The first error shown under a row: its write's or its fields'.
    #[cfg(all(test, feature = "test-support"))]
    pub(super) fn row_error(&self, row: &str) -> Option<String> {
        self.errors
            .iter()
            .find(|(key, _)| key.as_str() == row || field_row(key) == Some(row))
            .map(|(_, error)| error.clone())
    }

    /// The row showing `command`'s binding on `key`.
    #[cfg(all(test, feature = "test-support"))]
    pub(super) fn row_named(
        &self,
        command: CommandId,
        key: &str,
    ) -> Option<String> {
        rows(self.inventory.as_ref()?)
            .into_iter()
            .find(|(_, listed)| {
                listed.binding.command == command && listed.binding.key == key
            })
            .map(|(name, _)| name)
    }

    pub(super) fn new(
        swatch: Swatch,
        cx: &mut Context<'_, SettingsView>,
    ) -> Self {
        let search = cx.new(|cx| {
            Input::new("keybindings-search", "Search keybindings", swatch, cx)
                .with_placeholder("Search commands, shortcuts, or conditions")
                .with_chrome(Chrome::Grouped)
        });
        let _ = search.read(cx).focus_handle(cx).tab_stop(true);
        cx.subscribe(&search, |_, _, _: &Changed, cx| cx.notify())
            .detach();
        Self {
            inventory: None,
            open: HashSet::new(),
            filter: Filter::All,
            search,
            fields: HashMap::new(),
            synced: HashMap::new(),
            errors: HashMap::new(),
            completions: HashMap::new(),
            recorders: HashMap::new(),
            recorder: None,
            recorder_stale: false,
            editing: None,
            follow: None,
            stranded: false,
            identities: HashMap::new(),
            entries: Vec::new(),
            globals: Vec::new(),
            writing_row: None,
        }
    }
}

/// A row's binding, to recognize it when the entries move.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Identity {
    whole: String,
    /// The binding without its key.
    keyless: String,
    key: String,
    command: CommandId,
}

impl Identity {
    fn of(listed: &ListedBinding) -> Self {
        let keyless = Bound {
            key: String::new(),
            ..listed.binding.clone()
        };
        Self {
            whole: format!("{:?}", listed.binding),
            keyless: format!("{keyless:?}"),
            key: listed.binding.key.clone(),
            command: listed.binding.command,
        }
    }

    /// The same shortcut with another condition or other arguments.
    fn same_shortcut(&self, other: &Self) -> bool {
        self.key == other.key && self.command == other.command
    }

    /// The same binding on another key.
    fn rekeyed(&self, other: &Self) -> bool {
        self.key != other.key && self.keyless == other.keyless
    }
}

/// Every listed binding with its row name.
fn rows(inventory: &Inventory) -> Vec<(String, &ListedBinding)> {
    let mut defaults = 0;
    inventory
        .bindings
        .iter()
        .map(|listed| {
            let name = if let Some(entry) = listed.entry {
                format!("u{entry}")
            } else {
                defaults += 1;
                format!("d{}", defaults - 1)
            };
            (name, listed)
        })
        .collect()
}

fn row<'a>(inventory: &'a Inventory, name: &str) -> Option<&'a ListedBinding> {
    rows(inventory)
        .into_iter()
        .find(|(row, _)| row == name)
        .map(|(_, listed)| listed)
}

/// The entries Restore defaults removes for `command`: its user entries and
/// those turning its defaults off, in file order, and how many of them are
/// shortcuts the user added rather than changes to defaults.
fn restorable(
    inventory: &Inventory,
    command: CommandId,
) -> (Vec<usize>, usize) {
    let mut entries: Vec<usize> = inventory
        .bindings
        .iter()
        .filter(|listed| listed.binding.command == command)
        .flat_map(|listed| {
            let by = match &listed.state {
                BindingState::Disabled(by) => by.clone(),
                _ => Vec::new(),
            };
            listed.entry.into_iter().chain(by)
        })
        .collect();
    entries.sort_unstable();
    entries.dedup();
    let defaults: Vec<&ListedBinding> = inventory
        .bindings
        .iter()
        .filter(|listed| {
            listed.binding.command == command
                && listed.binding.origin == Origin::Default
        })
        .collect();
    // A user entry on a key a default of this command had is a change to
    // that default; any other is a shortcut the user added.
    let added = inventory
        .bindings
        .iter()
        .filter(|listed| {
            listed.binding.command == command
                && listed.entry.is_some()
                && !defaults
                    .iter()
                    .any(|default| default.binding.key == listed.binding.key)
        })
        .count();
    (entries, added)
}

/// The commands of each group, in display order.
fn grouped() -> Vec<(&'static str, Vec<&'static CommandSpec>)> {
    GROUPS
        .iter()
        .map(|(group, commands)| {
            (
                *group,
                commands
                    .iter()
                    .filter_map(|command| lookup(command.as_str()))
                    .collect(),
            )
        })
        .chain(std::iter::once((
            OVERLAYS,
            catalog()
                .iter()
                .filter(|spec| spec.context.is_some())
                .collect(),
        )))
        .collect()
}

/// The contexts completing the name typed before the caret, and where that
/// name starts; `None` until a name has been started.
fn suggestions(field: &Input) -> Option<(usize, Vec<WhenContext>)> {
    let (start, matches) = completions(field.text(), field.cursor());
    (start < field.cursor() && !matches.is_empty()).then_some((start, matches))
}

/// Where the context name running through `caret` ends: past the name
/// characters that follow it.
fn name_end(text: &str, caret: usize) -> usize {
    let after = text.get(caret..).unwrap_or_default();
    caret
        + after
            .find(|character: char| {
                !(character.is_ascii_alphanumeric()
                    || matches!(character, '_' | '-'))
            })
            .unwrap_or(after.len())
}

/// A context suggestion's row: its name in the monospace font, and its
/// description as secondary text in the interface font.
fn condition_row(
    option: &ListOption,
    highlighted: bool,
    _: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let swatch = tokens(&cx.global::<Desktop>().config.theme);
    let secondary = if highlighted {
        swatch.muted_on(swatch.selection)
    } else {
        swatch.muted
    };
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .child(
            div()
                .truncate()
                .font_family(mono_font_family())
                .child(option.label.clone()),
        )
        .when_some(option.description.clone(), |row, description| {
            row.child(
                div()
                    .truncate()
                    .text_size(size::TEXT_SMALL)
                    .text_color(secondary)
                    .child(description),
            )
        })
        .into_any_element()
}

/// A `when` field's suggestions: the contexts completing the name before
/// the caret, each with its description.
fn condition_suggestions(field: &Input) -> Vec<ListOption> {
    suggestions(field).map_or_else(Vec::new, |(_, matches)| {
        matches
            .into_iter()
            .map(|context| {
                ListOption::new(context.name, context.name)
                    .description(context.description)
            })
            .collect()
    })
}

/// The arguments a binding can carry in the file.
fn configurable(
    spec: &CommandSpec,
) -> impl Iterator<Item = &'static ArgumentSpec> {
    spec.args.iter().filter(|argument| {
        matches!(
            argument.kind,
            ArgumentKind::Bool
                | ArgumentKind::Integer { .. }
                | ArgumentKind::Text
                | ArgumentKind::QuakeProfile
                | ArgumentKind::Side
                | ArgumentKind::Layout
        )
    })
}

/// What an unset argument does: the command's default, or a prompt.
fn unset_label(argument: &ArgumentSpec) -> &'static str {
    if argument.kind == ArgumentKind::QuakeProfile {
        // An unset profile is the `default` profile.
        return "default";
    }
    match argument.required {
        Requirement::Optional => "Default",
        Requirement::Always | Requirement::OneOf(_) => "Ask",
    }
}

/// An argument's value as a control shows it.
fn argument_text(listed: &ListedBinding, name: &str) -> Option<String> {
    use huterm_protocol::CommandValue;
    listed
        .binding
        .args
        .iter()
        .find(|argument| argument.name == name)
        .map(|argument| match &argument.value {
            CommandValue::Bool(true) => "On".to_owned(),
            CommandValue::Bool(false) => "Off".to_owned(),
            CommandValue::Integer(value) => value.to_string(),
            CommandValue::Text(value) => value.clone(),
            _ => String::new(),
        })
}

fn binding_of(listed: &ListedBinding) -> Binding {
    Binding::new(
        &listed.binding.key,
        listed.binding.command,
        listed.binding.when.as_deref(),
        &listed.binding.args,
    )
}

/// A static menu id for a quake profile name. Each distinct name leaks
/// once; profiles are few and come from the user's file.
pub(super) fn interned(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(name) = names.get(name) {
        return name;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(leaked);
    leaked
}

impl SettingsView {
    pub(super) fn keys_read_only(&self) -> bool {
        self.keys_lock_reason().is_some()
    }

    /// Why the shortcut controls are read-only, if they are.
    pub(super) fn keys_lock_reason(&self) -> Option<SharedString> {
        self.lock_reason().or_else(|| {
            self.keys
                .inventory
                .is_none()
                .then(|| "Read-only until the keybindings load".into())
        })
    }

    /// Why a shortcut's key cap, condition, and argument fields refuse
    /// edits: the section is read-only, or the shortcut is not active.
    /// `None` for an active shortcut that can change.
    fn key_field_refusal(&self, active: bool) -> Option<SharedString> {
        self.keys_lock_reason().or_else(|| {
            (!active).then(|| "Only an active shortcut can change".into())
        })
    }

    /// Steps the shortcut filter, which is one tab stop, to the next or
    /// previous segment, as the arrows do; false when it lacks focus.
    pub(super) fn step_filter(
        &mut self,
        forward: bool,
        window: &Window,
    ) -> bool {
        if self.focused_action(window).as_deref() != Some("kfilter") {
            return false;
        }
        let index = Filter::ALL
            .iter()
            .position(|(filter, ..)| *filter == self.keys.filter)
            .unwrap_or(0);
        let next = if forward {
            (index + 1).min(Filter::ALL.len() - 1)
        } else {
            index.saturating_sub(1)
        };
        if let Some((filter, ..)) = Filter::ALL.get(next) {
            self.keys.filter = *filter;
        }
        true
    }

    /// Whether a command has a user entry, a default that is not active,
    /// or a global shortcut, which only the file adds.
    fn customized(
        inventory: &Inventory,
        globals: &[Global],
        command: CommandId,
    ) -> bool {
        globals
            .iter()
            .any(|global| global.command == command.as_str())
            || inventory.bindings.iter().any(|listed| {
                listed.binding.command == command
                    && (listed.binding.origin == Origin::User
                        || listed.state != BindingState::Active)
            })
    }

    fn command_matches(
        inventory: &Inventory,
        globals: &[Global],
        spec: &CommandSpec,
        filter: Filter,
        query: &str,
    ) -> bool {
        let listed: Vec<&ListedBinding> = inventory
            .bindings
            .iter()
            .filter(|listed| listed.binding.command == spec.id)
            .collect();
        let globals: Vec<&Global> = globals
            .iter()
            .filter(|global| global.command == spec.id.as_str())
            .collect();
        let passes = match filter {
            Filter::All => true,
            Filter::Customized => {
                Self::customized(inventory, &[], spec.id) || !globals.is_empty()
            }
            Filter::Conditional => listed.iter().any(|listed| {
                listed.state == BindingState::Active
                    && listed.binding.when.is_some()
            }),
            Filter::Disabled => {
                globals.iter().any(|global| global.disabled)
                    || listed.iter().any(|listed| {
                        matches!(
                            listed.state,
                            BindingState::Disabled(_)
                                | BindingState::SwitchedOff
                        )
                    })
            }
        };
        passes
            && (query.is_empty()
                || spec.title.to_lowercase().contains(query)
                || spec.id.as_str().contains(query)
                || globals.iter().any(|global| global.key.contains(query))
                || listed.iter().any(|listed| {
                    listed.binding.key.contains(query)
                        || listed.binding.when.as_deref().is_some_and(|when| {
                            when.to_lowercase().contains(query)
                        })
                }))
    }

    /// Commands matching the sidebar search, for its results.
    pub(super) fn matching_commands(
        &self,
        query: &str,
    ) -> Vec<&'static CommandSpec> {
        let Some(inventory) = &self.keys.inventory else {
            return Vec::new();
        };
        let globals = self.globals();
        grouped()
            .into_iter()
            .flat_map(|(_, commands)| commands)
            .filter(|spec| {
                Self::command_matches(
                    inventory,
                    &globals,
                    spec,
                    Filter::All,
                    query,
                )
            })
            .collect()
    }

    /// Runs a keybinding action key; `false` when `kind` is not one.
    pub(super) fn act_keys(
        &mut self,
        kind: &str,
        argument: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        match kind {
            "kcmd" => {
                if let Some(spec) = lookup(argument)
                    && !self.keys.open.remove(&spec.id)
                {
                    self.keys.open.insert(spec.id);
                }
            }
            "kfilter" => {
                if let Some((filter, ..)) =
                    Filter::ALL.iter().find(|(_, name, _)| *name == argument)
                {
                    self.keys.filter = *filter;
                }
            }
            "kgo" => {
                if let Some(spec) = lookup(argument) {
                    self.select_section(
                        huterm_config::settings::SectionId::Keybindings,
                        cx,
                    );
                    // The section lists just this command, so it shows
                    // without scrolling; clearing the search lists all.
                    self.keys.search.update(cx, |search, cx| {
                        search.set_text(spec.title.to_lowercase(), cx);
                    });
                    self.keys.open.insert(spec.id);
                    let handle =
                        self.action_stop(format!("kcmd:{argument}"), cx);
                    handle.focus(window, cx);
                }
            }
            // Only an active shortcut's key records.
            "krec" if !self.field_row_active(&format!("krec:{argument}")) => {}
            "krec" | "kadd" => {
                self.start_recording(&format!("{kind}:{argument}"), window, cx);
            }
            "kon" => self.switch_row(argument, cx),
            // Removing a control's row moves focus to its command, or to
            // the section's search.
            "kdel" => {
                if let Some(listed) = self
                    .keys
                    .inventory
                    .as_ref()
                    .and_then(|inventory| row(inventory, argument))
                    && let Some(entry) = listed.entry
                {
                    self.keys.follow = Some((
                        format!("kcmd:{}", listed.binding.command.as_str()),
                        format!("kdel:{argument}"),
                    ));
                    self.submit_keys(Edit::Remove(vec![entry]), None, None, cx);
                }
            }
            "kreset" => {
                if let Some(spec) = lookup(argument) {
                    self.keys.follow = Some((
                        format!("kcmd:{argument}"),
                        format!("kreset:{argument}"),
                    ));
                    self.restore_defaults(spec.id, cx);
                }
            }
            "kunbind" => {
                if let Ok(entry) = argument.parse() {
                    self.keys.follow = Some((
                        "search".to_owned(),
                        format!("kunbind:{argument}"),
                    ));
                    self.submit_keys(Edit::Remove(vec![entry]), None, None, cx);
                }
            }
            "karg" => {
                if let Some((row, name)) = argument.split_once(':') {
                    self.open_argument_menu(row, name, window, cx);
                }
            }
            "kreyes" => self.reassign(window, cx),
            "kreno" => self.close_dialog(window, cx),
            _ => return false,
        }
        true
    }

    /// Switches a row's binding on or off.
    fn switch_row(&mut self, name: &str, cx: &mut Context<'_, Self>) {
        let Some(listed) = self
            .keys
            .inventory
            .as_ref()
            .and_then(|inventory| row(inventory, name))
        else {
            return;
        };
        if self.default_change_pending(listed, name) {
            return;
        }
        let edit = match (&listed.state, listed.binding.origin, listed.entry) {
            (BindingState::Disabled(by), ..) => {
                Edit::SwitchOnDefault(by.clone())
            }
            (BindingState::Active, Origin::Default, _) => {
                Edit::SwitchOffDefault(binding_of(listed))
            }
            (BindingState::Active, Origin::User, Some(entry)) => {
                Edit::SwitchUser { entry, on: false }
            }
            (BindingState::SwitchedOff, _, Some(entry)) => {
                Edit::SwitchUser { entry, on: true }
            }
            _ => return,
        };
        self.submit_keys(edit, Some(name.to_owned()), None, cx);
    }

    /// Removes every user entry for `command`, including those that turn
    /// its defaults off. `unbind` entries stay.
    pub(super) fn restore_defaults(
        &mut self,
        command: CommandId,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(inventory) = &self.keys.inventory else {
            return;
        };
        let (entries, _) = restorable(inventory, command);
        if !entries.is_empty() {
            self.submit_keys(Edit::Remove(entries), None, None, cx);
        }
    }

    /// Changes one field of a row's binding: in place for a user entry,
    /// or as a disabled default plus a replacement. `false` when nothing was
    /// submitted.
    fn edit_row(
        &mut self,
        name: &str,
        field: Field,
        value: Option<Value>,
        origin: Option<(String, Option<String>)>,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        self.edit_row_removing(name, field, value, origin, &[], cx)
    }

    /// [`Self::edit_row`], removing the user entries `remove` in the same
    /// write.
    fn edit_row_removing(
        &mut self,
        name: &str,
        field: Field,
        value: Option<Value>,
        origin: Option<(String, Option<String>)>,
        remove: &[usize],
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(listed) = self
            .keys
            .inventory
            .as_ref()
            .and_then(|inventory| row(inventory, name))
        else {
            return false;
        };
        if self.default_change_pending(listed, name) {
            return false;
        }
        let edit = match (listed.binding.origin, listed.entry) {
            (Origin::User, Some(entry)) => Edit::ChangeUser {
                entry,
                field,
                value,
            },
            _ => Edit::ChangeDefault {
                default: binding_of(listed),
                disabled: matches!(listed.state, BindingState::Disabled(_)),
                field,
                value,
            },
        };
        self.submit_keys_removing(
            edit,
            Some(name.to_owned()),
            origin,
            remove,
            cx,
        )
    }

    /// Whether `row` shows a default with a change queued or being
    /// written. The list shows the default as it was until that change
    /// lands, so another change would append a second disabling entry and
    /// replacement; once it lands, focus follows to the new row.
    fn default_change_pending(
        &self,
        listed: &ListedBinding,
        row: &str,
    ) -> bool {
        matches!(listed.binding.origin, Origin::Default)
            && (self.keys.writing_row.as_deref() == Some(row)
                || self.queued.iter().any(|queued| {
                    matches!(
                        queued,
                        Queued::Keys(change) if change.row.as_deref() == Some(row)
                    )
                }))
    }

    /// After a changed default or a moved row lands, its binding is the
    /// file's last entry; when focus is still on the control that changed it,
    /// focus moves to the same control on the new row, so further typing
    /// edits that row rather than the default it replaced. A recorded key
    /// also keeps the drafts in its shortcut's other fields
    /// (`keep_drafts_for_new_key`).
    pub(super) fn follow_changed_default(
        &mut self,
        edit: &Edit,
        row: Option<&str>,
        patches: &[Patch],
    ) {
        self.keep_drafts_for_new_key(edit, row, patches);
        let Some(row) = row else {
            return;
        };
        let Some(last) = self.keys.inventory.as_ref().and_then(|inventory| {
            inventory
                .bindings
                .iter()
                .filter_map(|listed| listed.entry)
                .max()
        }) else {
            return;
        };
        // A row moved to the end of the file is its last entry too.
        if let Edit::MoveUser { .. } = edit {
            self.keys.follow =
                Some((format!("krec:u{last}"), format!("krec:{row}")));
            return;
        }
        let Edit::ChangeDefault { default, field, .. } = edit else {
            return;
        };
        let control = |row: &str| match field {
            Field::Key => format!("krec:{row}"),
            Field::When => format!("when:{row}"),
            // Selects are action stops; number and text arguments are
            // fields.
            Field::Argument(name) => {
                let select = lookup(default.command.as_str())
                    .and_then(|spec| spec.argument(name))
                    .is_some_and(|argument| {
                        !matches!(
                            argument.kind,
                            ArgumentKind::Integer { .. } | ArgumentKind::Text
                        )
                    });
                if select {
                    format!("karg:{row}:{name}")
                } else {
                    format!("arg:{row}:{name}")
                }
            }
        };
        self.keys.follow = Some((control(&format!("u{last}")), control(row)));
    }

    /// Keeps the drafts in a shortcut's condition and argument fields when
    /// its recorded key lands. Fields follow a shortcut by its binding, so
    /// a new key would otherwise count as another shortcut and drop them.
    /// The edited binding's identity before the write becomes its identity
    /// now, so `follow_moved_rows` moves its fields with every other row's,
    /// to wherever the entries now put it. A user entry is matched by the
    /// key its write replaced, and by its write's row among identical
    /// entries; a default's change is the file's last entry. Without one
    /// match, `follow_moved_rows` handles the fields as after any edit.
    fn keep_drafts_for_new_key(
        &mut self,
        edit: &Edit,
        changed: Option<&str>,
        patches: &[Patch],
    ) {
        let Some(inventory) = &self.keys.inventory else {
            return;
        };
        let (from, now) = match edit {
            Edit::ChangeUser {
                field: Field::Key,
                value: Some(Value::String(key)),
                ..
            } => {
                // Another editor can move the entries, so the write's
                // locator may no longer name the row; its content still
                // names the key it replaced. Its row tells identical
                // entries apart.
                let Some((replaced, located)) =
                    patches.iter().find_map(|patch| match patch {
                        Patch::SetEntryField { locator, field, .. }
                            if field == "key" =>
                        {
                            let replaced = locator
                                .content()
                                .get("key")
                                .and_then(Value::as_str)?;
                            Some((replaced, format!("u{}", locator.index())))
                        }
                        _ => None,
                    })
                else {
                    return;
                };
                let found: Vec<(String, Identity)> = rows(inventory)
                    .into_iter()
                    .filter(|(_, listed)| listed.binding.key == *key)
                    .filter_map(|(_, listed)| {
                        let now = Identity::of(listed);
                        let before: Vec<&String> = self
                            .keys
                            .identities
                            .iter()
                            .filter(|(_, before)| {
                                before.key == replaced && before.rekeyed(&now)
                            })
                            .map(|(name, _)| name)
                            .collect();
                        let from = match before.as_slice() {
                            [from] => Some(*from),
                            _ => before
                                .into_iter()
                                .find(|name| **name == located),
                        };
                        from.map(|from| (from.clone(), now))
                    })
                    .collect();
                let [found] = found.as_slice() else {
                    return;
                };
                found.clone()
            }
            Edit::ChangeDefault {
                field: Field::Key,
                value: Some(Value::String(key)),
                ..
            }
            | Edit::MoveUser { key, .. } => {
                let (Some(from), Some(last)) = (
                    changed,
                    inventory
                        .bindings
                        .iter()
                        .filter_map(|listed| listed.entry)
                        .max(),
                ) else {
                    return;
                };
                let command =
                    self.keys.identities.get(from).map(|before| before.command);
                let Some(listed) =
                    row(inventory, &format!("u{last}")).filter(|listed| {
                        listed.binding.key == *key
                            && Some(listed.binding.command) == command
                    })
                else {
                    return;
                };
                (from.to_owned(), Identity::of(listed))
            }
            _ => return,
        };
        self.keys.identities.insert(from, now);
    }

    /// Focuses a followed control once it draws.
    pub(super) fn focus_follow(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((target, origin)) = self.keys.follow.take() else {
            return;
        };
        let focused = self
            .focused_action(window)
            .or_else(|| self.focused_key_field(window, cx).map(|(key, _)| key));
        let Some(focused) = focused else {
            return;
        };
        // Focus on the changed control follows it to the new row. Focus
        // moved to another editing control of the default, which is now
        // switched off, moves to that control on the new row instead.
        let target = if focused == origin {
            target
        } else if let (Some(from), Some(to)) =
            (control_row(&origin), control_row(&target))
            && control_row(&focused) == Some(from)
            && !focused.starts_with("kon:")
        {
            focused.replacen(from, to, 1)
        } else {
            return;
        };
        if target == "search" {
            self.keys.search.read(cx).focus_handle(cx).focus(window, cx);
        } else if let Some(field) = self.keys.fields.get(&target) {
            field.read(cx).focus_handle(cx).focus(window, cx);
        } else if let Some(handle) = self.actions.get(&target)
            && self.drawn.contains(&target)
        {
            handle.focus(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
    }

    /// Starts the recorder for an action key, unless keybindings are
    /// read-only.
    pub(super) fn start_recording(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.keys_read_only() {
            return;
        }
        let recorder = self.recorder(key, cx);
        recorder.update(cx, |recorder, cx| recorder.start(window, cx));
    }

    /// The key recorder for an action key, created on first use with that
    /// key's focus handle, which marks it drawn.
    pub(super) fn recorder(
        &mut self,
        key: &str,
        cx: &mut Context<'_, Self>,
    ) -> Entity<KeyRecorder> {
        let focus = self.action_stop(key.to_owned(), cx);
        if let Some(recorder) = self.keys.recorders.get(key) {
            return recorder.clone();
        }
        // A global shortcut is one stroke.
        let strokes = if key.starts_with('g') { 1 } else { 2 };
        let recorder = cx.new(|_| {
            KeyRecorder::new(focus, |keys| {
                KeyHint::parse(keys, Platform::current()).glyphs()
            })
            .with_max_strokes(strokes)
        });
        cx.subscribe(&recorder, |view, recorder, event, cx| {
            view.recorder_event(&recorder, event, cx);
        })
        .detach();
        self.keys.recorders.insert(key.to_owned(), recorder.clone());
        recorder
    }

    fn recorder_event(
        &mut self,
        recorder: &Entity<KeyRecorder>,
        event: &KeyRecorderEvent,
        cx: &mut Context<'_, Self>,
    ) {
        match event {
            KeyRecorderEvent::Started => {
                self.keys.recorder_stale = false;
                // One recording at a time.
                if let Some(previous) =
                    self.keys.recorder.replace(recorder.clone())
                    && previous != *recorder
                {
                    previous.update(cx, KeyRecorder::cancel);
                }
            }
            KeyRecorderEvent::Recorded(keys) => {
                let target =
                    self.key_of_recorder(recorder).and_then(Target::of);
                if let Some(target) = target {
                    self.finish_recording(target, keys.clone(), cx);
                }
            }
            KeyRecorderEvent::Canceled => {}
        }
        cx.notify();
    }

    /// The action key of `recorder`.
    fn key_of_recorder(&self, recorder: &Entity<KeyRecorder>) -> Option<&str> {
        self.keys
            .recorders
            .iter()
            .find(|(_, held)| *held == recorder)
            .map(|(key, _)| key.as_str())
    }

    /// The action key of the recorder that started last.
    pub(super) fn recorder_key(&self) -> Option<&str> {
        self.key_of_recorder(self.keys.recorder.as_ref()?)
    }

    /// Cancels a recording whose global shortcut moved, which `set_globals`
    /// found without the context to cancel it.
    pub(super) fn cancel_stale_recording(
        &mut self,
        cx: &mut Context<'_, Self>,
    ) {
        if std::mem::take(&mut self.keys.recorder_stale)
            && let Some(recorder) = self.keys.recorder.clone()
        {
            recorder.update(cx, KeyRecorder::cancel);
        }
    }

    /// Feeds a keystroke to the recorder that started last, before any
    /// binding, menu, or field sees it. Returns whether it was taken.
    pub(super) fn intercept_key(
        &mut self,
        event: &KeystrokeEvent,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        self.cancel_stale_recording(cx);
        self.keys.recorder.clone().is_some_and(|recorder| {
            recorder.update(cx, |recorder, cx| recorder.intercept(event, cx))
        })
    }

    /// Applies a recorded shortcut, or first asks whether it should take
    /// its key from the commands that hold it.
    fn finish_recording(
        &mut self,
        target: Target,
        key: String,
        cx: &mut Context<'_, Self>,
    ) {
        let taking = self.taken_by(&target, &key);
        if taking.from.is_empty() {
            self.apply_recording(target, key, &[], taking.last, cx);
            return;
        }
        let Some(stop) = self.recorder_key().map(str::to_owned) else {
            return;
        };
        let return_to = self.action_stop(stop, cx);
        self.open_dialog(
            Prompt::Reassign {
                target,
                key,
                taking,
            },
            &return_to,
        );
    }

    /// What a recording of `key` for `target` takes the key from: the
    /// bindings with the same keys and predicate, other than the row itself
    /// and those that run the same command with the same arguments. The
    /// prompt names the active ones; the user entries among them go.
    fn taken_by(&self, target: &Target, key: &str) -> Taking {
        let Some(inventory) = self.keys.inventory.as_ref() else {
            return Taking::default();
        };
        let (own, command, args, when) = match target {
            Target::Row(name) => {
                let Some(listed) = row(inventory, name) else {
                    return Taking::default();
                };
                let binding = &listed.binding;
                (
                    Some(listed),
                    binding.command,
                    binding.args.as_slice(),
                    binding.when.as_deref(),
                )
            }
            Target::Add(command) => (None, *command, &[][..], None),
            Target::Global(_) | Target::AddGlobal(_) => {
                return Taking::default();
            }
        };
        let context = lookup(command.as_str()).and_then(|spec| spec.context);
        let mut taking = Taking {
            row: own.map(Identity::of),
            ..Taking::default()
        };
        let own_entry = own
            .filter(|own| own.binding.origin == Origin::User)
            .and_then(|own| own.entry);
        let after = |entry: Option<usize>| {
            own_entry.zip(entry).is_some_and(|(own, other)| other > own)
        };
        for other in &inventory.bindings {
            let binding = &other.binding;
            if own.is_some_and(|own| std::ptr::eq(own, other))
                || !would_replace(
                    Platform::current(),
                    key,
                    context,
                    when,
                    binding,
                )
            {
                continue;
            }
            taking.last |= after(other.entry);
            // A `disabled = true` entry that turned a binding off shows only
            // in that binding's state; after the row, it would turn the
            // recorded binding off too.
            if let BindingState::Disabled(by) = &other.state {
                taking.last |= by.iter().any(|entry| after(Some(*entry)));
            }
            if binding.command == command && same_arguments(&binding.args, args)
            {
                continue;
            }
            let title = lookup(binding.command.as_str()).map_or_else(
                || binding.command.as_str().to_owned(),
                |spec| spec.title.to_owned(),
            );
            if other.state == BindingState::Active
                && !taking.from.contains(&title)
            {
                taking.from.push(title);
            }
            if other.binding.origin == Origin::User
                && let Some(entry) = other.entry
            {
                taking.entries.push(entry);
            }
        }
        taking.last |= inventory.unbinds.iter().any(|(entry, unbound)| {
            after(Some(*entry)) && same_keys(Platform::current(), unbound, key)
        });
        taking
    }

    /// Answers the reassign prompt: applies the recording it holds, unless
    /// a read since it opened moved the recorded row or changed what the
    /// key would be taken from, since the prompt's answer covered only
    /// what it showed.
    fn reassign(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(Prompt::Reassign {
            target,
            key,
            taking,
        }) = self.dialog.prompt().cloned()
        else {
            return;
        };
        self.close_dialog(window, cx);
        if self.taken_by(&target, &key) != taking {
            self.status.failed(
                "config.toml changed while Settings asked about the \
                 shortcut. Record it again.",
            );
            return;
        }
        self.apply_recording(target, key, &taking.entries, taking.last, cx);
    }

    /// Applies a recorded shortcut, removing the user entries `remove`
    /// that hold its key in the same write. A user row that something
    /// later could take the key back from moves to the end of the file
    /// (`last`); a default's change and a new binding land there anyway.
    /// The write happens even when the key is the row's own, since a read
    /// during the recording can have given it to another entry since.
    fn apply_recording(
        &mut self,
        target: Target,
        key: String,
        remove: &[usize],
        last: bool,
        cx: &mut Context<'_, Self>,
    ) {
        match target {
            Target::Row(name) => {
                let Some((current, user_entry)) = self
                    .keys
                    .inventory
                    .as_ref()
                    .and_then(|inventory| row(inventory, &name))
                    .map(|listed| {
                        (
                            listed.binding.key.clone(),
                            listed.entry.filter(|_| {
                                listed.binding.origin == Origin::User
                            }),
                        )
                    })
                else {
                    return;
                };
                if let (true, Some(entry)) = (last, user_entry) {
                    self.submit_keys_removing(
                        Edit::MoveUser { entry, key },
                        Some(name),
                        None,
                        remove,
                        cx,
                    );
                } else if current != key || !remove.is_empty() {
                    self.edit_row_removing(
                        &name,
                        Field::Key,
                        Some(Value::String(key)),
                        None,
                        remove,
                        cx,
                    );
                }
            }
            Target::Add(command) => {
                self.keys.open.insert(command);
                self.submit_keys_removing(
                    Edit::Add { key, command },
                    None,
                    None,
                    remove,
                    cx,
                );
            }
            Target::Global(entry) => self.finish_global(entry, key, cx),
            Target::AddGlobal(command) => self.add_global(command, key, cx),
        }
    }

    /// A `when` or argument field for a row, created on first use.
    fn key_field(
        &mut self,
        key: String,
        text: String,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<Input> {
        self.drawn.insert(key.clone());
        if let Some(field) = self.keys.fields.get(&key) {
            return field.clone();
        }
        let swatch = Self::swatch(cx);
        let label = match key.split(':').collect::<Vec<_>>().as_slice() {
            ["when", ..] => "Condition".to_owned(),
            ["arg", _, name] => (*name).to_owned(),
            _ => key.clone(),
        };
        let field = cx.new(|cx| {
            let mut field = Input::new(
                gpui::SharedString::from(format!("keybinding-field-{key}")),
                label,
                swatch,
                cx,
            )
            .with_placeholder(placeholder.to_owned());
            field.set_text(text.clone(), cx);
            field
        });
        let handle = field.read(cx).focus_handle(cx).tab_stop(true);
        // Listeners find the field's key when they run: a field moves with
        // its shortcut when another editor shifts the entries.
        let id = field.entity_id();
        cx.on_focus(&handle, window, move |view, _, _| {
            view.keys.editing = view.keys.key_of(id);
        })
        .detach();
        cx.on_blur(&handle, window, move |view, window, cx| {
            // The field's own menu keeps its draft and its editing state.
            let Some(blurred) = view.keys.key_of(id) else {
                return;
            };
            if view.field_menu_open(id) {
                return;
            }
            if view.keys.editing.as_ref() == Some(&blurred) {
                view.keys.editing = None;
            }
            // Blur runs inside a draw, where a redraw request is lost; the
            // save waits until the draw is done, so an error it finds shows.
            cx.defer_in(window, move |view, _, cx| {
                view.commit_key_field(&blurred, cx);
            });
        })
        .detach();
        if key.starts_with("when:") {
            self.condition_list(&field, swatch, window, cx);
        }
        self.keys.fields.insert(key.clone(), field.clone());
        self.keys.synced.insert(key, text);
        field
    }

    /// The context suggestions under a `when` field. The first match is
    /// highlighted as the user types, so Return completes it; a pick
    /// completes the name before the caret, and only while it still
    /// completes that name. Names draw in the monospace font the field
    /// takes them in.
    fn condition_list(
        &mut self,
        field: &Entity<Input>,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let id = field.entity_id();
        let list = cx.new(|cx| {
            Combobox::new(
                SharedString::from(format!("settings-kwhen-{id}")),
                "Contexts",
                field,
                swatch,
                condition_suggestions,
                window,
                cx,
            )
            .with_auto_highlight(true)
            .with_content(Rc::new(condition_row))
        });
        let field = field.downgrade();
        cx.subscribe(&list, move |_, _, event: &ComboboxEvent, cx| {
            let ComboboxEvent::Picked(name) = event;
            let _ = field.update(cx, |field, cx| {
                if let Some((start, matches)) = suggestions(field)
                    && matches
                        .iter()
                        .any(|context| context.name == name.as_ref())
                {
                    // A name the caret sits inside goes whole when the pick
                    // completes all of it, so "Termin|a" keeps no tail, or
                    // when it is already a context, so "Settings|Control"
                    // becomes "SettingsGroup"; otherwise the pick goes in
                    // before the caret, so a name typed in front of another
                    // keeps it: "sel|split" becomes "selection|split".
                    let caret = field.cursor();
                    let end = name_end(field.text(), caret);
                    let whole =
                        field.text().get(start..end).unwrap_or_default();
                    let end = if name
                        .to_lowercase()
                        .starts_with(&whole.to_lowercase())
                        || CONTEXTS.iter().any(|context| {
                            context.name.eq_ignore_ascii_case(whole)
                        }) {
                        end
                    } else {
                        caret
                    };
                    field.replace(start..end, name, cx);
                }
            });
        })
        .detach();
        self.keys.completions.insert(id, list);
    }

    /// The suggestions under the `when` field held under `key`.
    pub(super) fn condition_for(&self, key: &str) -> Option<Entity<Combobox>> {
        let field = self.keys.fields.get(key)?;
        self.keys.completions.get(&field.entity_id()).cloned()
    }

    /// A field by key, created with the file's text as its row would.
    #[cfg(all(test, feature = "test-support"))]
    pub(super) fn key_field_for(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<Input> {
        let text = self.key_field_text(key).unwrap_or_default();
        self.key_field(key.to_owned(), text, "", window, cx)
    }

    /// The shortcut filter's segment, by its value.
    #[cfg(all(test, feature = "test-support"))]
    pub(in crate::desktop::windows) fn key_filter(&self) -> &'static str {
        Filter::ALL
            .iter()
            .find(|(filter, ..)| *filter == self.keys.filter)
            .map_or("", |(_, name, _)| name)
    }

    /// The file's text for a field key, or `None` when its row is gone.
    fn key_field_text(&self, key: &str) -> Option<String> {
        let inventory = self.keys.inventory.as_ref()?;
        if let Some(name) = key.strip_prefix("when:") {
            return row(inventory, name)
                .map(|listed| listed.binding.when.clone().unwrap_or_default());
        }
        let (name, argument) = key.strip_prefix("arg:")?.split_once(':')?;
        row(inventory, name)
            .map(|listed| argument_text(listed, argument).unwrap_or_default())
    }

    /// Brings every keybinding field up to the file. Fields follow their
    /// shortcut: one whose binding now sits at another row moves there with
    /// its draft; one whose binding changed in place, keeping its key and
    /// command, stays; any other row change drops it. A draft stays while
    /// it is being edited or shows an error, such as a failed write, and
    /// its shortcut is active. While the file is invalid fields wait.
    pub(super) fn sync_key_fields(&mut self, cx: &mut Context<'_, Self>) {
        let Some(inventory) = &self.keys.inventory else {
            return;
        };
        let identities: HashMap<String, Identity> = rows(inventory)
            .into_iter()
            .map(|(name, listed)| (name, Identity::of(listed)))
            .collect();
        let previous = std::mem::replace(&mut self.keys.identities, identities);
        self.follow_moved_rows(&previous);
        let keys: Vec<String> = self.keys.fields.keys().cloned().collect();
        for key in keys {
            let Some(text) = self.key_field_text(&key) else {
                // A field leaving while edited would keep focus with no
                // element to dispatch from.
                if self.keys.editing.as_ref() == Some(&key) {
                    self.keys.editing = None;
                    self.keys.stranded = true;
                }
                if let Some(field) = self.keys.fields.remove(&key) {
                    self.keys.completions.remove(&field.entity_id());
                }
                self.keys.synced.remove(&key);
                self.keys.errors.remove(&key);
                continue;
            };
            let Some(field) = self.keys.fields.get(&key).cloned() else {
                continue;
            };
            let current = field.read(cx).text().to_owned();
            let draft = self.keys.synced.get(&key) != Some(&current)
                && (self.keys.editing.as_ref() == Some(&key)
                    || self.keys.errors.contains_key(&key))
                && self.field_row_active(&key);
            if !draft {
                field.update(cx, |field, cx| field.set_text(text.clone(), cx));
            }
            self.keys.synced.insert(key, text);
        }
        cx.notify();
    }

    /// Moves or drops fields whose rows changed since `previous`.
    fn follow_moved_rows(&mut self, previous: &HashMap<String, Identity>) {
        let rows: HashSet<String> = self
            .keys
            .fields
            .keys()
            .filter_map(|key| field_row(key).map(str::to_owned))
            .collect();
        let mut moves: HashMap<String, Option<String>> = HashMap::new();
        for row in rows {
            let Some(before) = previous.get(&row) else {
                continue;
            };
            if self.keys.identities.get(&row) == Some(before) {
                continue;
            }
            let elsewhere: Vec<&String> = self
                .keys
                .identities
                .iter()
                .filter(|(_, now)| *now == before)
                .map(|(name, _)| name)
                .collect();
            let action = match elsewhere.as_slice() {
                [to] => Some((*to).clone()),
                _ if self
                    .keys
                    .identities
                    .get(&row)
                    .is_some_and(|now| now.same_shortcut(before)) =>
                {
                    continue;
                }
                _ => None,
            };
            moves.insert(row, action);
        }
        if moves.is_empty() {
            return;
        }
        // A field moving into a row displaces whatever stayed there.
        let targets: HashSet<String> =
            moves.values().flatten().cloned().collect();
        let mut fields = HashMap::new();
        let mut synced = HashMap::new();
        let mut errors = HashMap::new();
        let mut editing = None;
        for (key, field) in std::mem::take(&mut self.keys.fields) {
            let row = field_row(&key).map(str::to_owned);
            let to = match row.as_ref().map(|row| moves.get(row)) {
                Some(Some(Some(to))) => Some(key.replacen(
                    row.as_deref().unwrap_or_default(),
                    to,
                    1,
                )),
                Some(Some(None)) => None,
                _ if row.as_ref().is_some_and(|row| targets.contains(row)) => {
                    None
                }
                _ => Some(key.clone()),
            };
            let Some(to) = to else {
                if self.keys.editing.as_ref() == Some(&key) {
                    self.keys.stranded = true;
                }
                self.keys.completions.remove(&field.entity_id());
                continue;
            };
            if self.keys.editing.as_ref() == Some(&key) {
                editing = Some(to.clone());
            }
            if let Some(text) = self.keys.synced.remove(&key) {
                synced.insert(to.clone(), text);
            }
            if let Some(error) = self.keys.errors.remove(&key) {
                errors.insert(to.clone(), error);
            }
            fields.insert(to, field);
        }
        // Row-level errors stay with their rows.
        errors.extend(
            std::mem::take(&mut self.keys.errors)
                .into_iter()
                .filter(|(key, _)| field_row(key).is_none()),
        );
        self.keys.fields = fields;
        self.keys.synced = synced;
        self.keys.errors = errors;
        self.keys.editing = editing;
    }

    /// Whether a field key's row is an active shortcut, which edits apply
    /// to.
    fn field_row_active(&self, key: &str) -> bool {
        control_row(key)
            .and_then(|name| row(self.keys.inventory.as_ref()?, name))
            .is_some_and(|listed| listed.state == BindingState::Active)
    }

    /// Saves a `when` or argument field: checked first, written only when
    /// it changed.
    pub(super) fn commit_key_field(
        &mut self,
        key: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(field) = self.keys.fields.get(key) else {
            return;
        };
        let text = field.read(cx).text().trim().to_owned();
        let (name, field_kind) = if let Some(name) = key.strip_prefix("when:") {
            (name.to_owned(), None)
        } else if let Some((name, argument)) = key
            .strip_prefix("arg:")
            .and_then(|rest| rest.split_once(':'))
        {
            (name.to_owned(), Some(argument.to_owned()))
        } else {
            return;
        };
        if self.keys.synced.get(key).map(|synced| synced.trim())
            == Some(text.as_str())
        {
            if self.keys.errors.remove(key).is_some() {
                cx.notify();
            }
            return;
        }
        // While the file is invalid the draft waits, marked, for the fix.
        if self.keys.inventory.is_none() || self.status.invalid() {
            self.keys.errors.insert(
                key.to_owned(),
                "config.toml has an error. Not saved.".to_owned(),
            );
            cx.notify();
            return;
        }
        // Only an active shortcut is editable; anything else shows the file.
        if !self.field_row_active(key) {
            self.cancel_key_field(key, cx);
            return;
        }
        let value = match &field_kind {
            None => match check_condition(&text) {
                Err(message) => {
                    self.keys.errors.insert(
                        key.to_owned(),
                        format!("{}. Not saved.", sentence(&message)),
                    );
                    cx.notify();
                    return;
                }
                Ok(_) => {
                    (!text.is_empty()).then(|| Value::String(text.clone()))
                }
            },
            Some(argument) => {
                let Some(spec) = self
                    .keys
                    .inventory
                    .as_ref()
                    .and_then(|inventory| row(inventory, &name))
                    .and_then(|listed| lookup(listed.binding.command.as_str()))
                    .and_then(|spec| spec.argument(argument))
                else {
                    return;
                };
                match parse_argument(spec, &text) {
                    Ok(value) => value,
                    Err(message) => {
                        self.keys.errors.insert(key.to_owned(), message);
                        cx.notify();
                        return;
                    }
                }
            }
        };
        let field = match field_kind {
            None => Field::When,
            Some(argument) => Field::Argument(argument),
        };
        // The text counts as saved now, so leaving the field before the
        // write lands does not write it again.
        let saved = self.keys.synced.insert(key.to_owned(), text);
        let origin = Some((key.to_owned(), saved.clone()));
        if self.edit_row(&name, field, value, origin, cx) {
            self.keys.errors.remove(key);
        } else {
            // Not submitted: the text is still unsaved.
            match saved {
                Some(saved) => {
                    self.keys.synced.insert(key.to_owned(), saved);
                }
                None => {
                    self.keys.synced.remove(key);
                }
            }
        }
    }

    /// Puts a field back to its saved text.
    pub(super) fn cancel_key_field(
        &mut self,
        key: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(list) = self.condition_for(key) {
            list.update(cx, Combobox::close);
        }
        if let (Some(field), Some(text)) = (
            self.keys.fields.get(key).cloned(),
            self.keys.synced.get(key).cloned(),
        ) {
            field.update(cx, |field, cx| field.set_text(text, cx));
        }
        self.keys.errors.remove(key);
        cx.notify();
    }

    /// The keybinding field with focus, by key.
    pub(super) fn focused_key_field(
        &self,
        window: &Window,
        cx: &gpui::App,
    ) -> Option<(String, Entity<Input>)> {
        if self
            .keys
            .search
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
        {
            return Some(("search".to_owned(), self.keys.search.clone()));
        }
        self.keys
            .fields
            .iter()
            .find(|(_, field)| {
                field.read(cx).focus_handle(cx).is_focused(window)
            })
            .map(|(key, field)| (key.clone(), field.clone()))
    }

    /// Moves focus from the section's search to the first command it
    /// lists.
    pub(super) fn focus_first_command(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(inventory) = self.keys.inventory.clone() else {
            return;
        };
        let query = self.keys.search.read(cx).text().trim().to_lowercase();
        let globals = self.globals();
        let first = grouped()
            .into_iter()
            .flat_map(|(_, commands)| commands)
            .find(|spec| {
                Self::command_matches(
                    &inventory,
                    &globals,
                    spec,
                    self.keys.filter,
                    &query,
                )
            });
        if let Some(spec) = first {
            let handle =
                self.action_stop(format!("kcmd:{}", spec.id.as_str()), cx);
            handle.focus(window, cx);
        }
    }

    /// Steps a number argument's field by one within its range and saves
    /// it.
    pub(super) fn step_argument(
        &mut self,
        key: &str,
        up: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((name, argument)) = key
            .strip_prefix("arg:")
            .and_then(|rest| rest.split_once(':'))
        else {
            return;
        };
        let Some(ArgumentKind::Integer { min, max }) = self
            .keys
            .inventory
            .as_ref()
            .and_then(|inventory| row(inventory, name))
            .and_then(|listed| lookup(listed.binding.command.as_str()))
            .and_then(|spec| spec.argument(argument))
            .map(|spec| spec.kind)
        else {
            return;
        };
        let Some(field) = self.keys.fields.get(key).cloned() else {
            return;
        };
        // An empty field uses the command's default, which the catalog
        // does not state, so there is nothing to step from.
        let Some(current) = field.read(cx).text().trim().parse::<i64>().ok()
        else {
            return;
        };
        let next = if up {
            current.saturating_add(1)
        } else {
            current.saturating_sub(1)
        }
        .clamp(min, max);
        if current == next {
            return;
        }
        field.update(cx, |field, cx| field.set_text(next.to_string(), cx));
        self.commit_key_field(key, cx);
    }

    /// Opens the menu of values for a select argument.
    pub(super) fn open_argument_menu(
        &mut self,
        name: &str,
        argument: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.keys_read_only()
            || !self.field_row_active(&format!("karg:{name}:{argument}"))
        {
            return;
        }
        let Some(spec) = self
            .keys
            .inventory
            .as_ref()
            .and_then(|inventory| row(inventory, name))
            .and_then(|listed| lookup(listed.binding.command.as_str()))
            .and_then(|spec| spec.argument(argument))
        else {
            return;
        };
        // The quake profile's unset value is the `default` profile, which
        // the profiles list already offers.
        let mut rows = if spec.kind == ArgumentKind::QuakeProfile {
            Vec::new()
        } else {
            vec![MenuRow::Item(MenuItem::new(UNSET, unset_label(spec)))]
        };
        match spec.kind {
            ArgumentKind::Bool => {
                rows.push(MenuRow::Item(MenuItem::new("true", "On")));
                rows.push(MenuRow::Item(MenuItem::new("false", "Off")));
            }
            ArgumentKind::Side => {
                rows.extend(
                    SIDES
                        .iter()
                        .map(|side| MenuRow::Item(MenuItem::new(*side, *side))),
                );
            }
            ArgumentKind::Layout => {
                // The palette's titles, with the name the file stores as
                // the item's identity.
                let layouts = cx.global::<Desktop>().config.panes.layouts();
                rows.extend(layouts.iter().map(|layout| {
                    MenuRow::Item(MenuItem::new(
                        interned(&layout.name),
                        interned(&layout.title),
                    ))
                }));
            }
            ArgumentKind::QuakeProfile => {
                let profiles: Vec<String> = cx
                    .global::<Desktop>()
                    .config
                    .quake
                    .profiles
                    .keys()
                    .cloned()
                    .collect();
                rows.extend(profiles.iter().map(|profile| {
                    let profile = interned(profile);
                    MenuRow::Item(MenuItem::new(profile, profile))
                }));
            }
            _ => return,
        }
        let opener = self.action_stop(format!("karg:{name}:{argument}"), cx);
        self.show_menu(
            MenuFor::Argument {
                row: name.to_owned(),
                name: spec.name,
                kind: spec.kind,
            },
            MenuModel::new(rows),
            opener,
            window,
            cx,
        );
    }

    /// The menu item an argument's value matches: `true` or `false`, its
    /// text, or the unset item.
    pub(super) fn argument_choice(
        &self,
        name: &str,
        argument: &str,
    ) -> Option<String> {
        let listed = self
            .keys
            .inventory
            .as_ref()
            .and_then(|inventory| row(inventory, name))?;
        let profile = lookup(listed.binding.command.as_str())
            .and_then(|spec| spec.argument(argument))
            .is_some_and(|spec| spec.kind == ArgumentKind::QuakeProfile);
        Some(match binding_of(listed).args.get(argument) {
            Some(Value::Boolean(on)) => on.to_string(),
            Some(Value::String(text)) => text.clone(),
            Some(other) => other.to_string(),
            None if profile => "default".to_owned(),
            None => UNSET.to_owned(),
        })
    }

    /// Writes a value picked from an argument's menu.
    pub(super) fn pick_argument(
        &mut self,
        name: &str,
        argument: &'static str,
        kind: ArgumentKind,
        picked: &str,
        cx: &mut Context<'_, Self>,
    ) {
        // Only an active shortcut's arguments edit.
        if !self.field_row_active(&format!("karg:{name}:{argument}")) {
            return;
        }
        let value = match (picked, kind) {
            // An unset profile is the `default` profile.
            (UNSET, _) | ("default", ArgumentKind::QuakeProfile) => None,
            (picked, ArgumentKind::Bool) => {
                Some(Value::Boolean(picked == "true"))
            }
            (picked, _) => Some(Value::String(picked.to_owned())),
        };
        // Picking what the binding already has changes nothing.
        let current = self
            .keys
            .inventory
            .as_ref()
            .and_then(|inventory| row(inventory, name))
            .and_then(|listed| binding_of(listed).args.get(argument).cloned());
        if value == current {
            return;
        }
        self.edit_row(
            name,
            Field::Argument(argument.to_owned()),
            value,
            None,
            cx,
        );
    }
}

/// The row an editing control belongs to: a key cap `krec:<row>`, a switch
/// `kon:<row>`, or a field.
fn control_row(key: &str) -> Option<&str> {
    key.strip_prefix("krec:")
        .or_else(|| key.strip_prefix("kon:"))
        .or_else(|| {
            key.strip_prefix("karg:")
                .and_then(|rest| rest.split(':').next())
        })
        .or_else(|| field_row(key))
}

/// The row a field key belongs to: `when:<row>` or `arg:<row>:<name>`.
fn field_row(key: &str) -> Option<&str> {
    key.strip_prefix("when:").or_else(|| {
        key.strip_prefix("arg:")
            .and_then(|rest| rest.split(':').next())
    })
}

/// A message with its first letter capitalized, as a sentence.
fn sentence(message: &str) -> String {
    let mut characters = message.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

/// Parses an argument field's text; empty leaves the argument unset.
fn parse_argument(
    spec: &ArgumentSpec,
    text: &str,
) -> Result<Option<Value>, String> {
    if text.is_empty() {
        return Ok(None);
    }
    match spec.kind {
        ArgumentKind::Integer { min, max } => text
            .parse::<i64>()
            .ok()
            .filter(|value| (min..=max).contains(value))
            .map(|value| Some(Value::Integer(value)))
            .ok_or_else(|| {
                format!("Enter a whole number from {min} to {max}.")
            }),
        _ => Ok(Some(Value::String(text.to_owned()))),
    }
}

/// Rendering.
impl SettingsView {
    /// The Keybindings section's body.
    pub(super) fn render_keybindings(
        &mut self,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Vec<AnyElement> {
        let mut body = vec![self.render_key_tools(swatch, cx)];
        let Some(inventory) = self.keys.inventory.clone() else {
            body.push(
                div()
                    .text_size(ui(13.0))
                    .text_color(swatch.muted)
                    .child("Shortcuts show once config.toml has no errors.")
                    .into_any_element(),
            );
            return body;
        };
        let query = self.keys.search.read(cx).text().trim().to_lowercase();
        let filter = self.keys.filter;
        let globals = self.globals();
        let mut any = false;
        for (group, commands) in grouped() {
            let commands: Vec<&CommandSpec> = commands
                .into_iter()
                .filter(|spec| {
                    Self::command_matches(
                        &inventory, &globals, spec, filter, &query,
                    )
                })
                .collect();
            if commands.is_empty() {
                continue;
            }
            any = true;
            let count = commands.len();
            let rows = commands
                .into_iter()
                .enumerate()
                .map(|(index, spec)| {
                    let corners = card::row_corners(index, count);
                    self.render_command(
                        &inventory, &globals, spec, corners, swatch, window, cx,
                    )
                })
                .collect();
            body.push(group_card(
                id(format!("settings-kgroup-{group}")),
                group,
                rows,
                swatch,
            ));
        }
        if !any {
            body.push(
                div()
                    .text_size(ui(13.0))
                    .text_color(swatch.muted)
                    .child("No commands match.")
                    .into_any_element(),
            );
        }
        if query.is_empty() && filter != Filter::Conditional {
            body.extend(self.render_unbinds(&inventory, swatch, cx));
        }
        body.push(
            div()
                .text_size(ui(12.0))
                .text_color(swatch.muted)
                .child(
                    "A condition limits where a shortcut works, such as \
                     \"Terminal && selection\". Shortcuts on one key with \
                     different conditions coexist; the one matching closest \
                     to focus wins. Switching off a default writes a \
                     disabled = true entry for just that shortcut.",
                )
                .into_any_element(),
        );
        body
    }

    /// The `unbind` entries, each with a button that removes it.
    fn render_unbinds(
        &mut self,
        inventory: &Inventory,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        if inventory.unbinds.is_empty() {
            return None;
        }
        let mut rows = Vec::new();
        for (entry, key) in &inventory.unbinds {
            let remove = self
                .remove_button(
                    format!("settings-kunbind-{entry}"),
                    format!("kunbind:{entry}"),
                    key,
                    swatch,
                    cx,
                )
                .when_some(self.keys_lock_reason(), |button, reason| {
                    button.disabled(Some(reason))
                });
            let hint = KeyHint::parse(key, Platform::current());
            rows.push(
                Item::new(
                    id(format!("settings-kunbound-{entry}")),
                    hint.text(),
                    swatch,
                )
                .title_element(
                    hint.element(px(12.0), swatch.fg).debug_selector(
                        move || format!("settings-kunbound-keys-{entry}"),
                    ),
                )
                .description("Unbound; programs receive this key.")
                .action(remove)
                .into_any_element(),
            );
        }
        Some(group_card(
            "settings-kunbinds",
            "Passed through to programs",
            rows,
            swatch,
        ))
    }

    /// The section's search field and filter.
    fn render_key_tools(
        &mut self,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let search = Self::search_group(
            "keybindings-search-group",
            &self.keys.search,
            swatch,
            cx,
        );
        self.drawn.insert("search".to_owned());
        // One tab stop whose arrows step the filter, as a segmented
        // control's do.
        let handle = self.action_stop("kfilter".to_owned(), cx);
        let focus = handle.clone();
        let segments = Filter::ALL
            .iter()
            .fold(
                ToggleGroup::single(
                    "settings-kfilter",
                    "Show shortcuts",
                    &handle,
                    swatch,
                ),
                |group, (filter, name, label)| {
                    group.option(*name, *label, self.keys.filter == *filter)
                },
            )
            .key_context("SettingsControl")
            .on_select(Self::listen(
                cx,
                move |view, name: SharedString, window, cx| {
                    focus.focus(window, cx);
                    view.act(&format!("kfilter:{name}"), window, cx);
                },
            ));
        // The filter wraps below the search when the pane is narrow.
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .mb(px(12.0))
            .child(Self::text_field(cx).flex_1().min_w(px(200.0)).child(search))
            .child(segments)
            .into_any_element()
    }

    /// One command: a collapsible whose header item names it, with a dot
    /// when it is customized, and summarizes its shortcuts beside its
    /// trigger, and its shortcuts when expanded. `corners` round the
    /// trigger to the card row it fills.
    #[expect(
        clippy::too_many_arguments,
        reason = "a command row needs its bindings, globals, and place"
    )]
    fn render_command(
        &mut self,
        inventory: &Inventory,
        globals: &[Global],
        spec: &'static CommandSpec,
        corners: gpui::Corners<gpui::Pixels>,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let open = self.keys.open.contains(&spec.id);
        let listed: Vec<(String, &ListedBinding)> = rows(inventory)
            .into_iter()
            .filter(|(_, listed)| listed.binding.command == spec.id)
            .collect();
        let active: Vec<&ListedBinding> = listed
            .iter()
            .map(|(_, listed)| *listed)
            .filter(|listed| listed.state == BindingState::Active)
            .collect();
        let command = spec.id.as_str();
        // Global shortcuts list after the in-app ones, by entry.
        let globals: Vec<(usize, &Global)> = globals
            .iter()
            .enumerate()
            .filter(|(_, global)| global.command == command)
            .collect();
        let global_keys: Vec<&str> = globals
            .iter()
            .filter(|(_, global)| !global.disabled)
            .map(|(_, global)| global.key.as_str())
            .collect();
        let summary = summary(&active, &global_keys, swatch)
            .debug_selector(move || format!("settings-ksummary-{command}"));
        let item = Item::new(
            id(format!("settings-kitem-{command}")),
            spec.title,
            swatch,
        )
        .description(spec.description)
        .media(chevron(Icon::ChevronRight.asset_path(), open, swatch))
        .when(
            !globals.is_empty() || Self::customized(inventory, &[], spec.id),
            |item| item.before_title(changed_dot(swatch)),
        )
        .action(summary);
        let key = format!("kcmd:{command}");
        let handle = self.action_stop(key.clone(), cx);
        let focus = handle.clone();
        let mut section = Collapsible::new(
            id(format!("settings-kcmd-{command}")),
            spec.title,
            open,
            &handle,
            swatch,
        )
        .item(item)
        // The summary is content, so the trigger spans it: the whole row
        // hovers and toggles.
        .actions_in_trigger()
        .corners(corners)
        .key_context("SettingsControl")
        .on_toggle(Self::listen(cx, move |view, _, window, cx| {
            focus.focus(window, cx);
            view.act(&key, window, cx);
        }));
        if open {
            section =
                section.child(self.render_shortcuts(
                    spec, &listed, &globals, swatch, window, cx,
                ));
        }
        section.into_any_element()
    }

    /// An expanded command's shortcut rows, then its global shortcuts, and
    /// its buttons.
    fn render_shortcuts(
        &mut self,
        spec: &'static CommandSpec,
        listed: &[(String, &ListedBinding)],
        globals: &[(usize, &Global)],
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let mut block = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(14.0))
            .pt(px(8.0))
            .pb(px(12.0))
            .pl(px(24.0));
        for (name, binding) in listed {
            block = block.child(
                self.render_shortcut(spec, name, binding, swatch, window, cx),
            );
        }
        // The Quake windows section edits the same entries.
        for (index, global) in globals {
            block = block
                .child(self.render_global(*index, global, true, swatch, cx));
        }
        let add_key = format!("kadd:{}", spec.id.as_str());
        let recorder = self.recorder(&add_key, cx);
        let add = KeyRecorderButton::new(
            id(format!("settings-kadd-{}", spec.id.as_str())),
            format!("Add shortcut for {}", spec.title),
            &recorder,
            swatch,
        )
        .placeholder("Add shortcut")
        .key_context("SettingsControl")
        .px(px(10.0))
        .when_some(self.keys_lock_reason(), |add, reason| {
            add.disabled(Some(reason))
        });
        let add_global = self.render_add_global(spec, swatch, cx);
        let mut buttons = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .pt(px(4.0))
            .child(add)
            .children(add_global);
        // Restore defaults appears when it has entries to remove, and says
        // when those include shortcuts the user added.
        let (entries, added) = self
            .keys
            .inventory
            .as_ref()
            .map_or((Vec::new(), 0), |inventory| {
                restorable(inventory, spec.id)
            });
        if !entries.is_empty() {
            let reason = self.keys_lock_reason();
            buttons = buttons.child(
                self.action_button(
                    format!("settings-kreset-{}", spec.id.as_str()),
                    "Restore defaults",
                    format!("kreset:{}", spec.id.as_str()),
                    swatch,
                    cx,
                )
                .when_some(reason, |button, reason| {
                    button.disabled(Some(reason))
                }),
            );
            // With no defaults, removing what the user added is all the
            // button does, and it needs no note.
            let has_defaults =
                self.keys.inventory.as_ref().is_some_and(|inventory| {
                    inventory.bindings.iter().any(|listed| {
                        listed.binding.command == spec.id
                            && listed.binding.origin == Origin::Default
                    })
                });
            if added > 0 && has_defaults {
                buttons = buttons.child(small(
                    if added == 1 {
                        "Also removes the shortcut you added.".to_owned()
                    } else {
                        format!("Also removes the {added} shortcuts you added.")
                    },
                    swatch.muted,
                ));
            }
        }
        block
            .child(buttons)
            .children(self.recording_hint(&add_key, swatch, cx))
            .children(self.add_global_notes(spec, swatch, cx))
            .into_any_element()
    }

    /// The button that records a global shortcut for a quake command,
    /// which can also run while another application has focus.
    fn render_add_global(
        &mut self,
        spec: &'static CommandSpec,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Option<KeyRecorderButton> {
        if !quake::COMMANDS.contains(&spec.id) {
            return None;
        }
        let recorder = self.recorder(&quake::add_key(spec.id.as_str()), cx);
        Some(
            KeyRecorderButton::new(
                id(format!("settings-gadd-{}", spec.id.as_str())),
                format!("Add global shortcut for {}", spec.title),
                &recorder,
                swatch,
            )
            .placeholder("Add global shortcut")
            .key_context("SettingsControl")
            .px(px(10.0))
            .when_some(self.keys_lock_reason(), |add, reason| {
                add.disabled(Some(reason))
            }),
        )
    }

    /// What shows under a quake command's Add global shortcut: the
    /// recorder's hint, why a recording was refused, and that a Wayland
    /// session registers no global shortcuts.
    fn add_global_notes(
        &self,
        spec: &'static CommandSpec,
        swatch: Swatch,
        cx: &gpui::App,
    ) -> Vec<gpui::Div> {
        if !quake::COMMANDS.contains(&spec.id) {
            return Vec::new();
        }
        let key = quake::add_key(spec.id.as_str());
        let error = self.keys.errors.get(&key).cloned();
        self.recording_hint(&key, swatch, cx)
            .into_iter()
            .chain(error.map(|error| small(error, swatch.danger_text())))
            .chain(quake::wayland().then(|| {
                small(
                    "Global shortcuts need macOS or a native X11 session; \
                     this Wayland session does not support them."
                        .to_owned(),
                    swatch.muted,
                )
            }))
            .collect()
    }

    /// What the recorder for an action key is waiting for, under the row it
    /// records into, while it records.
    pub(super) fn recording_hint(
        &self,
        key: &str,
        swatch: Swatch,
        cx: &gpui::App,
    ) -> Option<gpui::Div> {
        let hint = self.keys.recorders.get(key)?.read(cx).hint()?;
        Some(small(hint.to_string(), swatch.accent_text()))
    }

    /// One shortcut: key, condition, arguments, tag, switch, and remove,
    /// with its messages below.
    #[expect(
        clippy::too_many_lines,
        reason = "one row lays out every control a shortcut has"
    )]
    fn render_shortcut(
        &mut self,
        spec: &'static CommandSpec,
        name: &str,
        listed: &ListedBinding,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let active = listed.state == BindingState::Active;
        // Why the key cap, condition, and arguments refuse edits, if they do.
        let refusal = self.key_field_refusal(active);
        let user = listed.binding.origin == Origin::User;
        // The key cap records a new key.
        let record_key = format!("krec:{name}");
        let key = {
            let recorder = self.recorder(&record_key, cx);
            KeyRecorderButton::new(
                id(format!("settings-krec-{name}")),
                format!("{} shortcut", spec.title),
                &recorder,
                swatch,
            )
            .shortcut(Some(listed.binding.key.clone()))
            .key_context("SettingsControl")
            .w(px(CAP_WIDTH))
            .border_color(if user {
                swatch.accent.opacity(0.5)
            } else {
                swatch.line_strong
            })
            .when_some(refusal.clone(), |cap, reason| {
                cap.disabled(Some(reason))
            })
        };
        // The condition, with suggestions floating below it while typing.
        let when_key = format!("when:{name}");
        let placeholder = spec.context.unwrap_or("Anywhere");
        let when = self.key_field(
            when_key.clone(),
            listed.binding.when.clone().unwrap_or_default(),
            placeholder,
            window,
            cx,
        );
        let invalid = self.keys.errors.contains_key(&when_key);
        when.update(cx, |field, cx| {
            field.set_disabled(refusal.is_some(), refusal.clone(), cx);
            field.set_invalid(invalid, cx);
        });
        let completion = self.condition_for(&when_key);
        let when_box = div()
            .flex_1()
            .min_w(px(110.0))
            .relative()
            .child(
                Self::text_field(cx)
                    .font_family(mono_font_family())
                    .child(when),
            )
            .children(completion);
        // Arguments the file can carry, on their own line.
        let mut arguments = Vec::new();
        for argument in configurable(spec) {
            arguments.push(self.render_argument(
                name,
                listed,
                argument,
                refusal.clone(),
                swatch,
                window,
                cx,
            ));
        }
        let tag = {
            let (label, color) = match (&listed.state, user) {
                (BindingState::Active, true) => {
                    ("Custom", swatch.accent_text())
                }
                (BindingState::Replaced(_), _) => ("Replaced", swatch.muted),
                (BindingState::Unbound(_), _) => ("Unbound", swatch.muted),
                (
                    BindingState::Disabled(_) | BindingState::SwitchedOff,
                    true,
                ) => ("Custom", swatch.muted),
                (
                    BindingState::Active
                    | BindingState::Disabled(_)
                    | BindingState::SwitchedOff,
                    false,
                ) => ("Default", swatch.muted),
            };
            // The text color tells an active custom shortcut from the rest.
            badge(
                id(format!("settings-ktag-{name}")),
                label,
                BadgeTone::Neutral,
                swatch,
            )
            .w(px(TAG_WIDTH))
            .text_color(color)
        };
        let switchable = matches!(
            listed.state,
            BindingState::Active
                | BindingState::Disabled(_)
                | BindingState::SwitchedOff
        );
        let reason = self.keys_lock_reason();
        let toggle = switchable.then(|| {
            let key = format!("kon:{name}");
            let handle = self.action_stop(key.clone(), cx);
            let focus = handle.clone();
            Switch::new(
                id(format!("settings-kon-{name}")),
                format!("{} on {}", spec.title, listed.binding.key),
                active,
                &handle,
                swatch,
            )
            .key_context("SettingsControl")
            .when_some(reason.clone(), |switch, reason| {
                switch.disabled(Some(reason))
            })
            .on_toggle(Self::listen(
                cx,
                move |view, _, window, cx| {
                    focus.focus(window, cx);
                    view.act(&key, window, cx);
                },
            ))
        });
        let remove = (user && listed.entry.is_some()).then(|| {
            // A key can repeat under different conditions, so the name
            // carries the command, as the row's switch does.
            self.remove_button(
                format!("settings-kdel-{name}"),
                format!("kdel:{name}"),
                &format!("{} on {}", spec.title, listed.binding.key),
                swatch,
                cx,
            )
            .when_some(reason, |button, reason| button.disabled(Some(reason)))
        });
        let messages = self.shortcut_messages(name, listed, swatch);
        // Fixed slots keep each row's controls in line with the others.
        let slot = |width: f32| {
            div()
                .flex_none()
                .w(px(width))
                .h(ui(24.0))
                .flex()
                .items_center()
                .justify_end()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(6.0))
                    .child(key)
                    .child(when_box)
                    .child(slot(TAG_WIDTH).child(tag))
                    .child(slot(34.0).children(toggle))
                    .child(slot(24.0).children(remove)),
            )
            .when(!arguments.is_empty(), |row| {
                row.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(10.0))
                        .pl(px(CAP_WIDTH + 6.0))
                        .children(arguments),
                )
            })
            .children(self.recording_hint(&record_key, swatch, cx))
            .children(messages)
            .into_any_element()
    }

    /// One argument's control: a number or text field, or a select.
    #[expect(
        clippy::too_many_arguments,
        reason = "a control needs its row, binding, argument, and state"
    )]
    fn render_argument(
        &mut self,
        name: &str,
        listed: &ListedBinding,
        argument: &'static ArgumentSpec,
        refusal: Option<SharedString>,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let value = argument_text(listed, argument.name);
        match argument.kind {
            ArgumentKind::Integer { .. } | ArgumentKind::Text => {
                let field = self.key_field(
                    format!("arg:{name}:{}", argument.name),
                    value.unwrap_or_default(),
                    unset_label(argument),
                    window,
                    cx,
                );
                let width = if matches!(argument.kind, ArgumentKind::Text) {
                    120.0
                } else {
                    64.0
                };
                let invalid = self
                    .keys
                    .errors
                    .contains_key(&format!("arg:{name}:{}", argument.name));
                field.update(cx, |field, cx| {
                    field.set_disabled(refusal.is_some(), refusal, cx);
                    field.set_invalid(invalid, cx);
                });
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(small(argument.name.to_owned(), swatch.muted))
                    .child(Self::text_field(cx).w(px(width)).child(field))
                    .into_any_element()
            }
            _ => {
                let label = match value {
                    // A built-in layout shows its title; the file keeps
                    // its name.
                    Some(name) if argument.kind == ArgumentKind::Layout => {
                        NamedLayout::built_in(&name)
                            .map_or(name, |layout| layout.title.clone())
                    }
                    Some(value) => value,
                    None => unset_label(argument).to_owned(),
                };
                let key = format!("karg:{name}:{}", argument.name);
                let select = self.select(
                    format!("settings-karg-{name}-{}", argument.name),
                    &key,
                    argument.name.to_owned(),
                    label,
                    Opener::Action(key.clone()),
                    refusal.is_some(),
                    px(0.0),
                    swatch,
                    cx,
                );
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(small(argument.name.to_owned(), swatch.muted))
                    .child(select)
                    .into_any_element()
            }
        }
    }

    /// The notes under a shortcut: an error with the typed text first,
    /// then what the file's binding means.
    fn shortcut_messages(
        &self,
        name: &str,
        listed: &ListedBinding,
        swatch: Swatch,
    ) -> Vec<AnyElement> {
        // The row's write failure, then its fields' input errors.
        let errors = self.keys.errors.iter().filter(|(key, _)| {
            key.as_str() == name || field_row(key) == Some(name)
        });
        let error = errors.map(|(_, error)| {
            small(error.clone(), swatch.danger_text()).into_any_element()
        });
        let notes = self
            .keys
            .inventory
            .as_ref()
            .map_or_else(Vec::new, |inventory| notes(inventory, listed));
        error
            .into_iter()
            .chain(notes.into_iter().map(|note| {
                small(
                    note.text,
                    if note.warning {
                        swatch.warning_text()
                    } else {
                        swatch.muted
                    },
                )
                .into_any_element()
            }))
            .collect()
    }

    /// The prompt that asks whether a recorded shortcut takes its key from
    /// the commands in `from`. Reassign applies it; Cancel leaves every
    /// binding as it was.
    pub(super) fn render_reassign(
        &mut self,
        target: &Target,
        key: &str,
        from: &[String],
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let command = match target {
            Target::Row(name) => self
                .keys
                .inventory
                .as_ref()
                .and_then(|inventory| row(inventory, name))
                .map(|listed| listed.binding.command),
            Target::Add(command) => Some(*command),
            Target::Global(_) | Target::AddGlobal(_) => None,
        };
        let title = command
            .and_then(|command| lookup(command.as_str()))
            .map_or("this command", |spec| spec.title);
        let holders = match from {
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
            [] => String::new(),
        };
        let keys = KeyHint::parse(key, Platform::current()).text();
        let (cancel, cancel_focus) =
            self.dialog_button("settings-kreno", "Cancel", "kreno", swatch, cx);
        let (reassign, reassign_focus) = self.dialog_button(
            "settings-kreyes",
            "Reassign",
            "kreyes",
            swatch,
            cx,
        );
        AlertDialog::new(
            "settings-reassign-key",
            format!("{keys} is assigned to {holders}"),
            self.dialog.modal(),
            swatch,
        )
        .description(format!(
            "Reassign it to {title}? {holders} will no longer have this \
             shortcut."
        ))
        .cancel(cancel, &cancel_focus)
        .action(reassign.variant(Variant::Primary), &reassign_focus)
        .into_any_element()
    }

    /// Commands matching the sidebar search, as links into the section.
    pub(super) fn render_key_results(
        &mut self,
        query: &str,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let commands = self.matching_commands(query);
        if commands.is_empty() {
            return None;
        }
        let globals = self.globals();
        let mut rows = Vec::new();
        let count = commands.len().min(20);
        for (index, spec) in commands.into_iter().take(count).enumerate() {
            let caps: Vec<String> = self
                .keys
                .inventory
                .as_ref()
                .map(|inventory| {
                    inventory
                        .bindings
                        .iter()
                        .filter(|listed| {
                            listed.binding.command == spec.id
                                && listed.state == BindingState::Active
                        })
                        .map(|listed| listed.binding.key.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
                .into_iter()
                .chain(
                    globals
                        .iter()
                        .filter(|global| {
                            global.command == spec.id.as_str()
                                && !global.disabled
                        })
                        .map(|global| global.key.clone()),
                )
                .take(2)
                .collect();
            let key = format!("kgo:{}", spec.id.as_str());
            let handle = self.action_stop(key.clone(), cx);
            let focus = handle.clone();
            let act = Self::listen(cx, move |view, (), window, cx| {
                focus.focus(window, cx);
                view.act(&key, window, cx);
            });
            let item = caps.iter().fold(
                Item::new(
                    id(format!("settings-kgo-{}", spec.id.as_str())),
                    spec.title,
                    swatch,
                ),
                |item, key| item.action(cap(key, Origin::Default, swatch)),
            );
            let corners = card::row_corners(index, count);
            rows.push(
                item.link(&handle)
                    .key_context("SettingsControl")
                    .rounded_tl(corners.top_left)
                    .rounded_tr(corners.top_right)
                    .rounded_br(corners.bottom_right)
                    .rounded_bl(corners.bottom_left)
                    .on_click(move |window, cx| act((), window, cx))
                    .into_any_element(),
            );
        }
        Some(group_card("settings-kresults", "Commands", rows, swatch))
    }
}

/// The tag that marks a global shortcut listed under its command, in the
/// slot a shortcut's Default or Custom tag takes.
pub(super) fn global_tag(index: usize, swatch: Swatch) -> gpui::Div {
    div()
        .flex_none()
        .w(px(TAG_WIDTH))
        .flex()
        .justify_end()
        .child(
            badge(
                id(format!("settings-gtag-{index}")),
                "Global",
                BadgeTone::Neutral,
                swatch,
            )
            .text_color(swatch.accent_text()),
        )
}

/// A collapsed command's shortcuts: up to two active keys, in-app ones
/// first, then its active global shortcuts' `globals`, a count of the rest,
/// a "when" tag when one has a condition and a "global" tag when a global
/// one shows, or "No shortcut". The tags draw outlined, since the rows they
/// sit in hover.
fn summary(
    active: &[&ListedBinding],
    globals: &[&str],
    swatch: Swatch,
) -> gpui::Div {
    let tag = |label: &'static str| {
        div()
            .px(px(6.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(swatch.accent_text().opacity(0.35))
            .text_size(ui(11.0))
            .text_color(swatch.accent_text())
            .child(label)
    };
    let mut summary = div().flex().flex_none().items_center().gap(px(6.0));
    let keys = active
        .iter()
        .map(|listed| (listed.binding.key.as_str(), listed.binding.origin))
        .chain(globals.iter().map(|key| (*key, Origin::User)));
    for (key, origin) in keys.take(2) {
        summary = summary.child(cap(key, origin, swatch));
    }
    let count = active.len() + globals.len();
    if count > 2 {
        summary = summary.child(small(format!("+{}", count - 2), swatch.muted));
    }
    if active.iter().any(|listed| listed.binding.when.is_some()) {
        summary = summary.child(tag("when"));
    }
    if !globals.is_empty() && active.len() < 2 {
        summary = summary.child(tag("global"));
    }
    if count == 0 {
        summary = summary.child(small("No shortcut".to_owned(), swatch.muted));
    }
    summary
}

impl SettingsView {
    /// A small × that removes an entry from the file, by running the
    /// action `key` names. It is named "Remove `subject`", where `subject`
    /// is the shortcut, or the command and its shortcut where a key can
    /// repeat, so each row's button reads differently.
    pub(super) fn remove_button(
        &mut self,
        name: String,
        key: String,
        subject: &str,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Button {
        self.action_button(name, format!("Remove {subject}"), key, swatch, cx)
            .variant(Variant::Ghost)
            .icon(
                gpui::svg()
                    .path(Icon::X.asset_path())
                    .size(px(12.0))
                    .text_color(swatch.muted),
            )
            .icon_only()
    }
}

pub(super) fn small(text: String, color: gpui::Hsla) -> gpui::Div {
    div().text_size(ui(11.5)).text_color(color).child(text)
}

/// A shortcut drawn as a key cap; a user binding gets an accent border.
fn cap(key: &str, origin: Origin, swatch: Swatch) -> gpui::Div {
    div()
        .flex_none()
        .px(px(5.0))
        .h(ui(20.0))
        .flex()
        .items_center()
        .rounded(px(4.0))
        .border_1()
        .border_color(if origin == Origin::User {
            swatch.accent
        } else {
            swatch.line_strong
        })
        // Letters and digits sized to the icons beside them, as menus
        // size their shortcuts; inherited text would dwarf the icons.
        .text_size(ui(11.5))
        .child(
            KeyHint::parse(key, Platform::current())
                .element(px(11.0), swatch.fg),
        )
}

/// What desktop tests drive in the keybindings section.
#[cfg(all(test, feature = "test-support"))]
impl SettingsView {
    /// Applies a recording of `key` on `row` as Reassign does, without the
    /// prompt, so a test can queue it behind another write.
    pub(in crate::desktop::windows) fn reassign_row(
        &mut self,
        row: &str,
        key: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let target = Target::Row(row.to_owned());
        let taking = self.taken_by(&target, key);
        self.apply_recording(
            target,
            key.to_owned(),
            &taking.entries,
            taking.last,
            cx,
        );
    }
}
