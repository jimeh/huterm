//! Keymap compilation: platform defaults plus user entries become GPUI
//! bindings, an effective-binding list, and the reserved-key set that keeps
//! bound chords away from terminal input.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    DummyKeyboardMapper, KeyBinding, KeyBindingContextPredicate, KeyContext,
    Keystroke, Modifiers,
};
use huterm_protocol::{
    ArgumentKind, CommandArgument, CommandId, CommandInvocation, CommandSpec,
    CommandValue, ids, lookup, validate_supplied,
};

use crate::commands::CommandAction;
use crate::config::{KeybindingEntry, keybinding_diagnostic};

/// Reserved command that removes every earlier binding for a key.
pub(crate) const UNBIND: &str = "unbind";

/// Host platform whose default table applies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Platform {
    MacOs,
    Linux,
}

impl Platform {
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// Whether a binding came from the shipped defaults or the user's file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Origin {
    Default,
    User,
}

/// One binding before compilation, shared by defaults and user entries.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BindingEntry {
    pub(crate) key: String,
    pub(crate) command: String,
    pub(crate) args: Vec<(String, CommandValue)>,
    pub(crate) when: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) origin: Origin,
    /// As written: `Some(true)` removes matching earlier bindings instead of
    /// binding, and any value is an error on `unbind`.
    pub(crate) disabled: Option<bool>,
    /// Whether the entry wrote `args`; a disabled entry without them matches
    /// bindings with any arguments.
    pub(crate) args_given: bool,
}

/// A binding that survived precedence resolution.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EffectiveBinding {
    pub(crate) key: String,
    pub(crate) command: CommandId,
    pub(crate) args: Vec<CommandArgument>,
    pub(crate) context: Option<&'static str>,
    pub(crate) when: Option<String>,
    pub(crate) predicate: Option<Rc<KeyBindingContextPredicate>>,
    pub(crate) description: String,
    pub(crate) origin: Origin,
}

impl EffectiveBinding {
    pub(crate) fn matches_context(&self, contexts: &[KeyContext]) -> bool {
        self.predicate
            .as_ref()
            .is_none_or(|predicate| predicate.depth_of(contexts).is_some())
    }

    pub(crate) fn matches_arguments(&self, args: &[CommandArgument]) -> bool {
        self.args == args
    }
}

/// Independently reserved strokes, compared by modifiers and key.
///
/// Multi-key bindings reserve only their first stroke. GPUI owns pending
/// sequences and matched final strokes. Conditional single-key bindings reserve
/// nothing, so their stroke reaches the terminal when the predicate is false.
#[derive(Clone, Debug, Default, PartialEq)]
// Keyed by key so a lookup can borrow the keystroke's key without copying it.
pub(crate) struct ReservedKeys(HashMap<String, HashSet<Modifiers>>);

impl ReservedKeys {
    /// Reports whether `keystroke` is independently reserved. `key_char` is
    /// ignored so `alt-r` stays reserved when macOS reports `®`.
    pub(crate) fn is_reserved(&self, keystroke: &Keystroke) -> bool {
        self.0
            .get(keystroke.key.as_str())
            .is_some_and(|modifiers| modifiers.contains(&keystroke.modifiers))
    }

    /// A later chord stroke is unbound when typed alone. Reserve only the
    /// starting stroke, except for conditional single-key bindings: reserving
    /// `ctrl-c` with `when = "selection"` would swallow shell interrupts.
    fn claim(&mut self, keystrokes: &[Keystroke], conditional: bool) {
        if (!conditional || keystrokes.len() > 1)
            && let Some(keystroke) = keystrokes.first()
        {
            self.0
                .entry(keystroke.key.clone())
                .or_default()
                .insert(keystroke.modifiers);
        }
    }
}

/// Result of compiling defaults and user entries.
pub(crate) struct CompiledKeymap {
    pub(crate) bindings: Vec<KeyBinding>,
    /// Retained so later UI can list bindings without re-parsing config.
    pub(crate) effective: Vec<EffectiveBinding>,
    pub(crate) reserved: ReservedKeys,
    /// Non-fatal precedence diagnostics in entry order.
    pub(crate) conflicts: Vec<String>,
}

/// The read-only portions of the currently installed keymap.
#[derive(Clone)]
pub(crate) struct InstalledKeymap {
    pub(crate) reserved: Arc<ReservedKeys>,
    pub(crate) effective: Arc<[EffectiveBinding]>,
}

impl InstalledKeymap {
    fn new(reserved: ReservedKeys, effective: Vec<EffectiveBinding>) -> Self {
        Self {
            reserved: Arc::new(reserved),
            effective: effective.into(),
        }
    }

    pub(crate) fn shortcuts(
        &self,
        command: CommandId,
        contexts: &[KeyContext],
        args: Option<&[CommandArgument]>,
    ) -> Vec<&EffectiveBinding> {
        let mut matches: Vec<_> = self
            .effective
            .iter()
            .filter(|binding| {
                binding.command == command
                    && binding.matches_context(contexts)
                    && args.is_none_or(|args| {
                        binding.args.is_empty()
                            || binding.matches_arguments(args)
                    })
            })
            .collect();
        if let Some(args) = args {
            matches.sort_by_key(|binding| !binding.matches_arguments(args));
        }
        matches
    }
}

impl CompiledKeymap {
    pub(crate) fn install_parts(self) -> (Vec<KeyBinding>, InstalledKeymap) {
        let installed = InstalledKeymap::new(self.reserved, self.effective);
        (self.bindings, installed)
    }
}

/// A fatal diagnostic naming the offending entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct KeymapError(String);

impl fmt::Display for KeymapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for KeymapError {}

/// Compiles the platform defaults followed by `user` entries in file order.
///
/// # Errors
/// Any entry with an invalid keystroke, predicate, command, or argument set
/// fails the whole compilation; the message carries the entry's 1-based
/// position and key.
pub(crate) fn compile(
    platform: Platform,
    user: &[KeybindingEntry],
) -> Result<CompiledKeymap, KeymapError> {
    let mut entries = defaults(platform);
    for (index, entry) in user.iter().enumerate() {
        let entry = BindingEntry::from_config(entry).map_err(|message| {
            KeymapError(keybinding_diagnostic(index + 1, &entry.key, message))
        })?;
        entries.push(entry);
    }
    compile_entries(&entries)
}

/// Compiles only the platform defaults; used when user entries fail.
pub(crate) fn compile_defaults(platform: Platform) -> CompiledKeymap {
    compile_entries(&defaults(platform))
        .unwrap_or_else(|error| panic!("default keymap must compile: {error}"))
}

impl BindingEntry {
    /// Converts a config entry, rejecting argument values that no catalog
    /// kind accepts so a float or table never reaches a text argument.
    fn from_config(entry: &KeybindingEntry) -> Result<Self, String> {
        let args = entry
            .args
            .iter()
            .flat_map(|table| table.iter())
            .map(|(name, value)| Ok((name.clone(), toml_value(name, value)?)))
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            key: entry.key.clone(),
            command: entry.command.clone(),
            args,
            when: entry.when.clone(),
            description: entry.description.clone(),
            origin: Origin::User,
            disabled: entry.disabled,
            args_given: entry.args.is_some(),
        })
    }
}

/// Converts a scalar TOML value; the catalog checks the kind afterwards.
fn toml_value(name: &str, value: &toml::Value) -> Result<CommandValue, String> {
    match value {
        toml::Value::Integer(integer) => Ok(CommandValue::Integer(*integer)),
        toml::Value::Boolean(boolean) => Ok(CommandValue::Bool(*boolean)),
        toml::Value::String(text) => Ok(CommandValue::Text(text.clone())),
        other => Err(format!(
            "argument `{name}` must be a string, integer, or boolean, not {}",
            other.type_str()
        )),
    }
}

/// Returns the catalog command a compiled binding dispatches.
#[cfg(test)]
pub(crate) fn bound_command(binding: &KeyBinding) -> Option<CommandId> {
    use crate::commands::{
        InvokeApp, InvokePalette, InvokeTerminal, InvokeWindow,
    };
    let action = binding.action().as_any();
    action
        .downcast_ref::<InvokeApp>()
        .map(|action| action.0.id)
        .or_else(|| {
            action
                .downcast_ref::<InvokeWindow>()
                .map(|action| action.0.id)
        })
        .or_else(|| {
            action
                .downcast_ref::<InvokeTerminal>()
                .map(|action| action.0.id)
        })
        .or_else(|| {
            action
                .downcast_ref::<InvokePalette>()
                .map(|action| action.0.id)
        })
}

struct Resolved {
    keystrokes: Vec<Keystroke>,
    predicate: Option<Rc<KeyBindingContextPredicate>>,
    action: CommandAction,
    effective: EffectiveBinding,
    position: usize,
}

impl Resolved {
    /// GPUI's macOS menus display the earliest binding for an action, while
    /// dispatch ties between different predicates go to the latest. Listing
    /// unconditional user bindings first puts them in menus; keeping
    /// conditional user bindings last lets them win ties against defaults.
    fn dispatch_rank(&self) -> u8 {
        match (self.effective.origin, self.predicate.is_some()) {
            (Origin::User, false) => 0,
            (Origin::Default, _) => 1,
            (Origin::User, true) => 2,
        }
    }
}

/// What became of one binding while later entries resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BindingState {
    Active,
    /// Turned off by the `disabled = true` user entries at these indexes.
    Disabled(Vec<usize>),
    /// Replaced by the user entry at this index, which has the same keys and
    /// combined predicate.
    Replaced(usize),
    /// Removed by the `unbind` user entry at this index.
    Unbound(usize),
    /// A `disabled = true` user entry that turned nothing off.
    SwitchedOff,
}

/// A binding's fields, without the compiled predicate, so a listing can
/// cross threads.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Bound {
    pub(crate) key: String,
    pub(crate) command: CommandId,
    pub(crate) args: Vec<CommandArgument>,
    /// The command's implied context, if any.
    pub(crate) context: Option<&'static str>,
    pub(crate) when: Option<String>,
    pub(crate) origin: Origin,
}

impl From<EffectiveBinding> for Bound {
    fn from(binding: EffectiveBinding) -> Self {
        Self {
            key: binding.key,
            command: binding.command,
            args: binding.args,
            context: binding.context,
            when: binding.when,
            origin: binding.origin,
        }
    }
}

/// One default or user binding with its outcome, for listing.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ListedBinding {
    pub(crate) binding: Bound,
    /// The user entry's 0-based index in `[[keybinding]]`.
    pub(crate) entry: Option<usize>,
    pub(crate) state: BindingState,
}

/// Every binding the defaults and the user's entries name, in resolution
/// order, with what became of each.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Inventory {
    /// Defaults and user bindings. A `disabled = true` entry that turned a
    /// binding off is listed only through that binding's state.
    pub(crate) bindings: Vec<ListedBinding>,
    /// `unbind` entries: their 0-based indexes and keys.
    pub(crate) unbinds: Vec<(usize, String)>,
}

/// Resolves defaults and user entries into the same inventory `compile`
/// uses.
///
/// # Errors
/// As [`compile`].
pub(crate) fn inventory(
    platform: Platform,
    user: &[KeybindingEntry],
) -> Result<Inventory, KeymapError> {
    let mut entries = defaults(platform);
    for (index, entry) in user.iter().enumerate() {
        let entry = BindingEntry::from_config(entry).map_err(|message| {
            KeymapError(keybinding_diagnostic(index + 1, &entry.key, message))
        })?;
        entries.push(entry);
    }
    let resolution = resolve_entries(&entries)?;
    Ok(Inventory {
        bindings: resolution
            .items
            .into_iter()
            .map(|item| ListedBinding {
                binding: item.resolved.effective.into(),
                entry: item.entry,
                state: item.state,
            })
            .collect(),
        unbinds: resolution.unbinds,
    })
}

struct Item {
    resolved: Resolved,
    entry: Option<usize>,
    state: BindingState,
}

struct Resolution {
    items: Vec<Item>,
    unbinds: Vec<(usize, String)>,
    conflicts: Vec<String>,
}

#[expect(
    clippy::too_many_lines,
    reason = "resolution validates and preserves one ordered binding pipeline"
)]
fn resolve_entries(
    entries: &[BindingEntry],
) -> Result<Resolution, KeymapError> {
    let mut items: Vec<Item> = Vec::new();
    let mut unbinds = Vec::new();
    let mut conflicts = Vec::new();
    let mut user_position = 0;
    for entry in entries {
        if entry.origin == Origin::User {
            user_position += 1;
        }
        let position = user_position;
        let index = (entry.origin == Origin::User).then(|| position - 1);
        let fail = |message: String| {
            KeymapError(match entry.origin {
                Origin::User => {
                    keybinding_diagnostic(position, &entry.key, message)
                }
                Origin::Default => {
                    format!("default keybinding ({:?}): {message}", entry.key)
                }
            })
        };
        let keystrokes = parse_keystrokes(&entry.key).map_err(fail)?;
        if entry.command == UNBIND {
            if !entry.args.is_empty() {
                return Err(fail("unbind takes no args".to_owned()));
            }
            if entry.disabled.is_some() {
                return Err(fail(
                    "unbind already removes bindings and takes no `disabled`"
                        .to_owned(),
                ));
            }
            if entry.when.is_some() {
                return Err(fail(
                    "unbind removes every binding for the key and takes no \
                     `when`"
                        .to_owned(),
                ));
            }
            for item in &mut items {
                if item.state == BindingState::Active
                    && same_keystrokes(&item.resolved.keystrokes, &keystrokes)
                {
                    item.state = BindingState::Unbound(position - 1);
                }
            }
            unbinds.push((position - 1, entry.key.clone()));
            continue;
        }
        let (spec, action, mut effective) =
            resolve_command(entry).map_err(fail)?;
        let predicate_source = match (spec.context, entry.when.as_deref()) {
            (Some(context), Some(when)) => {
                Some(format!("({context}) && ({when})"))
            }
            (Some(context), None) => Some(context.to_owned()),
            (None, Some(when)) => Some(when.to_owned()),
            (None, None) => None,
        };
        let predicate = predicate_source
            .as_deref()
            .map(|source| {
                KeyBindingContextPredicate::parse(source)
                    .map(Rc::new)
                    .map_err(|error| fail(format!("invalid when: {error}")))
            })
            .transpose()?;
        effective.predicate.clone_from(&predicate);
        let resolved = Resolved {
            keystrokes,
            predicate,
            action,
            effective,
            position,
        };
        if entry.disabled == Some(true) {
            // Binds nothing: turns off earlier bindings with the same keys,
            // command, and combined predicate, and the same arguments when
            // it gives any. Their reservations and menu accelerators go
            // with them.
            let mut matched = false;
            for item in &mut items {
                let existing = &item.resolved;
                if matches!(
                    item.state,
                    BindingState::Active | BindingState::Disabled(_)
                ) && same_keystrokes(
                    &existing.keystrokes,
                    &resolved.keystrokes,
                ) && existing.effective.command == resolved.effective.command
                    && existing.predicate == resolved.predicate
                    && (!entry.args_given
                        || same_arguments(
                            &existing.effective.args,
                            &resolved.effective.args,
                        ))
                {
                    match &mut item.state {
                        BindingState::Disabled(by) => by.push(position - 1),
                        state => {
                            *state = BindingState::Disabled(vec![position - 1]);
                        }
                    }
                    matched = true;
                }
            }
            if !matched {
                items.push(Item {
                    resolved,
                    entry: index,
                    state: BindingState::SwitchedOff,
                });
            }
            continue;
        }
        if let Some(earlier) = items.iter_mut().find(|item| {
            item.state == BindingState::Active
                && same_keystrokes(
                    &item.resolved.keystrokes,
                    &resolved.keystrokes,
                )
                && item.resolved.predicate == resolved.predicate
        }) {
            // Defaults come before every user entry, so a default that
            // replaces another is a catalog mistake with no user entry to
            // name: report it, and leave the earlier default as it was
            // rather than credit the clash to a user entry.
            if entry.origin == Origin::Default {
                conflicts.push(format!(
                    "default keybinding ({:?}): replaces an earlier default \
                     with the same key and when",
                    entry.key
                ));
            } else {
                earlier.state = BindingState::Replaced(position - 1);
            }
            if entry.origin == Origin::User
                && earlier.resolved.effective.origin == Origin::User
            {
                conflicts.push(keybinding_diagnostic(
                    position,
                    &entry.key,
                    format!(
                        "replaces keybinding {} with the same key and when",
                        earlier.resolved.position
                    ),
                ));
            }
        }
        items.push(Item {
            resolved,
            entry: index,
            state: BindingState::Active,
        });
    }
    Ok(Resolution {
        items,
        unbinds,
        conflicts,
    })
}

fn compile_entries(
    entries: &[BindingEntry],
) -> Result<CompiledKeymap, KeymapError> {
    let Resolution {
        items, conflicts, ..
    } = resolve_entries(entries)?;
    let mut resolved: Vec<Resolved> = items
        .into_iter()
        .filter(|item| item.state == BindingState::Active)
        .map(|item| item.resolved)
        .collect();
    let mut reserved = ReservedKeys::default();
    let mut effective = Vec::with_capacity(resolved.len());
    for binding in &resolved {
        reserved.claim(&binding.keystrokes, binding.predicate.is_some());
        effective.push(binding.effective.clone());
    }
    // `effective` keeps resolution order for display.
    resolved.sort_by_key(Resolved::dispatch_rank);
    let mut bindings = Vec::with_capacity(resolved.len());
    for binding in resolved {
        let key_binding = KeyBinding::load(
            &binding.effective.key,
            binding.action.into_boxed(),
            binding.predicate,
            false,
            None,
            &DummyKeyboardMapper,
        )
        .map_err(|error| {
            KeymapError(format!(
                "keybinding ({:?}): {error}",
                binding.effective.key
            ))
        })?;
        bindings.push(key_binding);
    }
    Ok(CompiledKeymap {
        bindings,
        effective,
        reserved,
        conflicts,
    })
}

fn parse_keystrokes(key: &str) -> Result<Vec<Keystroke>, String> {
    let keystrokes = key
        .split_whitespace()
        .map(|source| {
            // GPUI parses `cmd+<` as one unmodified key literally named
            // `cmd+<`, so a plus-joined chord binds nothing and unbinds
            // nothing without any error. A trailing `+` is the plus key.
            if source[..source.len() - 1].contains('+') {
                return Err(format!(
                    "invalid key {source:?}: join modifiers with `-`, not `+`"
                ));
            }
            Keystroke::parse(source)
                .map_err(|error| format!("invalid key: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if keystrokes.is_empty() {
        return Err("key must not be empty".to_owned());
    }
    Ok(keystrokes)
}

/// Argument lists with the same names and values, in any order.
fn same_arguments(left: &[CommandArgument], right: &[CommandArgument]) -> bool {
    left.len() == right.len()
        && left.iter().all(|argument| right.contains(argument))
}

fn same_keystrokes(left: &[Keystroke], right: &[Keystroke]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.modifiers == right.modifiers && left.key == right.key
        })
}

fn resolve_command(
    entry: &BindingEntry,
) -> Result<(&'static CommandSpec, CommandAction, EffectiveBinding), String> {
    let spec = lookup(&entry.command)
        .ok_or_else(|| format!("unknown command `{}`", entry.command))?;
    let mut args = Vec::with_capacity(entry.args.len());
    for (name, value) in &entry.args {
        if let Some(argument) = spec.argument(name)
            && matches!(
                argument.kind,
                ArgumentKind::Tab
                    | ArgumentKind::Workspace
                    | ArgumentKind::Session
                    | ArgumentKind::Pane
            )
        {
            return Err(format!(
                "argument `{name}` is a runtime identity resolved from the \
                 window and cannot be set in config"
            ));
        }
        args.push(CommandArgument::new(name.clone(), value.clone()));
    }
    let invocation = CommandInvocation::new(spec.id, args);
    validate_supplied(&invocation).map_err(|error| error.to_string())?;
    let description = entry
        .description
        .clone()
        .filter(|description| !description.trim().is_empty())
        .unwrap_or_else(|| describe(spec.title, &invocation.args));
    let action = CommandAction::new(invocation.clone())
        .map_err(|error| error.to_string())?;
    Ok((
        spec,
        action,
        EffectiveBinding {
            key: entry.key.clone(),
            command: spec.id,
            args: invocation.args,
            context: spec.context,
            when: entry.when.clone(),
            predicate: None,
            description,
            origin: entry.origin,
        },
    ))
}

/// Builds `Title (name: value, ...)` for bindings without a description.
fn describe(title: &str, args: &[CommandArgument]) -> String {
    if args.is_empty() {
        return title.to_owned();
    }
    let rendered = args
        .iter()
        .map(|argument| {
            let value = match &argument.value {
                CommandValue::Bool(value) => value.to_string(),
                CommandValue::Integer(value) => value.to_string(),
                CommandValue::Text(value) => value.clone(),
                CommandValue::Tab(_)
                | CommandValue::Workspace(_)
                | CommandValue::Session(_)
                | CommandValue::Pane(_) => "<id>".to_owned(),
            };
            format!("{}: {value}", argument.name)
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{title} ({rendered})")
}

fn default(key: &str, command: CommandId, description: &str) -> BindingEntry {
    default_with(key, command, &[], description)
}

/// A Settings binding that matches only while a slider, whose context sets
/// `control = slider`, has focus.
fn on_slider(key: &str, command: CommandId, description: &str) -> BindingEntry {
    BindingEntry {
        when: Some("control == slider".to_owned()),
        ..default(key, command, description)
    }
}

/// The key context a dialog's button carries: the dialog's own
/// `confirming`, with `control = button` on the same node, so a binding
/// under both matches the button alone and never a text field in the
/// dialog.
pub(crate) const DIALOG_BUTTON_CONTEXT: &str = "confirming control=button";

/// A dialog binding that matches only while a dialog button, whose context
/// is [`DIALOG_BUTTON_CONTEXT`], has focus.
fn on_button(key: &str, command: CommandId, description: &str) -> BindingEntry {
    BindingEntry {
        when: Some("control == button".to_owned()),
        ..default(key, command, description)
    }
}

/// A movement command bound with `select = true`, so it extends the
/// selection instead of moving the cursor.
fn selecting(key: &str, command: CommandId, description: &str) -> BindingEntry {
    default_with(
        key,
        command,
        &[("select", CommandValue::Bool(true))],
        description,
    )
}

fn default_with(
    key: &str,
    command: CommandId,
    args: &[(&str, CommandValue)],
    description: &str,
) -> BindingEntry {
    BindingEntry {
        key: key.to_owned(),
        command: command.as_str().to_owned(),
        args: args
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect(),
        when: None,
        description: Some(description.to_owned()),
        origin: Origin::Default,
        disabled: None,
        args_given: !args.is_empty(),
    }
}

/// Split and pane focus bindings, following Ghostty's and iTerm2's.
fn pane_defaults(platform: Platform) -> Vec<BindingEntry> {
    let (split_right, split_down, next, previous, focus) = match platform {
        Platform::MacOs => {
            ("cmd-d", "cmd-shift-d", "cmd-]", "cmd-[", "cmd-alt")
        }
        Platform::Linux => (
            "ctrl-shift-o",
            "ctrl-shift-e",
            "ctrl-super-]",
            "ctrl-super-[",
            "ctrl-alt",
        ),
    };
    let (resize, zoom) = match platform {
        Platform::MacOs => ("cmd-ctrl", "cmd-shift-enter"),
        Platform::Linux => ("ctrl-super-shift", "ctrl-shift-enter"),
    };
    // GPUI folds Shift+bracket into a brace, so the Linux cycle keys add
    // Alt to the `ctrl-super` focus keys instead.
    let (next_layout, previous_layout, balance) = match platform {
        Platform::MacOs => ("cmd-ctrl-]", "cmd-ctrl-[", "cmd-ctrl-="),
        Platform::Linux => {
            ("ctrl-super-alt-]", "ctrl-super-alt-[", "ctrl-super-=")
        }
    };
    let mut bindings = vec![
        default(
            &format!("{resize}-left"),
            ids::RESIZE_PANE_LEFT,
            "Resize the pane left",
        ),
        default(
            &format!("{resize}-right"),
            ids::RESIZE_PANE_RIGHT,
            "Resize the pane right",
        ),
        default(
            &format!("{resize}-up"),
            ids::RESIZE_PANE_UP,
            "Resize the pane up",
        ),
        default(
            &format!("{resize}-down"),
            ids::RESIZE_PANE_DOWN,
            "Resize the pane down",
        ),
        default(zoom, ids::TOGGLE_PANE_ZOOM, "Zoom the pane"),
        default(
            next_layout,
            ids::NEXT_PANE_LAYOUT,
            "Arrange the panes in the next layout",
        ),
        default(
            previous_layout,
            ids::PREVIOUS_PANE_LAYOUT,
            "Arrange the panes in the previous layout",
        ),
        default(balance, ids::BALANCE_PANES, "Balance the panes"),
    ];
    bindings.extend(vec![
        default(split_right, ids::SPLIT_RIGHT, "Split the pane to the right"),
        default(split_down, ids::SPLIT_DOWN, "Split the pane downward"),
        default(next, ids::FOCUS_NEXT_PANE, "Focus the next pane"),
        default(
            previous,
            ids::FOCUS_PREVIOUS_PANE,
            "Focus the previous pane",
        ),
        default(
            &format!("{focus}-left"),
            ids::FOCUS_PANE_LEFT,
            "Focus the pane to the left",
        ),
        default(
            &format!("{focus}-right"),
            ids::FOCUS_PANE_RIGHT,
            "Focus the pane to the right",
        ),
        default(
            &format!("{focus}-up"),
            ids::FOCUS_PANE_UP,
            "Focus the pane above",
        ),
        default(
            &format!("{focus}-down"),
            ids::FOCUS_PANE_DOWN,
            "Focus the pane below",
        ),
    ]);
    bindings
}

/// The shipped per-platform bindings, in precedence order.
pub(crate) fn defaults(platform: Platform) -> Vec<BindingEntry> {
    let mut entries = vec![
        default("shift-pageup", ids::SCROLL_PAGE_UP, "Scroll up one page"),
        default(
            "shift-pagedown",
            ids::SCROLL_PAGE_DOWN,
            "Scroll down one page",
        ),
        default("shift-end", ids::SCROLL_TO_BOTTOM, "Scroll to live output"),
    ];
    match platform {
        Platform::MacOs => entries.extend([
            default(
                "cmd-shift-p",
                ids::OPEN_COMMAND_PALETTE,
                "Open the command palette",
            ),
            default("cmd-enter", ids::TOGGLE_FULLSCREEN, "Toggle fullscreen"),
            default("f11", ids::TOGGLE_FULLSCREEN, "Toggle fullscreen"),
            default("cmd-c", ids::COPY, "Copy the selection"),
            default("cmd-v", ids::PASTE, "Paste from the clipboard"),
            default("cmd-a", ids::SELECT_ALL, "Select the whole terminal"),
            default("cmd-k", ids::CLEAR_SCROLLBACK, "Clear the scrollback"),
            default("cmd-,", ids::OPEN_SETTINGS, "Open Settings"),
            // GPUI folds Shift+comma into '<' and clears Shift on both
            // backends, so the binding names the resulting symbol.
            default("cmd-<", ids::RELOAD_CONFIG, "Reload the configuration"),
            default("cmd-q", ids::QUIT, "Quit Huterm"),
            default("cmd-m", ids::MINIMIZE, "Minimize the window"),
            default("cmd-h", ids::HIDE, "Hide Huterm"),
            default("cmd-alt-h", ids::HIDE_OTHERS, "Hide other applications"),
            default("cmd-z", ids::UNDO, "Undo the last layout change"),
            default("cmd-shift-z", ids::REDO, "Redo the undone layout change"),
        ]),
        Platform::Linux => entries.extend([
            default(
                "ctrl-shift-p",
                ids::OPEN_COMMAND_PALETTE,
                "Open the command palette",
            ),
            default("f11", ids::TOGGLE_FULLSCREEN, "Toggle fullscreen"),
            default("ctrl-shift-c", ids::COPY, "Copy the selection"),
            default("ctrl-shift-v", ids::PASTE, "Paste from the clipboard"),
            default(
                "ctrl-shift-a",
                ids::SELECT_ALL,
                "Select the whole terminal",
            ),
            default("ctrl-,", ids::OPEN_SETTINGS, "Open Settings"),
            default("ctrl-<", ids::RELOAD_CONFIG, "Reload the configuration"),
            // Plain Ctrl+Z is job control and stays with the shell.
            default("ctrl-shift-z", ids::UNDO, "Undo the last layout change"),
            default("ctrl-shift-y", ids::REDO, "Redo the undone layout change"),
        ]),
    }
    let (modifier, close_window) = match platform {
        Platform::MacOs => ("cmd", "cmd-shift-w"),
        Platform::Linux => ("ctrl-shift", "ctrl-shift-q"),
    };
    entries.extend([
        default(
            &format!("{modifier}-n"),
            ids::NEW_WINDOW,
            "Open a new window",
        ),
        default(&format!("{modifier}-t"), ids::NEW_TAB, "Open a new tab"),
        default(
            &format!("{modifier}-w"),
            ids::CLOSE_PANE,
            "Close the pane, or the tab when it has one pane",
        ),
        default(close_window, ids::CLOSE_WINDOW, "Close the window"),
        default("ctrl-tab", ids::NEXT_TAB, "Activate the next tab"),
        default(
            "ctrl-shift-tab",
            ids::PREVIOUS_TAB,
            "Activate the previous tab",
        ),
    ]);
    entries.extend(pane_defaults(platform));
    let modifier = match platform {
        Platform::MacOs => "cmd",
        Platform::Linux => "alt",
    };
    entries.extend((1..=9).map(|index| {
        default_with(
            &format!("{modifier}-{index}"),
            ids::SELECT_TAB,
            &[("index", CommandValue::Integer(index))],
            &if index == 9 {
                "Select the last tab".to_owned()
            } else {
                format!("Select tab {index}")
            },
        )
    }));
    entries.extend(dialog_defaults());
    entries.extend(menu_defaults());
    entries.extend(notice_defaults());
    entries.extend(settings_defaults(platform));
    entries.extend(palette_defaults(platform));
    entries
}

/// Dialog bindings, identical on both platforms. Their commands carry the
/// `confirming` context, so they reserve nothing from terminals. Space
/// presses only a focused button, so it still types in a dialog's field.
fn dialog_defaults() -> Vec<BindingEntry> {
    vec![
        default("enter", ids::DIALOG_CONFIRM, "Confirm"),
        on_button("space", ids::DIALOG_CONFIRM, "Press Button"),
        default("escape", ids::DIALOG_CANCEL, "Cancel"),
        default("tab", ids::DIALOG_FOCUS_NEXT, "Next Button"),
        default("right", ids::DIALOG_FOCUS_NEXT, "Next Button"),
        default("shift-tab", ids::DIALOG_FOCUS_PREVIOUS, "Previous Button"),
        default("left", ids::DIALOG_FOCUS_PREVIOUS, "Previous Button"),
    ]
}

/// Menu bindings, identical on both platforms. Their commands carry the
/// `menu` context, so they match only while a menu has focus and reserve
/// nothing from terminals.
fn menu_defaults() -> Vec<BindingEntry> {
    vec![
        default("down", ids::MENU_SELECT_NEXT, "Next Item"),
        default("up", ids::MENU_SELECT_PREVIOUS, "Previous Item"),
        default("home", ids::MENU_SELECT_FIRST, "First Item"),
        default("end", ids::MENU_SELECT_LAST, "Last Item"),
        default("right", ids::MENU_SELECT_RIGHT, "Next Button"),
        default("left", ids::MENU_SELECT_LEFT, "Previous Button"),
        default("enter", ids::MENU_CONFIRM, "Run Item"),
        default("space", ids::MENU_CONFIRM, "Run Item"),
        default("escape", ids::MENU_CLOSE, "Close"),
        default("tab", ids::MENU_CLOSE, "Close"),
        default("shift-tab", ids::MENU_CLOSE, "Close"),
    ]
}

/// Toast bindings, identical on both platforms. Their commands carry the
/// `notices` context, so they match only while a toast has focus.
fn notice_defaults() -> Vec<BindingEntry> {
    vec![
        default("down", ids::NOTICE_NEXT, "Next Notice"),
        default("up", ids::NOTICE_PREVIOUS, "Previous Notice"),
        default("enter", ids::NOTICE_RUN_ACTION, "Run Action"),
        default("escape", ids::NOTICE_DISMISS, "Dismiss"),
        default("delete", ids::NOTICE_DISMISS, "Dismiss"),
    ]
}

/// Settings window bindings. Search and focus movement carry the `Settings`
/// context; toggling, stepping, and reset carry `SettingsControl`, which
/// only switches, selects, sliders, and section rows set, so Space and the
/// arrows still reach a focused text field, whose `Palette` context handles
/// them. Page Up, Page Down, Home, and End match only on a slider: a number
/// field's `Palette` context pages it and keeps Home and End for its text,
/// and on any other control the keys stay unbound. A roving group's items
/// nest a `SettingsGroup` context inside their `SettingsControl` one, where
/// the arrows move between the items instead of stepping: Down and Right
/// forward, Up and Left back. Those bindings match deeper than any
/// `SettingsControl` one, so rebinding the stepping commands never reaches
/// the groups, nor the reverse. The window also reads the close and
/// tab-switching shortcuts, so they need no bindings of their own.
fn settings_defaults(platform: Platform) -> Vec<BindingEntry> {
    let mut entries = vec![
        default("tab", ids::SETTINGS_FOCUS_NEXT, "Next Control"),
        default(
            "shift-tab",
            ids::SETTINGS_FOCUS_PREVIOUS,
            "Previous Control",
        ),
        default("space", ids::SETTINGS_TOGGLE, "Toggle"),
        default("enter", ids::SETTINGS_TOGGLE, "Toggle"),
        default("up", ids::SETTINGS_INCREASE, "Increase"),
        default("right", ids::SETTINGS_INCREASE, "Increase"),
        default("down", ids::SETTINGS_DECREASE, "Decrease"),
        default("left", ids::SETTINGS_DECREASE, "Decrease"),
        on_slider("pageup", ids::SETTINGS_PAGE_UP, "Page Up"),
        on_slider("pagedown", ids::SETTINGS_PAGE_DOWN, "Page Down"),
        on_slider("home", ids::SETTINGS_FIRST, "Minimum"),
        on_slider("end", ids::SETTINGS_LAST, "Maximum"),
        default("down", ids::SETTINGS_NEXT, "Next Item"),
        default("right", ids::SETTINGS_NEXT, "Next Item"),
        default("up", ids::SETTINGS_PREVIOUS, "Previous Item"),
        default("left", ids::SETTINGS_PREVIOUS, "Previous Item"),
    ];
    entries.extend(match platform {
        Platform::MacOs => [
            default("cmd-f", ids::SETTINGS_SEARCH, "Search"),
            default("cmd-backspace", ids::SETTINGS_RESET, "Reset"),
        ],
        Platform::Linux => [
            default("ctrl-f", ids::SETTINGS_SEARCH, "Search"),
            default("ctrl-backspace", ids::SETTINGS_RESET, "Reset"),
        ],
    });
    entries
}

fn palette_defaults(platform: Platform) -> Vec<BindingEntry> {
    let mut entries = vec![
        default("down", ids::PALETTE_SELECT_NEXT, "Next Item"),
        default("up", ids::PALETTE_SELECT_PREVIOUS, "Previous Item"),
        default("pagedown", ids::PALETTE_PAGE_DOWN, "Page Down"),
        default("pageup", ids::PALETTE_PAGE_UP, "Page Up"),
        default("enter", ids::PALETTE_CONFIRM, "Confirm"),
        default("escape", ids::PALETTE_BACK, "Back"),
        default("tab", ids::PALETTE_EXPAND, "Edit Arguments"),
        default("shift-tab", ids::PALETTE_PREVIOUS_SLOT, "Previous Argument"),
        default("ctrl-n", ids::PALETTE_SELECT_NEXT, "Next Item"),
        default("ctrl-p", ids::PALETTE_SELECT_PREVIOUS, "Previous Item"),
        default("backspace", ids::TEXT_DELETE_BACKWARD, "Delete Backward"),
        default("delete", ids::TEXT_DELETE_FORWARD, "Delete Forward"),
        default("left", ids::TEXT_MOVE_LEFT, "Move Left"),
        default("right", ids::TEXT_MOVE_RIGHT, "Move Right"),
        default("alt-left", ids::TEXT_MOVE_WORD_LEFT, "Move Word Left"),
        default("alt-right", ids::TEXT_MOVE_WORD_RIGHT, "Move Word Right"),
        default("alt-b", ids::TEXT_MOVE_WORD_LEFT, "Move Word Left"),
        default("alt-f", ids::TEXT_MOVE_WORD_RIGHT, "Move Word Right"),
        default("home", ids::TEXT_LINE_START, "Line Start"),
        default("end", ids::TEXT_LINE_END, "Line End"),
        selecting("shift-left", ids::TEXT_MOVE_LEFT, "Select Left"),
        selecting("shift-right", ids::TEXT_MOVE_RIGHT, "Select Right"),
        selecting(
            "shift-alt-left",
            ids::TEXT_MOVE_WORD_LEFT,
            "Select Word Left",
        ),
        selecting(
            "shift-alt-right",
            ids::TEXT_MOVE_WORD_RIGHT,
            "Select Word Right",
        ),
        selecting("shift-alt-b", ids::TEXT_MOVE_WORD_LEFT, "Select Word Left"),
        selecting(
            "shift-alt-f",
            ids::TEXT_MOVE_WORD_RIGHT,
            "Select Word Right",
        ),
        selecting("shift-home", ids::TEXT_LINE_START, "Select to Line Start"),
        selecting("shift-end", ids::TEXT_LINE_END, "Select to Line End"),
        default(
            "alt-backspace",
            ids::TEXT_DELETE_WORD_BACKWARD,
            "Delete Word Backward",
        ),
        default(
            "alt-d",
            ids::TEXT_DELETE_WORD_FORWARD,
            "Delete Word Forward",
        ),
    ];
    match platform {
        Platform::MacOs => entries.extend([
            default("cmd-a", ids::TEXT_SELECT_ALL, "Select All"),
            default("cmd-x", ids::TEXT_CUT, "Cut"),
            default("cmd-c", ids::TEXT_COPY, "Copy"),
            default("cmd-v", ids::TEXT_PASTE, "Paste"),
            default("ctrl-a", ids::TEXT_LINE_START, "Line Start"),
            default("ctrl-e", ids::TEXT_LINE_END, "Line End"),
            default("ctrl-d", ids::TEXT_DELETE_FORWARD, "Delete Forward"),
            default("cmd-left", ids::TEXT_LINE_START, "Line Start"),
            default("cmd-right", ids::TEXT_LINE_END, "Line End"),
            selecting(
                "shift-ctrl-a",
                ids::TEXT_LINE_START,
                "Select to Line Start",
            ),
            selecting("shift-ctrl-e", ids::TEXT_LINE_END, "Select to Line End"),
            selecting(
                "shift-cmd-left",
                ids::TEXT_LINE_START,
                "Select to Line Start",
            ),
            selecting(
                "shift-cmd-right",
                ids::TEXT_LINE_END,
                "Select to Line End",
            ),
            default(
                "cmd-backspace",
                ids::TEXT_DELETE_LINE_START,
                "Delete to Line Start",
            ),
        ]),
        Platform::Linux => entries.extend([
            default("ctrl-a", ids::TEXT_SELECT_ALL, "Select All"),
            default("ctrl-x", ids::TEXT_CUT, "Cut"),
            default("ctrl-shift-x", ids::TEXT_CUT, "Cut"),
            default("ctrl-c", ids::TEXT_COPY, "Copy"),
            default("ctrl-shift-c", ids::TEXT_COPY, "Copy"),
            default("ctrl-v", ids::TEXT_PASTE, "Paste"),
            default("ctrl-shift-v", ids::TEXT_PASTE, "Paste"),
        ]),
    }
    entries
}

#[cfg(test)]
mod tests;
