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

#[expect(
    clippy::too_many_lines,
    reason = "resolution validates and preserves one ordered binding pipeline"
)]
fn compile_entries(
    entries: &[BindingEntry],
) -> Result<CompiledKeymap, KeymapError> {
    let mut resolved: Vec<Resolved> = Vec::new();
    let mut conflicts = Vec::new();
    let mut user_position = 0;
    for entry in entries {
        if entry.origin == Origin::User {
            user_position += 1;
        }
        let position = user_position;
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
        let keystrokes = parse_keystrokes(&entry.key).map_err(&fail)?;
        if entry.command == UNBIND {
            if !entry.args.is_empty() {
                return Err(fail("unbind takes no args".to_owned()));
            }
            if entry.when.is_some() {
                return Err(fail(
                    "unbind removes every binding for the key and takes no \
                     `when`"
                        .to_owned(),
                ));
            }
            resolved.retain(|existing| {
                !same_keystrokes(&existing.keystrokes, &keystrokes)
            });
            continue;
        }
        let (spec, action, mut effective) =
            resolve_command(entry).map_err(&fail)?;
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
        if let Some(index) = resolved.iter().position(|existing| {
            same_keystrokes(&existing.keystrokes, &keystrokes)
                && existing.predicate == predicate
        }) {
            let earlier = resolved.remove(index);
            if earlier.effective.origin == Origin::User {
                conflicts.push(keybinding_diagnostic(
                    position,
                    &entry.key,
                    format!(
                        "replaces keybinding {} with the same key and when",
                        earlier.position
                    ),
                ));
            }
        }
        resolved.push(Resolved {
            keystrokes,
            predicate,
            action,
            effective,
            position,
        });
    }
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
                | CommandValue::Session(_) => "<id>".to_owned(),
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
    }
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
            default("cmd-,", ids::OPEN_SETTINGS, "Open the configuration file"),
            // GPUI folds Shift+comma into '<' and clears Shift on both
            // backends, so the binding names the resulting symbol.
            default("cmd-<", ids::RELOAD_CONFIG, "Reload the configuration"),
            default("cmd-q", ids::QUIT, "Quit Huterm"),
            default("cmd-m", ids::MINIMIZE, "Minimize the window"),
            default("cmd-h", ids::HIDE, "Hide Huterm"),
            default("cmd-alt-h", ids::HIDE_OTHERS, "Hide other applications"),
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
            default(
                "ctrl-,",
                ids::OPEN_SETTINGS,
                "Open the configuration file",
            ),
            default("ctrl-<", ids::RELOAD_CONFIG, "Reload the configuration"),
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
        default(&format!("{modifier}-w"), ids::CLOSE_TAB, "Close the tab"),
        default(close_window, ids::CLOSE_WINDOW, "Close the window"),
        default("ctrl-tab", ids::NEXT_TAB, "Activate the next tab"),
        default(
            "ctrl-shift-tab",
            ids::PREVIOUS_TAB,
            "Activate the previous tab",
        ),
    ]);
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
    entries.extend(palette_defaults(platform));
    entries
}

/// Close confirmation bindings, identical on both platforms. Their commands
/// carry the `confirming` context, so they reserve nothing from terminals.
fn dialog_defaults() -> Vec<BindingEntry> {
    vec![
        default("enter", ids::DIALOG_CONFIRM, "Confirm"),
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
