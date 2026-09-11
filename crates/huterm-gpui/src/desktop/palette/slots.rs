//! Argument slot state machine for the command palette.
//!
//! One command chip, committed argument chips, and a single active slot.
//! Enter commits and runs when nothing required is missing; Tab commits and
//! moves; Backspace on an empty field pops the previous chip. Everything
//! here is pure so the transitions can be tested without a window.

use huterm_protocol::{
    ArgumentKind, ArgumentSpec, CommandArgument, CommandError,
    CommandInvocation, CommandSpec, CommandValue, Requirement, validate,
};

/// What the client knows about identity domains and captured defaults.
pub(super) trait SlotDomain {
    /// Every value an identity kind can take, or `None` while loading.
    fn values(&self, kind: ArgumentKind) -> Option<Vec<CommandValue>>;
    /// The captured target for an identity kind, when one exists.
    fn default(&self, kind: ArgumentKind) -> Option<CommandValue>;
}

/// How a slot came to hold its value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SlotState {
    /// Awaiting input; `value` is only a preselection.
    Empty,
    /// The user committed the value.
    Committed,
    /// An optional identity holding the captured target; Tab edits it.
    Prefilled,
    /// A required identity whose domain has exactly one value.
    Sole,
    /// Supplied by the caller; shown as a chip, never edited.
    Explicit,
}

#[derive(Clone, Debug)]
pub(super) struct Slot {
    pub(super) spec: &'static ArgumentSpec,
    pub(super) value: Option<CommandValue>,
    pub(super) state: SlotState,
}

impl Slot {
    fn is_chip(&self) -> bool {
        !matches!(self.state, SlotState::Empty)
    }
}

/// Result of committing the active slot.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Commit {
    /// Every required argument is satisfied; run this.
    Run(CommandInvocation),
    /// Moved to the next slot that needs input.
    Next,
    /// The identity domain has not loaded; retry when it does.
    Pending,
}

/// Where leaving the slot stage goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Exit {
    /// Back to command search with the query intact.
    Search,
    /// Close the palette; the stage was reached from a keybinding.
    Close,
}

#[derive(Clone, Debug)]
pub(super) struct SlotEditor {
    spec: &'static CommandSpec,
    slots: Vec<Slot>,
    active: usize,
    text: String,
    requested: bool,
}

pub(super) fn is_identity(kind: ArgumentKind) -> bool {
    matches!(
        kind,
        ArgumentKind::Session
            | ArgumentKind::Workspace
            | ArgumentKind::Tab
            | ArgumentKind::QuakeProfile
    )
}

/// Whether Enter on this command runs it without opening slots: every
/// required prompted argument resolves to exactly one value. `None` while an
/// identity domain the decision depends on is still loading.
pub(super) fn runs_without_prompt(
    spec: &CommandSpec,
    domain: &dyn SlotDomain,
) -> Option<bool> {
    let empty = CommandInvocation::new(spec.id, Vec::new());
    for argument in spec.missing_prompted(&empty) {
        if !is_identity(argument.kind) {
            return Some(false);
        }
        if domain.values(argument.kind)?.len() != 1 {
            return Some(false);
        }
    }
    Some(true)
}

impl SlotEditor {
    pub(super) fn new(
        spec: &'static CommandSpec,
        supplied: &[CommandArgument],
        domain: &dyn SlotDomain,
        requested: bool,
    ) -> Self {
        let slots = spec
            .args
            .iter()
            .filter_map(|argument| {
                let explicit = supplied
                    .iter()
                    .find(|supplied| supplied.name == argument.name);
                if let Some(explicit) = explicit {
                    return Some(Slot {
                        spec: argument,
                        value: Some(explicit.value.clone()),
                        state: SlotState::Explicit,
                    });
                }
                if !argument.prompt {
                    return None;
                }
                // A one-of member whose group the caller already satisfied
                // gets no slot: prompting or auto-filling it would conflict.
                if let Some(group) = argument.group()
                    && supplied.iter().any(|supplied| {
                        spec.argument(&supplied.name)
                            .is_some_and(|member| member.group() == Some(group))
                    })
                {
                    return None;
                }
                let (value, state) =
                    match (argument.required, is_identity(argument.kind)) {
                        (Requirement::Optional, true) => {
                            let default = domain.default(argument.kind);
                            let state = if default.is_some() {
                                SlotState::Prefilled
                            } else {
                                SlotState::Empty
                            };
                            (default, state)
                        }
                        (Requirement::Always | Requirement::OneOf(_), true) => {
                            // Required identities carry no preselection: the
                            // picker's first row (most recently used) is the
                            // likeliest choice.
                            match domain.values(argument.kind) {
                                Some(values) if values.len() == 1 => {
                                    (values.into_iter().next(), SlotState::Sole)
                                }
                                _ => (None, SlotState::Empty),
                            }
                        }
                        (_, false) => (None, SlotState::Empty),
                    };
                Some(Slot {
                    spec: argument,
                    value,
                    state,
                })
            })
            .collect::<Vec<_>>();
        let mut editor = Self {
            spec,
            slots,
            active: 0,
            text: String::new(),
            requested,
        };
        editor.active = editor
            .next_needing_input(None)
            .or_else(|| {
                editor
                    .slots
                    .iter()
                    .position(|slot| slot.state != SlotState::Explicit)
            })
            .unwrap_or(0);
        editor.text = editor.reopen_text();
        editor
    }

    pub(super) fn spec(&self) -> &'static CommandSpec {
        self.spec
    }
    pub(super) fn slots(&self) -> &[Slot] {
        &self.slots
    }
    pub(super) fn active_index(&self) -> usize {
        self.active
    }
    pub(super) fn active(&self) -> &Slot {
        &self.slots[self.active]
    }
    pub(super) fn text(&self) -> &str {
        &self.text
    }
    pub(super) fn set_text(&mut self, text: &str) {
        text.clone_into(&mut self.text);
    }
    pub(super) fn requested(&self) -> bool {
        self.requested
    }

    /// Whether the command can run right now with the values held.
    pub(super) fn complete(&self) -> bool {
        self.invocation().is_ok()
    }

    /// The invocation built from every slot holding a value.
    pub(super) fn invocation(&self) -> Result<CommandInvocation, CommandError> {
        let args = self
            .slots
            .iter()
            .filter_map(|slot| {
                slot.value
                    .clone()
                    .map(|value| CommandArgument::new(slot.spec.name, value))
            })
            .collect();
        let invocation = CommandInvocation::new(self.spec.id, args);
        validate(&invocation)?;
        Ok(invocation)
    }

    /// Whether the slot at `index` still needs the user's input: it holds no
    /// committed value and its requirement is unsatisfied.
    fn needs_input(&self, index: usize) -> bool {
        let slot = &self.slots[index];
        if slot.is_chip() {
            return false;
        }
        match slot.spec.required {
            Requirement::Always => true,
            Requirement::Optional => false,
            Requirement::OneOf(group) => !self.group_satisfied(group),
        }
    }

    fn group_satisfied(&self, group: &str) -> bool {
        self.slots.iter().any(|slot| {
            slot.spec.group() == Some(group)
                && slot.is_chip()
                && slot.value.is_some()
        })
    }

    fn next_needing_input(&self, after: Option<usize>) -> Option<usize> {
        let start = after.map_or(0, |index| index + 1);
        (start..self.slots.len()).find(|index| self.needs_input(*index))
    }

    /// Commits the active slot. Pickers pass the highlighted value; scalar
    /// slots parse the draft text. Runs when nothing required remains.
    ///
    /// # Errors
    /// Returns a message written for people when the value is invalid.
    pub(super) fn commit(
        &mut self,
        picked: Option<CommandValue>,
        domain: &dyn SlotDomain,
    ) -> Result<Commit, String> {
        if !self.store_active(picked, domain)? {
            return Ok(Commit::Pending);
        }
        self.finish_commit()
    }

    fn finish_commit(&mut self) -> Result<Commit, String> {
        if let Ok(invocation) = self.invocation() {
            return Ok(Commit::Run(invocation));
        }
        match self.next_needing_input(None) {
            Some(next) => {
                self.active = next;
                self.text = self.reopen_text();
                Ok(Commit::Next)
            }
            None => self
                .invocation()
                .map(Commit::Run)
                .map_err(|error| Self::humanize(&error)),
        }
    }

    /// Stores the active slot's value. Returns `Ok(false)` when an identity
    /// domain is still loading and nothing could be stored.
    fn store_active(
        &mut self,
        picked: Option<CommandValue>,
        domain: &dyn SlotDomain,
    ) -> Result<bool, String> {
        let slot = &self.slots[self.active];
        let name = slot.spec.name;
        let label = label(name);
        if is_identity(slot.spec.kind) {
            let Some(value) = picked else {
                if domain.values(slot.spec.kind).is_none() {
                    return Ok(false);
                }
                return Err(format!("No matching {name}"));
            };
            let slot = &mut self.slots[self.active];
            slot.value = Some(value);
            slot.state = SlotState::Committed;
            return Ok(true);
        }
        let text = self.text.trim();
        // Blank text is a value for text arguments: renames clear the custom
        // name on a blank, so Enter on an emptied name field runs the clear.
        let value = if text.is_empty() && slot.spec.kind != ArgumentKind::Text {
            let required = match slot.spec.required {
                Requirement::Always => true,
                Requirement::Optional => false,
                Requirement::OneOf(group) => !self.group_satisfied(group),
            };
            if required {
                return Err(format!("{label} is required"));
            }
            None
        } else {
            Some(parse_scalar(slot.spec, text)?)
        };
        let slot = &mut self.slots[self.active];
        slot.value = value;
        slot.state = SlotState::Committed;
        Ok(true)
    }

    /// Tab: commit valid text or the highlighted picker row, then move to the
    /// next editable slot. Empty text leaves the slot uncommitted.
    ///
    /// # Errors
    /// Returns a message when non-empty text is invalid.
    pub(super) fn next_slot(
        &mut self,
        picked: Option<CommandValue>,
        domain: &dyn SlotDomain,
    ) -> Result<(), String> {
        let slot = &self.slots[self.active];
        if is_identity(slot.spec.kind) {
            if let Some(value) = picked {
                let slot = &mut self.slots[self.active];
                slot.value = Some(value);
                slot.state = SlotState::Committed;
            }
        } else if !self.text.trim().is_empty() {
            self.store_active(None, domain)?;
        }
        if let Some(next) = self.editable_after(self.active) {
            self.edit(next);
        }
        Ok(())
    }

    /// Shift-Tab: move to the previous editable slot without committing.
    pub(super) fn previous_slot(&mut self) {
        if let Some(previous) = self.editable_before(self.active) {
            self.edit(previous);
        }
    }

    fn editable_after(&self, index: usize) -> Option<usize> {
        (index + 1..self.slots.len()).find(|candidate| {
            self.slots[*candidate].state != SlotState::Explicit
        })
    }

    fn editable_before(&self, index: usize) -> Option<usize> {
        (0..index).rev().find(|candidate| {
            self.slots[*candidate].state != SlotState::Explicit
        })
    }

    /// Backspace on an empty field: reopen the previous chip, or leave the
    /// stage when there is none.
    pub(super) fn pop(&mut self) -> Option<Exit> {
        let previous = (0..self.active).rev().find(|index| {
            matches!(
                self.slots[*index].state,
                SlotState::Committed | SlotState::Prefilled
            )
        });
        match previous {
            Some(index) => {
                self.edit(index);
                None
            }
            None => Some(self.exit()),
        }
    }

    /// Escape: leave the stage.
    pub(super) fn exit(&self) -> Exit {
        if self.requested {
            Exit::Close
        } else {
            Exit::Search
        }
    }

    /// Reopens slot `index` for editing. Its value stays as the preselection
    /// and, for scalars, as the draft text.
    pub(super) fn edit(&mut self, index: usize) {
        if self.slots[index].state == SlotState::Explicit {
            return;
        }
        self.active = index;
        self.slots[index].state = SlotState::Empty;
        self.text = self.reopen_text();
    }

    fn reopen_text(&self) -> String {
        let slot = &self.slots[self.active];
        if is_identity(slot.spec.kind) {
            return String::new();
        }
        slot.value.as_ref().map(scalar_text).unwrap_or_default()
    }

    /// Turns a protocol error into a sentence for the palette.
    pub(super) fn humanize(error: &CommandError) -> String {
        match error {
            CommandError::MissingArgument { name, .. } => {
                format!("{} is required", label(name))
            }
            CommandError::ArgumentRange { name, min, max, .. } => {
                format!("{} must be between {min} and {max}", label(name))
            }
            CommandError::ArgumentType { name, .. } => {
                format!("{} has the wrong type", label(name))
            }
            CommandError::ConflictingArguments { names, .. } => {
                format!("Choose only one of {}", names.join(", "))
            }
            other => other.to_string(),
        }
    }
}

fn label(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn scalar_text(value: &CommandValue) -> String {
    match value {
        CommandValue::Bool(value) => value.to_string(),
        CommandValue::Integer(value) => value.to_string(),
        CommandValue::Text(value) => value.clone(),
        CommandValue::Session(_)
        | CommandValue::Workspace(_)
        | CommandValue::Tab(_) => String::new(),
    }
}

fn parse_scalar(
    spec: &ArgumentSpec,
    text: &str,
) -> Result<CommandValue, String> {
    let label = label(spec.name);
    match spec.kind {
        ArgumentKind::Bool => text
            .parse::<bool>()
            .map(CommandValue::Bool)
            .map_err(|_| format!("{label} must be true or false")),
        ArgumentKind::Integer { min, max } => {
            let value = text.parse::<i64>().map_err(|_| {
                format!(
                    "{label} must be a whole number between {min} and {max}"
                )
            })?;
            if !(min..=max).contains(&value) {
                return Err(format!("{label} must be between {min} and {max}"));
            }
            Ok(CommandValue::Integer(value))
        }
        ArgumentKind::Text => Ok(CommandValue::Text(text.to_owned())),
        ArgumentKind::QuakeProfile
        | ArgumentKind::Tab
        | ArgumentKind::Workspace
        | ArgumentKind::Session => {
            Err(format!("{label} must be chosen from the list"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use huterm_protocol::{RuntimeId, TabId, WorkspaceId, catalog, ids};

    struct Domain {
        tabs: Option<Vec<TabId>>,
        workspaces: Option<Vec<WorkspaceId>>,
        active_tab: Option<TabId>,
    }

    impl SlotDomain for Domain {
        fn values(&self, kind: ArgumentKind) -> Option<Vec<CommandValue>> {
            match kind {
                ArgumentKind::Tab => Some(
                    self.tabs
                        .clone()?
                        .into_iter()
                        .map(CommandValue::Tab)
                        .collect(),
                ),
                ArgumentKind::Workspace => Some(
                    self.workspaces
                        .clone()?
                        .into_iter()
                        .map(CommandValue::Workspace)
                        .collect(),
                ),
                ArgumentKind::QuakeProfile => {
                    Some(vec![CommandValue::Text("default".into())])
                }
                _ => Some(Vec::new()),
            }
        }
        fn default(&self, kind: ArgumentKind) -> Option<CommandValue> {
            match kind {
                ArgumentKind::Tab => self.active_tab.map(CommandValue::Tab),
                ArgumentKind::Workspace => self
                    .workspaces
                    .as_ref()?
                    .first()
                    .copied()
                    .map(CommandValue::Workspace),
                ArgumentKind::QuakeProfile => {
                    Some(CommandValue::Text("default".into()))
                }
                _ => None,
            }
        }
    }

    fn tab(n: u64) -> TabId {
        TabId::in_runtime(RuntimeId::new(1), n)
    }

    fn with_tabs(tabs: &[u64]) -> Domain {
        Domain {
            tabs: Some(tabs.iter().map(|n| tab(*n)).collect()),
            workspaces: Some(vec![WorkspaceId::in_runtime(
                RuntimeId::new(1),
                1,
            )]),
            active_tab: tabs.first().map(|n| tab(*n)),
        }
    }

    fn spec(id: huterm_protocol::CommandId) -> &'static CommandSpec {
        catalog().iter().find(|spec| spec.id == id).unwrap()
    }

    #[test]
    fn rename_runs_after_one_enter_with_the_prefilled_tab() {
        let domain = with_tabs(&[1, 2]);
        let mut editor =
            SlotEditor::new(spec(ids::RENAME_TAB), &[], &domain, false);
        assert_eq!(editor.active().spec.name, "name");
        assert_eq!(editor.slots()[1].state, SlotState::Prefilled);
        editor.set_text("work");
        let Ok(Commit::Run(invocation)) = editor.commit(None, &domain) else {
            panic!("rename should run after the name");
        };
        assert_eq!(invocation.text("name"), Some("work"));
        assert_eq!(invocation.tab("tab"), Some(tab(1)));
    }

    #[test]
    fn blank_name_commits_as_a_clear_and_empty_pickers_report_a_message() {
        let domain = with_tabs(&[1]);
        let mut editor =
            SlotEditor::new(spec(ids::RENAME_TAB), &[], &domain, false);
        editor.set_text("  ");
        let Ok(Commit::Run(invocation)) = editor.commit(None, &domain) else {
            panic!("a blank name runs the rename, which clears the name");
        };
        assert_eq!(invocation.text("name"), Some(""));
        let mut select = SlotEditor::new(
            spec(ids::SELECT_TAB),
            &[],
            &with_tabs(&[1, 2]),
            false,
        );
        assert_eq!(select.active().spec.name, "tab");
        assert_eq!(
            select.commit(None, &with_tabs(&[1, 2])),
            Err("No matching tab".into())
        );
    }

    #[test]
    fn select_tab_prompts_only_for_tab_and_runs_on_pick() {
        let domain = with_tabs(&[1, 2, 3]);
        assert_eq!(
            runs_without_prompt(spec(ids::SELECT_TAB), &domain),
            Some(false)
        );
        let mut editor =
            SlotEditor::new(spec(ids::SELECT_TAB), &[], &domain, true);
        assert_eq!(editor.slots().len(), 1, "index is unprompted");
        assert!(editor.requested());
        let Ok(Commit::Run(invocation)) =
            editor.commit(Some(CommandValue::Tab(tab(3))), &domain)
        else {
            panic!("picking a tab runs select_tab");
        };
        assert_eq!(invocation.tab("tab"), Some(tab(3)));
        assert_eq!(invocation.integer("index"), None);
    }

    #[test]
    fn optional_only_and_sole_domain_commands_run_without_prompting() {
        let domain = with_tabs(&[1]);
        assert_eq!(
            runs_without_prompt(spec(ids::TOGGLE_QUAKE), &domain),
            Some(true)
        );
        assert_eq!(
            runs_without_prompt(spec(ids::NEW_TAB), &domain),
            Some(true)
        );
        assert_eq!(
            runs_without_prompt(spec(ids::RENAME_TAB), &domain),
            Some(false)
        );
        let loading = Domain {
            tabs: None,
            workspaces: None,
            active_tab: None,
        };
        assert_eq!(runs_without_prompt(spec(ids::SELECT_TAB), &loading), None);
        // Two tabs: select_tab still prompts; one tab would be the sole value.
        assert_eq!(
            runs_without_prompt(spec(ids::SELECT_TAB), &domain),
            Some(true)
        );
        let editor =
            SlotEditor::new(spec(ids::SELECT_TAB), &[], &domain, false);
        assert_eq!(editor.slots()[0].state, SlotState::Sole);
        assert!(editor.complete());
    }

    #[test]
    fn tab_commits_text_moves_on_and_leaves_empty_slots_uncommitted() {
        let domain = with_tabs(&[1, 2]);
        let mut editor =
            SlotEditor::new(spec(ids::RENAME_TAB), &[], &domain, false);
        editor.set_text("work");
        editor.next_slot(None, &domain).unwrap();
        assert_eq!(editor.active().spec.name, "tab");
        assert_eq!(editor.slots()[0].state, SlotState::Committed);
        assert_eq!(editor.slots()[1].state, SlotState::Empty);
        assert_eq!(
            editor.slots()[1].value,
            Some(CommandValue::Tab(tab(1))),
            "preselection kept"
        );
        editor.next_slot(None, &domain).unwrap();
        assert_eq!(editor.active().spec.name, "tab", "last slot stays");
        editor.previous_slot();
        assert_eq!(editor.active().spec.name, "name");
        assert_eq!(editor.text(), "work");
        editor.set_text("");
        editor.next_slot(None, &domain).unwrap();
        assert_eq!(editor.slots()[0].state, SlotState::Empty);
        assert_eq!(
            editor.slots()[0].value,
            Some(CommandValue::Text("work".into()))
        );
    }

    #[test]
    fn explicit_group_member_suppresses_its_siblings() {
        let domain = with_tabs(&[1]);
        let mut editor = SlotEditor::new(
            spec(ids::SELECT_TAB),
            &[CommandArgument::new("index", CommandValue::Integer(1))],
            &domain,
            false,
        );
        // Explicit index satisfies the group; nothing to prompt.
        assert!(editor.complete());
        assert_eq!(editor.slots()[0].state, SlotState::Explicit);
        editor.edit(0);
        assert_eq!(
            editor.slots()[0].state,
            SlotState::Explicit,
            "explicit chips are not editable"
        );
    }

    #[test]
    fn pop_reopens_the_previous_chip_then_exits() {
        let domain = with_tabs(&[1, 2]);
        let mut editor =
            SlotEditor::new(spec(ids::RENAME_TAB), &[], &domain, false);
        editor.set_text("work");
        editor.next_slot(None, &domain).unwrap();
        assert_eq!(editor.pop(), None);
        assert_eq!(editor.active().spec.name, "name");
        assert_eq!(editor.text(), "work");
        assert_eq!(editor.pop(), Some(Exit::Search));
        let requested =
            SlotEditor::new(spec(ids::SELECT_TAB), &[], &domain, true);
        assert_eq!(requested.exit(), Exit::Close);
    }

    #[test]
    fn pending_domain_defers_the_commit() {
        let loading = Domain {
            tabs: None,
            workspaces: None,
            active_tab: None,
        };
        let mut editor =
            SlotEditor::new(spec(ids::SELECT_TAB), &[], &loading, true);
        assert_eq!(editor.commit(None, &loading), Ok(Commit::Pending));
    }

    #[test]
    fn humanized_messages_cover_ranges_and_conflicts() {
        assert_eq!(
            SlotEditor::humanize(&CommandError::ArgumentRange {
                command: ids::SELECT_TAB,
                name: "index",
                value: 10,
                min: 1,
                max: 9
            }),
            "Index must be between 1 and 9"
        );
        assert_eq!(
            parse_scalar(&spec(ids::SELECT_TAB).args[0], "10"),
            Err("Index must be between 1 and 9".into())
        );
        assert_eq!(
            parse_scalar(&spec(ids::SELECT_TAB).args[0], "abc"),
            Err("Index must be a whole number between 1 and 9".into())
        );
    }
}
