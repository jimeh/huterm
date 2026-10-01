//! Command-palette workflow and its window-independent state model.
//!
//! The palette has two stages. Command search ranks the catalog with the
//! fuzzy matcher and usage history. The slot stage collects arguments one
//! active slot at a time, showing everything else as chips. Every keyboard
//! operation arrives as a `Palette`-scope catalog command through
//! [`CommandPalette::run`].

mod search;
mod slots;

pub(super) use search::{CommandFrequency, HistoryView, RecentCommands};

use std::collections::HashMap;
use std::time::Instant;

use gpui::{
    App, BoxShadow, ClickEvent, Context, Entity, EventEmitter, Focusable,
    FontWeight, HighlightStyle, KeyBindingContextPredicate, KeyContext,
    MouseButton, MouseDownEvent, MouseUpEvent, Render, ScrollHandle,
    ScrollWheelEvent, Subscription, WeakEntity, Window, div, hsla, point,
    prelude::*, px,
};
use huterm_config::PalettePlacement;
use huterm_protocol::{
    ArgumentKind, CommandArgument, CommandError, CommandId, CommandInvocation,
    CommandOutcome, CommandScope, CommandSpec, CommandValue, HierarchyState,
    SessionId, TabId, TerminalId, WorkspaceId, catalog, ids,
};
use search::{CommandHistory, CommandMatch, CommandSearch, PickerMatcher};
use slots::{
    Commit, Exit, Slot, SlotDomain, SlotEditor, SlotState, is_identity,
};

use super::TerminalView;
use super::key_hint::KeyHint;
use super::overlay::{OverlayColors, Swatch, footer_hints, key_cap};
use crate::keymap::{InstalledKeymap, Platform};
use crate::ui::list_scrollbar::ListScrollbar;
use crate::ui::text_field::{Changed, TextField};

const PAGE_STEP: isize = 8;

const FORWARDED_TEXT_COMMANDS: &[CommandId] = &[
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
const LIST_PADDING: f32 = 6.0;
const LIST_MAX_HEIGHT: f32 = ROW_HEIGHT * VISIBLE_ROWS + LIST_PADDING * 2.0;
/// The panel at its tallest: input line, full list, and footer. Centred
/// placement positions this height so filtering never moves the input.
const PANEL_MAX_HEIGHT: f32 = 412.0;
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
    pub(super) colors: OverlayColors,
    pub(super) placement: PalettePlacement,
    pub(super) history: OwnedHistory,
    pub(super) profiles: Vec<QuakeProfileRow>,
    /// A command invoked interactively without its required arguments.
    pub(super) request: Option<&'a CommandInvocation>,
    /// A query retained from a recent cancellation.
    pub(super) retained_query: Option<String>,
    /// Identity rows built from the window's hierarchy projection.
    pub(super) hierarchy: PaletteHierarchy,
    /// The projection sequence `hierarchy` reflects.
    pub(super) hierarchy_seq: u64,
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
            custom_name: false,
        })
        .collect()
}

/// Session, workspace, and tab rows for identity pickers.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PaletteHierarchy {
    sessions: Vec<IdentityRow>,
    workspaces: Vec<IdentityRow>,
    tabs: Vec<IdentityRow>,
}

impl PaletteHierarchy {
    /// Builds rows from a hierarchy projection, in session, workspace, and
    /// tab order. A tab is labelled by its custom name, then `title`, the
    /// title its window published, then its fallback name. Tabs follow
    /// `tab_order`, most recently used first; tabs it omits keep their
    /// structural order after the ordered ones.
    pub(super) fn from_projection<'t>(
        state: &HierarchyState,
        title: impl Fn(TabId) -> Option<&'t str>,
        tab_order: &[TabId],
    ) -> Self {
        let mut sessions = Vec::new();
        let mut workspaces = Vec::new();
        let mut tabs = Vec::new();
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
                            .unwrap_or(&tab.fallback_name)
                            .to_owned(),
                        detail: format!(
                            "{} › {}",
                            session.display_name(),
                            workspace.display_name(),
                        ),
                        position: i64::try_from(index + 1).ok(),
                        workspace: Some(workspace_id),
                        custom_name: tab.custom_name.is_some(),
                    });
                }
            }
        }
        // Most recently used first; tabs the window never activated keep
        // their structural order after the ordered ones.
        let rank = |value: &CommandValue| match value {
            CommandValue::Tab(id) => tab_order
                .iter()
                .position(|ordered| ordered == id)
                .unwrap_or(usize::MAX),
            _ => usize::MAX,
        };
        tabs.sort_by_key(|row| rank(&row.value));
        Self {
            sessions,
            workspaces,
            tabs,
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
            _ => &[],
        }
    }
}

/// The slot editor's view of identity domains: the hierarchy projection for
/// sessions, workspaces, and tabs; configured profiles for quake commands.
struct DomainView<'a> {
    hierarchy: &'a PaletteHierarchy,
    profiles: &'a [IdentityRow],
    target: &'a PaletteTarget,
    /// Window-scoped commands can only act on this window's tabs.
    window_only: bool,
}

impl DomainView<'_> {
    /// Rows for `kind` in domain order.
    fn rows(&self, kind: ArgumentKind) -> Vec<&IdentityRow> {
        match kind {
            ArgumentKind::QuakeProfile => self.profiles.iter().collect(),
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
            ArgumentKind::Bool
            | ArgumentKind::Integer { .. }
            | ArgumentKind::Text => None,
        }
    }
}

#[derive(Clone, Debug)]
struct SearchState {
    query: String,
    results: Vec<CommandMatch>,
    selected: usize,
}

impl SearchState {
    fn selected(&self) -> Option<&CommandMatch> {
        self.results.get(self.selected)
    }

    fn move_selection(&mut self, delta: isize) {
        if self.results.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self
                .selected
                .saturating_add_signed(delta)
                .min(self.results.len() - 1);
        }
    }
}

enum Stage {
    Search(SearchState),
    Slots(SlotEditor),
}

/// What an identity picker highlights.
#[derive(Clone, Debug, Default, PartialEq)]
enum Highlight {
    /// The slot's value when it is listed, else the first row.
    #[default]
    Initial,
    /// A value the user highlighted, or one a rebuild kept. When the value
    /// is not listed, nothing is highlighted.
    Value(CommandValue),
    /// A rebuild removed the highlighted value. Nothing is highlighted, and
    /// Enter does nothing, until the user moves the highlight or filters.
    Lost,
}

/// The highlighted row among `rows` filtered rows. `position` finds a
/// value's row; `slot` is the slot's preselected value. Only the initial
/// highlight falls back to the slot's value or the first row: an explicit
/// value that is not listed highlights nothing, so Enter can never reach
/// another row.
fn highlighted_row(
    highlight: &Highlight,
    position: impl Fn(&CommandValue) -> Option<usize>,
    slot: Option<&CommandValue>,
    rows: usize,
) -> Option<usize> {
    match highlight {
        Highlight::Initial => {
            slot.and_then(&position).or_else(|| (rows > 0).then_some(0))
        }
        Highlight::Value(value) => position(value),
        Highlight::Lost => None,
    }
}

/// The highlight a click on a picker row sets. GPUI dispatches clicks
/// against the previous frame, so the row's value may already be gone after
/// a rebuild; then the click is lost and commits nothing.
fn clicked_highlight(value: CommandValue, listed: bool) -> Highlight {
    if listed {
        Highlight::Value(value)
    } else {
        Highlight::Lost
    }
}

/// The row a hierarchy rebuild scrolls to: the highlighted row, only when
/// its index changed. A rebuild otherwise leaves the scroll offset alone.
fn rebuild_scroll(
    before: Option<usize>,
    after: Option<usize>,
) -> Option<usize> {
    after.filter(|_| after != before)
}

/// The row a selection move lands on. With no highlight, moving down
/// selects the first row and moving up the last.
fn moved_row(
    current: Option<usize>,
    delta: isize,
    rows: usize,
) -> Option<usize> {
    let last = rows.checked_sub(1)?;
    Some(match current {
        Some(index) => index.saturating_add_signed(delta).min(last),
        None if delta < 0 => last,
        None => 0,
    })
}

/// Whether Enter does nothing: a rebuild removed the highlighted target of
/// an identity picker, and Enter must never hand the command to another row.
fn enter_blocked(highlight: &Highlight, kind: ArgumentKind) -> bool {
    *highlight == Highlight::Lost && is_identity(kind)
}

/// The highlight after a rebuild: the highlighted value stays when it is
/// still listed, and is lost, never replaced by another row, when it is not.
fn rebuilt_highlight(
    current: &Highlight,
    highlighted: Option<CommandValue>,
    listed: impl Fn(&CommandValue) -> bool,
) -> Highlight {
    match highlighted {
        Some(value) if listed(&value) => Highlight::Value(value),
        Some(_) => Highlight::Lost,
        None => current.clone(),
    }
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
    input: Entity<TextField>,
    engine: CommandSearch,
    matcher: PickerMatcher,
    history: OwnedHistory,
    listed: Vec<&'static CommandSpec>,
    hierarchy: PaletteHierarchy,
    /// The projection sequence `hierarchy` reflects.
    hierarchy_seq: u64,
    profiles: Vec<IdentityRow>,
    availability: HashMap<CommandId, String>,
    keymap: InstalledKeymap,
    colors: OverlayColors,
    placement: PalettePlacement,
    diagnostic: Option<String>,
    /// Filtered row indices for the active identity slot.
    picker: Vec<usize>,
    /// What the picker highlights, kept by value so it survives reordering
    /// and rebuilds.
    highlight: Highlight,
    /// Whether the name slot's text is automatic or the user's.
    name_text: NameText,
    hover: Option<usize>,
    scroll: ScrollHandle,
    /// Fading overlay scrollbar on the list; shown after list changes and
    /// scrolling so it also signals rows beyond the visible six.
    scrollbar: ListScrollbar,
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
            colors,
            placement,
            history,
            profiles,
            request,
            retained_query,
            hierarchy,
            hierarchy_seq,
        } = open;
        let input = cx
            .new(|cx| TextField::new("Search commands", colors.foreground, cx));
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
                palette.input_changed(&text);
                cx.notify();
            });
        let listed = catalog()
            .iter()
            .filter(|spec| context_matches(spec.context, &target.contexts))
            .collect();
        let scroll = ScrollHandle::new();
        let mut palette = Self {
            target,
            stage: Stage::Search(SearchState {
                query: String::new(),
                results: Vec::new(),
                selected: 0,
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
            availability,
            keymap,
            colors,
            placement,
            diagnostic: None,
            picker: Vec::new(),
            highlight: Highlight::Initial,
            name_text: NameText::Automatic,
            hover: None,
            scroll: scroll.clone(),
            scrollbar: ListScrollbar::new(scroll),
            wheel_events: 0,
            _subscriptions: vec![subscription],
        };
        let requested = request.and_then(|request| {
            huterm_protocol::lookup(request.id.as_str())
                .map(|spec| (spec, request))
        });
        if let Some((spec, request)) = requested {
            let editor = SlotEditor::new(
                spec,
                &request.args,
                &palette.domain_for(spec),
                true,
            );
            if editor.slots().is_empty() {
                palette.rank("");
            } else {
                palette.enter_slots(editor, cx);
            }
        } else {
            let query = retained_query.unwrap_or_default();
            palette.rank(&query);
            palette.input.update(cx, |input, cx| {
                input.set_text(query, cx);
                input.select_all_text(cx);
            });
        }
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
    ) {
        self.availability = availability;
    }

    /// Applies reloaded theme colours, placement, and profiles to an open
    /// palette.
    pub(super) fn set_presentation(
        &mut self,
        colors: OverlayColors,
        placement: PalettePlacement,
        profiles: Vec<QuakeProfileRow>,
        cx: &mut Context<'_, Self>,
    ) {
        self.colors = colors;
        self.placement = placement;
        self.profiles = profile_rows(profiles);
        self.input.update(cx, |input, cx| {
            input.set_foreground(colors.foreground, cx);
        });
        self.refresh_picker_rows();
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
        let highlighted = self.picker_value();
        let before = self.picker_index();
        self.hierarchy = hierarchy;
        self.filter_picker_rows();
        self.highlight =
            rebuilt_highlight(&self.highlight, highlighted, |value| {
                self.picker_position(value).is_some()
            });
        // Keep the user's scroll position and leave the scrollbar hidden:
        // a rebuild scrolls only when the highlighted row moved.
        if let Some(index) = rebuild_scroll(before, self.picker_index()) {
            self.scroll.scroll_to_item(index);
        }
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

    fn domain_for(&self, spec: &CommandSpec) -> DomainView<'_> {
        DomainView {
            hierarchy: &self.hierarchy,
            profiles: &self.profiles,
            target: &self.target,
            window_only: spec.scope == CommandScope::Window,
        }
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
        DomainView {
            hierarchy: &self.hierarchy,
            profiles: &self.profiles,
            target: &self.target,
            window_only: self.window_only(),
        }
    }

    fn rank(&mut self, query: &str) {
        let results =
            self.engine
                .rank(query, self.listed.iter().copied(), &self.history);
        self.stage = Stage::Search(SearchState {
            query: query.to_owned(),
            results,
            selected: 0,
        });
        self.picker.clear();
        self.scroll.scroll_to_item(0);
        self.show_scrollbar();
    }

    /// Reveals the list scrollbar; the animation clock fades it out.
    fn show_scrollbar(&mut self) {
        self.scrollbar.show();
    }

    fn observe_scroll_wheel(&mut self, cx: &mut Context<'_, Self>) {
        self.wheel_events = self.wheel_events.saturating_add(1);
        self.show_scrollbar();
        cx.notify();
    }

    /// Advances the scrollbar fade and expansion from the window animation clock.
    pub(super) fn advance(&mut self, now: Instant, cx: &mut Context<'_, Self>) {
        if self.scrollbar.advance(now) {
            cx.notify();
        }
    }

    fn input_changed(&mut self, text: &str) {
        self.diagnostic = None;
        match &mut self.stage {
            Stage::Search(_) => self.rank(text),
            Stage::Slots(editor) => {
                if editor.active().spec.kind == ArgumentKind::Text {
                    self.name_text = NameText::Edited;
                }
                editor.set_text(text);
                self.highlight = Highlight::Initial;
                self.refresh_picker_rows();
            }
        }
    }

    /// Refilters the picker after a filter change or slot navigation,
    /// revealing the highlighted row and the scrollbar.
    fn refresh_picker_rows(&mut self) {
        self.filter_picker_rows();
        if let Some(index) = self.picker_index() {
            self.scroll.scroll_to_item(index);
        }
        self.show_scrollbar();
    }

    /// Recomputes the filtered picker rows without scrolling.
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
            .map(|row| (row.label.clone(), row.detail.clone()))
            .collect::<Vec<_>>();
        self.picker = self.matcher.filter(editor.text(), rows.into_iter());
    }

    /// The filtered picker row that lists `value`.
    fn picker_position(&self, value: &CommandValue) -> Option<usize> {
        let Stage::Slots(editor) = &self.stage else {
            return None;
        };
        let domain = self.domain();
        let rows = domain.rows(editor.active().spec.kind);
        self.picker.iter().position(|index| {
            rows.get(*index).map(|row| &row.value) == Some(value)
        })
    }

    /// Row index into the filtered picker that is currently highlighted.
    fn picker_index(&self) -> Option<usize> {
        let Stage::Slots(editor) = &self.stage else {
            return None;
        };
        highlighted_row(
            &self.highlight,
            |value| self.picker_position(value),
            editor.active().value.as_ref(),
            self.picker.len(),
        )
    }

    fn picker_value_at(&self, row: usize) -> Option<CommandValue> {
        let Stage::Slots(editor) = &self.stage else {
            return None;
        };
        let domain = self.domain();
        let rows = domain.rows(editor.active().spec.kind);
        rows.get(*self.picker.get(row)?)
            .map(|row| row.value.clone())
    }

    fn picker_value(&self) -> Option<CommandValue> {
        self.picker_value_at(self.picker_index()?)
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        self.diagnostic = None;
        match &mut self.stage {
            Stage::Search(search) => {
                search.move_selection(delta);
                self.scroll.scroll_to_item(search.selected);
            }
            Stage::Slots(_) => {
                if let Some(next) =
                    moved_row(self.picker_index(), delta, self.picker.len())
                {
                    self.highlight = self
                        .picker_value_at(next)
                        .map_or(Highlight::Initial, Highlight::Value);
                    self.scroll.scroll_to_item(next);
                }
            }
        }
        self.show_scrollbar();
        cx.notify();
    }

    fn enter_slots(&mut self, editor: SlotEditor, cx: &mut Context<'_, Self>) {
        if let Stage::Search(search) = &self.stage {
            self.return_query = Some(search.query.clone());
        }
        self.stage = Stage::Slots(editor);
        self.highlight = Highlight::Initial;
        // Each command's name slot starts with its target's current name.
        self.name_text = NameText::Automatic;
        self.sync_input(cx);
    }

    /// Enter on a command row.
    fn confirm_search(&mut self, cx: &mut Context<'_, Self>) {
        let Stage::Search(search) = &self.stage else {
            return;
        };
        let Some(matched) = search.selected() else {
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
        if !slots::runs_without_prompt(spec, &self.domain_for(spec)) {
            self.expand_search(cx);
            return;
        }
        let editor = SlotEditor::new(spec, &[], &self.domain_for(spec), false);
        match editor.invocation() {
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
        let Stage::Search(search) = &self.stage else {
            return;
        };
        let Some(matched) = search.selected() else {
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
        let editor = SlotEditor::new(spec, &[], &self.domain_for(spec), false);
        if editor.slots().is_empty() {
            return;
        }
        self.enter_slots(editor, cx);
    }

    /// Enter in a slot.
    fn commit_slot(&mut self, cx: &mut Context<'_, Self>) {
        if let Stage::Slots(editor) = &self.stage
            && enter_blocked(&self.highlight, editor.active().spec.kind)
        {
            return;
        }
        let picked = self.picker_value();
        let Stage::Slots(editor) = &mut self.stage else {
            return;
        };
        match editor.commit(picked) {
            Ok(Commit::Run(invocation)) => {
                cx.emit(PaletteEvent::Execute(invocation));
            }
            Ok(Commit::Next) => {
                self.highlight = Highlight::Initial;
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
        let picked = self.picker_value();
        let Stage::Slots(editor) = &mut self.stage else {
            return;
        };
        match editor.next_slot(picked) {
            Ok(()) => {
                self.highlight = Highlight::Initial;
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
            self.highlight = Highlight::Initial;
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
                self.highlight = Highlight::Initial;
                self.sync_input(cx);
            }
            Some(exit) => self.leave_slots(exit, cx),
        }
    }

    fn edit_slot(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Stage::Slots(editor) = &mut self.stage {
            editor.edit(index);
            self.highlight = Highlight::Initial;
            self.diagnostic = None;
            self.sync_input(cx);
        }
    }

    /// Writes the stage's text into the field and refreshes the picker.
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
        self.refresh_picker_rows();
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
            input.select_all_text(cx);
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
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        match invocation.id {
            ids::PALETTE_SELECT_NEXT => self.move_selection(1, cx),
            ids::PALETTE_SELECT_PREVIOUS => self.move_selection(-1, cx),
            ids::PALETTE_PAGE_DOWN => self.move_selection(PAGE_STEP, cx),
            ids::PALETTE_PAGE_UP => self.move_selection(-PAGE_STEP, cx),
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
                    field.run(invocation, window, cx)
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
            .filter_map(|index| rows.get(*index).map(|row| row.label.clone()))
            .collect()
    }

    pub(super) fn smoke_state(&self, cx: &App) -> String {
        let stage = match &self.stage {
            Stage::Search(search) => format!(
                "commands selected={} query={:?} unavailable={:?} results={} hover={:?}",
                search
                    .selected()
                    .map_or("", |matched| matched.spec.id.as_str()),
                search.query,
                search.selected().and_then(|matched| self
                    .availability
                    .get(&matched.spec.id)),
                search.results.len(),
                self.hover,
            ),
            Stage::Slots(editor) => format!(
                "slots command={} requested={} active={} picker={} selected={:?} chips={} picker_rows={:?}",
                editor.spec().id,
                editor.requested(),
                editor.active().spec.name,
                self.picker.len(),
                self.picker_value().map(|value| display_value(&value)),
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
        let scroll_offset = -f32::from(self.scroll.offset().y);
        let (scrollbar_x, scrollbar_thumb_y) =
            self.scrollbar.geometry().map_or((-1.0, -1.0), |geometry| {
                (
                    f32::from(self.scroll.bounds().right()) - 4.0,
                    f32::from(self.scroll.bounds().top())
                        + geometry.thumb_start
                        + geometry.thumb_size / 2.0,
                )
            });
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
            self.scrollbar.dragging(),
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
/// `⇥ options`, whose key draws as an icon.
fn tag(hint: &KeyHint, swatch: Swatch, dashed: bool) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(4.0))
        .when(dashed, |tag| {
            tag.border_1().border_dashed().border_color(swatch.dim)
        })
        .when(!dashed, |tag| tag.bg(swatch.chip))
        .text_size(px(10.0))
        .text_color(swatch.muted)
        .child(hint.element(px(9.5), swatch.muted))
}

fn band(message: String, swatch: Swatch, error: bool) -> gpui::AnyElement {
    div()
        .px(px(12.0))
        .py(px(6.0))
        .border_b_1()
        .border_color(swatch.line)
        .text_size(px(12.5))
        .when(error, |band| {
            band.text_color(swatch.accent)
                .bg(swatch.accent.opacity(0.08))
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
        color: Some(swatch.accent),
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

/// Chips for the slot line, the picker list, bands beneath the line, and
/// footer hints.
type SlotRender = (
    Vec<gpui::AnyElement>,
    gpui::Stateful<gpui::Div>,
    Vec<gpui::AnyElement>,
    Vec<(KeyHint, String)>,
);

impl CommandPalette {
    #[expect(
        clippy::too_many_lines,
        reason = "one presentation tree for the result rows and hints"
    )]
    fn render_search(
        &self,
        search: &SearchState,
        swatch: Swatch,
        window: &Window,
        list_max: f32,
        cx: &mut Context<'_, Self>,
    ) -> (
        gpui::Stateful<gpui::Div>,
        Vec<gpui::AnyElement>,
        Vec<(KeyHint, String)>,
    ) {
        let mut list = div()
            .id("palette-results")
            .flex()
            .flex_col()
            .p(px(LIST_PADDING))
            .max_h(px(list_max))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(
                |palette, _: &ScrollWheelEvent, _, cx| {
                    palette.observe_scroll_wheel(cx);
                },
            ));
        if search.results.is_empty() {
            list = list.child(
                div()
                    .p(px(18.0))
                    .text_color(swatch.dim)
                    .text_size(px(13.0))
                    .child(format!("No commands match “{}”", search.query)),
            );
        }
        for (row, matched) in search.results.iter().enumerate() {
            let spec = matched.spec;
            let unavailable = self.availability.get(&spec.id).cloned();
            let selected = row == search.selected;
            let hovered = self.hover == Some(row);
            let badge = if spec.args.is_empty() {
                None
            } else if slots::runs_without_prompt(spec, &self.domain_for(spec)) {
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
            let recent = search.query.is_empty()
                && self.history.recency(spec.id).is_some();
            let mut meta = div().flex().flex_none().items_center().gap(px(6.0));
            if recent {
                meta = meta.child(tag(
                    &KeyHint::keys(&[], Platform::current())
                        .with_suffix("recent"),
                    swatch,
                    false,
                ));
            }
            if let Some((hint, dashed)) = badge {
                meta = meta.child(tag(&hint, swatch, dashed));
            }
            for key in self.shortcut_keys(spec, None) {
                meta = meta.child(key_cap(
                    &KeyHint::parse(&key, Platform::current()),
                    swatch,
                ));
            }
            meta = meta.child(
                div()
                    .text_size(px(10.0))
                    .text_color(swatch.dim)
                    .child(format!("{:?}", spec.scope).to_uppercase()),
            );
            let description = match &unavailable {
                Some(reason) => format!("Unavailable: {reason}"),
                None => spec.description.to_owned(),
            };
            let item = div()
                .id(("palette-command", row))
                .flex()
                .items_center()
                .gap(px(10.0))
                .px(px(8.0))
                .h(px(ROW_HEIGHT))
                .flex_none()
                .rounded(px(6.0))
                .cursor_pointer()
                .when(selected, |item| item.bg(swatch.selection))
                .when(hovered && !selected, |item| {
                    item.bg(swatch.fg.opacity(0.05))
                })
                .active(|item| item.bg(swatch.selection_pressed()))
                .when(unavailable.is_some(), |item| item.opacity(0.55))
                .on_hover(cx.listener(
                    move |palette, hovering: &bool, _, cx| {
                        palette.set_hover(row, *hovering, cx);
                    },
                ))
                .on_click(cx.listener(move |palette, _: &ClickEvent, _, cx| {
                    if let Stage::Search(search) = &mut palette.stage {
                        search.selected = row;
                    }
                    palette.confirm_search(cx);
                    cx.notify();
                }))
                .child(selection_bar(selected, swatch))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(highlighted_title(
                            spec.title,
                            &matched.title_indices,
                            swatch,
                            window,
                        ))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.5))
                                .text_color(if unavailable.is_some() {
                                    swatch.accent
                                } else {
                                    swatch.muted
                                })
                                .child(description),
                        ),
                )
                .child(meta);
            list = list.child(item);
        }
        let mut below = Vec::new();
        if let Some(message) = &self.diagnostic {
            below.push(band(message.clone(), swatch, true));
        }
        let selected_spec = search.selected().map(|matched| matched.spec);
        let has_args = selected_spec.is_some_and(|spec| !spec.args.is_empty());
        let runs = selected_spec.is_some_and(|spec| {
            spec.args.is_empty()
                || slots::runs_without_prompt(spec, &self.domain_for(spec))
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
        (list, below, hints)
    }

    fn set_hover(
        &mut self,
        row: usize,
        hovering: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if hovering {
            self.hover = Some(row);
            cx.notify();
        } else if self.hover == Some(row) {
            self.hover = None;
            cx.notify();
        }
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
                        .text_color(swatch.dim)
                        .child(
                            KeyHint::keys(&["tab"], Platform::current())
                                .element(px(9.0), swatch.dim),
                        ),
                )
            });
        Some(chip.into_any_element())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one presentation tree for the slot line, picker, and hints"
    )]
    fn render_slots(
        &self,
        editor: &SlotEditor,
        swatch: Swatch,
        list_max: f32,
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
        let mut list = div()
            .id("palette-results")
            .flex()
            .flex_col()
            .p(px(LIST_PADDING))
            .max_h(px(list_max))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(
                |palette, _: &ScrollWheelEvent, _, cx| {
                    palette.observe_scroll_wheel(cx);
                },
            ));
        if is_identity(slot.spec.kind) {
            let selected_index = self.picker_index();
            let domain = self.domain();
            let rows = domain.rows(slot.spec.kind);
            if self.picker.is_empty() {
                list = list.child(
                    div()
                        .p(px(18.0))
                        .text_color(swatch.dim)
                        .text_size(px(13.0))
                        .child(format!("No matching {}s", slot.spec.name)),
                );
            }
            let select_tab = huterm_protocol::lookup(ids::SELECT_TAB.as_str());
            for (row, index) in self.picker.iter().enumerate() {
                let Some(item) = rows.get(*index).copied() else {
                    continue;
                };
                let selected = selected_index == Some(row);
                let hovered = self.hover == Some(row);
                let key = item.position.and_then(|position| {
                    let index =
                        select_tab_shortcut_index(position, rows.len())?;
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
                let value = item.value.clone();
                let mut meta =
                    div().flex().flex_none().items_center().gap(px(6.0));
                if let Some(key) = key {
                    meta = meta.child(key_cap(
                        &KeyHint::parse(&key, Platform::current()),
                        swatch,
                    ));
                }
                let element = div()
                    .id(("palette-picker", row))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(8.0))
                    .h(px(ROW_HEIGHT))
                    .flex_none()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .when(selected, |item| item.bg(swatch.selection))
                    .when(hovered && !selected, |item| {
                        item.bg(swatch.fg.opacity(0.05))
                    })
                    .active(|item| item.bg(swatch.selection_pressed()))
                    .on_hover(cx.listener(
                        move |palette, hovering: &bool, _, cx| {
                            palette.set_hover(row, *hovering, cx);
                        },
                    ))
                    .on_click(cx.listener(
                        move |palette, _: &ClickEvent, _, cx| {
                            let listed =
                                palette.picker_position(&value).is_some();
                            palette.highlight =
                                clicked_highlight(value.clone(), listed);
                            palette.commit_slot(cx);
                            cx.notify();
                        },
                    ))
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
                                    .text_color(swatch.muted)
                                    .child(item.detail.clone()),
                            ),
                    )
                    .child(meta);
                list = list.child(element);
            }
        } else {
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
        let can_pop = editor.slots()[..active].iter().any(|slot| {
            matches!(slot.state, SlotState::Committed | SlotState::Prefilled)
        });
        let leave = if requested { "close" } else { "back" };
        hints.push((
            KeyHint::keys(&["backspace"], platform).with_suffix(" on empty"),
            if can_pop { "previous" } else { leave }.to_owned(),
        ));
        hints.push((KeyHint::keys(&["escape"], platform), leave.to_owned()));
        (line, list, below, hints)
    }
}

impl super::refresh::Animated for CommandPalette {
    fn animation_schedule(
        &self,
        now: Instant,
    ) -> crate::ui::animation::AnimationSchedule {
        self.scrollbar.schedule(now)
    }

    fn advance_animation(
        &mut self,
        now: Instant,
        _frame: bool,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.advance(now, cx);
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
        let palette = cx.entity();
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
        let (list, below, hints) = match &self.stage {
            Stage::Search(search) => {
                let (list, below, hints) =
                    self.render_search(search, swatch, window, list_max, cx);
                line = line
                    .child(div().text_color(swatch.dim).child("›"))
                    .child(self.input.clone())
                    .child(
                        div()
                            .ml_auto()
                            .flex_none()
                            .text_size(px(11.0))
                            .text_color(swatch.dim)
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
                (list, below, hints)
            }
            Stage::Slots(editor) => {
                let (chips, list, below, hints) =
                    self.render_slots(editor, swatch, list_max, cx);
                line = line.children(chips);
                (list, below, hints)
            }
        };

        let footer = footer_hints(hints, swatch);

        let scrollbar = self.scrollbar.elements(
            "palette-scrollbar",
            swatch.scrollbar,
            &palette,
            |palette| &mut palette.scrollbar,
            cx,
        );

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
                    .relative()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(list)
                    .children(scrollbar),
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
