//! The keybinding editor's pure parts: command groups, the contexts a `when`
//! condition can name, condition checks, and the patches each edit writes.

use gpui::KeyBindingContextPredicate;
use huterm_config::document::{EntryLocator, List, Patch, Snapshot};
use huterm_protocol::{CommandArgument, CommandId, CommandValue, ids};
use toml::{Table, Value};

/// The group for commands that act inside an overlay or text field, which
/// every command with an implied context joins.
pub(crate) const OVERLAYS: &str = "Overlays and text fields";

/// Command groups in display order; [`OVERLAYS`] follows them.
pub(crate) const GROUPS: &[(&str, &[CommandId])] = &[
    (
        "Tabs",
        &[
            ids::NEW_TAB,
            ids::CLOSE_TAB,
            ids::CLOSE_OTHER_TABS,
            ids::CLOSE_TABS_AFTER,
            ids::NEXT_TAB,
            ids::PREVIOUS_TAB,
            ids::SELECT_TAB,
            ids::SELECT_RECENT_TAB,
            ids::RENAME_TAB,
            ids::COPY_TAB_DIRECTORY,
            ids::OPEN_TAB_DIRECTORY,
            ids::MOVE_TAB_TO_PANE,
        ],
    ),
    (
        "Panes",
        &[
            ids::SPLIT_RIGHT,
            ids::SPLIT_DOWN,
            ids::SPLIT_LEFT,
            ids::SPLIT_UP,
            ids::CLOSE_PANE,
            ids::FOCUS_NEXT_PANE,
            ids::FOCUS_PREVIOUS_PANE,
            ids::FOCUS_PANE_LEFT,
            ids::FOCUS_PANE_RIGHT,
            ids::FOCUS_PANE_UP,
            ids::FOCUS_PANE_DOWN,
            ids::RESIZE_PANE_LEFT,
            ids::RESIZE_PANE_RIGHT,
            ids::RESIZE_PANE_UP,
            ids::RESIZE_PANE_DOWN,
            ids::TOGGLE_PANE_ZOOM,
            ids::SWAP_PANE_NEXT,
            ids::SWAP_PANE_PREVIOUS,
            ids::SWAP_PANE_LEFT,
            ids::SWAP_PANE_RIGHT,
            ids::SWAP_PANE_UP,
            ids::SWAP_PANE_DOWN,
            ids::MOVE_PANE_NEXT,
            ids::MOVE_PANE_PREVIOUS,
            ids::MOVE_PANE_LEFT,
            ids::MOVE_PANE_RIGHT,
            ids::MOVE_PANE_UP,
            ids::MOVE_PANE_DOWN,
            ids::MOVE_PANE_TO_PANE,
            ids::MOVE_PANE_TO_NEW_TAB,
        ],
    ),
    (
        "Window",
        &[
            ids::NEW_WINDOW,
            ids::CLOSE_WINDOW,
            ids::TOGGLE_FULLSCREEN,
            ids::TOGGLE_NATIVE_FULLSCREEN,
            ids::TOGGLE_NON_NATIVE_FULLSCREEN,
            ids::MINIMIZE,
            ids::ZOOM,
            ids::OPEN_MENU,
            ids::OPEN_COMMAND_PALETTE,
            ids::OPEN_SETTINGS,
            ids::OPEN_CONFIG_FILE,
            ids::RELOAD_CONFIG,
            ids::ABOUT,
            ids::FOCUS_NOTICES,
            ids::DISMISS_ALL_NOTICES,
        ],
    ),
    (
        "Terminal",
        &[
            ids::COPY,
            ids::PASTE,
            ids::SELECT_ALL,
            ids::CLEAR_SCROLLBACK,
            ids::RESET_TERMINAL,
            ids::SCROLL_PAGE_UP,
            ids::SCROLL_PAGE_DOWN,
            ids::SCROLL_TO_BOTTOM,
            ids::OPEN_CONTEXT_MENU,
        ],
    ),
    (
        "Application",
        &[
            ids::QUIT,
            ids::HIDE,
            ids::HIDE_OTHERS,
            ids::SHOW_ALL,
            ids::CHECK_FOR_UPDATES,
            ids::RENAME_WORKSPACE,
            ids::RENAME_SESSION,
        ],
    ),
    (
        "Quake",
        &[ids::TOGGLE_QUAKE, ids::SHOW_QUAKE, ids::HIDE_QUAKE],
    ),
];

/// A context a `when` condition can name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Context {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
}

const fn context(name: &'static str, description: &'static str) -> Context {
    Context { name, description }
}

/// The contexts `docs/keybindings.md` documents, in its order.
pub(crate) const CONTEXTS: &[Context] = &[
    context("Terminal", "A terminal view has focus."),
    context("selection", "That terminal has selected text."),
    context("exited", "That terminal's root shell has exited."),
    context("split", "That terminal's tab has two or more panes."),
    context(
        "pane_zoomed",
        "The window shows that terminal's pane zoomed.",
    ),
    context("has_pane_left", "There is a pane to the left to move to."),
    context("has_pane_right", "There is a pane to the right to move to."),
    context("has_pane_up", "There is a pane above to move to."),
    context("has_pane_down", "There is a pane below to move to."),
    context("Workspace", "Always, on the window's root."),
    context(
        "confirming",
        "A close confirmation or the About panel is open.",
    ),
    context("menu", "The window menu has keyboard focus."),
    context("notices", "A notice has keyboard focus."),
    context("reordering", "A tab drag is in progress."),
    context("fullscreen", "The window has finished entering fullscreen."),
    context("palette", "The window owns an open command palette."),
    context("Palette", "The command palette or a text field has focus."),
    context("Settings", "The Settings window has focus."),
    context(
        "SettingsControl",
        "A Settings switch, select, or section has focus.",
    ),
];

/// Checks a `when` condition: `Err` with the parser's message when it does
/// not parse, else the context names it uses that no context provides. An
/// empty condition is fine.
pub(crate) fn check_condition(text: &str) -> Result<Vec<String>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let predicate = KeyBindingContextPredicate::parse(text)
        .map_err(|error| error.to_string())?;
    let mut unknown = Vec::new();
    unknown_names(&predicate, &mut unknown);
    Ok(unknown)
}

fn unknown_names(
    predicate: &KeyBindingContextPredicate,
    unknown: &mut Vec<String>,
) {
    use KeyBindingContextPredicate as P;
    match predicate {
        P::Identifier(name) | P::Equal(name, _) | P::NotEqual(name, _) => {
            if !CONTEXTS.iter().any(|context| context.name == name.as_ref())
                && !unknown.iter().any(|seen| seen == name.as_ref())
            {
                unknown.push(name.to_string());
            }
        }
        P::Not(inner) => unknown_names(inner, unknown),
        P::Descendant(left, right)
        | P::And(left, right)
        | P::Or(left, right) => {
            unknown_names(left, unknown);
            unknown_names(right, unknown);
        }
    }
}

/// The context name being typed before byte offset `caret` in `text`: its
/// start, and the contexts that complete it, excluding an exact match.
pub(crate) fn completions(text: &str, caret: usize) -> (usize, Vec<Context>) {
    let before = text.get(..caret).unwrap_or(text);
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, character)| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
        })
        .last()
        .map_or(caret.min(text.len()), |(index, _)| index);
    let prefix = before.get(start..).unwrap_or_default().to_lowercase();
    let matches = CONTEXTS
        .iter()
        .filter(|context| {
            let name = context.name.to_lowercase();
            name.starts_with(&prefix) && name != prefix
        })
        .copied()
        .collect();
    (start, matches)
}

/// A binding as a `[[keybinding]]` entry writes it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Binding {
    pub(crate) key: String,
    pub(crate) command: CommandId,
    pub(crate) when: Option<String>,
    pub(crate) args: Table,
}

impl Binding {
    /// The binding's arguments as config values; identities never appear
    /// in a binding the keymap compiled.
    pub(crate) fn new(
        key: &str,
        command: CommandId,
        when: Option<&str>,
        args: &[CommandArgument],
    ) -> Self {
        let args = args
            .iter()
            .filter_map(|argument| {
                let value = match &argument.value {
                    CommandValue::Bool(value) => Value::Boolean(*value),
                    CommandValue::Integer(value) => Value::Integer(*value),
                    CommandValue::Text(value) => Value::String(value.clone()),
                    CommandValue::Tab(_)
                    | CommandValue::Workspace(_)
                    | CommandValue::Session(_)
                    | CommandValue::Pane(_) => return None,
                };
                Some((argument.name.clone(), value))
            })
            .collect();
        Self {
            key: key.to_owned(),
            command,
            when: when.map(str::to_owned),
            args,
        }
    }

    /// The entry for this binding, with `disabled = true` when `disabled`.
    pub(crate) fn entry(&self, disabled: bool) -> Table {
        let mut entry = Table::new();
        entry.insert("key".into(), Value::String(self.key.clone()));
        entry.insert(
            "command".into(),
            Value::String(self.command.as_str().to_owned()),
        );
        if !self.args.is_empty() {
            entry.insert("args".into(), Value::Table(self.args.clone()));
        }
        if let Some(when) = &self.when {
            entry.insert("when".into(), Value::String(when.clone()));
        }
        if disabled {
            entry.insert("disabled".into(), Value::Boolean(true));
        }
        entry
    }
}

/// One field of a binding an edit changes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Field {
    Key,
    When,
    Argument(String),
}

/// One change the editor makes, resolved into patches against the snapshot
/// the previous write left.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Edit {
    /// Turn off an active default with an entry that matches only it.
    SwitchOffDefault(Binding),
    /// Turn a default back on by removing the entries that disable it.
    SwitchOnDefault(Vec<usize>),
    /// Set or clear a user entry's own `disabled`.
    SwitchUser { entry: usize, on: bool },
    /// Change a default: disable it, unless entries already do, and add the
    /// changed binding.
    ChangeDefault {
        default: Binding,
        disabled: bool,
        field: Field,
        value: Option<Value>,
    },
    /// Set or clear one field of a user entry.
    ChangeUser {
        entry: usize,
        field: Field,
        value: Option<Value>,
    },
    /// Add a binding for a command.
    Add { key: String, command: CommandId },
    /// Remove user entries: one, or every entry for a command.
    Remove(Vec<usize>),
}

impl Edit {
    /// The patches this edit writes against `snapshot`.
    ///
    /// # Errors
    /// When an entry it addresses is not in the snapshot.
    pub(crate) fn patches(
        &self,
        snapshot: &Snapshot,
    ) -> Result<Vec<Patch>, String> {
        let locate = |index: usize| -> Result<EntryLocator, String> {
            snapshot.entry(List::Keybinding, index).ok_or_else(|| {
                format!("keybinding entry {} is gone", index + 1)
            })
        };
        let append = |entry| Patch::AppendEntry {
            list: List::Keybinding,
            entry,
        };
        Ok(match self {
            Self::SwitchOffDefault(binding) => {
                vec![append(binding.entry(true))]
            }
            Self::SwitchOnDefault(entries) | Self::Remove(entries) => entries
                .iter()
                .map(|&index| {
                    Ok(Patch::RemoveEntry {
                        locator: locate(index)?,
                    })
                })
                .collect::<Result<_, String>>()?,
            Self::SwitchUser { entry, on: true } => {
                vec![Patch::ClearEntryField {
                    locator: locate(*entry)?,
                    field: "disabled".into(),
                }]
            }
            Self::SwitchUser { entry, on: false } => {
                vec![Patch::SetEntryField {
                    locator: locate(*entry)?,
                    field: "disabled".into(),
                    value: Value::Boolean(true),
                }]
            }
            Self::ChangeDefault {
                default,
                disabled,
                field,
                value,
            } => change_default(default, *disabled, field, value.as_ref()),
            Self::ChangeUser {
                entry,
                field,
                value,
            } => {
                let locator = locate(*entry)?;
                let field = match field {
                    Field::Key => "key".to_owned(),
                    Field::When => "when".to_owned(),
                    Field::Argument(name) => {
                        return Ok(vec![argument_patch(
                            locator,
                            name,
                            value.as_ref(),
                        )]);
                    }
                };
                vec![match value {
                    Some(value) => Patch::SetEntryField {
                        locator,
                        field,
                        value: value.clone(),
                    },
                    None => Patch::ClearEntryField { locator, field },
                }]
            }
            Self::Add { key, command } => {
                vec![append(
                    Binding::new(key, *command, None, &[]).entry(false),
                )]
            }
        })
    }
}

/// A changed default: an entry that disables it, unless entries already
/// do, and the changed binding.
fn change_default(
    default: &Binding,
    disabled: bool,
    field: &Field,
    value: Option<&Value>,
) -> Vec<Patch> {
    let mut changed = default.clone();
    match field {
        Field::Key => {
            if let Some(Value::String(key)) = value {
                changed.key.clone_from(key);
            }
        }
        Field::When => {
            changed.when = match value {
                Some(Value::String(when)) => Some(when.clone()),
                _ => None,
            };
        }
        Field::Argument(name) => match value {
            Some(value) => {
                changed.args.insert(name.clone(), value.clone());
            }
            None => {
                changed.args.remove(name);
            }
        },
    }
    let append = |entry| Patch::AppendEntry {
        list: List::Keybinding,
        entry,
    };
    let mut patches = Vec::with_capacity(2);
    if !disabled {
        patches.push(append(default.entry(true)));
    }
    patches.push(append(changed.entry(false)));
    patches
}

/// Sets or clears one argument of an entry, rewriting its `args` table and
/// removing it when the last argument goes.
fn argument_patch(
    locator: EntryLocator,
    name: &str,
    value: Option<&Value>,
) -> Patch {
    let mut args = match locator.content().get("args") {
        Some(Value::Table(args)) => args.clone(),
        _ => Table::new(),
    };
    match value {
        Some(value) => {
            args.insert(name.to_owned(), value.clone());
        }
        None => {
            args.remove(name);
        }
    }
    if args.is_empty() {
        Patch::ClearEntryField {
            locator,
            field: "args".into(),
        }
    } else {
        Patch::SetEntryField {
            locator,
            field: "args".into(),
            value: Value::Table(args),
        }
    }
}

#[cfg(test)]
mod tests;
