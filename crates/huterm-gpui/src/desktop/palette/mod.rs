//! Command-palette workflow and its window-independent state model.
//!
//! The palette has two stages. Command search ranks the catalog with the
//! fuzzy matcher and usage history. The slot stage collects arguments one
//! active slot at a time, showing everything else as chips. Every keyboard
//! operation arrives as a `Palette`-scope catalog command through
//! [`CommandPalette::run`]. The results and an identity slot's picker rows
//! draw in a `huterm-ui` `Command` under the search field: the palette
//! ranks or filters the rows, and the list keeps their highlight.

// User input picks positions here, so a stale index must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

mod search;
mod slots;

pub(super) use search::{CommandFrequency, HistoryView, RecentCommands};

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, BoxShadow, ClickEvent, Context, Entity, EventEmitter,
    Focusable, FontWeight, HighlightStyle, Hsla, KeyBindingContextPredicate,
    KeyContext, MouseButton, MouseDownEvent, MouseUpEvent, Render,
    ScrollWheelEvent, Subscription, WeakEntity, Window, div, hsla, point,
    prelude::*, px,
};
use huterm_config::PalettePlacement;
use huterm_protocol::{
    ArgumentKind, CommandArgument, CommandError, CommandId, CommandInvocation,
    CommandOutcome, CommandScope, CommandSpec, CommandValue, HierarchyState,
    NamedLayout, SIDES, SessionId, TabId, TerminalId, WorkspaceId, catalog,
    ids,
};
use search::{CommandHistory, CommandMatch, CommandSearch, PickerMatcher};
use slots::{
    Commit, Exit, Slot, SlotDomain, SlotEditor, SlotState, is_identity,
};

use super::TerminalView;
use super::key_hint::KeyHint;
use super::overlay::{OverlayColors, Swatch, footer_hints, key_cap_on};
use super::refresh::FrameClock;
use crate::keymap::{InstalledKeymap, Platform};
use huterm_ui::components::command::{
    Command, CommandEvent, HighlightFallback,
};
use huterm_ui::components::input::{Changed, Chrome, Input};
use huterm_ui::listbox::ItemId;
use huterm_ui::option_list::ListOption;

/// The text-field commands the palette and the Licenses view pass to their
/// search field.
pub(super) const FORWARDED_TEXT_COMMANDS: &[CommandId] = &[
    ids::TEXT_DELETE_BACKWARD,
    ids::TEXT_DELETE_FORWARD,
    ids::TEXT_DELETE_WORD_BACKWARD,
    ids::TEXT_DELETE_WORD_FORWARD,
    ids::TEXT_DELETE_LINE_START,
    ids::TEXT_MOVE_LEFT,
    ids::TEXT_MOVE_RIGHT,
    ids::TEXT_MOVE_WORD_LEFT,
    ids::TEXT_MOVE_WORD_RIGHT,
    ids::TEXT_LINE_START,
    ids::TEXT_LINE_END,
    ids::TEXT_SELECT_ALL,
    ids::TEXT_CUT,
    ids::TEXT_COPY,
    ids::TEXT_PASTE,
];

#[derive(Clone)]
pub(super) struct PaletteTarget {
    pub(super) session: Option<SessionId>,
    pub(super) workspace: Option<WorkspaceId>,
    pub(super) tab: Option<TabId>,
    pub(super) terminal: Option<TerminalId>,
    pub(super) terminal_view: Option<WeakEntity<TerminalView>>,
    pub(super) contexts: Vec<KeyContext>,
}

/// The window's current session, workspace, active tab, and that tab's
/// terminal, which an open palette's window-scoped pickers, identity
/// defaults, and Terminal-scope commands follow.
#[derive(Clone)]
pub(super) struct PaletteScope {
    pub(super) session: Option<SessionId>,
    pub(super) workspace: Option<WorkspaceId>,
    pub(super) tab: Option<TabId>,
    pub(super) terminal: Option<TerminalId>,
    pub(super) terminal_view: Option<WeakEntity<TerminalView>>,
}

impl PaletteTarget {
    /// Moves the target to the window's current scope; returns whether it
    /// changed. Committed and prefilled slot values keep what they hold.
    fn rescope(&mut self, scope: PaletteScope) -> bool {
        if self.session == scope.session
            && self.workspace == scope.workspace
            && self.tab == scope.tab
            && self.terminal == scope.terminal
            && self.terminal_view == scope.terminal_view
        {
            return false;
        }
        self.session = scope.session;
        self.workspace = scope.workspace;
        self.tab = scope.tab;
        self.terminal = scope.terminal;
        self.terminal_view = scope.terminal_view;
        true
    }
}

/// Height of one result or picker row in points. Fixed so the list's
/// maximum height is a whole number of rows and native smokes can address
/// rows by position.
const ROW_HEIGHT: f32 = 54.0;
/// Rows visible before the list scrolls.
const VISIBLE_ROWS: f32 = 6.0;
/// The space `Command` keeps between its list's edge and its rows.
const LIST_PADDING: f32 = 4.0;
const LIST_MAX_HEIGHT: f32 = ROW_HEIGHT * VISIBLE_ROWS + LIST_PADDING * 2.0;
/// The panel at its tallest: input line, full list, and footer. Centred
/// placement positions this height so filtering never moves the input.
const PANEL_MAX_HEIGHT: f32 = 408.0;
const PANEL_CHROME_HEIGHT: f32 = PANEL_MAX_HEIGHT - LIST_MAX_HEIGHT;
/// Distance from the window's top edge for top placement, and the minimum
/// for centred placement in short windows.
const PANEL_TOP_INSET: f32 = 36.0;

/// One configured quake profile for the profile picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct QuakeProfileRow {
    pub(super) name: String,
    pub(super) detail: String,
}

/// Usage history captured when the palette opens.
#[derive(Clone, Debug, Default)]
pub(super) struct OwnedHistory {
    recency: HashMap<CommandId, usize>,
    frequency: HashMap<CommandId, u32>,
}

impl OwnedHistory {
    pub(super) fn capture(history: &HistoryView<'_>) -> Self {
        let mut owned = Self::default();
        for spec in catalog() {
            if let Some(position) = history.recency(spec.id) {
                owned.recency.insert(spec.id, position);
            }
            let count = history.frequency(spec.id);
            if count > 0 {
                owned.frequency.insert(spec.id, count);
            }
        }
        owned
    }
}

impl CommandHistory for OwnedHistory {
    fn recency(&self, id: CommandId) -> Option<usize> {
        self.recency.get(&id).copied()
    }
    fn frequency(&self, id: CommandId) -> u32 {
        self.frequency.get(&id).copied().unwrap_or(0)
    }
}

/// Everything the window supplies when it opens a palette.
pub(super) struct PaletteOpen<'a> {
    pub(super) target: PaletteTarget,
    pub(super) keymap: InstalledKeymap,
    pub(super) availability: HashMap<CommandId, String>,
    /// Row titles that extend a command's title, such as "Undo Move Pane"
    /// for `undo`. Each starts with the command's own title, so search
    /// highlights still land on it.
    pub(super) titles: HashMap<CommandId, String>,
    pub(super) colors: OverlayColors,
    pub(super) placement: PalettePlacement,
    pub(super) history: OwnedHistory,
    pub(super) profiles: Vec<QuakeProfileRow>,
    /// Every layout `arrange_panes` can name, in picker order.
    pub(super) layouts: Vec<NamedLayout>,
    /// A command invoked interactively without its required arguments.
    pub(super) request: Option<&'a CommandInvocation>,
    /// A query retained from a recent cancellation.
    pub(super) retained_query: Option<String>,
    /// Identity rows built from the window's hierarchy projection.
    pub(super) hierarchy: PaletteHierarchy,
    /// The projection sequence `hierarchy` reflects.
    pub(super) hierarchy_seq: u64,
    /// The window's frame clock, which fades the list's scroll indicator.
    pub(super) clock: Rc<FrameClock>,
}

#[derive(Clone, Debug, PartialEq)]
struct IdentityRow {
    value: CommandValue,
    label: String,
    detail: String,
    /// One-based tab position, for `select_tab` shortcut key caps.
    position: Option<i64>,
    /// Owning workspace, so window-scoped commands list only their own tabs.
    workspace: Option<WorkspaceId>,
    /// A pane's terminal, so a move's pane picker can leave out the pane
    /// being moved.
    terminal: Option<TerminalId>,
    /// A pane's tab, so a tab move's pane picker can leave out the tab's
    /// own panes.
    tab: Option<TabId>,
    custom_name: bool,
}

fn profile_rows(profiles: Vec<QuakeProfileRow>) -> Vec<IdentityRow> {
    profiles
        .into_iter()
        .map(|row| IdentityRow {
            value: CommandValue::Text(row.name.clone()),
            label: row.name,
            detail: row.detail,
            position: None,
            workspace: None,
            terminal: None,
            tab: None,
            custom_name: false,
        })
        .collect()
}

/// The rows a side picker offers, in [`SIDES`] order.
static SIDE_ROWS: std::sync::LazyLock<Vec<IdentityRow>> =
    std::sync::LazyLock::new(side_rows);

fn side_rows() -> Vec<IdentityRow> {
    SIDES
        .iter()
        .map(|side| IdentityRow {
            value: CommandValue::Text((*side).to_owned()),
            label: capitalize(side),
            detail: "side".to_owned(),
            position: None,
            workspace: None,
            terminal: None,
            tab: None,
            custom_name: false,
        })
        .collect()
}

/// The rows a layout picker offers for `layouts`, by title, with their
/// templates as detail.
fn layout_rows(layouts: &[NamedLayout]) -> Vec<IdentityRow> {
    layouts
        .iter()
        .map(|layout| IdentityRow {
            value: CommandValue::Text(layout.name.clone()),
            label: layout.title.clone(),
            detail: layout.template.to_string(),
            position: None,
            workspace: None,
            terminal: None,
            tab: None,
            custom_name: false,
        })
        .collect()
}

/// Session, workspace, tab, and pane rows for identity pickers.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PaletteHierarchy {
    sessions: Vec<IdentityRow>,
    workspaces: Vec<IdentityRow>,
    tabs: Vec<IdentityRow>,
    panes: Vec<IdentityRow>,
}

impl PaletteHierarchy {
    /// Builds rows from a hierarchy projection with panes labelled by
    /// their launched program names; see [`Self::from_projection_with`].
    #[cfg(test)]
    pub(super) fn from_projection<'t>(
        state: &HierarchyState,
        title: impl Fn(TabId) -> Option<&'t str>,
        tab_order: &[TabId],
    ) -> Self {
        Self::from_projection_with(state, title, |_| None, tab_order)
    }

    /// Builds rows from a hierarchy projection, in session, workspace, and
    /// tab order. A tab is labelled by its custom name, then `title`, the
    /// title its window published, then its fallback name; a pane by
    /// `pane_title`, the title a window published for its terminal, then
    /// its fallback name. Tabs follow `tab_order`, most recently used
    /// first, and tabs it omits keep their structural order after the
    /// ordered ones; panes follow their tabs, in traversal order within
    /// each.
    pub(super) fn from_projection_with<'t>(
        state: &HierarchyState,
        title: impl Fn(TabId) -> Option<&'t str>,
        pane_title: impl Fn(TerminalId) -> Option<&'t str>,
        tab_order: &[TabId],
    ) -> Self {
        let mut sessions = Vec::new();
        let mut workspaces = Vec::new();
        let mut tabs = Vec::new();
        let mut panes = Vec::new();
        for &session_id in state.sessions() {
            let Some(session) = state.session(session_id) else {
                continue;
            };
            sessions.push(IdentityRow {
                value: CommandValue::Session(session_id),
                label: session.display_name().to_owned(),
                detail: "session".to_owned(),
                position: None,
                workspace: None,
                terminal: None,
                tab: None,
                custom_name: session.custom_name.is_some(),
            });
            for &workspace_id in
                state.session_workspaces(session_id).unwrap_or_default()
            {
                let Some(workspace) = state.workspace(workspace_id) else {
                    continue;
                };
                workspaces.push(IdentityRow {
                    value: CommandValue::Workspace(workspace_id),
                    label: workspace.display_name().to_owned(),
                    detail: session.display_name().to_owned(),
                    position: None,
                    workspace: Some(workspace_id),
                    terminal: None,
                    tab: None,
                    custom_name: workspace.custom_name.is_some(),
                });
                let members =
                    state.workspace_tabs(workspace_id).unwrap_or_default();
                for (index, &tab_id) in members.iter().enumerate() {
                    let Some(tab) = state.tab(tab_id) else {
                        continue;
                    };
                    tabs.push(IdentityRow {
                        value: CommandValue::Tab(tab_id),
                        label: tab
                            .custom_name
                            .as_deref()
                            .or_else(|| title(tab_id))
                            .unwrap_or(tab.fallback_name())
                            .to_owned(),
                        detail: format!(
                            "{} › {}",
                            session.display_name(),
                            workspace.display_name(),
                        ),
                        position: i64::try_from(index + 1).ok(),
                        workspace: Some(workspace_id),
                        terminal: None,
                        tab: None,
                        custom_name: tab.custom_name.is_some(),
                    });
                    let tab_label =
                        tabs.last().map_or("", |row: &IdentityRow| &row.label);
                    for pane in tab.layout.panes() {
                        panes.push(IdentityRow {
                            value: CommandValue::Pane(pane.id),
                            label: pane_title(pane.terminal_id)
                                .unwrap_or(&pane.fallback_name)
                                .to_owned(),
                            detail: format!(
                                "{} › {} › {tab_label}",
                                session.display_name(),
                                workspace.display_name(),
                            ),
                            position: None,
                            workspace: Some(workspace_id),
                            terminal: Some(pane.terminal_id),
                            tab: Some(tab_id),
                            custom_name: false,
                        });
                    }
                }
            }
        }
        // Most recently used first; tabs the window never activated keep
        // their structural order after the ordered ones, and panes follow
        // their tabs.
        let rank = |value: &CommandValue| {
            let tab = match value {
                CommandValue::Tab(id) => Some(*id),
                CommandValue::Pane(id) => state.pane_tab(*id),
                _ => None,
            };
            tab.and_then(|tab| tab_order.iter().position(|id| *id == tab))
                .unwrap_or(usize::MAX)
        };
        tabs.sort_by_key(|row| rank(&row.value));
        panes.sort_by_key(|row| rank(&row.value));
        Self {
            sessions,
            workspaces,
            tabs,
            panes,
        }
    }

    /// Whether the workspace domain lists `workspace`.
    pub(super) fn lists_workspace(&self, workspace: WorkspaceId) -> bool {
        self.workspaces
            .iter()
            .any(|row| row.value == CommandValue::Workspace(workspace))
    }

    fn rows(&self, kind: ArgumentKind) -> &[IdentityRow] {
        match kind {
            ArgumentKind::Session => &self.sessions,
            ArgumentKind::Workspace => &self.workspaces,
            ArgumentKind::Tab => &self.tabs,
            ArgumentKind::Pane => &self.panes,
            _ => &[],
        }
    }
}

/// The slot editor's view of identity domains: the hierarchy projection for
/// sessions, workspaces, and tabs; configured profiles for quake commands.
#[derive(Clone, Copy)]
struct DomainView<'a> {
    hierarchy: &'a PaletteHierarchy,
    profiles: &'a [IdentityRow],
    layouts: &'a [IdentityRow],
    target: &'a PaletteTarget,
    /// Window-scoped commands can only act on this window's tabs.
    window_only: bool,
    /// The tab a tab move moves, whose own panes are no destination.
    moving_tab: Option<TabId>,
}

impl DomainView<'_> {
    /// Rows for `kind` in domain order.
    fn rows(&self, kind: ArgumentKind) -> Vec<&IdentityRow> {
        match kind {
            ArgumentKind::QuakeProfile => self.profiles.iter().collect(),
            ArgumentKind::Side => SIDE_ROWS.iter().collect(),
            ArgumentKind::Layout => self.layouts.iter().collect(),
            // A pane is never moved beside itself, nor a tab into one of
            // its own panes.
            ArgumentKind::Pane => self
                .hierarchy
                .rows(kind)
                .iter()
                .filter(|row| match self.moving_tab {
                    Some(tab) => row.tab != Some(tab),
                    None => row.terminal != self.target.terminal,
                })
                .collect(),
            ArgumentKind::Session
            | ArgumentKind::Workspace
            | ArgumentKind::Tab => self
                .hierarchy
                .rows(kind)
                .iter()
                .filter(|row| {
                    !(self.window_only && kind == ArgumentKind::Tab)
                        || row.workspace == self.target.workspace
                })
                .collect(),
            ArgumentKind::Bool
            | ArgumentKind::Integer { .. }
            | ArgumentKind::Text => Vec::new(),
        }
    }

    fn label(
        &self,
        kind: ArgumentKind,
        value: &CommandValue,
    ) -> Option<String> {
        self.rows(kind)
            .into_iter()
            .find(|row| &row.value == value)
            .map(|row| row.label.clone())
    }
}

impl SlotDomain for DomainView<'_> {
    fn values(&self, kind: ArgumentKind) -> Vec<CommandValue> {
        self.rows(kind)
            .into_iter()
            .map(|row| row.value.clone())
            .collect()
    }

    fn default(&self, kind: ArgumentKind) -> Option<CommandValue> {
        match kind {
            ArgumentKind::Session => {
                self.target.session.map(CommandValue::Session)
            }
            ArgumentKind::Workspace => {
                self.target.workspace.map(CommandValue::Workspace)
            }
            ArgumentKind::Tab => self.target.tab.map(CommandValue::Tab),
            ArgumentKind::QuakeProfile => {
                Some(CommandValue::Text("default".to_owned()))
            }
            ArgumentKind::Pane
            | ArgumentKind::Side
            | ArgumentKind::Layout
            | ArgumentKind::Bool
            | ArgumentKind::Integer { .. }
            | ArgumentKind::Text => None,
        }
    }
}

#[derive(Clone, Debug)]
struct SearchState {
    query: String,
    results: Vec<CommandMatch>,
}

enum Stage {
    Search(SearchState),
    Slots(SlotEditor),
}

/// The id a picker row's `value` takes in the list: unique within a
/// domain, so the list keeps its highlight on the same value through
/// filtering and rebuilds.
fn value_id(value: &CommandValue) -> ItemId {
    format!("{value:?}").into()
}

/// Whether a name slot's text is the automatic prefill or the user's own.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum NameText {
    /// The current custom name of the rename's target, or empty; rebuilds
    /// refresh it.
    #[default]
    Automatic,
    /// Typed, edited, or committed by the user, including an edited blank;
    /// rebuilds never touch it.
    Edited,
}

/// The name slot's mode when the slot is entered or redrawn. A committed
/// value or typed text is the user's; an empty, uncommitted slot keeps its
/// mode, so a name the user deliberately blanked stays blank, and Enter
/// still clears it, instead of being prefilled again.
fn entered_name_mode(mode: NameText, committed: bool, text: &str) -> NameText {
    if committed || !text.is_empty() {
        NameText::Edited
    } else {
        mode
    }
}

/// The text a rebuild writes into the name slot, if any: automatic text
/// follows the target's current custom name; edited text stays.
fn refreshed_name(
    mode: NameText,
    text: &str,
    current: Option<&str>,
) -> Option<String> {
    let current = current.unwrap_or_default();
    (mode == NameText::Automatic && text != current).then(|| current.to_owned())
}

#[derive(Clone, Debug)]
pub(super) enum PaletteEvent {
    /// The palette closed without running anything. Carries the search
    /// query to retain when cancellation happened in command search.
    Cancel {
        query: Option<String>,
    },
    Execute(CommandInvocation),
}

pub(super) struct CommandPalette {
    pub(super) target: PaletteTarget,
    stage: Stage,
    /// The search query to restore when leaving slots entered from search.
    return_query: Option<String>,
    input: Entity<Input>,
    engine: CommandSearch,
    matcher: PickerMatcher,
    history: OwnedHistory,
    listed: Vec<&'static CommandSpec>,
    hierarchy: PaletteHierarchy,
    /// The projection sequence `hierarchy` reflects.
    hierarchy_seq: u64,
    profiles: Vec<IdentityRow>,
    layouts: Vec<IdentityRow>,
    /// Why each unavailable command is unavailable.
    pub(super) availability: HashMap<CommandId, String>,
    /// Row titles that extend a command's own, as [`PaletteOpen`] gives
    /// them.
    pub(super) titles: HashMap<CommandId, String>,
    keymap: InstalledKeymap,
    colors: OverlayColors,
    placement: PalettePlacement,
    diagnostic: Option<String>,
    /// The active identity slot's filtered rows, each with its id in the
    /// list and its index among the domain's rows.
    picker: Vec<(ItemId, usize)>,
    /// The list under the line: the command results, or the active
    /// identity slot's picker rows. A slot that takes text has none.
    list: Option<Entity<Command>>,
    /// Whether a rebuild removed the picker's highlighted row. Nothing is
    /// then highlighted, and Enter does nothing, until the user moves the
    /// highlight or filters; a later rebuild leaves it that way.
    highlight_lost: bool,
    /// Whether the name slot's text is automatic or the user's.
    name_text: NameText,
    clock: Rc<FrameClock>,
    /// Monotonic acknowledgement for native smoke-test wheel input.
    wheel_events: u64,
    _subscriptions: Vec<Subscription>,
}

impl CommandPalette {
    pub(super) fn open(
        open: PaletteOpen<'_>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let PaletteOpen {
            target,
            keymap,
            availability,
            titles,
            colors,
            placement,
            history,
            profiles,
            layouts,
            request,
            retained_query,
            hierarchy,
            hierarchy_seq,
            clock,
        } = open;
        let input = cx.new(|cx| {
            Input::new(
                "palette-input",
                "Command palette",
                Swatch::new(colors),
                cx,
            )
            .with_placeholder("Search commands")
            .with_chrome(Chrome::Bare)
        });
        let subscription =
            cx.subscribe(&input, |palette, input, _: &Changed, cx| {
                let text = input.read(cx).text().to_owned();
                let unchanged = match &palette.stage {
                    Stage::Search(search) => search.query == text,
                    Stage::Slots(editor) => editor.text() == text,
                };
                if unchanged {
                    return;
                }
                palette.input_changed(&text, cx);
                cx.notify();
            });
        let listed = catalog()
            .iter()
            .filter(|spec| context_matches(spec.context, &target.contexts))
            .collect();
        let mut palette = Self {
            target,
            stage: Stage::Search(SearchState {
                query: String::new(),
                results: Vec::new(),
            }),
            return_query: None,
            input,
            engine: CommandSearch::new(),
            matcher: PickerMatcher::new(),
            history,
            listed,
            hierarchy,
            hierarchy_seq,
            profiles: profile_rows(profiles),
            layouts: layout_rows(&layouts),
            availability,
            titles,
            keymap,
            colors,
            placement,
            diagnostic: None,
            picker: Vec::new(),
            list: None,
            highlight_lost: false,
            name_text: NameText::Automatic,
            clock,
            wheel_events: 0,
            _subscriptions: vec![subscription],
        };
        let editor = request.and_then(|request| {
            let spec = huterm_protocol::lookup(request.id.as_str())?;
            let domain = palette.domain_for(spec, &request.args);
            SlotEditor::new(spec, &request.args, &domain, true)
        });
        if let Some(editor) = editor {
            palette.enter_slots(editor, cx);
            return palette;
        }
        // A request with nothing to prompt opens on search, as none does;
        // the window retains no query for a request.
        let query = retained_query.unwrap_or_default();
        palette.rank(&query);
        palette.input.update(cx, |input, cx| {
            input.set_text(query, cx);
            input.select_all(cx);
        });
        palette.mount_list(cx);
        palette
    }

    pub(super) fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    pub(super) fn cancellation_query(&self) -> Option<String> {
        match &self.stage {
            Stage::Search(search) if !search.query.is_empty() => {
                Some(search.query.clone())
            }
            Stage::Search(_) | Stage::Slots(_) => None,
        }
    }

    pub(super) fn set_keymap(&mut self, keymap: InstalledKeymap) {
        self.keymap = keymap;
    }

    pub(super) fn set_availability(
        &mut self,
        availability: HashMap<CommandId, String>,
        titles: HashMap<CommandId, String>,
    ) {
        self.availability = availability;
        self.titles = titles;
    }

    /// Applies reloaded theme colours, placement, profiles, and layouts to
    /// an open palette.
    pub(super) fn set_presentation(
        &mut self,
        colors: OverlayColors,
        placement: PalettePlacement,
        profiles: Vec<QuakeProfileRow>,
        layouts: &[NamedLayout],
        cx: &mut Context<'_, Self>,
    ) {
        self.colors = colors;
        self.placement = placement;
        self.profiles = profile_rows(profiles);
        self.layouts = layout_rows(layouts);
        self.input.update(cx, |input, cx| {
            input.set_tokens(Swatch::new(colors), cx);
        });
        if let Some(list) = &self.list {
            list.update(cx, |list, cx| {
                list.set_tokens(Swatch::new(colors), cx);
            });
        }
        self.rebuild_picker_rows(cx);
        cx.notify();
    }

    pub(super) fn set_error(
        &mut self,
        error: impl Into<String>,
        cx: &mut Context<'_, Self>,
    ) {
        self.diagnostic = Some(error.into());
        cx.notify();
    }

    /// Replaces the identity rows from a newer projection. Rows that did not
    /// change notify nothing. The highlighted value stays when it is still
    /// listed and is otherwise lost, committed slot values stay for the
    /// executor to validate, and automatic name text follows its target.
    pub(super) fn set_hierarchy(
        &mut self,
        hierarchy: PaletteHierarchy,
        scope: PaletteScope,
        seq: u64,
        cx: &mut Context<'_, Self>,
    ) {
        self.hierarchy_seq = seq;
        // The window's active tab or workspace may have changed with the
        // rebuild; window-scoped pickers and later defaults follow it.
        let rescoped = self.target.rescope(scope);
        if hierarchy == self.hierarchy && !rescoped {
            return;
        }
        self.hierarchy = hierarchy;
        self.rebuild_picker_rows(cx);
        self.refresh_automatic_name(cx);
        self.refresh_placeholder(cx);
        cx.notify();
    }

    /// Updates the slot placeholder, which can name the rename target, when
    /// a rebuild changed it.
    fn refresh_placeholder(&mut self, cx: &mut Context<'_, Self>) {
        let Stage::Slots(editor) = &self.stage else {
            return;
        };
        let placeholder = self.placeholder(editor);
        self.input.update(cx, |input, cx| {
            if input.placeholder() != placeholder {
                input.set_placeholder(placeholder);
                cx.notify();
            }
        });
    }

    /// The projection sequence the identity rows reflect.
    pub(super) fn hierarchy_seq(&self) -> u64 {
        self.hierarchy_seq
    }

    /// Whether the workspace picker domain lists `workspace`.
    pub(super) fn lists_workspace(&self, workspace: WorkspaceId) -> bool {
        self.hierarchy.lists_workspace(workspace)
    }

    fn domain_for(
        &self,
        spec: &CommandSpec,
        args: &[CommandArgument],
    ) -> DomainView<'_> {
        let tab = args.iter().find_map(|arg| match arg.value {
            CommandValue::Tab(tab) if arg.name == "tab" => Some(tab),
            _ => None,
        });
        DomainView {
            hierarchy: &self.hierarchy,
            profiles: &self.profiles,
            layouts: &self.layouts,
            target: &self.target,
            window_only: spec.scope == CommandScope::Window,
            moving_tab: self.moving_tab(spec, tab),
        }
    }

    /// The tab `spec` moves when it is a tab move: its `tab` argument, else
    /// the active tab.
    fn moving_tab(
        &self,
        spec: &CommandSpec,
        tab: Option<TabId>,
    ) -> Option<TabId> {
        (spec.id == ids::MOVE_TAB_TO_PANE)
            .then(|| tab.or(self.target.tab))
            .flatten()
    }

    /// Whether the slot stage's command may only target this window.
    fn window_only(&self) -> bool {
        matches!(
            &self.stage,
            Stage::Slots(editor) if editor.spec().scope == CommandScope::Window
        )
    }

    /// The domain for the slot stage's command.
    fn domain(&self) -> DomainView<'_> {
        let moving_tab = match &self.stage {
            Stage::Slots(editor) => {
                let tab =
                    editor.slots().iter().find_map(|slot| match slot.value {
                        Some(CommandValue::Tab(tab))
                            if slot.spec.name == "tab" =>
                        {
                            Some(tab)
                        }
                        _ => None,
                    });
                self.moving_tab(editor.spec(), tab)
            }
            Stage::Search(_) => None,
        };
        DomainView {
            hierarchy: &self.hierarchy,
            profiles: &self.profiles,
            layouts: &self.layouts,
            target: &self.target,
            window_only: self.window_only(),
            moving_tab,
        }
    }

    fn rank(&mut self, query: &str) {
        let results =
            self.engine
                .rank(query, self.listed.iter().copied(), &self.history);
        self.stage = Stage::Search(SearchState {
            query: query.to_owned(),
            results,
        });
        self.picker.clear();
    }

    /// Mounts the list the stage shows, named for it, and fills it: the
    /// results, or the active identity slot's picker rows. A slot that
    /// takes text has no list, so its field reports none.
    fn mount_list(&mut self, cx: &mut Context<'_, Self>) {
        let (label, fallback) = match &self.stage {
            Stage::Search(_) => {
                ("Commands".to_owned(), HighlightFallback::First)
            }
            Stage::Slots(editor) if is_identity(editor.active().spec.kind) => {
                // Enter must never reach a row other than the one the user
                // highlighted.
                (
                    capitalize(editor.active().spec.name),
                    HighlightFallback::Clear,
                )
            }
            Stage::Slots(_) => {
                self.list = None;
                self.input.update(cx, |input, cx| {
                    input.set_list_state(None, cx);
                });
                return;
            }
        };
        let palette = cx.weak_entity();
        let input = self.input.clone();
        let tokens = Swatch::new(self.colors);
        let clock = Rc::clone(&self.clock);
        let list = cx.new(|cx| {
            clock.observe(cx);
            // The pointer only hovers, so Enter runs the row the keys
            // highlighted, not whichever row the pointer last crossed.
            Command::new("palette-list", label, &input, tokens, cx)
                .with_pointer_highlight(false)
                .with_highlight_fallback(fallback)
                .with_row_height(px(ROW_HEIGHT))
                .with_content(Rc::new(
                    move |option: &ListOption,
                          highlighted,
                          window: &mut Window,
                          cx: &mut App| {
                        palette.upgrade().map_or_else(
                            || div().into_any_element(),
                            |palette| {
                                palette.read(cx).row(
                                    option,
                                    highlighted,
                                    window,
                                )
                            },
                        )
                    },
                ))
        });
        // The subscription ends with the list it watches. A list the stage
        // replaced can still report a press on the frame it last drew, which
        // must not act on its successor.
        cx.subscribe(&list, |palette, list, event, cx| {
            if palette.list.as_ref() != Some(&list) {
                return;
            }
            match event {
                CommandEvent::Highlighted(_) => cx.notify(),
                // The list highlights a clicked row before it reports it.
                CommandEvent::Picked(_) => match palette.stage {
                    Stage::Search(_) => palette.confirm_search(cx),
                    Stage::Slots(_) => palette.commit_slot(cx),
                },
            }
        })
        .detach();
        self.list = Some(list);
        match self.stage {
            Stage::Search(_) => self.show_results(cx),
            Stage::Slots(_) => self.refresh_picker_rows(cx),
        }
    }

    /// Puts the ranked results in the list, highlighting the first.
    fn show_results(&mut self, cx: &mut Context<'_, Self>) {
        let (Stage::Search(search), Some(list)) = (&self.stage, &self.list)
        else {
            return;
        };
        let options = search
            .results
            .iter()
            .map(|matched| {
                ListOption::new(matched.spec.id.as_str(), self.title(matched))
            })
            .collect();
        let empty = format!("No commands match “{}”", search.query);
        list.update(cx, |list, cx| {
            list.set_empty(empty, cx);
            list.set_options(options, cx);
            list.select_first(cx);
        });
    }

    /// A result's title: the command's, or the longer one the window gave
    /// its row.
    fn title(&self, matched: &CommandMatch) -> String {
        self.titles
            .get(&matched.spec.id)
            .map_or(matched.spec.title, String::as_str)
            .to_owned()
    }

    fn observe_scroll_wheel(&mut self, cx: &mut Context<'_, Self>) {
        self.wheel_events = self.wheel_events.saturating_add(1);
        cx.notify();
    }

    fn input_changed(&mut self, text: &str, cx: &mut Context<'_, Self>) {
        self.diagnostic = None;
        match &mut self.stage {
            Stage::Search(_) => {
                self.rank(text);
                self.show_results(cx);
            }
            Stage::Slots(editor) => {
                if editor.active().spec.kind == ArgumentKind::Text {
                    self.name_text = NameText::Edited;
                }
                editor.set_text(text);
                self.refresh_picker_rows(cx);
            }
        }
    }

    /// Refilters the picker after a filter change or slot navigation, and
    /// highlights its initial row: the slot's value when it is listed, else
    /// the first row.
    fn refresh_picker_rows(&mut self, cx: &mut Context<'_, Self>) {
        self.highlight_lost = false;
        self.show_picker_rows(cx);
        self.highlight_initial(cx);
    }

    /// Refilters the picker after its domain changed. The highlighted row
    /// stays highlighted while it is still listed and is otherwise lost,
    /// never replaced by another row; a picker that had no row to
    /// highlight takes its initial one.
    fn rebuild_picker_rows(&mut self, cx: &mut Context<'_, Self>) {
        let had = self.picker_value(cx).is_some();
        self.show_picker_rows(cx);
        let has = self.picker_value(cx).is_some();
        if had {
            self.highlight_lost = !has;
        } else if !self.highlight_lost {
            self.highlight_initial(cx);
        }
    }

    /// Filters the picker's rows and puts them in the list.
    fn show_picker_rows(&mut self, cx: &mut Context<'_, Self>) {
        self.filter_picker_rows();
        let (Stage::Slots(editor), Some(list)) = (&self.stage, &self.list)
        else {
            return;
        };
        let kind = editor.active().spec.kind;
        let domain = self.domain();
        let rows = domain.rows(kind);
        let options = self
            .picker
            .iter()
            .filter_map(|(id, index)| {
                let row = rows.get(*index)?;
                Some(
                    ListOption::new(id.clone(), row.label.clone())
                        .description(row.detail.clone()),
                )
            })
            .collect();
        let empty = format!("No matching {}s", editor.active().spec.name);
        list.update(cx, |list, cx| {
            list.set_empty(empty, cx);
            list.set_options(options, cx);
        });
    }

    /// Highlights the slot's value when the picker lists it, else the
    /// first row.
    fn highlight_initial(&self, cx: &mut Context<'_, Self>) {
        let (Stage::Slots(editor), Some(list)) = (&self.stage, &self.list)
        else {
            return;
        };
        let slot = editor.active().value.as_ref().map(value_id);
        list.update(cx, |list, cx| {
            match slot.filter(|id| list.options().iter().any(|o| &o.id == id)) {
                Some(id) => list.select(&id, cx),
                None => list.select_first(cx),
            }
        });
    }

    /// Recomputes the filtered picker rows.
    fn filter_picker_rows(&mut self) {
        let Stage::Slots(editor) = &self.stage else {
            self.picker.clear();
            return;
        };
        let slot = editor.active();
        if !is_identity(slot.spec.kind) {
            self.picker.clear();
            return;
        }
        let rows = self
            .domain()
            .rows(slot.spec.kind)
            .into_iter()
            .map(|row| {
                (value_id(&row.value), row.label.clone(), row.detail.clone())
            })
            .collect::<Vec<_>>();
        let matched = self.matcher.filter(
            editor.text(),
            rows.iter()
                .map(|(_, label, detail)| (label.clone(), detail.clone())),
        );
        self.picker = matched
            .into_iter()
            .filter_map(|index| {
                rows.get(index).map(|(id, _, _)| (id.clone(), index))
            })
            .collect();
    }

    /// The value of the highlighted picker row.
    fn picker_value(&self, cx: &App) -> Option<CommandValue> {
        let Stage::Slots(editor) = &self.stage else {
            return None;
        };
        let id = self.list.as_ref()?.read(cx).highlighted()?.id.clone();
        let (_, index) = self.picker.iter().find(|(row, _)| *row == id)?;
        self.domain()
            .rows(editor.active().spec.kind)
            .get(*index)
            .map(|row| row.value.clone())
    }

    /// The highlighted result.
    fn selected_match(&self, cx: &App) -> Option<&CommandMatch> {
        let Stage::Search(search) = &self.stage else {
            return None;
        };
        let id = self.list.as_ref()?.read(cx).highlighted()?.id.clone();
        search
            .results
            .iter()
            .find(|matched| matched.spec.id.as_str() == id.as_ref())
    }

    /// Moves the list's highlight with `step`, which stops at the ends.
    fn move_highlight(
        &mut self,
        step: impl FnOnce(&mut Command, &mut Context<'_, Command>),
        cx: &mut Context<'_, Self>,
    ) {
        self.diagnostic = None;
        if let Some(list) = &self.list {
            list.update(cx, step);
        }
        cx.notify();
    }

    fn enter_slots(&mut self, editor: SlotEditor, cx: &mut Context<'_, Self>) {
        if let Stage::Search(search) = &self.stage {
            self.return_query = Some(search.query.clone());
        }
        self.stage = Stage::Slots(editor);
        // Each command's name slot starts with its target's current name.
        self.name_text = NameText::Automatic;
        self.sync_input(cx);
    }

    /// Enter on a command row.
    fn confirm_search(&mut self, cx: &mut Context<'_, Self>) {
        let Some(matched) = self.selected_match(cx) else {
            return;
        };
        let spec = matched.spec;
        if let Some(reason) = self.availability.get(&spec.id) {
            self.diagnostic = Some(format!("Unavailable: {reason}"));
            cx.notify();
            return;
        }
        if spec.id == ids::OPEN_COMMAND_PALETTE {
            return;
        }
        if spec.args.is_empty() {
            cx.emit(PaletteEvent::Execute(CommandInvocation::new(
                spec.id,
                Vec::new(),
            )));
            return;
        }
        if !slots::runs_without_prompt(spec, &self.domain_for(spec, &[])) {
            self.expand_search(cx);
            return;
        }
        match slots::unprompted_invocation(spec, &self.domain_for(spec, &[])) {
            Ok(invocation) => {
                cx.emit(PaletteEvent::Execute(invocation));
            }
            Err(error) => {
                self.diagnostic = Some(SlotEditor::humanize(&error));
                cx.notify();
            }
        }
    }

    /// Tab on a command row: open its slots even when Enter would run it.
    fn expand_search(&mut self, cx: &mut Context<'_, Self>) {
        let Some(matched) = self.selected_match(cx) else {
            return;
        };
        let spec = matched.spec;
        if spec.args.is_empty() || spec.id == ids::OPEN_COMMAND_PALETTE {
            return;
        }
        if let Some(reason) = self.availability.get(&spec.id) {
            self.diagnostic = Some(format!("Unavailable: {reason}"));
            cx.notify();
            return;
        }
        // A command whose arguments are all unprompted has nothing to open.
        let Some(editor) =
            SlotEditor::new(spec, &[], &self.domain_for(spec, &[]), false)
        else {
            return;
        };
        self.enter_slots(editor, cx);
    }

    /// Enter in a slot. With its highlight lost, a picker commits nothing.
    fn commit_slot(&mut self, cx: &mut Context<'_, Self>) {
        let picked = self.picker_value(cx);
        if let Stage::Slots(editor) = &self.stage
            && is_identity(editor.active().spec.kind)
            && self.highlight_lost
            && picked.is_none()
        {
            return;
        }
        let Stage::Slots(editor) = &mut self.stage else {
            return;
        };
        match editor.commit(picked) {
            Ok(Commit::Run(invocation)) => {
                cx.emit(PaletteEvent::Execute(invocation));
            }
            Ok(Commit::Next) => {
                self.sync_input(cx);
            }
            Err(message) => {
                self.diagnostic = Some(message);
                cx.notify();
            }
        }
    }

    /// Tab in a slot.
    fn next_slot(&mut self, cx: &mut Context<'_, Self>) {
        let picked = self.picker_value(cx);
        let Stage::Slots(editor) = &mut self.stage else {
            return;
        };
        match editor.next_slot(picked) {
            Ok(()) => {
                self.sync_input(cx);
            }
            Err(message) => {
                self.diagnostic = Some(message);
                cx.notify();
            }
        }
    }

    fn previous_slot(&mut self, cx: &mut Context<'_, Self>) {
        if let Stage::Slots(editor) = &mut self.stage {
            editor.previous_slot();
            self.sync_input(cx);
        }
    }

    fn leave_slots(&mut self, exit: Exit, cx: &mut Context<'_, Self>) {
        match exit {
            Exit::Search => {
                let query = self.return_query.take().unwrap_or_default();
                self.rank(&query);
                self.sync_input(cx);
            }
            Exit::Close => cx.emit(PaletteEvent::Cancel { query: None }),
        }
    }

    fn back(&mut self, cx: &mut Context<'_, Self>) {
        match &self.stage {
            Stage::Search(search) => {
                let query =
                    (!search.query.is_empty()).then(|| search.query.clone());
                cx.emit(PaletteEvent::Cancel { query });
            }
            Stage::Slots(editor) => {
                let exit = editor.exit();
                self.leave_slots(exit, cx);
            }
        }
    }

    fn pop(&mut self, cx: &mut Context<'_, Self>) {
        let Stage::Slots(editor) = &mut self.stage else {
            return;
        };
        match editor.pop() {
            None => {
                self.sync_input(cx);
            }
            Some(exit) => self.leave_slots(exit, cx),
        }
    }

    fn edit_slot(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Stage::Slots(editor) = &mut self.stage {
            editor.edit(index);
            self.diagnostic = None;
            self.sync_input(cx);
        }
    }

    /// Writes the stage's text into the field and mounts the stage's list.
    fn sync_input(&mut self, cx: &mut Context<'_, Self>) {
        let (text, placeholder) = match &self.stage {
            Stage::Search(search) => {
                (search.query.clone(), "Search commands".to_owned())
            }
            Stage::Slots(editor) => {
                (editor.text().to_owned(), self.placeholder(editor))
            }
        };
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder);
            input.set_text(text, cx);
        });
        self.diagnostic = None;
        self.mount_list(cx);
        self.prefill_name(cx);
        cx.notify();
    }

    /// Seeds an untouched rename name slot with the target's current custom
    /// name, selected so typing replaces it and Enter keeps it. A slot
    /// reopened with a committed value holds the user's text instead.
    fn prefill_name(&mut self, cx: &mut Context<'_, Self>) {
        let Stage::Slots(editor) = &self.stage else {
            return;
        };
        let slot = editor.active();
        if slot.spec.kind != ArgumentKind::Text {
            return;
        }
        self.name_text = entered_name_mode(
            self.name_text,
            slot.value.is_some(),
            editor.text(),
        );
        if self.name_text == NameText::Automatic {
            self.refresh_automatic_name(cx);
        }
    }

    /// Writes the target's current custom name into an automatic name
    /// slot when it differs, selected; edited text is never touched.
    fn refresh_automatic_name(&mut self, cx: &mut Context<'_, Self>) {
        let Stage::Slots(editor) = &self.stage else {
            return;
        };
        if editor.active().spec.kind != ArgumentKind::Text {
            return;
        }
        let current = self.current_name(editor);
        let Some(name) =
            refreshed_name(self.name_text, editor.text(), current.as_deref())
        else {
            return;
        };
        // The editor takes the text first, so the field's change event sees
        // no edit.
        if let Stage::Slots(editor) = &mut self.stage {
            editor.set_text(&name);
        }
        self.input.update(cx, |input, cx| {
            input.set_text(name, cx);
            input.select_all(cx);
        });
    }

    /// The custom name a rename would replace, when its target has one.
    fn current_name(&self, editor: &SlotEditor) -> Option<String> {
        current_custom_name(editor, &self.domain())
    }

    fn placeholder(&self, editor: &SlotEditor) -> String {
        let slot = editor.active();
        match slot.spec.kind {
            ArgumentKind::Tab => "Filter tabs…".to_owned(),
            ArgumentKind::Workspace => "Filter workspaces…".to_owned(),
            ArgumentKind::Session => "Filter sessions…".to_owned(),
            ArgumentKind::QuakeProfile => "Filter profiles…".to_owned(),
            ArgumentKind::Pane => "Filter panes…".to_owned(),
            ArgumentKind::Side => "Choose a side…".to_owned(),
            ArgumentKind::Layout => "Filter layouts…".to_owned(),
            ArgumentKind::Integer { min, max } => format!("{min} to {max}"),
            ArgumentKind::Bool => "true or false".to_owned(),
            ArgumentKind::Text => match self.rename_target(editor) {
                Some((label, _)) => format!("New name for “{label}”"),
                None => capitalize(slot.spec.name),
            },
        }
    }

    /// The label and argument name of the identity a rename changes.
    fn rename_target(
        &self,
        editor: &SlotEditor,
    ) -> Option<(String, &'static str)> {
        rename_target_in(editor, &self.domain())
    }

    /// Runs one `Palette`-scope catalog command.
    ///
    /// # Errors
    /// Reports `UnknownCommand` for anything outside the palette's set.
    pub(super) fn run(
        &mut self,
        invocation: &CommandInvocation,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        match invocation.id {
            ids::PALETTE_SELECT_NEXT => {
                self.move_highlight(Command::select_next, cx);
            }
            ids::PALETTE_SELECT_PREVIOUS => {
                self.move_highlight(Command::select_previous, cx);
            }
            ids::PALETTE_PAGE_DOWN => {
                self.move_highlight(Command::select_page_down, cx);
            }
            ids::PALETTE_PAGE_UP => {
                self.move_highlight(Command::select_page_up, cx);
            }
            ids::PALETTE_CONFIRM => match &self.stage {
                Stage::Search(_) => self.confirm_search(cx),
                Stage::Slots(_) => self.commit_slot(cx),
            },
            ids::PALETTE_EXPAND | ids::PALETTE_NEXT_SLOT => match &self.stage {
                Stage::Search(_) => self.expand_search(cx),
                Stage::Slots(_) => self.next_slot(cx),
            },
            ids::PALETTE_PREVIOUS_SLOT => self.previous_slot(cx),
            ids::PALETTE_BACK => self.back(cx),
            ids::PALETTE_POP => self.pop(cx),
            ids::TEXT_DELETE_BACKWARD
                if matches!(self.stage, Stage::Slots(_))
                    && self.input.read(cx).is_empty() =>
            {
                self.pop(cx);
            }
            id if FORWARDED_TEXT_COMMANDS.contains(&id) => {
                self.input.update(cx, |field, cx| {
                    crate::ui::input::run(field, invocation, cx)
                })?;
            }
            other => return Err(CommandError::UnknownCommand(other)),
        }
        Ok(CommandOutcome::Completed)
    }

    fn shortcut_keys(
        &self,
        spec: &CommandSpec,
        args: Option<&[CommandArgument]>,
    ) -> Vec<String> {
        shortcut_keys_for(&self.keymap, &self.target.contexts, spec, args)
    }

    fn slot_label(&self, slot: &Slot) -> Option<String> {
        let value = slot.value.as_ref()?;
        if is_identity(slot.spec.kind) {
            self.domain()
                .label(slot.spec.kind, value)
                .or_else(|| Some(display_value(value)))
        } else if matches!(value, CommandValue::Text(text) if text.trim().is_empty())
        {
            Some("blank".to_owned())
        } else {
            Some(display_value(value))
        }
    }

    /// The filtered picker rows' labels, in display order.
    fn picker_labels(&self) -> Vec<String> {
        let Stage::Slots(editor) = &self.stage else {
            return Vec::new();
        };
        let domain = self.domain();
        let rows = domain.rows(editor.active().spec.kind);
        self.picker
            .iter()
            .filter_map(|(_, index)| {
                rows.get(*index).map(|row| row.label.clone())
            })
            .collect()
    }

    /// The row under the pointer, which the pointer only hovers.
    fn hovered_row(&self, cx: &App) -> Option<usize> {
        let list = self.list.as_ref()?.read(cx);
        let id = &list.hovered()?.id;
        list.options().iter().position(|option| &option.id == id)
    }

    pub(super) fn smoke_state(&self, cx: &App) -> String {
        let stage = match &self.stage {
            Stage::Search(search) => {
                let selected = self.selected_match(cx);
                format!(
                    "commands selected={} query={:?} unavailable={:?} results={} highlighted_row={:?} hover={:?}",
                    selected.map_or("", |matched| matched.spec.id.as_str()),
                    search.query,
                    selected.and_then(|matched| self
                        .availability
                        .get(&matched.spec.id)),
                    search.results.len(),
                    selected
                        .and_then(|matched| search.results.iter().position(
                            |result| result.spec.id == matched.spec.id
                        )),
                    self.hovered_row(cx),
                )
            }
            Stage::Slots(editor) => format!(
                "slots command={} requested={} active={} picker={} selected={:?} chips={} picker_rows={:?}",
                editor.spec().id,
                editor.requested(),
                editor.active().spec.name,
                self.picker.len(),
                self.picker_value(cx).map(|value| display_value(&value)),
                editor
                    .slots()
                    .iter()
                    .map(|slot| format!(
                        "{}:{}",
                        slot.spec.name,
                        match slot.state {
                            SlotState::Empty => "empty",
                            SlotState::Committed => "committed",
                            SlotState::Prefilled => "prefilled",
                            SlotState::Sole => "sole",
                            SlotState::Explicit => "explicit",
                        }
                    ))
                    .collect::<Vec<_>>()
                    .join(","),
                self.picker_labels().join(";"),
            ),
        };
        let list = self.list.as_ref().map(|list| list.read(cx));
        let (offset, bounds) =
            list.map(Command::scroll_position).unwrap_or_default();
        let scroll_offset = f32::from(offset);
        let (scrollbar_x, scrollbar_thumb_y) = list
            .and_then(|list| list.scrollbar().geometry())
            .map_or((-1.0, -1.0), |geometry| {
                (
                    f32::from(bounds.right()) - 4.0,
                    f32::from(bounds.top())
                        + geometry.thumb_start
                        + geometry.thumb_size / 2.0,
                )
            });
        let scrollbar_drag =
            list.is_some_and(|list| list.scrollbar().dragging());
        // Quake picker rows as `name=detail`, so smokes can check the live
        // state each configured profile reported when the palette opened.
        let profile_rows = self
            .profiles
            .iter()
            .map(|row| format!("{}={}", row.label, row.detail))
            .collect::<Vec<_>>()
            .join(";");
        format!(
            "{stage} input={:?} diagnostic={:?} scroll_offset={scroll_offset:.1} scrollbar_drag={} scrollbar_x={scrollbar_x:.1} scrollbar_thumb_y={scrollbar_thumb_y:.1} wheel_events={} profile_rows={profile_rows:?} hierarchy_seq={}",
            self.input.read(cx).text(),
            self.diagnostic,
            scrollbar_drag,
            self.wheel_events,
            self.hierarchy_seq,
        )
    }

    /// Scrim click: close regardless of stage.
    fn close_from_scrim(&mut self, cx: &mut Context<'_, Self>) {
        let query = self.cancellation_query();
        cx.emit(PaletteEvent::Cancel { query });
    }
}

/// The panel's distance from the top of a window `height` points tall. Centred
/// placement centres the panel's clamped height, so it remains centred in a
/// short window while the result list grows and shrinks beneath the input.
fn panel_top(
    placement: PalettePlacement,
    height: f32,
    panel_height: f32,
) -> f32 {
    match placement {
        PalettePlacement::Top => PANEL_TOP_INSET,
        PalettePlacement::Center => {
            ((height - panel_height) / 2.0).max(PANEL_TOP_INSET)
        }
    }
}

fn context_matches(context: Option<&str>, contexts: &[KeyContext]) -> bool {
    let Some(context) = context else {
        return true;
    };
    KeyBindingContextPredicate::parse(context)
        .is_ok_and(|predicate| predicate.depth_of(contexts).is_some())
}

fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn display_value(value: &CommandValue) -> String {
    match value {
        CommandValue::Bool(value) => value.to_string(),
        CommandValue::Integer(value) => value.to_string(),
        CommandValue::Text(value) => value.clone(),
        CommandValue::Session(id) => format!("session {}", id.get()),
        CommandValue::Workspace(id) => format!("workspace {}", id.get()),
        CommandValue::Tab(id) => format!("tab {}", id.get()),
        CommandValue::Pane(id) => format!("pane {}", id.get()),
    }
}

fn rename_target_in(
    editor: &SlotEditor,
    domain: &DomainView<'_>,
) -> Option<(String, &'static str)> {
    if !editor.spec().id.as_str().starts_with("rename_") {
        return None;
    }
    let slot = editor
        .slots()
        .iter()
        .find(|slot| is_identity(slot.spec.kind))?;
    let label = domain.label(slot.spec.kind, slot.value.as_ref()?)?;
    Some((label, slot.spec.name))
}

fn current_custom_name(
    editor: &SlotEditor,
    domain: &DomainView<'_>,
) -> Option<String> {
    let (_, argument) = rename_target_in(editor, domain)?;
    let slot = editor
        .slots()
        .iter()
        .find(|slot| slot.spec.name == argument)?;
    let value = slot.value.as_ref()?;
    domain
        .rows(slot.spec.kind)
        .into_iter()
        .find(|row| &row.value == value)
        .filter(|row| row.custom_name)
        .map(|row| row.label.clone())
}

fn shortcut_keys_for(
    keymap: &InstalledKeymap,
    contexts: &[KeyContext],
    spec: &CommandSpec,
    args: Option<&[CommandArgument]>,
) -> Vec<String> {
    keymap
        .shortcuts(spec.id, contexts, args)
        .iter()
        .filter(|binding| match args {
            Some(args) => binding.matches_arguments(args),
            None => binding.args.is_empty(),
        })
        .take(2)
        .map(|binding| binding.key.clone())
        .collect()
}

fn select_tab_shortcut_index(position: i64, tab_count: usize) -> Option<i64> {
    if (1..=8).contains(&position) {
        Some(position)
    } else if usize::try_from(position).ok() == Some(tab_count) {
        Some(9)
    } else {
        None
    }
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

// ---- presentation ---------------------------------------------------------

/// A small label beside a row: `recent`, or an argument badge such as
/// `⇥ options`, whose key draws as an icon. It reads in `text`, the row's
/// secondary color, and draws outlined, as the row's key caps do, since the
/// row hovers, presses, and selects beneath it.
fn tag(
    hint: &KeyHint,
    swatch: Swatch,
    dashed: bool,
    text: Hsla,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(4.0))
        .when(dashed, |tag| {
            tag.border_1().border_dashed().border_color(swatch.dim)
        })
        .when(!dashed, |tag| {
            tag.border_1().border_color(text.opacity(0.35))
        })
        .text_size(px(10.0))
        .text_color(text)
        .child(hint.element(px(9.5), text))
}

fn band(message: String, swatch: Swatch, error: bool) -> gpui::AnyElement {
    div()
        .px(px(12.0))
        .py(px(6.0))
        .border_b_1()
        .border_color(swatch.line)
        .text_size(px(12.5))
        .when(error, |band| {
            band.text_color(swatch.accent_text())
                .bg(swatch.accent.opacity(0.05))
        })
        .when(!error, |band| band.text_color(swatch.muted))
        .child(message)
        .into_any_element()
}

fn selection_bar(selected: bool, swatch: Swatch) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(3.0))
        .h(px(28.0))
        .rounded(px(2.0))
        .when(selected, |bar| bar.bg(swatch.accent))
}

fn title_highlight_ranges(
    title: &str,
    indices: &[u32],
) -> Vec<std::ops::Range<usize>> {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for (index, ch) in title.char_indices() {
        if !indices.iter().any(|matched| *matched as usize == index) {
            continue;
        }
        let end = index + ch.len_utf8();
        if let Some(previous) = ranges.last_mut()
            && previous.end == index
        {
            previous.end = end;
        } else {
            ranges.push(index..end);
        }
    }
    ranges
}

fn highlighted_title(
    title: &str,
    indices: &[u32],
    swatch: Swatch,
    window: &Window,
) -> gpui::AnyElement {
    let highlight = HighlightStyle {
        color: Some(swatch.accent_text()),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..HighlightStyle::default()
    };
    let highlights = title_highlight_ranges(title, indices)
        .into_iter()
        .map(|range| (range, highlight));
    div()
        .truncate()
        .text_size(px(13.5))
        .child(
            gpui::StyledText::new(title.to_owned())
                .with_default_highlights(&window.text_style(), highlights),
        )
        .into_any_element()
}

/// Chips for the slot line, bands beneath the line, and footer hints.
type SlotRender = (
    Vec<gpui::AnyElement>,
    Vec<gpui::AnyElement>,
    Vec<(KeyHint, String)>,
);

impl CommandPalette {
    /// The content of the list's row for `option`, which the list draws
    /// inside the row's fill.
    fn row(
        &self,
        option: &ListOption,
        highlighted: bool,
        window: &Window,
    ) -> AnyElement {
        let swatch = Swatch::new(self.colors);
        let row = match &self.stage {
            Stage::Search(search) => search
                .results
                .iter()
                .find(|matched| matched.spec.id.as_str() == option.id.as_ref())
                .map(|matched| {
                    self.command_row(
                        matched,
                        search.query.is_empty(),
                        highlighted,
                        swatch,
                        window,
                    )
                }),
            Stage::Slots(editor) => {
                self.picker_row(editor, option, highlighted, swatch)
            }
        };
        row.unwrap_or_else(|| div().into_any_element())
    }

    /// A result's selection bar, title with its matched characters marked,
    /// description, tags, key caps, and scope.
    fn command_row(
        &self,
        matched: &CommandMatch,
        unfiltered: bool,
        selected: bool,
        swatch: Swatch,
        window: &Window,
    ) -> AnyElement {
        let spec = matched.spec;
        let unavailable = self.availability.get(&spec.id);
        let badge = if !slots::has_slots(spec) {
            None
        } else if slots::runs_without_prompt(spec, &self.domain_for(spec, &[]))
        {
            Some((
                KeyHint::keys(&["tab"], Platform::current())
                    .with_suffix(" options"),
                true,
            ))
        } else {
            Some((
                KeyHint::keys(&["enter"], Platform::current())
                    .with_suffix(" prompts"),
                false,
            ))
        };
        let recent = unfiltered && self.history.recency(spec.id).is_some();
        // Secondary text on a selected row reads against its fill.
        let secondary = if selected {
            swatch.muted_on(swatch.selection)
        } else {
            swatch.muted
        };
        let mut meta = div().flex().flex_none().items_center().gap(px(6.0));
        if recent {
            meta = meta.child(tag(
                &KeyHint::keys(&[], Platform::current()).with_suffix("recent"),
                swatch,
                false,
                secondary,
            ));
        }
        if let Some((hint, dashed)) = badge {
            meta = meta.child(tag(&hint, swatch, dashed, secondary));
        }
        for key in self.shortcut_keys(spec, None) {
            // A row hovers, presses, and selects, so its caps draw
            // outlined, with no fill of their own on those fills.
            meta = meta.child(key_cap_on(
                &KeyHint::parse(&key, Platform::current()),
                swatch.fg,
            ));
        }
        meta = meta.child(
            div()
                .text_size(px(10.0))
                .text_color(secondary)
                .child(format!("{:?}", spec.scope).to_uppercase()),
        );
        let description = match unavailable {
            Some(reason) => format!("Unavailable: {reason}"),
            None => spec.description.to_owned(),
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .when(unavailable.is_some(), |row| row.opacity(0.55))
            .child(selection_bar(selected, swatch))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(highlighted_title(
                        &self.title(matched),
                        &matched.title_indices,
                        swatch,
                        window,
                    ))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(if unavailable.is_some() {
                                swatch.accent_text()
                            } else {
                                secondary
                            })
                            .child(description),
                    ),
            )
            .child(meta)
            .into_any_element()
    }

    /// A picker row's selection bar, label, detail, and, for a tab, the
    /// key cap that selects it.
    fn picker_row(
        &self,
        editor: &SlotEditor,
        option: &ListOption,
        selected: bool,
        swatch: Swatch,
    ) -> Option<AnyElement> {
        let domain = self.domain();
        let rows = domain.rows(editor.active().spec.kind);
        let (_, index) = self.picker.iter().find(|(id, _)| *id == option.id)?;
        let item = rows.get(*index)?;
        let select_tab = huterm_protocol::lookup(ids::SELECT_TAB.as_str());
        let key = item.position.and_then(|position| {
            let index = select_tab_shortcut_index(position, rows.len())?;
            self.shortcut_keys(
                select_tab?,
                Some(&[CommandArgument::new(
                    "index",
                    CommandValue::Integer(index),
                )]),
            )
            .into_iter()
            .next()
        });
        let mut meta = div().flex().flex_none().items_center().gap(px(6.0));
        if let Some(key) = key {
            meta = meta.child(key_cap_on(
                &KeyHint::parse(&key, Platform::current()),
                swatch.fg,
            ));
        }
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(selection_bar(selected, swatch))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(13.5))
                                .child(item.label.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.5))
                                .text_color(if selected {
                                    swatch.muted_on(swatch.selection)
                                } else {
                                    swatch.muted
                                })
                                .child(item.detail.clone()),
                        ),
                )
                .child(meta)
                .into_any_element(),
        )
    }

    /// Bands beneath the line and footer hints for command search.
    fn render_search(
        &self,
        swatch: Swatch,
        cx: &App,
    ) -> (Vec<gpui::AnyElement>, Vec<(KeyHint, String)>) {
        let mut below = Vec::new();
        if let Some(message) = &self.diagnostic {
            below.push(band(message.clone(), swatch, true));
        }
        let selected_spec = self.selected_match(cx).map(|matched| matched.spec);
        let has_args = selected_spec.is_some_and(slots::has_slots);
        let runs = selected_spec.is_some_and(|spec| {
            spec.args.is_empty()
                || slots::runs_without_prompt(spec, &self.domain_for(spec, &[]))
        });
        let platform = Platform::current();
        let mut hints = vec![
            (
                KeyHint::keys(&["up", "down"], platform),
                "navigate".to_owned(),
            ),
            (
                KeyHint::keys(&["enter"], platform),
                if runs { "run" } else { "next" }.to_owned(),
            ),
        ];
        if has_args {
            hints.push((
                KeyHint::keys(&["tab"], platform),
                if runs { "set arguments" } else { "arguments" }.to_owned(),
            ));
        }
        hints.push((KeyHint::keys(&["escape"], platform), "close".to_owned()));
        (below, hints)
    }

    fn render_chip(
        &self,
        index: usize,
        slot: &Slot,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Option<gpui::AnyElement> {
        let (dashed, hint) = match slot.state {
            SlotState::Empty => return None,
            SlotState::Committed | SlotState::Explicit => (false, false),
            SlotState::Prefilled => (true, true),
            SlotState::Sole => (true, false),
        };
        let label = self.slot_label(slot)?;
        let editable = slot.state != SlotState::Explicit;
        let chip = div()
            .id(("palette-chip", index))
            .flex_none()
            .h(px(24.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(px(6.0))
            .text_size(px(12.5))
            .when(dashed, |chip| {
                chip.border_1()
                    .border_dashed()
                    .border_color(swatch.dim)
                    .text_color(swatch.muted)
            })
            .when(!dashed, |chip| chip.bg(swatch.chip))
            .when(editable, |chip| {
                chip.cursor_pointer()
                    .active(|chip| chip.bg(swatch.pressed()))
            })
            .on_click(cx.listener(move |palette, _: &ClickEvent, _, cx| {
                palette.edit_slot(index, cx);
            }))
            .child(div().text_color(swatch.muted).child(slot.spec.name))
            .child(div().text_color(swatch.fg).child(label))
            .when(hint, |chip| {
                chip.child(
                    div()
                        .text_size(px(9.5))
                        .px(px(3.0))
                        .rounded(px(3.0))
                        .border_1()
                        .border_color(swatch.dim)
                        .text_color(swatch.muted)
                        .child(
                            KeyHint::keys(&["tab"], Platform::current())
                                .element(px(9.0), swatch.muted),
                        ),
                )
            });
        Some(chip.into_any_element())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one presentation tree for the slot line, bands, and hints"
    )]
    fn render_slots(
        &self,
        editor: &SlotEditor,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> SlotRender {
        let requested = editor.requested();
        let active = editor.active_index();
        let mut line: Vec<gpui::AnyElement> = vec![
            div()
                .id("palette-command-chip")
                .flex_none()
                .h(px(24.0))
                .px(px(8.0))
                .flex()
                .items_center()
                .rounded(px(6.0))
                .bg(swatch.selection)
                .border_1()
                .border_color(swatch.accent.opacity(0.4))
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(12.5))
                .when(!requested, |chip| {
                    chip.cursor_pointer()
                        .active(|chip| chip.bg(swatch.selection_pressed()))
                })
                .on_click(cx.listener(move |palette, _: &ClickEvent, _, cx| {
                    if !requested {
                        palette.leave_slots(Exit::Search, cx);
                    }
                }))
                .child(editor.spec().title)
                .into_any_element(),
        ];
        for (index, slot) in editor.slots().iter().enumerate() {
            if index == active {
                let optional =
                    !slot.spec.is_required() && slot.spec.group().is_none();
                line.push(
                    div()
                        .flex_none()
                        .text_size(px(12.5))
                        .text_color(swatch.muted)
                        .child(format!(
                            "{}{} ›",
                            slot.spec.name,
                            if optional { " (optional)" } else { "" }
                        ))
                        .into_any_element(),
                );
                line.push(self.input.clone().into_any_element());
            } else if let Some(chip) = self.render_chip(index, slot, swatch, cx)
            {
                line.push(chip);
            }
        }

        let mut below = Vec::new();
        if let Some(message) = &self.diagnostic {
            below.push(band(message.clone(), swatch, true));
        }
        let slot = editor.active();
        if !is_identity(slot.spec.kind) {
            let note = match slot.spec.kind {
                ArgumentKind::Integer { min, max } => {
                    Some(format!("Whole number from {min} to {max}."))
                }
                _ => match self.rename_target(editor) {
                    Some((label, name)) => Some(format!(
                        "Renames {label}; a blank name restores the default. \
                         Press ⇥ to choose a different {name}."
                    )),
                    None if !slot.spec.is_required()
                        && slot.spec.group().is_none() =>
                    {
                        Some("Leave empty to use the default.".to_owned())
                    }
                    None => None,
                },
            };
            if let Some(note) = note {
                below.push(band(note, swatch, false));
            }
        }

        let runs = editor.complete()
            || !editor.text().trim().is_empty()
            || editor.remaining_required_satisfied();
        let platform = Platform::current();
        let mut hints = vec![(
            KeyHint::keys(&["enter"], platform),
            if runs { "run" } else { "next" }.to_owned(),
        )];
        if editor.slots().len() > 1 {
            hints.push((
                KeyHint::keys(&["tab"], platform),
                "next argument".to_owned(),
            ));
        }
        let can_pop = editor.slots().iter().take(active).any(|slot| {
            matches!(slot.state, SlotState::Committed | SlotState::Prefilled)
        });
        let leave = if requested { "close" } else { "back" };
        hints.push((
            KeyHint::keys(&["backspace"], platform).with_suffix(" on empty"),
            if can_pop { "previous" } else { leave }.to_owned(),
        ));
        hints.push((KeyHint::keys(&["escape"], platform), leave.to_owned()));
        (line, below, hints)
    }
}

impl Render for CommandPalette {
    #[expect(
        clippy::too_many_lines,
        reason = "the overlay, panel, line, list, and footer form one tree"
    )]
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let swatch = Swatch::new(self.colors);
        let viewport = f32::from(window.viewport_size().height);
        let available = viewport - PANEL_TOP_INSET * 2.0 - PANEL_CHROME_HEIGHT;
        let list_max =
            available.clamp(ROW_HEIGHT + LIST_PADDING * 2.0, LIST_MAX_HEIGHT);
        let panel_height = list_max + PANEL_CHROME_HEIGHT;
        let top = panel_top(self.placement, viewport, panel_height);
        let mut line = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(10.0))
            .py(px(6.0))
            .min_h(px(42.0))
            .border_b_1()
            .border_color(swatch.line);
        let (below, hints) = match &self.stage {
            Stage::Search(search) => {
                let (below, hints) = self.render_search(swatch, cx);
                line = line
                    .child(div().text_color(swatch.muted).child("›"))
                    .child(self.input.clone())
                    .child(
                        div()
                            .ml_auto()
                            .flex_none()
                            .text_size(px(11.0))
                            .text_color(swatch.muted)
                            .child(format!(
                                "{} command{}",
                                search.results.len(),
                                if search.results.len() == 1 {
                                    ""
                                } else {
                                    "s"
                                }
                            )),
                    );
                (below, hints)
            }
            Stage::Slots(editor) => {
                let (chips, below, hints) =
                    self.render_slots(editor, swatch, cx);
                line = line.children(chips);
                (below, hints)
            }
        };

        let footer = footer_hints(hints, swatch);
        if let Some(list) = &self.list {
            list.update(cx, |list, cx| {
                list.set_tokens(swatch, cx);
                list.set_max_height(Some(px(list_max)), cx);
            });
        }

        let input_focus = self.focus_handle(cx);
        let panel = div()
            .id("palette-panel")
            .w(px(620.0))
            .max_w_full()
            .bg(swatch.bg)
            .border_1()
            .border_color(swatch.fg.opacity(0.22))
            .rounded(px(10.0))
            .shadow(vec![BoxShadow {
                color: hsla(0.0, 0.0, 0.0, 0.55),
                offset: point(px(0.0), px(16.0)),
                blur_radius: px(48.0),
                spread_radius: px(0.0),
                inset: false,
            }])
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_mouse_down(
                MouseButton::Left,
                move |_: &MouseDownEvent, window, cx| {
                    input_focus.focus(window, cx);
                    cx.stop_propagation();
                },
            )
            .on_mouse_up(MouseButton::Left, |_: &MouseUpEvent, _, cx| {
                cx.stop_propagation();
            })
            .on_click(|_: &ClickEvent, _, cx| cx.stop_propagation())
            .child(line)
            .children(below)
            .child(
                div()
                    .id("palette-list-frame")
                    .w_full()
                    .flex()
                    .flex_col()
                    .on_scroll_wheel(cx.listener(
                        |palette, _: &ScrollWheelEvent, _, cx| {
                            palette.observe_scroll_wheel(cx);
                        },
                    ))
                    .children(self.list.clone()),
            )
            .child(footer);

        div()
            .id("palette-overlay")
            .absolute()
            .inset_0()
            .bg(swatch.bg.opacity(0.55))
            .flex()
            .justify_center()
            .items_start()
            .pt(px(top))
            .key_context("Palette")
            .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_mouse_up(MouseButton::Left, |_: &MouseUpEvent, _, cx| {
                cx.stop_propagation();
            })
            .on_click(cx.listener(|palette, _: &ClickEvent, _, cx| {
                // Clicks inside the panel stop before reaching here.
                palette.close_from_scrim(cx);
            }))
            .on_scroll_wheel(|_: &ScrollWheelEvent, _, cx| {
                cx.stop_propagation();
            })
            .child(panel)
    }
}

#[cfg(test)]
mod tests;
