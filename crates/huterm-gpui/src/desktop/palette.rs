//! Command-palette workflow and its window-independent state model.

use std::collections::{HashMap, HashSet};

use gpui::{
    App, Context, Entity, EventEmitter, Focusable, Hsla, KeyBinding,
    KeyContext, MouseButton, Render, ScrollHandle, ScrollWheelEvent,
    Subscription, WeakEntity, Window, div, prelude::*, px,
};
use huterm_core::HierarchySnapshot;
use huterm_protocol::{
    ArgumentKind, CommandArgument, CommandError, CommandId, CommandInvocation,
    CommandSpec, CommandValue, SessionId, TabId, TerminalId, WorkspaceId,
    catalog, ids, validate,
};

use super::TerminalView;
use crate::keymap::InstalledKeymap;
use crate::ui::picker::{PickerItem, PickerList};
use crate::ui::text_field::{Changed, TextField};

gpui::actions!(huterm_palette, [Up, Down, Confirm, Cancel]);

pub(crate) fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("up", Up, Some("Palette")),
        KeyBinding::new("down", Down, Some("Palette")),
        KeyBinding::new("enter", Confirm, Some("Palette")),
        KeyBinding::new("escape", Cancel, Some("Palette")),
    ]
}

#[derive(Clone)]
pub(super) struct PaletteTarget {
    pub(super) session: Option<SessionId>,
    pub(super) workspace: Option<WorkspaceId>,
    pub(super) tab: Option<TabId>,
    pub(super) terminal: Option<TerminalId>,
    pub(super) terminal_view: Option<WeakEntity<TerminalView>>,
    pub(super) contexts: Vec<KeyContext>,
}

#[derive(Clone, Debug)]
struct IdentityRow {
    value: CommandValue,
    label: String,
    detail: String,
}

#[derive(Clone, Debug, Default)]
struct PaletteHierarchy {
    sessions: Vec<IdentityRow>,
    workspaces: Vec<IdentityRow>,
    tabs: Vec<IdentityRow>,
}

impl PaletteHierarchy {
    fn from_snapshot(
        snapshot: &HierarchySnapshot,
        live_titles: &HashMap<TabId, String>,
    ) -> Self {
        let session_names: HashMap<_, _> = snapshot
            .sessions
            .iter()
            .map(|session| (session.id, session.display_name().to_owned()))
            .collect();
        let workspace_names: HashMap<_, _> = snapshot
            .workspaces
            .iter()
            .map(|workspace| {
                (workspace.id, workspace.display_name().to_owned())
            })
            .collect();
        let sessions = snapshot
            .sessions
            .iter()
            .map(|session| IdentityRow {
                value: CommandValue::Session(session.id),
                label: session.display_name().to_owned(),
                detail: format!("session {:?}", session.id),
            })
            .collect();
        let workspaces = snapshot
            .workspaces
            .iter()
            .map(|workspace| IdentityRow {
                value: CommandValue::Workspace(workspace.id),
                label: workspace.display_name().to_owned(),
                detail: format!(
                    "{} · workspace {:?}",
                    session_names
                        .get(&workspace.session_id)
                        .map_or("unknown session", String::as_str),
                    workspace.id
                ),
            })
            .collect();
        let tabs = snapshot
            .workspaces
            .iter()
            .flat_map(|workspace| {
                workspace.tabs.iter().map(|tab| IdentityRow {
                    value: CommandValue::Tab(tab.id),
                    label: live_titles
                        .get(&tab.id)
                        .cloned()
                        .unwrap_or_else(|| tab.display_name("").to_owned()),
                    detail: format!(
                        "{} · {} · tab {:?}",
                        session_names
                            .get(&workspace.session_id)
                            .map_or("unknown session", String::as_str),
                        workspace_names
                            .get(&workspace.id)
                            .map_or("unknown workspace", String::as_str),
                        tab.id
                    ),
                })
            })
            .collect();
        Self {
            sessions,
            workspaces,
            tabs,
        }
    }

    fn items(&self, kind: ArgumentKind) -> Vec<PickerItem<CommandValue>> {
        let rows = match kind {
            ArgumentKind::Session => &self.sessions,
            ArgumentKind::Workspace => &self.workspaces,
            ArgumentKind::Tab => &self.tabs,
            _ => return Vec::new(),
        };
        rows.iter()
            .map(|row| PickerItem {
                value: row.value.clone(),
                label: row.label.clone(),
                detail: row.detail.clone(),
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
struct CommandSearch {
    query: String,
    filtered: Vec<usize>,
    selected: usize,
}

impl CommandSearch {
    fn new() -> Self {
        let mut search = Self {
            query: String::new(),
            filtered: Vec::new(),
            selected: 0,
        };
        search.update("");
        search
    }

    fn update(&mut self, query: &str) {
        query.clone_into(&mut self.query);
        let terms: Vec<_> =
            query.split_whitespace().map(str::to_lowercase).collect();
        let mut matches: Vec<_> = catalog()
            .iter()
            .enumerate()
            .filter(|(_, spec)| spec.id != ids::OPEN_COMMAND_PALETTE)
            .filter_map(|(index, spec)| {
                search_rank(spec, &terms).map(|rank| (rank, index))
            })
            .collect();
        matches.sort_by_key(|(rank, index)| (*rank, *index));
        self.filtered = matches.into_iter().map(|(_, index)| index).collect();
        self.selected =
            self.selected.min(self.filtered.len().saturating_sub(1));
    }

    fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self
                .selected
                .saturating_add_signed(delta)
                .min(self.filtered.len() - 1);
        }
    }

    fn selected(&self) -> Option<&'static CommandSpec> {
        self.filtered
            .get(self.selected)
            .and_then(|index| catalog().get(*index))
    }

    fn selected_unavailable<'a>(
        &self,
        availability: &'a HashMap<CommandId, String>,
    ) -> Option<&'a str> {
        self.selected()
            .and_then(|spec| availability.get(&spec.id))
            .map(String::as_str)
    }
}

fn term_rank(spec: &CommandSpec, term: &str) -> Option<u8> {
    let title = spec.title.to_lowercase();
    let id = spec.id.as_str().to_lowercase();
    let description = spec.description.to_lowercase();
    if title.starts_with(term) {
        Some(0)
    } else if title.split_whitespace().any(|word| word.starts_with(term)) {
        Some(1)
    } else if id.contains(term) {
        Some(2)
    } else if description.contains(term) {
        Some(3)
    } else {
        None
    }
}

fn search_rank(spec: &CommandSpec, terms: &[String]) -> Option<(u8, u16)> {
    let ranks: Vec<_> = terms
        .iter()
        .map(|term| term_rank(spec, term))
        .collect::<Option<_>>()?;
    Some((
        ranks.iter().copied().max().unwrap_or(0),
        ranks.into_iter().map(u16::from).sum(),
    ))
}

#[derive(Clone, Debug)]
struct ArgumentEditor {
    spec: &'static CommandSpec,
    values: HashMap<&'static str, CommandValue>,
    scalar: HashMap<&'static str, String>,
    explicit: HashSet<&'static str>,
    active: usize,
    picker: Option<PickerList<CommandValue>>,
    picker_loading: bool,
}

impl ArgumentEditor {
    fn new(
        spec: &'static CommandSpec,
        supplied: &[CommandArgument],
        target: &PaletteTarget,
    ) -> Self {
        let mut values = HashMap::new();
        let mut scalar = HashMap::new();
        let mut explicit = HashSet::new();
        for argument in supplied {
            if let Some(declared) = spec.argument(&argument.name) {
                explicit.insert(declared.name);
                values.insert(declared.name, argument.value.clone());
                if let Some(text) = scalar_text(&argument.value) {
                    scalar.insert(declared.name, text);
                }
            }
        }
        for argument in spec.args {
            if explicit.contains(argument.name) {
                continue;
            }
            let value = match argument.kind {
                ArgumentKind::Session => {
                    target.session.map(CommandValue::Session)
                }
                ArgumentKind::Workspace => {
                    target.workspace.map(CommandValue::Workspace)
                }
                ArgumentKind::Tab => target.tab.map(CommandValue::Tab),
                _ => None,
            };
            if let Some(value) = value {
                values.insert(argument.name, value);
            }
        }
        let active = spec
            .args
            .iter()
            .position(|argument| !explicit.contains(argument.name))
            .unwrap_or(spec.args.len());
        Self {
            spec,
            values,
            scalar,
            explicit,
            active,
            picker: None,
            picker_loading: false,
        }
    }

    fn active_spec(&self) -> Option<&'static huterm_protocol::ArgumentSpec> {
        self.spec.args.get(self.active)
    }

    fn apply_target_defaults(&mut self, target: &PaletteTarget) {
        for argument in self.spec.args {
            if self.explicit.contains(argument.name)
                || self.values.contains_key(argument.name)
            {
                continue;
            }
            let value = match argument.kind {
                ArgumentKind::Session => {
                    target.session.map(CommandValue::Session)
                }
                ArgumentKind::Workspace => {
                    target.workspace.map(CommandValue::Workspace)
                }
                ArgumentKind::Tab => target.tab.map(CommandValue::Tab),
                _ => None,
            };
            if let Some(value) = value {
                self.values.insert(argument.name, value);
            }
        }
    }

    fn set_scalar(&mut self, text: &str) {
        if let Some(argument) = self.active_spec() {
            self.scalar.insert(argument.name, text.to_owned());
            self.values.remove(argument.name);
        }
    }

    fn set_active_value(&mut self, value: CommandValue) {
        if let Some(argument) = self.active_spec() {
            self.values.insert(argument.name, value);
        }
    }

    fn load_identity_picker(
        &mut self,
        hierarchy: &PaletteHierarchy,
        query: &str,
    ) {
        let Some(argument) = self.active_spec() else {
            return;
        };
        let mut picker = PickerList::new(hierarchy.items(argument.kind));
        if let Some(current) = self.values.get(argument.name) {
            picker.select_where(|value| value == current);
        }
        picker.filter(query);
        self.picker = Some(picker);
        self.picker_loading = false;
    }

    fn validate_active(&mut self) -> Result<(), CommandError> {
        let Some(argument) = self.active_spec() else {
            return Ok(());
        };
        if matches!(
            argument.kind,
            ArgumentKind::Session | ArgumentKind::Workspace | ArgumentKind::Tab
        ) {
            return if argument.required
                && !self.values.contains_key(argument.name)
            {
                Err(CommandError::MissingArgument {
                    command: self.spec.id,
                    name: argument.name,
                })
            } else {
                Ok(())
            };
        }
        let text = self.scalar.get(argument.name).map_or("", String::as_str);
        if text.trim().is_empty() {
            if argument.required {
                return Err(CommandError::MissingArgument {
                    command: self.spec.id,
                    name: argument.name,
                });
            }
            self.values.remove(argument.name);
            return Ok(());
        }
        let value = parse_scalar(self.spec.id, argument, text)?;
        self.values.insert(argument.name, value);
        Ok(())
    }

    fn advance(&mut self) -> Result<bool, CommandError> {
        self.validate_active()?;
        self.active = ((self.active + 1)..self.spec.args.len())
            .find(|index| !self.explicit.contains(self.spec.args[*index].name))
            .unwrap_or(self.spec.args.len());
        self.picker = None;
        self.picker_loading = false;
        Ok(self.active >= self.spec.args.len())
    }

    fn invocation(&self) -> Result<CommandInvocation, CommandError> {
        let args = self
            .spec
            .args
            .iter()
            .filter_map(|spec| {
                self.values
                    .get(spec.name)
                    .map(|value| CommandArgument::new(spec.name, value.clone()))
            })
            .collect();
        let invocation = CommandInvocation::new(self.spec.id, args);
        validate(&invocation)?;
        Ok(invocation)
    }
}

fn scalar_text(value: &CommandValue) -> Option<String> {
    match value {
        CommandValue::Bool(value) => Some(value.to_string()),
        CommandValue::Integer(value) => Some(value.to_string()),
        CommandValue::Text(value) => Some(value.clone()),
        _ => None,
    }
}

fn display_value(value: &CommandValue) -> String {
    scalar_text(value).unwrap_or_else(|| match value {
        CommandValue::Session(value) => format!("{value:?}"),
        CommandValue::Workspace(value) => format!("{value:?}"),
        CommandValue::Tab(value) => format!("{value:?}"),
        CommandValue::Bool(_)
        | CommandValue::Integer(_)
        | CommandValue::Text(_) => unreachable!("handled scalar value"),
    })
}

fn parse_scalar(
    command: CommandId,
    spec: &huterm_protocol::ArgumentSpec,
    text: &str,
) -> Result<CommandValue, CommandError> {
    match spec.kind {
        ArgumentKind::Bool => text
            .trim()
            .parse::<bool>()
            .map(CommandValue::Bool)
            .map_err(|_| CommandError::ArgumentType {
                command,
                name: spec.name,
                expected: spec.kind,
            }),
        ArgumentKind::Integer { min, max } => {
            let value = text.trim().parse::<i64>().map_err(|_| {
                CommandError::ArgumentType {
                    command,
                    name: spec.name,
                    expected: spec.kind,
                }
            })?;
            if !(min..=max).contains(&value) {
                return Err(CommandError::ArgumentRange {
                    command,
                    name: spec.name,
                    value,
                    min,
                    max,
                });
            }
            Ok(CommandValue::Integer(value))
        }
        ArgumentKind::Text => Ok(CommandValue::Text(text.to_owned())),
        _ => Err(CommandError::ArgumentType {
            command,
            name: spec.name,
            expected: spec.kind,
        }),
    }
}

#[derive(Clone, Debug)]
enum PaletteStage {
    Commands(CommandSearch),
    Arguments(ArgumentEditor),
}

#[derive(Clone, Debug)]
pub(super) enum PaletteEvent {
    Cancel,
    Execute(CommandInvocation),
}

pub(super) struct CommandPalette {
    pub(super) target: PaletteTarget,
    stage: PaletteStage,
    input: Entity<TextField>,
    hierarchy: Option<PaletteHierarchy>,
    availability: HashMap<CommandId, String>,
    keymap: InstalledKeymap,
    diagnostic: Option<String>,
    scroll: ScrollHandle,
    foreground: Hsla,
    background: Hsla,
    _subscriptions: Vec<Subscription>,
}

impl CommandPalette {
    pub(super) fn new_with_request(
        target: PaletteTarget,
        keymap: InstalledKeymap,
        availability: HashMap<CommandId, String>,
        foreground: Hsla,
        background: Hsla,
        request: Option<&CommandInvocation>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let input =
            cx.new(|cx| TextField::new("Type a command", foreground, cx));
        let subscription =
            cx.subscribe(&input, |palette, input, _: &Changed, cx| {
                let text = input.read(cx).text().to_owned();
                palette.input_changed(&text);
                palette.diagnostic = None;
                cx.notify();
            });
        let stage = request
            .and_then(|request| {
                huterm_protocol::lookup(request.id.as_str()).map(|spec| {
                    PaletteStage::Arguments(ArgumentEditor::new(
                        spec,
                        &request.args,
                        &target,
                    ))
                })
            })
            .unwrap_or_else(|| PaletteStage::Commands(CommandSearch::new()));
        let mut palette = Self {
            target,
            stage,
            input,
            hierarchy: None,
            availability,
            keymap,
            diagnostic: None,
            scroll: ScrollHandle::new(),
            foreground,
            background,
            _subscriptions: vec![subscription],
        };
        palette.prepare_active_picker(None);
        palette
    }

    pub(super) fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    pub(super) fn set_keymap(&mut self, keymap: InstalledKeymap) {
        self.keymap = keymap;
    }
    pub(super) fn set_availability(
        &mut self,
        availability: HashMap<CommandId, String>,
    ) {
        self.availability = availability;
    }
    pub(super) fn set_error(
        &mut self,
        error: impl Into<String>,
        cx: &mut Context<'_, Self>,
    ) {
        self.diagnostic = Some(error.into());
        cx.notify();
    }

    pub(super) fn set_hierarchy(
        &mut self,
        target: huterm_core::SelectionTarget,
        snapshot: &HierarchySnapshot,
        live_titles: &HashMap<TabId, String>,
        cx: &mut Context<'_, Self>,
    ) {
        let query = self.input.read(cx).text().to_owned();
        self.target.session = Some(target.session);
        self.target.workspace = target.workspace;
        self.target.tab = target.tab;
        self.hierarchy =
            Some(PaletteHierarchy::from_snapshot(snapshot, live_titles));
        if let PaletteStage::Arguments(editor) = &mut self.stage {
            editor.apply_target_defaults(&self.target);
        }
        self.prepare_active_picker(Some(&query));
        self.diagnostic = None;
        cx.notify();
    }

    fn input_changed(&mut self, text: &str) {
        match &mut self.stage {
            PaletteStage::Commands(search) => search.update(text),
            PaletteStage::Arguments(editor) => {
                if let Some(picker) = &mut editor.picker {
                    picker.filter(text);
                } else if !editor.active_spec().is_some_and(|argument| {
                    matches!(
                        argument.kind,
                        ArgumentKind::Session
                            | ArgumentKind::Workspace
                            | ArgumentKind::Tab
                    )
                }) {
                    editor.set_scalar(text);
                }
            }
        }
        self.scroll_to_selection();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        match &mut self.stage {
            PaletteStage::Commands(search) => search.move_selection(delta),
            PaletteStage::Arguments(editor) => {
                if let Some(picker) = &mut editor.picker {
                    picker.move_selection(delta);
                } else if let Some(argument) = editor.active_spec()
                    && matches!(
                        argument.kind,
                        ArgumentKind::Session
                            | ArgumentKind::Workspace
                            | ArgumentKind::Tab
                    )
                    && let Some(hierarchy) = &self.hierarchy
                {
                    let mut picker =
                        PickerList::new(hierarchy.items(argument.kind));
                    if let Some(current) = editor.values.get(argument.name) {
                        picker.select_where(|value| value == current);
                    }
                    picker.move_selection(delta);
                    editor.picker = Some(picker);
                    editor.picker_loading = false;
                } else if editor.active_spec().is_some_and(|argument| {
                    matches!(
                        argument.kind,
                        ArgumentKind::Session
                            | ArgumentKind::Workspace
                            | ArgumentKind::Tab
                    )
                }) {
                    editor.picker_loading = true;
                } else if matches!(
                    editor.active_spec().map(|spec| spec.kind),
                    Some(ArgumentKind::Bool)
                ) {
                    let current = editor
                        .scalar
                        .get(editor.active_spec().expect("active").name)
                        .is_some_and(|value| value == "true");
                    editor.set_scalar(if current { "false" } else { "true" });
                }
            }
        }
        self.scroll_to_selection();
        cx.notify();
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.move_selection(-1, cx);
    }
    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.move_selection(1, cx);
    }

    fn confirm(
        &mut self,
        _: &Confirm,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match &mut self.stage {
            PaletteStage::Commands(search) => {
                let Some(spec) = search.selected() else {
                    return;
                };
                if let Some(reason) =
                    search.selected_unavailable(&self.availability)
                {
                    self.diagnostic = Some(reason.to_owned());
                    cx.notify();
                    return;
                }
                if spec.args.is_empty() {
                    cx.emit(PaletteEvent::Execute(CommandInvocation::new(
                        spec.id,
                        Vec::new(),
                    )));
                    return;
                }
                let editor = ArgumentEditor::new(spec, &[], &self.target);
                self.stage = PaletteStage::Arguments(editor);
                self.prepare_active_picker(None);
                self.sync_input(window, cx);
            }
            PaletteStage::Arguments(editor) => {
                if editor.picker_loading {
                    self.diagnostic =
                        Some("Command targets are still loading".to_owned());
                    cx.notify();
                    return;
                }
                if let Some(picker) = &editor.picker {
                    let Some(item) = picker.selected() else {
                        self.diagnostic =
                            Some("No matching targets".to_owned());
                        cx.notify();
                        return;
                    };
                    editor.set_active_value(item.value.clone());
                    editor.picker = None;
                }
                match editor.advance().and_then(|complete| {
                    if complete {
                        editor.invocation().map(Some)
                    } else {
                        Ok(None)
                    }
                }) {
                    Ok(Some(invocation)) => {
                        cx.emit(PaletteEvent::Execute(invocation));
                    }
                    Ok(None) => {
                        self.prepare_active_picker(None);
                        self.sync_input(window, cx);
                    }
                    Err(error) => {
                        self.diagnostic = Some(error.to_string());
                        cx.notify();
                    }
                }
            }
        }
    }

    fn cancel(
        &mut self,
        _: &Cancel,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match &mut self.stage {
            PaletteStage::Commands(_) => cx.emit(PaletteEvent::Cancel),
            PaletteStage::Arguments(_) => {
                self.stage = PaletteStage::Commands(CommandSearch::new());
                self.sync_input(window, cx);
            }
        }
    }

    fn sync_input(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let text = match &self.stage {
            PaletteStage::Commands(search) => search.query.clone(),
            PaletteStage::Arguments(editor) => editor
                .active_spec()
                .and_then(|argument| editor.scalar.get(argument.name))
                .cloned()
                .unwrap_or_default(),
        };
        self.input.update(cx, |input, cx| input.set_text(text, cx));
        self.input.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    fn prepare_active_picker(&mut self, query: Option<&str>) {
        let PaletteStage::Arguments(editor) = &mut self.stage else {
            return;
        };
        let Some(argument) = editor.active_spec() else {
            return;
        };
        if !matches!(
            argument.kind,
            ArgumentKind::Session | ArgumentKind::Workspace | ArgumentKind::Tab
        ) {
            editor.picker = None;
            editor.picker_loading = false;
            return;
        }
        let Some(hierarchy) = &self.hierarchy else {
            editor.picker = None;
            editor.picker_loading = true;
            return;
        };
        editor.load_identity_picker(hierarchy, query.unwrap_or_default());
    }

    fn scroll_to_selection(&self) {
        let row = match &self.stage {
            PaletteStage::Commands(search) => search.selected,
            PaletteStage::Arguments(editor) => editor
                .picker
                .as_ref()
                .map_or(editor.active, PickerList::selected_index),
        };
        self.scroll.scroll_to_item(row);
    }

    fn shortcuts(
        &self,
        spec: &CommandSpec,
        invocation: Option<&CommandInvocation>,
    ) -> String {
        self.keymap
            .shortcuts(
                spec.id,
                &self.target.contexts,
                invocation.map(|value| value.args.as_slice()),
            )
            .iter()
            .take(3)
            .map(|binding| {
                if binding.args.is_empty() {
                    binding.key.clone()
                } else {
                    format!("{} ({})", binding.key, binding.description)
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
    }

    pub(super) fn smoke_state(&self, cx: &App) -> String {
        let stage = match &self.stage {
            PaletteStage::Commands(search) => format!(
                "commands selected={} query={:?} unavailable={:?}",
                search.selected().map_or("", |spec| spec.id.as_str()),
                search.query,
                search.selected_unavailable(&self.availability)
            ),
            PaletteStage::Arguments(editor) => format!(
                "arguments command={} active={} picker={} loading={}",
                editor.spec.id,
                editor
                    .active_spec()
                    .map_or("complete", |argument| argument.name),
                editor.picker.is_some(),
                editor.picker_loading
            ),
        };
        format!(
            "{stage} input={:?} diagnostic={:?}",
            self.input.read(cx).text(),
            self.diagnostic
        )
    }
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl Render for CommandPalette {
    #[expect(
        clippy::too_many_lines,
        reason = "the two palette stages share one small presentation tree"
    )]
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let foreground = self.foreground;
        let background = self.background;
        let mut list = div()
            .id("palette-results")
            .flex()
            .flex_col()
            .max_h(px(420.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        match &self.stage {
            PaletteStage::Commands(search) => {
                if search.filtered.is_empty() {
                    list =
                        list.child(div().p_3().child("No matching commands"));
                }
                for (row, index) in search.filtered.iter().enumerate() {
                    let spec = &catalog()[*index];
                    let unavailable = self.availability.get(&spec.id);
                    let shortcut = self.shortcuts(spec, None);
                    let selected = row == search.selected;
                    let mut item = div()
                        .id(("palette-command", row))
                        .p_2()
                        .flex()
                        .flex_col()
                        .border_b_1()
                        .border_color(foreground.opacity(0.12));
                    if selected {
                        item = item.bg(foreground.opacity(0.14));
                    }
                    item = item
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .child(if selected {
                                    format!("› {}", spec.title)
                                } else {
                                    format!("  {}", spec.title)
                                })
                                .child(shortcut),
                        )
                        .child(div().text_size(px(11.0)).opacity(0.72).child(
                            format!("{} · {:?}", spec.description, spec.scope),
                        ));
                    if let Some(reason) = unavailable {
                        item = item.child(
                            div()
                                .text_size(px(11.0))
                                .child(format!("Unavailable: {reason}")),
                        );
                    }
                    list = list.child(item);
                }
            }
            PaletteStage::Arguments(editor) => {
                let invocation = editor.invocation().ok();
                let shortcuts =
                    self.shortcuts(editor.spec, invocation.as_ref());
                list = list.child(
                    div()
                        .p_2()
                        .flex()
                        .justify_between()
                        .child(format!("{} arguments", editor.spec.title))
                        .child(shortcuts),
                );
                for (index, argument) in editor.spec.args.iter().enumerate() {
                    let value = editor
                        .values
                        .get(argument.name)
                        .map(display_value)
                        .or_else(|| editor.scalar.get(argument.name).cloned())
                        .unwrap_or_else(|| {
                            if index == editor.active {
                                "Editing…".into()
                            } else {
                                "Unset".into()
                            }
                        });
                    let mut item = div()
                        .p_2()
                        .flex()
                        .justify_between()
                        .child(argument.name)
                        .child(value);
                    if index == editor.active {
                        item = item.bg(foreground.opacity(0.14));
                    }
                    list = list.child(item);
                }
                if let Some(picker) = &editor.picker {
                    if picker.is_empty() {
                        list = list
                            .child(div().p_2().child("No matching targets"));
                    }
                    for (selected, item) in picker.visible() {
                        let mut row = div()
                            .p_2()
                            .flex()
                            .flex_col()
                            .child(item.label.clone())
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .opacity(0.72)
                                    .child(item.detail.clone()),
                            );
                        if selected {
                            row = row.bg(foreground.opacity(0.14));
                        }
                        list = list.child(row);
                    }
                } else if editor.picker_loading {
                    list = list.child(div().p_2().child("Loading targets…"));
                }
            }
        }
        let diagnostic = self.diagnostic.clone().map(|message| {
            div()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(foreground.opacity(0.25))
                .child(message)
        });
        let input_focus = self.focus_handle(cx);
        div()
            .absolute()
            .inset_0()
            .bg(background.opacity(0.84))
            .flex()
            .justify_center()
            .items_start()
            .pt(px(48.0))
            .key_context("Palette")
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                input_focus.focus(window);
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_scroll_wheel(|_: &ScrollWheelEvent, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .w(px(640.0))
                    .max_w_full()
                    .max_h(px(520.0))
                    .bg(background)
                    .border_1()
                    .border_color(foreground.opacity(0.4))
                    .rounded_md()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(self.input.clone())
                    .child(list)
                    .children(diagnostic),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use huterm_core::Mux;
    use huterm_protocol::{
        CellSize, GridSize, TerminalCommand, TerminalEngineKind,
    };

    fn target() -> PaletteTarget {
        PaletteTarget {
            session: None,
            workspace: None,
            tab: None,
            terminal: None,
            terminal_view: None,
            contexts: Vec::new(),
        }
    }

    fn terminal_command() -> TerminalCommand {
        TerminalCommand {
            engine: TerminalEngineKind::Alacritty,
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), "read value".into()],
            working_directory: std::env::current_dir().unwrap(),
            environment: Vec::new(),
            grid_size: GridSize::clamped(40, 8),
            cell_size: CellSize {
                width: 8,
                height: 16,
            },
        }
    }

    #[test]
    fn search_is_case_insensitive_multi_term_ranked_and_stable() {
        let mut search = CommandSearch::new();
        search.update("RENAME tab");
        assert_eq!(
            search.selected().map(|spec| spec.id),
            Some(ids::RENAME_TAB)
        );
        assert!(search.filtered.iter().all(|index| {
            let spec = &catalog()[*index];
            let text =
                format!("{} {} {}", spec.title, spec.id, spec.description)
                    .to_lowercase();
            text.contains("rename") && text.contains("tab")
        }));
        search.update("");
        let expected: Vec<_> = catalog()
            .iter()
            .filter(|spec| spec.id != ids::OPEN_COMMAND_PALETTE)
            .map(|spec| spec.id)
            .collect();
        let actual: Vec<_> = search
            .filtered
            .iter()
            .map(|index| catalog()[*index].id)
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn filtering_clamps_selection_and_handles_no_results() {
        let mut search = CommandSearch::new();
        search.move_selection(isize::MAX);
        search.update("rename session");
        assert_eq!(
            search.selected().map(|spec| spec.id),
            Some(ids::RENAME_SESSION)
        );
        search.update("no such command anywhere");
        assert!(search.selected().is_none());
    }

    #[test]
    fn unavailable_search_result_keeps_its_reason() {
        let mut search = CommandSearch::new();
        search.update("new tab");
        assert_eq!(search.selected().map(|spec| spec.id), Some(ids::NEW_TAB));
        let availability = HashMap::from([(
            ids::NEW_TAB,
            "structural operation in progress".to_owned(),
        )]);
        assert_eq!(
            search.selected_unavailable(&availability),
            Some("structural operation in progress")
        );
        assert!(
            search
                .filtered
                .iter()
                .any(|index| { catalog()[*index].id == ids::NEW_TAB })
        );
    }

    #[test]
    fn arguments_block_required_values_parse_ranges_and_keep_optional_omitted()
    {
        let target = target();
        let select = catalog()
            .iter()
            .find(|spec| spec.id == ids::SELECT_TAB)
            .unwrap();
        let mut editor = ArgumentEditor::new(select, &[], &target);
        assert!(matches!(
            editor.advance(),
            Err(CommandError::MissingArgument { .. })
        ));
        editor.set_scalar("10");
        assert!(matches!(
            editor.advance(),
            Err(CommandError::ArgumentRange { .. })
        ));
        editor.set_scalar("9");
        assert_eq!(editor.advance(), Ok(true));
        assert_eq!(editor.invocation().unwrap().integer("index"), Some(9));

        let quake = catalog()
            .iter()
            .find(|spec| spec.id == ids::SHOW_QUAKE)
            .unwrap();
        let mut optional = ArgumentEditor::new(quake, &[], &target);
        assert_eq!(optional.advance(), Ok(true));
        assert!(optional.invocation().unwrap().args.is_empty());
    }

    #[test]
    fn captured_identity_defaults_and_explicit_values_use_scoped_ids() {
        let target = PaletteTarget {
            session: Some(SessionId::in_runtime(
                huterm_protocol::RuntimeId::new(7),
                1,
            )),
            workspace: None,
            tab: Some(TabId::in_runtime(huterm_protocol::RuntimeId::new(7), 2)),
            terminal: None,
            terminal_view: None,
            contexts: Vec::new(),
        };
        let rename = catalog()
            .iter()
            .find(|spec| spec.id == ids::RENAME_TAB)
            .unwrap();
        let explicit = TabId::in_runtime(huterm_protocol::RuntimeId::new(7), 3);
        let editor = ArgumentEditor::new(
            rename,
            &[
                CommandArgument::new(
                    "name",
                    CommandValue::Text("chosen".into()),
                ),
                CommandArgument::new("tab", CommandValue::Tab(explicit)),
            ],
            &target,
        );
        assert!(editor.active_spec().is_none());
        assert_eq!(editor.invocation().unwrap().tab("tab"), Some(explicit));
    }

    #[test]
    fn delayed_identity_picker_filters_with_the_visible_query() {
        let runtime = huterm_protocol::RuntimeId::new(4);
        let first = TabId::in_runtime(runtime, 1);
        let second = TabId::in_runtime(runtime, 2);
        let rename = catalog()
            .iter()
            .find(|spec| spec.id == ids::RENAME_TAB)
            .unwrap();
        let mut editor = ArgumentEditor::new(
            rename,
            &[CommandArgument::new(
                "name",
                CommandValue::Text("renamed".into()),
            )],
            &PaletteTarget {
                tab: Some(first),
                ..target()
            },
        );
        editor.picker_loading = true;
        editor.load_identity_picker(
            &PaletteHierarchy {
                tabs: vec![
                    IdentityRow {
                        value: CommandValue::Tab(first),
                        label: "alpha".into(),
                        detail: "first".into(),
                    },
                    IdentityRow {
                        value: CommandValue::Tab(second),
                        label: "beta".into(),
                        detail: "second".into(),
                    },
                ],
                ..PaletteHierarchy::default()
            },
            "beta",
        );
        let labels: Vec<_> = editor
            .picker
            .as_ref()
            .unwrap()
            .visible()
            .map(|(_, item)| item.label.as_str())
            .collect();
        assert_eq!(labels, ["beta"]);
    }

    #[test]
    fn identity_replacement_and_duplicate_labels_keep_scoped_ids() {
        let mut mux = Mux::default();
        let first_session = mux.create_session(Some("duplicate")).unwrap();
        let second_session = mux.create_session(Some("duplicate")).unwrap();
        let first_workspace = mux
            .create_workspace(first_session, Some("duplicate"))
            .unwrap();
        let second_workspace = mux
            .create_workspace(second_session, Some("duplicate"))
            .unwrap();
        let hierarchy = PaletteHierarchy::from_snapshot(
            &mux.capture_hierarchy(),
            &HashMap::new(),
        );
        assert_eq!(hierarchy.sessions[0].label, hierarchy.sessions[1].label);
        assert_ne!(hierarchy.sessions[0].value, hierarchy.sessions[1].value);
        assert_eq!(
            hierarchy.workspaces[0].label,
            hierarchy.workspaces[1].label
        );
        assert_ne!(
            hierarchy.workspaces[0].value,
            hierarchy.workspaces[1].value
        );

        let rename = catalog()
            .iter()
            .find(|spec| spec.id == ids::RENAME_WORKSPACE)
            .unwrap();
        let target = PaletteTarget {
            session: Some(first_session),
            workspace: Some(first_workspace),
            ..target()
        };
        let mut editor = ArgumentEditor::new(rename, &[], &target);
        editor.set_scalar("chosen");
        assert_eq!(editor.advance(), Ok(false));
        editor.set_active_value(CommandValue::Workspace(second_workspace));
        assert_eq!(editor.advance(), Ok(true));
        let invocation = editor.invocation().unwrap();
        assert_eq!(
            invocation
                .args
                .iter()
                .map(|arg| arg.name.as_str())
                .collect::<Vec<_>>(),
            ["name", "workspace"]
        );
        assert_eq!(invocation.workspace("workspace"), Some(second_workspace));
    }

    #[test]
    fn palette_invocation_renames_exact_tab_and_stale_target_changes_nothing() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let first = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
        let second = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
        mux.rename_tab(first.id, Some("duplicate")).unwrap();
        mux.rename_tab(second.id, Some("duplicate")).unwrap();

        let rename = catalog()
            .iter()
            .find(|spec| spec.id == ids::RENAME_TAB)
            .unwrap();
        let target = PaletteTarget {
            session: Some(session),
            workspace: Some(workspace),
            tab: Some(second.id),
            ..target()
        };
        let mut editor = ArgumentEditor::new(rename, &[], &target);
        editor.set_scalar("chosen");
        assert_eq!(editor.advance(), Ok(false));
        assert_eq!(editor.advance(), Ok(true));
        let invocation = editor.invocation().unwrap();
        assert_eq!(
            huterm_core::execute(&mut mux, &invocation),
            Ok(huterm_protocol::CommandOutcome::Completed)
        );
        assert_eq!(mux.tab(first.id).unwrap().custom_name(), Some("duplicate"));
        assert_eq!(mux.tab(second.id).unwrap().custom_name(), Some("chosen"));

        mux.close_tab(workspace, second.id).unwrap();
        assert_eq!(
            huterm_core::execute(&mut mux, &invocation),
            Err(CommandError::StaleTarget)
        );
        assert_eq!(mux.tab(first.id).unwrap().custom_name(), Some("duplicate"));
        mux.close_session(session).unwrap();
    }
}
