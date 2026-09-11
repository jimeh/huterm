//! Keymap compilation: platform defaults plus user entries become GPUI
//! bindings, an effective-binding list, and the reserved-key set that keeps
//! bound chords away from terminal input.

use std::collections::HashSet;
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
pub(crate) struct ReservedKeys(HashSet<(Modifiers, String)>);

impl ReservedKeys {
    /// Reports whether `keystroke` is independently reserved. `key_char` is
    /// ignored so `alt-r` stays reserved when macOS reports `®`.
    pub(crate) fn is_reserved(&self, keystroke: &Keystroke) -> bool {
        self.0
            .contains(&(keystroke.modifiers, keystroke.key.clone()))
    }

    /// A later chord stroke is unbound when typed alone. Reserve only the
    /// starting stroke, except for conditional single-key bindings: reserving
    /// `ctrl-c` with `when = "selection"` would swallow shell interrupts.
    fn claim(&mut self, keystrokes: &[Keystroke], conditional: bool) {
        if (!conditional || keystrokes.len() > 1)
            && let Some(keystroke) = keystrokes.first()
        {
            self.0.insert((keystroke.modifiers, keystroke.key.clone()));
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
    entries.extend(palette_defaults(platform));
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
        default("home", ids::TEXT_LINE_START, "Line Start"),
        default("end", ids::TEXT_LINE_END, "Line End"),
        default("shift-left", ids::TEXT_SELECT_LEFT, "Select Left"),
        default("shift-right", ids::TEXT_SELECT_RIGHT, "Select Right"),
        default(
            "alt-backspace",
            ids::TEXT_DELETE_WORD_BACKWARD,
            "Delete Word Backward",
        ),
    ];
    match platform {
        Platform::MacOs => entries.extend([
            default("cmd-a", ids::TEXT_SELECT_ALL, "Select All"),
            default("cmd-c", ids::TEXT_COPY, "Copy"),
            default("cmd-v", ids::TEXT_PASTE, "Paste"),
            default("ctrl-a", ids::TEXT_LINE_START, "Line Start"),
            default("ctrl-e", ids::TEXT_LINE_END, "Line End"),
            default("cmd-left", ids::TEXT_LINE_START, "Line Start"),
            default("cmd-right", ids::TEXT_LINE_END, "Line End"),
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
mod tests {
    use super::*;
    use crate::commands::invoke;
    use gpui::{KeyContext, Keymap};

    fn entry(key: &str, command: &str) -> KeybindingEntry {
        KeybindingEntry {
            key: key.into(),
            command: command.into(),
            args: None,
            when: None,
            description: None,
        }
    }

    fn with_args(
        mut entry: KeybindingEntry,
        args: &[(&str, toml::Value)],
    ) -> KeybindingEntry {
        entry.args = Some(
            args.iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
        );
        entry
    }

    fn keystroke(source: &str) -> Keystroke {
        Keystroke::parse(source).unwrap()
    }

    fn command_of(binding: &KeyBinding) -> CommandId {
        bound_command(binding).expect("binding carries a catalog action")
    }

    #[test]
    fn defaults_compile_on_both_platforms_with_descriptions() {
        for platform in [Platform::MacOs, Platform::Linux] {
            let compiled = compile(platform, &[]).unwrap();
            let defaults = defaults(platform);
            assert_eq!(compiled.effective.len(), defaults.len());
            assert!(compiled.conflicts.is_empty());
            for (entry, effective) in defaults.iter().zip(&compiled.effective) {
                assert!(
                    entry
                        .description
                        .as_deref()
                        .is_some_and(|d| !d.trim().is_empty()),
                    "{}: default without description",
                    entry.key
                );
                assert_eq!(effective.origin, Origin::Default);
                assert_eq!(effective.key, entry.key);
            }
            assert!(compiled.reserved.is_reserved(&keystroke("shift-pageup")));
            assert!(!compiled.reserved.is_reserved(&keystroke("ctrl-c")));
            let palette = compiled
                .effective
                .iter()
                .find(|binding| binding.command == ids::OPEN_COMMAND_PALETTE)
                .expect("platform default opens the command palette");
            assert_eq!(
                palette.key,
                match platform {
                    Platform::MacOs => "cmd-shift-p",
                    Platform::Linux => "ctrl-shift-p",
                }
            );
            let palette_key = palette.key.clone();
            let keymap = Keymap::new(compiled.bindings);
            let contexts = [KeyContext::parse("Workspace").unwrap()];
            let (bindings, pending) = keymap
                .bindings_for_input(&[keystroke(&palette_key)], &contexts);
            assert!(!pending);
            assert_eq!(
                bindings.first().map(command_of),
                Some(ids::OPEN_COMMAND_PALETTE)
            );
        }
        let macos = compile(Platform::MacOs, &[]).unwrap().reserved;
        for chord in [
            "cmd-c",
            "cmd-v",
            "cmd-h",
            "cmd-alt-h",
            "cmd-enter",
            "cmd-<",
            "cmd-9",
            "cmd-shift-w",
            "ctrl-tab",
        ] {
            assert!(macos.is_reserved(&keystroke(chord)), "{chord}");
        }
        for chord in ["cmd-alt-c", "ctrl-shift-c", "alt-9"] {
            assert!(!macos.is_reserved(&keystroke(chord)), "{chord}");
        }
        let linux = compile(Platform::Linux, &[]).unwrap().reserved;
        for chord in [
            "ctrl-shift-c",
            "ctrl-shift-v",
            "ctrl-<",
            "alt-9",
            "ctrl-shift-q",
            "ctrl-shift-tab",
        ] {
            assert!(linux.is_reserved(&keystroke(chord)), "{chord}");
        }
        for chord in ["ctrl-shift-alt-c", "cmd-c", "cmd-9"] {
            assert!(!linux.is_reserved(&keystroke(chord)), "{chord}");
        }
        let mut with_function = keystroke("cmd-c");
        with_function.modifiers.function = true;
        assert!(!macos.is_reserved(&with_function));
    }

    #[test]
    fn descriptions_resolve_from_title_and_arguments() {
        let user = [with_args(
            entry("cmd-3", "select_tab"),
            &[("index", toml::Value::Integer(3))],
        )];
        let compiled = compile(Platform::MacOs, &user).unwrap();
        let effective = compiled.effective.last().unwrap();
        assert_eq!(effective.description, "Select Tab (index: 3)");
        assert_eq!(effective.origin, Origin::User);
        assert_eq!(
            compile(Platform::MacOs, &[entry("cmd-t", "new_tab")])
                .unwrap()
                .effective
                .last()
                .unwrap()
                .description,
            "New Tab"
        );
    }

    #[test]
    fn override_replaces_default_and_unbind_removes_all_variants() {
        let user = [
            entry("cmd-t", "new_window"),
            entry("cmd-c", "unbind"),
            KeybindingEntry {
                when: Some("Terminal".into()),
                ..entry("cmd-v", "copy")
            },
            entry("cmd-v", "unbind"),
        ];
        let compiled = compile(Platform::MacOs, &user).unwrap();
        assert!(compiled.conflicts.is_empty());
        let for_key = |key: &str| {
            compiled
                .effective
                .iter()
                .filter(|binding| binding.key == key)
                .collect::<Vec<_>>()
        };
        let new_tab_keys = for_key("cmd-t");
        assert_eq!(new_tab_keys.len(), 1);
        assert_eq!(new_tab_keys[0].command, ids::NEW_WINDOW);
        assert_eq!(new_tab_keys[0].origin, Origin::User);
        assert!(for_key("cmd-c").is_empty());
        assert!(for_key("cmd-v").is_empty());
        assert!(!compiled.reserved.is_reserved(&keystroke("cmd-c")));
        assert!(!compiled.reserved.is_reserved(&keystroke("cmd-v")));
        assert!(compiled.reserved.is_reserved(&keystroke("cmd-t")));
        let bound = compiled
            .bindings
            .iter()
            .filter(|binding| {
                binding.match_keystrokes(&[keystroke("cmd-t")]) == Some(false)
            })
            .map(command_of)
            .collect::<Vec<_>>();
        assert_eq!(bound, vec![ids::NEW_WINDOW]);
    }

    #[test]
    fn duplicate_user_entries_report_a_conflict_and_the_last_wins() {
        let user = [entry("alt-x", "new_tab"), entry("alt-x", "new_window")];
        let compiled = compile(Platform::Linux, &user).unwrap();
        assert_eq!(
            compiled.conflicts,
            vec![
                "keybinding 2 (\"alt-x\"): replaces keybinding 1 with the same key and when".to_owned()
            ]
        );
        let bound = compiled
            .effective
            .iter()
            .filter(|binding| binding.key == "alt-x")
            .map(|binding| binding.command)
            .collect::<Vec<_>>();
        assert_eq!(bound, vec![ids::NEW_WINDOW]);
    }

    #[test]
    fn unconditional_user_bindings_lead_menus_and_conditional_ones_win_ties() {
        let user = [
            KeybindingEntry {
                when: Some("Terminal".into()),
                ..entry("cmd-w", "toggle_fullscreen")
            },
            entry("cmd-r", "reload_config"),
        ];
        let compiled = compile(Platform::MacOs, &user).unwrap();
        let keymap = Keymap::new(compiled.bindings);
        // Menus take the first binding listed for the action.
        let reload = invoke(ids::RELOAD_CONFIG).into_boxed();
        let shown =
            keymap
                .bindings_for_action(reload.as_ref())
                .next()
                .map(|binding| {
                    binding
                        .keystrokes()
                        .iter()
                        .map(|keystroke| keystroke.inner().clone())
                        .collect::<Vec<_>>()
                });
        assert_eq!(shown, Some(vec![keystroke("cmd-r")]));
        // Equal-depth ties go to the later binding, so a conditional user
        // binding must follow the default for the same key.
        let stack = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        let (bindings, _) =
            keymap.bindings_for_input(&[keystroke("cmd-w")], &stack);
        assert_eq!(
            bindings.first().map(command_of),
            Some(ids::TOGGLE_FULLSCREEN)
        );
    }

    #[test]
    fn a_trailing_plus_is_the_plus_key() {
        let compiled =
            compile(Platform::MacOs, &[entry("cmd-+", "new_tab")]).unwrap();
        assert!(compiled.reserved.is_reserved(&keystroke("cmd-+")));
    }

    #[test]
    fn invalid_entries_fail_with_index_and_key() {
        let cases: Vec<(KeybindingEntry, &str)> = vec![
            (entry("cmd-x-y", "new_tab"), "invalid key"),
            (entry("cmd+<", "unbind"), "join modifiers with `-`, not `+`"),
            (
                KeybindingEntry {
                    when: Some("Terminal &&".into()),
                    ..entry("cmd-t", "new_tab")
                },
                "invalid when",
            ),
            (entry("cmd-t", "teleport"), "unknown command `teleport`"),
            (
                with_args(
                    entry("cmd-t", "rename_tab"),
                    &[
                        ("name", toml::Value::String("x".into())),
                        ("tab", toml::Value::Integer(1)),
                    ],
                ),
                "argument `tab` is a runtime identity",
            ),
            (
                with_args(
                    entry("cmd-3", "select_tab"),
                    &[("index", toml::Value::Integer(12))],
                ),
                "expected 1 to 9",
            ),
            (
                with_args(
                    entry("cmd-3", "select_tab"),
                    &[("index", toml::Value::String("3".into()))],
                ),
                "must be",
            ),
            (
                with_args(
                    entry("cmd-t", "rename_tab"),
                    &[("name", toml::Value::Float(1.5))],
                ),
                "argument `name` must be a string, integer, or boolean, not \
                 float",
            ),
            (
                with_args(
                    entry("cmd-t", "unbind"),
                    &[("index", toml::Value::Integer(1))],
                ),
                "unbind takes no args",
            ),
            (
                KeybindingEntry {
                    when: Some("Terminal".into()),
                    ..entry("cmd-t", "unbind")
                },
                "takes no `when`",
            ),
        ];
        for (user, expected) in cases {
            let key = user.key.clone();
            let entries = [entry("cmd-t", "new_tab"), user];
            let error = compile(Platform::MacOs, &entries)
                .err()
                .unwrap_or_else(|| panic!("{key}: expected {expected}"))
                .to_string();
            assert!(
                error.starts_with(&format!("keybinding 2 ({key:?}): ")),
                "{error}"
            );
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn reserved_keys_follow_bound_and_unbound_chords() {
        let user = [
            entry("ctrl-k ctrl-t", "new_tab"),
            entry("alt-r", "reload_config"),
            entry("shift-pageup", "unbind"),
        ];
        let compiled = compile(Platform::Linux, &user).unwrap();
        for chord in ["ctrl-k", "alt-r"] {
            assert!(
                compiled.reserved.is_reserved(&keystroke(chord)),
                "{chord}"
            );
        }
        assert!(!compiled.reserved.is_reserved(&keystroke("ctrl-t")));
        assert!(!compiled.reserved.is_reserved(&keystroke("shift-pageup")));
        let mut typed = keystroke("alt-r");
        typed.key_char = Some("®".into());
        assert!(compiled.reserved.is_reserved(&typed));
    }

    #[test]
    fn later_chord_strokes_are_unbound_without_their_prefix() {
        for chord in ["alt-k alt-r", "alt-k alt-r alt-j"] {
            let compiled =
                compile(Platform::MacOs, &[entry(chord, "reload_config")])
                    .unwrap();
            let matcher = Keymap::new(compiled.bindings);
            let context = [
                KeyContext::parse("Workspace").unwrap(),
                KeyContext::parse("Terminal").unwrap(),
            ];
            let strokes =
                chord.split_whitespace().map(keystroke).collect::<Vec<_>>();
            let (bindings, pending) =
                matcher.bindings_for_input(&strokes[..1], &context);
            assert!(bindings.is_empty());
            assert!(pending);
            assert!(compiled.reserved.is_reserved(&strokes[0]));
            for stroke in &strokes[1..] {
                let (bindings, pending) = matcher
                    .bindings_for_input(std::slice::from_ref(stroke), &context);
                assert!(bindings.is_empty());
                assert!(!pending);
                assert!(
                    !compiled.reserved.is_reserved(stroke),
                    "standalone {} from {chord}",
                    stroke.key
                );
            }
            let (bindings, pending) =
                matcher.bindings_for_input(&strokes, &context);
            assert!(!pending);
            assert_eq!(
                bindings.iter().map(command_of).collect::<Vec<_>>(),
                vec![ids::RELOAD_CONFIG]
            );
        }
    }

    #[test]
    fn conditional_bindings_reserve_only_multi_key_starting_strokes() {
        let user = [
            KeybindingEntry {
                when: Some("selection".into()),
                ..entry("ctrl-c", "copy")
            },
            KeybindingEntry {
                when: Some("Terminal".into()),
                ..entry("ctrl-k ctrl-x", "new_tab")
            },
        ];
        let compiled = compile(Platform::Linux, &user).unwrap();
        // Ctrl-C must still interrupt the shell when nothing is selected.
        assert!(!compiled.reserved.is_reserved(&keystroke("ctrl-c")));
        assert!(compiled.reserved.is_reserved(&keystroke("ctrl-k")));
        assert!(!compiled.reserved.is_reserved(&keystroke("ctrl-x")));
        // The bindings themselves still exist for GPUI to match.
        let bound = compiled
            .bindings
            .iter()
            .filter_map(bound_command)
            .collect::<Vec<_>>();
        assert!(bound.contains(&ids::COPY) && bound.contains(&ids::NEW_TAB));
    }

    #[test]
    fn compiled_bindings_match_through_the_real_gpui_keymap() {
        for (platform, reload) in
            [(Platform::MacOs, "cmd-<"), (Platform::Linux, "ctrl-<")]
        {
            let user = [
                entry("alt-r", "new_window"),
                KeybindingEntry {
                    when: Some("Terminal && !confirming".into()),
                    ..entry("alt-p", "paste")
                },
            ];
            let compiled = compile(platform, &user).unwrap();
            let keymap = Keymap::new(compiled.bindings);
            let terminal = [
                KeyContext::parse("Workspace").unwrap(),
                KeyContext::parse("Terminal").unwrap(),
            ];
            let confirming = [
                KeyContext::parse("Workspace confirming").unwrap(),
                KeyContext::parse("Terminal").unwrap(),
            ];
            let matched = |event: Keystroke, contexts: &[KeyContext]| {
                let (bindings, pending) =
                    keymap.bindings_for_input(&[event], contexts);
                assert!(!pending);
                bindings.iter().map(command_of).collect::<Vec<_>>()
            };
            // Both native backends fold Shift+comma into '<' without Shift.
            assert_eq!(
                matched(keystroke(reload), &terminal),
                vec![ids::RELOAD_CONFIG]
            );
            let mut option_r = keystroke("alt-r");
            option_r.key_char = Some("®".into());
            assert_eq!(matched(option_r, &terminal), vec![ids::NEW_WINDOW]);
            assert_eq!(
                matched(keystroke("alt-p"), &terminal),
                vec![ids::PASTE]
            );
            assert!(matched(keystroke("alt-p"), &confirming).is_empty());
            let select_last = keystroke(if platform == Platform::MacOs {
                "cmd-9"
            } else {
                "alt-9"
            });
            assert_eq!(matched(select_last, &terminal), vec![ids::SELECT_TAB]);
        }
    }

    #[test]
    fn window_context_predicates_need_the_descendant_form_to_beat_defaults() {
        // GPUI ranks a predicate-free binding at the full stack depth and an
        // identifier predicate at the depth of the context that contains it,
        // so `fullscreen` alone ranks below the default and only the
        // descendant form `fullscreen > Terminal` wins.
        let terminal = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        let fullscreen = [
            KeyContext::parse("Workspace fullscreen").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        let first = |when: &str, contexts: &[KeyContext]| {
            let user = [KeybindingEntry {
                when: Some(when.into()),
                ..entry("cmd-w", "toggle_fullscreen")
            }];
            let compiled = compile(Platform::MacOs, &user).unwrap();
            let keymap = Keymap::new(compiled.bindings);
            let (bindings, _) =
                keymap.bindings_for_input(&[keystroke("cmd-w")], contexts);
            bindings.first().map(command_of)
        };
        assert_eq!(first("fullscreen", &fullscreen), Some(ids::CLOSE_TAB));
        assert_eq!(
            first("fullscreen > Terminal", &fullscreen),
            Some(ids::TOGGLE_FULLSCREEN)
        );
        assert_eq!(
            first("fullscreen > Terminal", &terminal),
            Some(ids::CLOSE_TAB)
        );
    }

    #[test]
    fn installed_shortcuts_use_captured_context_and_exact_arguments() {
        let user = [
            KeybindingEntry {
                when: Some("Terminal && !confirming".into()),
                ..with_args(
                    entry("alt-r", "rename_tab"),
                    &[("name", toml::Value::String("work".into()))],
                )
            },
            with_args(
                entry("alt-4", "select_tab"),
                &[("index", toml::Value::Integer(4))],
            ),
            entry("alt-q", "show_quake"),
            with_args(
                entry("alt-l", "show_quake"),
                &[("profile", toml::Value::String("logs".into()))],
            ),
        ];
        let (_, installed) =
            compile(Platform::Linux, &user).unwrap().install_parts();
        let terminal = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        let confirming = [
            KeyContext::parse("Workspace confirming").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        assert_eq!(
            installed.shortcuts(ids::RENAME_TAB, &terminal, None).len(),
            1
        );
        assert!(
            installed
                .shortcuts(ids::RENAME_TAB, &confirming, None)
                .is_empty()
        );
        assert!(
            installed
                .shortcuts(
                    ids::SELECT_TAB,
                    &terminal,
                    Some(&[CommandArgument::new(
                        "index",
                        CommandValue::Integer(4),
                    )]),
                )
                .iter()
                .any(|binding| binding.key == "alt-4")
        );
        assert!(
            installed
                .shortcuts(
                    ids::SELECT_TAB,
                    &terminal,
                    Some(&[CommandArgument::new(
                        "index",
                        CommandValue::Integer(5),
                    )]),
                )
                .iter()
                .all(|binding| binding.key != "alt-4")
        );
        let quake = installed.shortcuts(
            ids::SHOW_QUAKE,
            &terminal,
            Some(&[CommandArgument::new(
                "profile",
                CommandValue::Text("logs".into()),
            )]),
        );
        assert_eq!(
            quake.first().map(|binding| binding.key.as_str()),
            Some("alt-l")
        );
        assert!(quake.iter().any(|binding| binding.key == "alt-q"));
    }

    #[test]
    fn palette_defaults_dispatch_through_invoke_palette() {
        let compiled = compile(Platform::MacOs, &[]).unwrap();
        let keymap = Keymap::new(compiled.bindings);
        let palette = [
            KeyContext::parse("Workspace palette").unwrap(),
            KeyContext::parse("Palette").unwrap(),
        ];
        let matched = |key: &str, contexts: &[KeyContext]| {
            let (bindings, pending) =
                keymap.bindings_for_input(&[keystroke(key)], contexts);
            assert!(!pending);
            bindings.first().map(command_of)
        };
        assert_eq!(matched("cmd-c", &palette), Some(ids::TEXT_COPY));
        assert_eq!(matched("down", &palette), Some(ids::PALETTE_SELECT_NEXT));
        assert_eq!(matched("enter", &palette), Some(ids::PALETTE_CONFIRM));

        let terminal = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        assert_eq!(matched("cmd-c", &terminal), Some(ids::COPY));
        assert_eq!(matched("down", &terminal), None);
    }

    #[test]
    fn user_palette_override_wins_at_equal_depth() {
        let compiled =
            compile(Platform::MacOs, &[entry("up", "text_line_start")])
                .unwrap();
        let keymap = Keymap::new(compiled.bindings);
        let contexts = [
            KeyContext::parse("Workspace palette").unwrap(),
            KeyContext::parse("Palette").unwrap(),
        ];
        let (bindings, pending) =
            keymap.bindings_for_input(&[keystroke("up")], &contexts);
        assert!(!pending);
        assert_eq!(
            bindings.first().map(command_of),
            Some(ids::TEXT_LINE_START)
        );
    }

    #[test]
    fn palette_bindings_never_reserve_terminal_strokes() {
        let compiled =
            compile(Platform::MacOs, &[entry("ctrl-c", "text_copy")]).unwrap();
        assert!(!compiled.reserved.is_reserved(&keystroke("ctrl-c")));
        let binding = compiled
            .effective
            .iter()
            .find(|binding| {
                binding.origin == Origin::User && binding.key == "ctrl-c"
            })
            .expect("user binding is effective");
        assert_eq!(binding.context, Some("Palette"));
        assert_eq!(binding.when, None);
    }

    #[test]
    fn context_and_when_combine() {
        let user = [KeybindingEntry {
            when: Some("!confirming".into()),
            ..entry("ctrl-n", "palette_select_next")
        }];
        let compiled = compile(Platform::MacOs, &user).unwrap();
        let binding = compiled
            .effective
            .iter()
            .find(|binding| {
                binding.origin == Origin::User && binding.key == "ctrl-n"
            })
            .expect("user binding is effective");
        let palette = [
            KeyContext::parse("Workspace palette").unwrap(),
            KeyContext::parse("Palette").unwrap(),
        ];
        let confirming = [
            KeyContext::parse("Workspace palette confirming").unwrap(),
            KeyContext::parse("Palette").unwrap(),
        ];
        assert!(binding.matches_context(&palette));
        assert!(!binding.matches_context(&confirming));
    }

    #[test]
    fn partial_bindings_compile_but_bad_values_fail() {
        assert!(
            compile(Platform::MacOs, &[entry("cmd-shift-o", "select_tab")],)
                .is_ok()
        );

        let range = compile(
            Platform::MacOs,
            &[with_args(
                entry("cmd-shift-o", "select_tab"),
                &[("index", toml::Value::Integer(12))],
            )],
        )
        .err()
        .expect("out-of-range index fails")
        .to_string();
        assert!(range.contains("expected 1 to 9"), "{range}");

        let identity = compile(
            Platform::MacOs,
            &[with_args(
                entry("cmd-shift-o", "select_tab"),
                &[("tab", toml::Value::Integer(1))],
            )],
        )
        .err()
        .expect("configured runtime identity fails")
        .to_string();
        assert!(identity.contains("runtime identity"), "{identity}");
    }
}
