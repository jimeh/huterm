//! The Keybindings section: command rows that expand to their shortcuts,
//! each with a key recorder, a `when` field, argument controls, and an
//! enable switch. Every change becomes a keybinding [`Edit`] on the
//! window's write queue.
//!
//! Rows are named by where their binding comes from: `d<n>` for the nth
//! platform default and `u<n>` for user entry `n`, so a row keeps its name
//! across reads while the file keeps its entries.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use gpui::{
    AnyElement, Context, ElementId, Entity, FocusHandle, Focusable, Keystroke,
    Subscription, Window, div, prelude::*, px,
};
use huterm_protocol::{
    ArgumentKind, ArgumentSpec, CommandId, CommandSpec, Requirement, SIDES,
    catalog, lookup,
};
use toml::Value;

use super::super::super::key_hint::KeyHint;
use super::super::super::menu::{MenuItem, MenuModel, MenuRow};
use super::super::super::overlay::{Swatch, mono_font_family};
use super::super::super::settings::keybindings::{
    Binding, Context as WhenContext, Edit, Field, GROUPS, OVERLAYS,
    check_condition, completions, notes,
};
use super::super::Desktop;
use super::saving::Queued;
use super::{MenuFor, SettingsView, button};
use crate::assets::Icon;
use crate::keymap::{BindingState, Inventory, ListedBinding, Origin, Platform};
use crate::ui::text_field::{Changed, TextField};

/// How long the recorder waits for a chord's second stroke: GPUI's own
/// chord timeout.
pub(crate) const CHORD_TIMEOUT: Duration = Duration::from_secs(1);

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
    AddGlobal,
}

pub(super) struct Recording {
    pub(super) target: Target,
    pub(super) strokes: Vec<String>,
    /// Bumped by each stroke, so only the latest stroke's timer finishes.
    generation: u64,
    _blur: Subscription,
}

/// The section's state, held by the view.
pub(super) struct Keys {
    /// The file's bindings and what became of each; `None` until a read
    /// finds bindings that compile.
    pub(super) inventory: Option<Inventory>,
    pub(super) open: HashSet<CommandId>,
    pub(super) filter: Filter,
    pub(super) search: Entity<TextField>,
    /// `when` and argument fields, keyed `when:<row>` or
    /// `arg:<row>:<name>`.
    pub(super) fields: HashMap<String, Entity<TextField>>,
    /// Each field's text from the file; a focused field that differs
    /// holds a draft.
    pub(super) synced: HashMap<String, String>,
    /// Errors by row.
    pub(super) errors: HashMap<String, String>,
    pub(super) recording: Option<Recording>,
    /// The completion list under a `when` field: the field's key and the
    /// selected suggestion.
    pub(super) completion: Option<(String, usize)>,
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
    /// Strokes recorded across every recording; a chord timer finishes its
    /// recording only while no later stroke has arrived.
    pub(super) strokes: u64,
    /// When the recorder last saved by its chord timeout: a Return or Space
    /// meant to save, arriving just after, must not start recording again.
    pub(super) timed_out_at: Option<std::time::Instant>,
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
            TextField::new(
                "Search commands, shortcuts, or conditions",
                swatch.fg,
                cx,
            )
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
            recording: None,
            completion: None,
            editing: None,
            follow: None,
            stranded: false,
            identities: HashMap::new(),
            entries: Vec::new(),
            globals: Vec::new(),
            writing_row: None,
            strokes: 0,
            timed_out_at: None,
        }
    }
}

/// A row's binding, to recognize it when the entries move.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Identity {
    whole: String,
    key: String,
    command: CommandId,
}

impl Identity {
    fn of(listed: &ListedBinding) -> Self {
        Self {
            whole: format!("{:?}", listed.binding),
            key: listed.binding.key.clone(),
            command: listed.binding.command,
        }
    }

    /// The same shortcut with another condition or other arguments.
    fn same_shortcut(&self, other: &Self) -> bool {
        self.key == other.key && self.command == other.command
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
fn suggestions(field: &TextField) -> Option<(usize, Vec<WhenContext>)> {
    let (start, matches) = completions(field.text(), field.cursor());
    (start < field.cursor() && !matches.is_empty()).then_some((start, matches))
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
        self.locked() || self.keys.inventory.is_none()
    }

    /// Whether a command has a user entry or a default that is not active.
    fn customized(inventory: &Inventory, command: CommandId) -> bool {
        inventory.bindings.iter().any(|listed| {
            listed.binding.command == command
                && (listed.binding.origin == Origin::User
                    || listed.state != BindingState::Active)
        })
    }

    fn command_matches(
        inventory: &Inventory,
        spec: &CommandSpec,
        filter: Filter,
        query: &str,
    ) -> bool {
        let listed: Vec<&ListedBinding> = inventory
            .bindings
            .iter()
            .filter(|listed| listed.binding.command == spec.id)
            .collect();
        let passes = match filter {
            Filter::All => true,
            Filter::Customized => Self::customized(inventory, spec.id),
            Filter::Conditional => listed.iter().any(|listed| {
                listed.state == BindingState::Active
                    && listed.binding.when.is_some()
            }),
            Filter::Disabled => listed.iter().any(|listed| {
                matches!(
                    listed.state,
                    BindingState::Disabled(_) | BindingState::SwitchedOff
                )
            }),
        };
        passes
            && (query.is_empty()
                || spec.title.to_lowercase().contains(query)
                || spec.id.as_str().contains(query)
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
        grouped()
            .into_iter()
            .flat_map(|(_, commands)| commands)
            .filter(|spec| {
                Self::command_matches(inventory, spec, Filter::All, query)
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
            "krec" => {
                let handle = self.action_stop(format!("krec:{argument}"), cx);
                self.start_recording(
                    Target::Row(argument.to_owned()),
                    &handle,
                    window,
                    cx,
                );
            }
            "kadd" => {
                if let Some(spec) = lookup(argument) {
                    let handle =
                        self.action_stop(format!("kadd:{argument}"), cx);
                    self.start_recording(
                        Target::Add(spec.id),
                        &handle,
                        window,
                        cx,
                    );
                }
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
        self.submit_keys(edit, Some(name.to_owned()), origin, cx)
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

    /// After a changed default lands, the changed binding is the file's
    /// last entry; when focus is still on the control that changed it,
    /// focus moves to the same control on the new row, so further typing
    /// edits that row rather than the default it replaced.
    pub(super) fn follow_changed_default(
        &mut self,
        edit: &Edit,
        row: Option<&str>,
    ) {
        let (Edit::ChangeDefault { default, field, .. }, Some(row)) =
            (edit, row)
        else {
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

    pub(super) fn start_recording(
        &mut self,
        target: Target,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.keys_read_only() {
            return;
        }
        // A Return meant to save, landing just after the chord timeout
        // already saved, would otherwise start a new recording.
        if self.keys.timed_out_at.take().is_some_and(|at| {
            cx.background_executor().now().duration_since(at)
                < CHORD_TIMEOUT / 2
        }) {
            return;
        }
        focus.focus(window, cx);
        let blur = cx.on_blur(focus, window, |view, _, cx| {
            view.cancel_recording(cx);
        });
        self.keys.recording = Some(Recording {
            target,
            strokes: Vec::new(),
            generation: 0,
            _blur: blur,
        });
        cx.notify();
    }

    pub(super) fn cancel_recording(&mut self, cx: &mut Context<'_, Self>) {
        if self.keys.recording.take().is_some() {
            cx.notify();
        }
    }

    /// Takes a key-down while recording, before any binding or menu sees
    /// it. Returns whether it was taken.
    pub(super) fn record(
        &mut self,
        keystroke: &Keystroke,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(recording) = &mut self.keys.recording else {
            return false;
        };
        let plain = !keystroke.modifiers.modified();
        match keystroke.key.as_str() {
            "escape" if plain => {
                self.cancel_recording(cx);
                return true;
            }
            "enter" if plain && !recording.strokes.is_empty() => {
                self.finish_recording(cx);
                return true;
            }
            "shift" | "control" | "alt" | "platform" | "function" => {
                return true;
            }
            _ => {}
        }
        let generation = self.keys.strokes + 1;
        self.keys.strokes = generation;
        let Some(recording) = &mut self.keys.recording else {
            return false;
        };
        recording.strokes.push(keystroke.unparse());
        recording.generation = generation;
        // A global shortcut is one stroke.
        let global =
            matches!(recording.target, Target::Global(_) | Target::AddGlobal);
        if global || recording.strokes.len() >= 2 {
            self.finish_recording(cx);
            return true;
        }
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(CHORD_TIMEOUT).await;
            let _ =
                view.update(cx, |view, cx| {
                    if view.keys.recording.as_ref().is_some_and(|recording| {
                        recording.generation == generation
                    }) {
                        view.finish_recording(cx);
                        view.keys.timed_out_at =
                            Some(cx.background_executor().now());
                    }
                });
        })
        .detach();
        cx.notify();
        true
    }

    fn finish_recording(&mut self, cx: &mut Context<'_, Self>) {
        let Some(recording) = self.keys.recording.take() else {
            return;
        };
        let key = recording.strokes.join(" ");
        if key.is_empty() {
            cx.notify();
            return;
        }
        match recording.target {
            Target::Row(name) => {
                let current = self
                    .keys
                    .inventory
                    .as_ref()
                    .and_then(|inventory| row(inventory, &name))
                    .map(|listed| listed.binding.key.clone());
                if current.as_deref() != Some(key.as_str()) {
                    self.edit_row(
                        &name,
                        Field::Key,
                        Some(Value::String(key)),
                        None,
                        cx,
                    );
                }
            }
            Target::Add(command) => {
                self.keys.open.insert(command);
                self.submit_keys(Edit::Add { key, command }, None, None, cx);
            }
            Target::Global(entry) => self.finish_global(Some(entry), key, cx),
            Target::AddGlobal => self.finish_global(None, key, cx),
        }
        cx.notify();
    }

    /// A `when` or argument field for a row, created on first use.
    fn key_field(
        &mut self,
        key: String,
        text: String,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<TextField> {
        self.drawn.insert(key.clone());
        if let Some(field) = self.keys.fields.get(&key) {
            return field.clone();
        }
        let swatch = Self::swatch(cx);
        let field = cx.new(|cx| {
            let mut field = TextField::new(placeholder, swatch.fg, cx);
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
            let Some(blurred) = view.keys.key_of(id) else {
                return;
            };
            if view.keys.editing.as_ref() == Some(&blurred) {
                view.keys.editing = None;
            }
            view.keys.completion = None;
            // Blur runs inside a draw, where a redraw request is lost; the
            // save waits until the draw is done, so an error it finds shows.
            cx.defer_in(window, move |view, _, cx| {
                view.commit_key_field(&blurred, cx);
            });
        })
        .detach();
        if key.starts_with("when:") {
            // Typing a name in the focused field offers its completions;
            // an empty name, or text set from the file, offers none.
            cx.subscribe_in(
                &field,
                window,
                move |view, field, _: &Changed, window, cx| {
                    let open = {
                        let field = field.read(cx);
                        field.focus_handle(cx).is_focused(window)
                            && suggestions(field).is_some()
                    };
                    view.keys.completion = view
                        .keys
                        .key_of(id)
                        .filter(|_| open)
                        .map(|key| (key, 0));
                    cx.notify();
                },
            )
            .detach();
        }
        self.keys.fields.insert(key.clone(), field.clone());
        self.keys.synced.insert(key, text);
        field
    }

    /// A field by key, created with the file's text as its row would.
    #[cfg(all(test, feature = "test-support"))]
    pub(super) fn key_field_for(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<TextField> {
        let text = self.key_field_text(key).unwrap_or_default();
        self.key_field(key.to_owned(), text, "", window, cx)
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
                self.keys.fields.remove(&key);
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
        self.keys.completion = None;
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
        self.keys.completion = None;
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
    ) -> Option<(String, Entity<TextField>)> {
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

    /// Moves the completion selection; `false` when no list is open for
    /// `key`.
    pub(super) fn step_completion(
        &mut self,
        key: &str,
        forward: bool,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(field) = self.keys.fields.get(key) else {
            return false;
        };
        let count =
            suggestions(field.read(cx)).map_or(0, |(_, matches)| matches.len());
        let Some((open, selected)) = &mut self.keys.completion else {
            return false;
        };
        if open != key || count == 0 {
            return false;
        }
        *selected = if forward {
            (*selected + 1) % count
        } else {
            (*selected + count - 1) % count
        };
        cx.notify();
        true
    }

    /// Accepts the selected completion; `false` when no list is open for
    /// `key`.
    pub(super) fn accept_completion(
        &mut self,
        key: &str,
        index: Option<usize>,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some((open, selected)) = self.keys.completion.clone() else {
            return false;
        };
        if open != key {
            return false;
        }
        let Some(field) = self.keys.fields.get(key).cloned() else {
            return false;
        };
        let Some((start, cursor, matches)) = ({
            let field = field.read(cx);
            suggestions(field)
                .map(|(start, matches)| (start, field.cursor(), matches))
        }) else {
            return false;
        };
        let Some(context) = matches.get(index.unwrap_or(selected)) else {
            return false;
        };
        field.update(cx, |field, cx| {
            field.replace(start..cursor, context.name, cx);
        });
        self.keys.completion = None;
        cx.notify();
        true
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
        let first = grouped()
            .into_iter()
            .flat_map(|(_, commands)| commands)
            .find(|spec| {
                Self::command_matches(
                    &inventory,
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
    fn open_argument_menu(
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
                        .map(|side| MenuRow::Item(MenuItem::new(side, *side))),
                );
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
                    .text_size(px(13.0))
                    .text_color(swatch.muted)
                    .child("Shortcuts show once config.toml has no errors.")
                    .into_any_element(),
            );
            return body;
        };
        let query = self.keys.search.read(cx).text().trim().to_lowercase();
        let filter = self.keys.filter;
        let mut any = false;
        for (group, commands) in grouped() {
            let commands: Vec<&CommandSpec> = commands
                .into_iter()
                .filter(|spec| {
                    Self::command_matches(&inventory, spec, filter, &query)
                })
                .collect();
            if commands.is_empty() {
                continue;
            }
            any = true;
            let mut card = card(swatch);
            for (index, spec) in commands.into_iter().enumerate() {
                let command =
                    self.render_command(&inventory, spec, swatch, window, cx);
                card = card.child(
                    div()
                        .when(index > 0, |row| {
                            row.border_t_1().border_color(swatch.line)
                        })
                        .child(command),
                );
            }
            body.push(group_block(group, card, swatch));
        }
        if !any {
            body.push(
                div()
                    .text_size(px(13.0))
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
                .text_size(px(12.0))
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
        let mut card = card(swatch);
        for (index, (entry, key)) in inventory.unbinds.iter().enumerate() {
            let remove = self.actionable(
                remove_button(format!("settings-kunbind-{entry}"), swatch),
                format!("kunbind:{entry}"),
                swatch,
                cx,
            );
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(14.0))
                    .py(px(10.0))
                    .when(index > 0, |row| {
                        row.border_t_1().border_color(swatch.line)
                    })
                    .child(
                        KeyHint::parse(key, Platform::current())
                            .element(px(12.0), swatch.fg),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .text_color(swatch.muted)
                            .child("Unbound; programs receive this key."),
                    )
                    .child(remove),
            );
        }
        Some(group_block("Passed through to programs", card, swatch))
    }

    /// The section's search field and filter.
    fn render_key_tools(
        &mut self,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let search = self.keys.search.clone();
        self.drawn.insert("search".to_owned());
        let mut segments = div()
            .flex()
            .flex_none()
            .p(px(2.0))
            .gap(px(2.0))
            .rounded(px(6.0))
            .bg(swatch.chip);
        for (filter, name, label) in Filter::ALL {
            let on = self.keys.filter == filter;
            let segment = div()
                .id(id(format!("settings-kfilter-{name}")))
                .px(px(8.0))
                .h(px(22.0))
                .flex()
                .items_center()
                .rounded(px(4.0))
                .text_size(px(12.0))
                .text_color(if on { swatch.fg } else { swatch.muted })
                .when(on, |segment| segment.bg(swatch.selection))
                .child(label);
            segments = segments.child(self.actionable(
                segment,
                format!("kfilter:{name}"),
                swatch,
                cx,
            ));
        }
        // The filter wraps below the search when the pane is narrow.
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .mb(px(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(200.0))
                    .h(px(26.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(swatch.line_strong)
                    .bg(swatch.bg)
                    .text_size(px(12.5))
                    .key_context("Palette")
                    .child(search),
            )
            .child(segments)
            .into_any_element()
    }

    /// One command: its header, and its shortcuts when expanded.
    fn render_command(
        &mut self,
        inventory: &Inventory,
        spec: &'static CommandSpec,
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
        let summary = summary(&active, swatch);
        let customized = Self::customized(inventory, spec.id);
        let head = div()
            .id(id(format!("settings-kcmd-{}", spec.id.as_str())))
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(14.0))
            .py(px(9.0))
            .hover(|style| style.bg(swatch.hover))
            .child(
                gpui::svg()
                    .path(Icon::ChevronRight.asset_path())
                    .flex_none()
                    .size(px(12.0))
                    .text_color(swatch.muted)
                    .when(open, |chevron| {
                        chevron.with_transformation(
                            gpui::Transformation::rotate(gpui::percentage(
                                0.25,
                            )),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_size(px(13.0))
                            .text_color(swatch.fg)
                            .when(customized, |line| {
                                line.child(
                                    div()
                                        .flex_none()
                                        .size(px(6.0))
                                        .rounded_full()
                                        .bg(swatch.accent),
                                )
                            })
                            .child(spec.title),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(swatch.muted)
                            .truncate()
                            .child(spec.description.replace('`', "")),
                    ),
            )
            .child(summary);
        let head = self.actionable(
            head,
            format!("kcmd:{}", spec.id.as_str()),
            swatch,
            cx,
        );
        let mut command = div().flex().flex_col().child(head);
        if open {
            command = command.child(
                self.render_shortcuts(spec, &listed, swatch, window, cx),
            );
        }
        command.into_any_element()
    }

    /// An expanded command's shortcut rows and its buttons.
    fn render_shortcuts(
        &mut self,
        spec: &'static CommandSpec,
        listed: &[(String, &ListedBinding)],
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let mut block = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(14.0))
            .pb(px(12.0))
            .pl(px(24.0));
        for (name, binding) in listed {
            block = block.child(
                self.render_shortcut(spec, name, binding, swatch, window, cx),
            );
        }
        let recording_add =
            self.keys.recording.as_ref().is_some_and(|recording| {
                recording.target == Target::Add(spec.id)
            });
        let read_only = self.keys_read_only();
        let add = {
            let label = if recording_add {
                self.recording_content(swatch)
            } else {
                "Add shortcut".into_any_element()
            };
            let element =
                button(format!("settings-kadd-{}", spec.id.as_str()), swatch)
                    .when(recording_add, |button| {
                        button.border_color(swatch.accent)
                    })
                    .when(read_only, |button| button.opacity(0.5))
                    .child(label);
            self.actionable(
                element,
                format!("kadd:{}", spec.id.as_str()),
                swatch,
                cx,
            )
        };
        let mut buttons = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .pt(px(4.0))
            .child(add);
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
            let element =
                button(format!("settings-kreset-{}", spec.id.as_str()), swatch)
                    .when(read_only, |button| button.opacity(0.5))
                    .child("Restore defaults");
            buttons = buttons.child(self.actionable(
                element,
                format!("kreset:{}", spec.id.as_str()),
                swatch,
                cx,
            ));
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
        let block = block.child(buttons);
        block
            .when(recording_add, |block| {
                block.child(self.recording_hint(swatch))
            })
            .into_any_element()
    }

    /// The keys recorded so far as the platform draws them, or a prompt.
    pub(super) fn recording_content(&self, swatch: Swatch) -> AnyElement {
        match &self.keys.recording {
            Some(recording) if !recording.strokes.is_empty() => div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(
                    KeyHint::parse(
                        &recording.strokes.join(" "),
                        Platform::current(),
                    )
                    .element(px(12.0), swatch.accent),
                )
                .child(small("…".to_owned(), swatch.accent))
                .into_any_element(),
            _ => {
                small("Press keys".to_owned(), swatch.accent).into_any_element()
            }
        }
    }

    /// What the recorder is waiting for, under the row it records into.
    pub(super) fn recording_hint(&self, swatch: Swatch) -> gpui::Div {
        let waiting = self
            .keys
            .recording
            .as_ref()
            .is_some_and(|recording| !recording.strokes.is_empty());
        small(
            if waiting {
                "Saves in a moment. Press a second key for a chord, Return to \
                 save now, or Esc to cancel."
                    .to_owned()
            } else {
                "Press the new shortcut. Esc cancels.".to_owned()
            },
            swatch.accent,
        )
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
        let read_only = self.keys_read_only();
        let active = listed.state == BindingState::Active;
        let editable = active && !read_only;
        let user = listed.binding.origin == Origin::User;
        let recording = self.keys.recording.as_ref().is_some_and(|recording| {
            recording.target == Target::Row(name.to_owned())
        });
        // The key cap records a new key.
        let key = {
            let content = if recording {
                self.recording_content(swatch)
            } else {
                KeyHint::parse(&listed.binding.key, Platform::current())
                    .element(
                        px(12.0),
                        if active { swatch.fg } else { swatch.dim },
                    )
                    .into_any_element()
            };
            let element = div()
                .id(id(format!("settings-krec-{name}")))
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
                } else if user {
                    swatch.accent.opacity(0.5)
                } else {
                    swatch.line_strong
                })
                .when(!editable, |cap| cap.opacity(0.6))
                .child(content);
            let element =
                self.actionable(element, format!("krec:{name}"), swatch, cx);
            let _ = self
                .action_stop(format!("krec:{name}"), cx)
                .tab_stop(editable);
            element
        };
        // The condition, with completion while typing.
        let when_key = format!("when:{name}");
        let placeholder = spec.context.unwrap_or("Anywhere");
        let when = self.key_field(
            when_key.clone(),
            listed.binding.when.clone().unwrap_or_default(),
            placeholder,
            window,
            cx,
        );
        let _ = when.read(cx).focus_handle(cx).tab_stop(editable);
        let invalid = self.keys.errors.contains_key(&when_key);
        let completion =
            self.render_completion(&when_key, &when, swatch, window, cx);
        let when_box = div()
            .flex_1()
            .min_w(px(110.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(24.0))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(if invalid {
                        swatch.danger
                    } else {
                        swatch.line_strong
                    })
                    .overflow_hidden()
                    .bg(swatch.bg)
                    .text_size(px(12.0))
                    .font_family(mono_font_family())
                    .key_context("Palette")
                    .when(!editable, |field| field.opacity(0.6))
                    .child(when),
            )
            .children(completion);
        // Arguments the file can carry, on their own line.
        let mut arguments = Vec::new();
        for argument in configurable(spec) {
            arguments.push(self.render_argument(
                name, listed, argument, editable, swatch, window, cx,
            ));
        }
        let tag = {
            let (label, color) = match (&listed.state, user) {
                (BindingState::Active, true) => ("Custom", swatch.accent),
                (BindingState::Active, false) => ("Default", swatch.muted),
                (BindingState::Replaced(_), _) => ("Replaced", swatch.dim),
                (BindingState::Unbound(_), _) => ("Unbound", swatch.dim),
                (
                    BindingState::Disabled(_) | BindingState::SwitchedOff,
                    true,
                ) => ("Custom", swatch.dim),
                (
                    BindingState::Disabled(_) | BindingState::SwitchedOff,
                    false,
                ) => ("Default", swatch.dim),
            };
            div()
                .flex_none()
                .w(px(TAG_WIDTH))
                .flex()
                .justify_center()
                .rounded(px(4.0))
                .bg(swatch.chip)
                .text_size(px(10.5))
                .text_color(color)
                .child(label)
        };
        let switchable = matches!(
            listed.state,
            BindingState::Active
                | BindingState::Disabled(_)
                | BindingState::SwitchedOff
        );
        let toggle = switchable.then(|| {
            let on = active;
            let element = div()
                .id(id(format!("settings-kon-{name}")))
                .flex_none()
                .w(px(30.0))
                .h(px(18.0))
                .p(px(2.0))
                .rounded_full()
                .bg(if on {
                    swatch.accent
                } else {
                    swatch.line_strong
                })
                .when(read_only, |control| control.opacity(0.5))
                .child(
                    div()
                        .size(px(14.0))
                        .rounded_full()
                        .bg(swatch.bg)
                        .when(on, |knob| knob.ml(px(12.0))),
                );
            let element =
                self.actionable(element, format!("kon:{name}"), swatch, cx);
            let _ = self
                .action_stop(format!("kon:{name}"), cx)
                .tab_stop(!read_only);
            element
        });
        let remove = (user && listed.entry.is_some()).then(|| {
            let element =
                remove_button(format!("settings-kdel-{name}"), swatch);
            let element =
                self.actionable(element, format!("kdel:{name}"), swatch, cx);
            let _ = self
                .action_stop(format!("kdel:{name}"), cx)
                .tab_stop(!read_only);
            element
        });
        let messages = self.shortcut_messages(name, listed, swatch);
        // Fixed slots keep each row's controls in line with the others.
        let slot = |width: f32| {
            div()
                .flex_none()
                .w(px(width))
                .h(px(24.0))
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
                    .child(slot(30.0).children(toggle))
                    .child(slot(20.0).children(remove)),
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
            .when(recording, |row| row.child(self.recording_hint(swatch)))
            .children(messages)
            .into_any_element()
    }

    /// The completion list under a focused `when` field.
    fn render_completion(
        &mut self,
        key: &str,
        field: &Entity<TextField>,
        swatch: Swatch,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let (open, selected) = self.keys.completion.clone()?;
        if open != key || !field.read(cx).focus_handle(cx).is_focused(window) {
            return None;
        }
        let (_, matches) = suggestions(field.read(cx))?;
        let mut list = div()
            .flex()
            .flex_col()
            .mt(px(2.0))
            .p(px(2.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(swatch.line_strong)
            .bg(swatch.surface);
        for (index, context) in matches.into_iter().take(8).enumerate() {
            let key = key.to_owned();
            list = list.child(
                div()
                    .id(id(format!("settings-kwhen-{index}")))
                    .flex()
                    .flex_col()
                    .px(px(6.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .when(index == selected, |item| item.bg(swatch.selection))
                    .hover(|item| item.bg(swatch.hover))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            view.accept_completion(&key, Some(index), cx);
                        }),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(swatch.fg)
                            .font_family(mono_font_family())
                            .child(context.name),
                    )
                    .child(small(context.description.to_owned(), swatch.muted)),
            );
        }
        Some(list.into_any_element())
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
        editable: bool,
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
                let _ = field.read(cx).focus_handle(cx).tab_stop(editable);
                let width = if matches!(argument.kind, ArgumentKind::Text) {
                    120.0
                } else {
                    64.0
                };
                let invalid = self
                    .keys
                    .errors
                    .contains_key(&format!("arg:{name}:{}", argument.name));
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(small(argument.name.to_owned(), swatch.muted))
                    .child(
                        div()
                            .w(px(width))
                            .h(px(24.0))
                            .px(px(6.0))
                            .flex()
                            .items_center()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(if invalid {
                                swatch.danger
                            } else {
                                swatch.line_strong
                            })
                            .overflow_hidden()
                            .bg(swatch.bg)
                            .text_size(px(12.0))
                            .key_context("Palette")
                            .when(!editable, |field| field.opacity(0.6))
                            .child(field),
                    )
                    .into_any_element()
            }
            _ => {
                let label =
                    value.unwrap_or_else(|| unset_label(argument).to_owned());
                let element = button(
                    format!("settings-karg-{name}-{}", argument.name),
                    swatch,
                )
                .gap(px(4.0))
                .when(!editable, |button| button.opacity(0.6))
                .child(label);
                let element = self.actionable(
                    element,
                    format!("karg:{name}:{}", argument.name),
                    swatch,
                    cx,
                );
                let _ = self
                    .action_stop(format!("karg:{name}:{}", argument.name), cx)
                    .tab_stop(editable);
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(small(argument.name.to_owned(), swatch.muted))
                    .child(self.with_menu(
                        &format!("karg:{name}:{}", argument.name),
                        element.into_any_element(),
                    ))
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
            small(error.clone(), swatch.danger).into_any_element()
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
                        swatch.warning
                    } else {
                        swatch.muted
                    },
                )
                .into_any_element()
            }))
            .collect()
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
        let mut card = card(swatch);
        for (index, spec) in commands.into_iter().take(20).enumerate() {
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
                        .take(2)
                        .map(|listed| listed.binding.key.clone())
                        .collect()
                })
                .unwrap_or_default();
            let mut keys = div().flex().flex_none().gap(px(6.0));
            for key in &caps {
                keys = keys.child(cap(key, Origin::Default, swatch));
            }
            let element = div()
                .id(id(format!("settings-kgo-{}", spec.id.as_str())))
                .flex()
                .items_center()
                .gap(px(10.0))
                .px(px(14.0))
                .py(px(8.0))
                .when(index > 0, |row| {
                    row.border_t_1().border_color(swatch.line)
                })
                .hover(|style| style.bg(swatch.hover))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.0))
                        .text_color(swatch.fg)
                        .child(spec.title),
                )
                .child(keys);
            card = card.child(self.actionable(
                element,
                format!("kgo:{}", spec.id.as_str()),
                swatch,
                cx,
            ));
        }
        Some(group_block("Commands", card, swatch))
    }
}

/// A collapsed command's shortcuts: up to two active keys, a count of the
/// rest, a "when" chip when one has a condition, or "No shortcut".
fn summary(active: &[&ListedBinding], swatch: Swatch) -> gpui::Div {
    let mut summary = div().flex().flex_none().items_center().gap(px(6.0));
    for listed in active.iter().take(2) {
        summary = summary.child(cap(
            &listed.binding.key,
            listed.binding.origin,
            swatch,
        ));
    }
    if active.len() > 2 {
        summary = summary
            .child(small(format!("+{}", active.len() - 2), swatch.muted));
    }
    if active.iter().any(|listed| listed.binding.when.is_some()) {
        summary = summary.child(
            div()
                .px(px(6.0))
                .rounded(px(8.0))
                .bg(swatch.chip)
                .text_size(px(11.0))
                .text_color(swatch.accent)
                .child("when"),
        );
    }
    if active.is_empty() {
        summary = summary.child(small("No shortcut".to_owned(), swatch.muted));
    }
    summary
}

/// A small × that removes an entry from the file.
pub(super) fn remove_button(
    name: String,
    swatch: Swatch,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id(name))
        .flex()
        .items_center()
        .justify_center()
        .size(px(20.0))
        .rounded(px(4.0))
        .text_color(swatch.muted)
        .hover(|style| style.bg(swatch.hover).text_color(swatch.fg))
        .child(
            gpui::svg()
                .path(Icon::X.asset_path())
                .size(px(12.0))
                .text_color(swatch.muted),
        )
}

pub(super) fn card(swatch: Swatch) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(px(8.0))
        .border_1()
        .border_color(swatch.line)
        .bg(swatch.surface)
}

pub(super) fn group_block(
    title: &str,
    card: gpui::Div,
    swatch: Swatch,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .mb(px(12.0))
        .child(
            div()
                .text_size(px(11.0))
                .text_color(swatch.muted)
                .child(title.to_uppercase()),
        )
        .child(card)
        .into_any_element()
}

pub(super) fn small(text: String, color: gpui::Hsla) -> gpui::Div {
    div().text_size(px(11.5)).text_color(color).child(text)
}

/// A shortcut drawn as a key cap; a user binding gets an accent border.
fn cap(key: &str, origin: Origin, swatch: Swatch) -> gpui::Div {
    div()
        .flex_none()
        .px(px(5.0))
        .h(px(20.0))
        .flex()
        .items_center()
        .rounded(px(4.0))
        .border_1()
        .border_color(if origin == Origin::User {
            swatch.accent
        } else {
            swatch.line_strong
        })
        .child(
            KeyHint::parse(key, Platform::current())
                .element(px(11.0), swatch.fg),
        )
}
