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
    FontWeight, Hsla, KeyBindingContextPredicate, KeyContext, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render, ScrollHandle,
    ScrollWheelEvent, Subscription, WeakEntity, Window, div, hsla, point,
    prelude::*, px,
};
use huterm_config::PalettePlacement;
use huterm_core::HierarchySnapshot;
use huterm_protocol::{
    ArgumentKind, CommandArgument, CommandError, CommandId, CommandInvocation,
    CommandOutcome, CommandScope, CommandSpec, CommandValue, SessionId, TabId,
    TerminalId, WorkspaceId, catalog, ids,
};
use search::{CommandHistory, CommandMatch, CommandSearch, PickerMatcher};
use slots::{
    Commit, Exit, Slot, SlotDomain, SlotEditor, SlotState, is_identity,
};

use super::TerminalView;
use crate::keymap::InstalledKeymap;
use crate::scroll::{
    IndicatorVisibility, ScrollbarExpansion, ScrollbarGeometry, TrackMargins,
};
use crate::ui::text_field::{Changed, TextField};

const PAGE_STEP: isize = 8;

#[derive(Clone)]
pub(super) struct PaletteTarget {
    pub(super) session: Option<SessionId>,
    pub(super) workspace: Option<WorkspaceId>,
    pub(super) tab: Option<TabId>,
    pub(super) terminal: Option<TerminalId>,
    pub(super) terminal_view: Option<WeakEntity<TerminalView>>,
    pub(super) contexts: Vec<KeyContext>,
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
/// Distance from the window's top edge for top placement, and the minimum
/// for centred placement in short windows.
const PANEL_TOP_INSET: f32 = 36.0;
/// Pointer strip at the list's right edge that owns scrollbar gestures.
const SCROLLBAR_WIDTH: f32 = 12.0;
const SCROLLBAR_EXPANDED_WIDTH: f32 = 18.0;

/// Theme colours the palette derives its presentation from.
#[derive(Clone, Copy, Debug)]
pub(super) struct PaletteColors {
    pub(super) foreground: Hsla,
    pub(super) background: Hsla,
    pub(super) selection: Hsla,
    pub(super) accent: Hsla,
}

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
    pub(super) colors: PaletteColors,
    pub(super) placement: PalettePlacement,
    pub(super) history: OwnedHistory,
    pub(super) profiles: Vec<QuakeProfileRow>,
    /// A command invoked interactively without its required arguments.
    pub(super) request: Option<&'a CommandInvocation>,
    /// A query retained from a recent cancellation.
    pub(super) retained_query: Option<String>,
    /// Tabs in most-recently-used order with the active tab last.
    pub(super) tab_order: Vec<TabId>,
}

#[derive(Clone, Debug)]
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
        tab_order: &[TabId],
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
                detail: "session".to_owned(),
                position: None,
                workspace: None,
                custom_name: session.custom_name().is_some(),
            })
            .collect();
        let workspaces = snapshot
            .workspaces
            .iter()
            .map(|workspace| IdentityRow {
                value: CommandValue::Workspace(workspace.id),
                label: workspace.display_name().to_owned(),
                detail: session_names
                    .get(&workspace.session_id)
                    .map_or("unknown session", String::as_str)
                    .to_owned(),
                position: None,
                workspace: Some(workspace.id),
                custom_name: workspace.custom_name().is_some(),
            })
            .collect();
        let mut tabs: Vec<IdentityRow> = snapshot
            .workspaces
            .iter()
            .flat_map(|workspace| {
                workspace.tabs.iter().enumerate().map(|(index, tab)| {
                    IdentityRow {
                        value: CommandValue::Tab(tab.id),
                        // Window tab records are initial snapshots, so a
                        // custom name set later is only in the hierarchy.
                        label: tab
                            .custom_name()
                            .map(str::to_owned)
                            .or_else(|| live_titles.get(&tab.id).cloned())
                            .unwrap_or_else(|| tab.display_name("").to_owned()),
                        detail: format!(
                            "{} › {}",
                            session_names
                                .get(&workspace.session_id)
                                .map_or("unknown session", String::as_str),
                            workspace_names
                                .get(&workspace.id)
                                .map_or("unknown workspace", String::as_str),
                        ),
                        position: i64::try_from(index + 1).ok(),
                        workspace: Some(workspace.id),
                        custom_name: tab.custom_name().is_some(),
                    }
                })
            })
            .collect();
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

    fn rows(&self, kind: ArgumentKind) -> &[IdentityRow] {
        match kind {
            ArgumentKind::Session => &self.sessions,
            ArgumentKind::Workspace => &self.workspaces,
            ArgumentKind::Tab => &self.tabs,
            _ => &[],
        }
    }
}

/// The slot editor's view of identity domains: the hierarchy snapshot for
/// sessions, workspaces, and tabs; configured profiles for quake commands.
struct DomainView<'a> {
    hierarchy: Option<&'a PaletteHierarchy>,
    profiles: &'a [IdentityRow],
    target: &'a PaletteTarget,
    /// Window-scoped commands can only act on this window's tabs.
    window_only: bool,
}

impl DomainView<'_> {
    /// Rows for `kind` in domain order; `None` while the hierarchy loads.
    fn rows(&self, kind: ArgumentKind) -> Option<Vec<&IdentityRow>> {
        match kind {
            ArgumentKind::QuakeProfile => Some(self.profiles.iter().collect()),
            ArgumentKind::Session
            | ArgumentKind::Workspace
            | ArgumentKind::Tab => Some(
                self.hierarchy?
                    .rows(kind)
                    .iter()
                    .filter(|row| {
                        !(self.window_only && kind == ArgumentKind::Tab)
                            || row.workspace == self.target.workspace
                    })
                    .collect(),
            ),
            ArgumentKind::Bool
            | ArgumentKind::Integer { .. }
            | ArgumentKind::Text => Some(Vec::new()),
        }
    }

    fn label(
        &self,
        kind: ArgumentKind,
        value: &CommandValue,
    ) -> Option<String> {
        self.rows(kind)?
            .into_iter()
            .find(|row| &row.value == value)
            .map(|row| row.label.clone())
    }
}

impl SlotDomain for DomainView<'_> {
    fn values(&self, kind: ArgumentKind) -> Option<Vec<CommandValue>> {
        self.rows(kind)
            .map(|rows| rows.into_iter().map(|row| row.value.clone()).collect())
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

/// What Enter was waiting on when an identity domain had not loaded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pending {
    /// Enter on a command row whose prompting decision needs the domain.
    Confirm,
    /// Enter in an identity slot whose rows have not arrived.
    Commit,
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
    /// Set while the palette writes the field itself, so the `Changed`
    /// subscription does not treat its own write as user input.
    syncing: bool,
    engine: CommandSearch,
    matcher: PickerMatcher,
    history: OwnedHistory,
    listed: Vec<&'static CommandSpec>,
    hierarchy: Option<PaletteHierarchy>,
    profiles: Vec<IdentityRow>,
    tab_order: Vec<TabId>,
    availability: HashMap<CommandId, String>,
    keymap: InstalledKeymap,
    colors: PaletteColors,
    placement: PalettePlacement,
    diagnostic: Option<String>,
    pending: Option<Pending>,
    /// Filtered row indices for the active identity slot.
    picker: Vec<usize>,
    /// The identity the user last highlighted in the picker, so selection
    /// survives reordering and refresh.
    picker_selected: Option<CommandValue>,
    hover: Option<usize>,
    scroll: ScrollHandle,
    /// Fading overlay scrollbar on the list; shown after list changes and
    /// scrolling so it also signals rows beyond the visible six.
    indicator: IndicatorVisibility,
    /// Widens the scrollbar while the pointer is over it or dragging.
    scrollbar_expansion: ScrollbarExpansion,
    scrollbar_hovering: bool,
    /// Pointer distance from the thumb's top while dragging it.
    scrollbar_drag: Option<f32>,
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
            tab_order,
        } = open;
        let input = cx
            .new(|cx| TextField::new("Search commands", colors.foreground, cx));
        let subscription =
            cx.subscribe(&input, |palette, input, _: &Changed, cx| {
                if palette.syncing {
                    return;
                }
                let text = input.read(cx).text().to_owned();
                palette.input_changed(&text);
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
                selected: 0,
            }),
            return_query: None,
            input,
            syncing: false,
            engine: CommandSearch::new(),
            matcher: PickerMatcher::new(),
            history,
            listed,
            hierarchy: None,
            profiles: profile_rows(profiles),
            tab_order,
            availability,
            keymap,
            colors,
            placement,
            diagnostic: None,
            pending: None,
            picker: Vec::new(),
            picker_selected: None,
            hover: None,
            scroll: ScrollHandle::new(),
            indicator: IndicatorVisibility::default(),
            scrollbar_expansion: ScrollbarExpansion::default(),
            scrollbar_hovering: false,
            scrollbar_drag: None,
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
            palette.enter_slots(editor, cx);
        } else {
            let query = retained_query.unwrap_or_default();
            palette.rank(&query);
            palette.syncing = true;
            palette.input.update(cx, |input, cx| {
                input.set_text(query, cx);
                input.select_all_text(cx);
            });
            palette.syncing = false;
        }
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

    /// Applies reloaded theme colours, placement, and profiles to an open
    /// palette.
    pub(super) fn set_presentation(
        &mut self,
        colors: PaletteColors,
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

    pub(super) fn set_hierarchy(
        &mut self,
        target: huterm_core::SelectionTarget,
        snapshot: &HierarchySnapshot,
        live_titles: &HashMap<TabId, String>,
        cx: &mut Context<'_, Self>,
    ) {
        self.target.session = Some(target.session);
        self.target.workspace = target.workspace;
        self.target.tab = target.tab;
        self.hierarchy = Some(PaletteHierarchy::from_snapshot(
            snapshot,
            live_titles,
            &self.tab_order,
        ));
        self.diagnostic = None;
        self.refresh_picker_rows();
        self.prefill_name(cx);
        match self.pending.take() {
            Some(Pending::Confirm) => self.confirm_search(cx),
            Some(Pending::Commit) => self.commit_slot(cx),
            None => {}
        }
        cx.notify();
    }

    /// The hierarchy request failed; commands that need no identity still
    /// run, and any pending Enter is dropped.
    pub(super) fn hierarchy_failed(
        &mut self,
        error: impl Into<String>,
        cx: &mut Context<'_, Self>,
    ) {
        self.pending = None;
        self.set_error(error, cx);
    }

    fn domain_for(&self, spec: &CommandSpec) -> DomainView<'_> {
        DomainView {
            hierarchy: self.hierarchy.as_ref(),
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
            hierarchy: self.hierarchy.as_ref(),
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

    /// Reveals the list scrollbar; the window pump fades it out.
    fn show_scrollbar(&mut self) {
        self.indicator.activate(Instant::now());
    }

    /// Advances the scrollbar fade and expansion from the window refresh
    /// pump.
    pub(super) fn advance(&mut self, now: Instant, cx: &mut Context<'_, Self>) {
        let interacting =
            self.scrollbar_hovering || self.scrollbar_drag.is_some();
        let mut changed = self.indicator.update(now, interacting);
        changed |= self.scrollbar_expansion.update(
            now,
            self.indicator.opacity > 0.0,
            interacting,
        );
        if changed {
            cx.notify();
        }
    }

    /// Scrollbar geometry for the list's current overflow, if any.
    fn scrollbar_geometry(&self) -> Option<ScrollbarGeometry> {
        let viewport = f32::from(self.scroll.bounds().size.height);
        let overflow = f32::from(self.scroll.max_offset().height);
        ScrollbarGeometry::for_pixels(
            viewport,
            viewport + overflow,
            viewport,
            -f32::from(self.scroll.offset().y),
            TrackMargins::EVEN,
        )
    }

    /// Whether `position` is over the scrollbar strip at the list's right
    /// edge, with the strip's local y.
    fn scrollbar_hit(&self, position: gpui::Point<Pixels>) -> Option<f32> {
        let geometry = self.scrollbar_geometry()?;
        let bounds = self.scroll.bounds();
        let width = if self.scrollbar_expansion.active() {
            SCROLLBAR_EXPANDED_WIDTH
        } else {
            SCROLLBAR_WIDTH
        };
        let y = f32::from(position.y - bounds.top());
        (self.indicator.opacity > 0.0
            && position.x >= bounds.right() - px(width)
            && position.x < bounds.right()
            && geometry.track_contains(y))
        .then_some(y)
    }

    fn scrollbar_pointer_moved(
        &mut self,
        position: gpui::Point<Pixels>,
        cx: &mut Context<'_, Self>,
    ) {
        let hovering = self.scrollbar_hit(position).is_some();
        if hovering {
            self.scrollbar_expansion.activate(Instant::now());
            self.show_scrollbar();
        }
        if hovering != self.scrollbar_hovering {
            self.scrollbar_hovering = hovering;
            cx.notify();
        }
    }

    /// Mouse down on the strip: grab the thumb, or jump it under the pointer
    /// and grab it there.
    fn scrollbar_press(
        &mut self,
        position: gpui::Point<Pixels>,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(y) = self.scrollbar_hit(position) else {
            return false;
        };
        let Some(geometry) = self.scrollbar_geometry() else {
            return false;
        };
        let grab = if geometry.contains(y) {
            y - geometry.thumb_start
        } else {
            let grab = geometry.thumb_size / 2.0;
            self.scrollbar_seek(geometry, y - grab);
            grab
        };
        self.scrollbar_drag = Some(grab);
        self.scrollbar_expansion.activate(Instant::now());
        self.show_scrollbar();
        cx.notify();
        true
    }

    fn scrollbar_drag_to(
        &mut self,
        position: gpui::Point<Pixels>,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(grab) = self.scrollbar_drag else {
            return;
        };
        let Some(geometry) = self.scrollbar_geometry() else {
            return;
        };
        let y = f32::from(position.y - self.scroll.bounds().top());
        self.scrollbar_seek(geometry, y - grab);
        self.show_scrollbar();
        cx.notify();
    }

    fn scrollbar_release(&mut self, cx: &mut Context<'_, Self>) {
        if self.scrollbar_drag.take().is_some() {
            cx.notify();
        }
    }

    fn scrollbar_seek(&self, geometry: ScrollbarGeometry, thumb_start: f32) {
        let offset = geometry.pixel_offset_for_thumb_start(thumb_start);
        self.scroll.set_offset(point(px(0.0), px(-offset)));
    }

    fn input_changed(&mut self, text: &str) {
        self.diagnostic = None;
        self.pending = None;
        match &mut self.stage {
            Stage::Search(_) => self.rank(text),
            Stage::Slots(editor) => {
                editor.set_text(text);
                self.picker_selected = None;
                self.refresh_picker_rows();
            }
        }
    }

    fn refresh_picker_rows(&mut self) {
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
            .map(|rows| {
                rows.into_iter()
                    .map(|row| (row.label.clone(), row.detail.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.picker = self.matcher.filter(editor.text(), rows.into_iter());
        if let Some(index) = self.picker_index() {
            self.scroll.scroll_to_item(index);
        }
        self.show_scrollbar();
    }

    /// Row index into the filtered picker that is currently highlighted.
    fn picker_index(&self) -> Option<usize> {
        let Stage::Slots(editor) = &self.stage else {
            return None;
        };
        let domain = self.domain();
        let rows = domain.rows(editor.active().spec.kind)?;
        let position = |value: &CommandValue| {
            self.picker.iter().position(|index| {
                rows.get(*index).map(|row| &row.value) == Some(value)
            })
        };
        self.picker_selected
            .as_ref()
            .and_then(position)
            .or_else(|| editor.active().value.as_ref().and_then(position))
            .or_else(|| (!self.picker.is_empty()).then_some(0))
    }

    fn picker_value_at(&self, row: usize) -> Option<CommandValue> {
        let Stage::Slots(editor) = &self.stage else {
            return None;
        };
        let domain = self.domain();
        let rows = domain.rows(editor.active().spec.kind)?;
        rows.get(*self.picker.get(row)?)
            .map(|row| row.value.clone())
    }

    fn picker_value(&self) -> Option<CommandValue> {
        self.picker_value_at(self.picker_index()?)
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        self.pending = None;
        self.diagnostic = None;
        match &mut self.stage {
            Stage::Search(search) => {
                search.move_selection(delta);
                self.scroll.scroll_to_item(search.selected);
            }
            Stage::Slots(_) => {
                if let Some(index) = self.picker_index() {
                    let next = index
                        .saturating_add_signed(delta)
                        .min(self.picker.len().saturating_sub(1));
                    self.picker_selected = self.picker_value_at(next);
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
        self.picker_selected = None;
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
        match slots::runs_without_prompt(spec, &self.domain_for(spec)) {
            Some(true) => {
                let editor =
                    SlotEditor::new(spec, &[], &self.domain_for(spec), false);
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
            Some(false) => self.expand_search(cx),
            None => {
                self.pending = Some(Pending::Confirm);
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
        self.enter_slots(editor, cx);
    }

    /// Enter in a slot.
    fn commit_slot(&mut self, cx: &mut Context<'_, Self>) {
        let picked = self.picker_value();
        let domain = DomainView {
            hierarchy: self.hierarchy.as_ref(),
            profiles: &self.profiles,
            target: &self.target,
            window_only: self.window_only(),
        };
        let Stage::Slots(editor) = &mut self.stage else {
            return;
        };
        match editor.commit(picked, &domain) {
            Ok(Commit::Run(invocation)) => {
                cx.emit(PaletteEvent::Execute(invocation));
            }
            Ok(Commit::Next) => {
                self.picker_selected = None;
                self.sync_input(cx);
            }
            Ok(Commit::Pending) => {
                self.pending = Some(Pending::Commit);
                cx.notify();
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
        let domain = DomainView {
            hierarchy: self.hierarchy.as_ref(),
            profiles: &self.profiles,
            target: &self.target,
            window_only: self.window_only(),
        };
        let Stage::Slots(editor) = &mut self.stage else {
            return;
        };
        match editor.next_slot(picked, &domain) {
            Ok(()) => {
                self.picker_selected = None;
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
            self.picker_selected = None;
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
        self.pending = None;
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
        self.pending = None;
        let Stage::Slots(editor) = &mut self.stage else {
            return;
        };
        match editor.pop() {
            None => {
                self.picker_selected = None;
                self.sync_input(cx);
            }
            Some(exit) => self.leave_slots(exit, cx),
        }
    }

    fn edit_slot(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Stage::Slots(editor) = &mut self.stage {
            editor.edit(index);
            self.picker_selected = None;
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
        self.syncing = true;
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder);
            input.set_text(text, cx);
        });
        self.syncing = false;
        self.diagnostic = None;
        self.refresh_picker_rows();
        self.prefill_name(cx);
        cx.notify();
    }

    /// Seeds an untouched rename name slot with the target's current custom
    /// name, selected so typing replaces it and Enter keeps it. Does nothing
    /// once the user has typed or committed a name.
    fn prefill_name(&mut self, cx: &mut Context<'_, Self>) {
        let Stage::Slots(editor) = &self.stage else {
            return;
        };
        let slot = editor.active();
        if slot.spec.kind != ArgumentKind::Text
            || slot.value.is_some()
            || !editor.text().is_empty()
        {
            return;
        }
        let Some(name) = self.current_name(editor) else {
            return;
        };
        if let Stage::Slots(editor) = &mut self.stage {
            editor.set_text(&name);
        }
        self.syncing = true;
        self.input.update(cx, |input, cx| {
            input.set_text(name, cx);
            input.select_all_text(cx);
        });
        self.syncing = false;
    }

    /// The custom name a rename would replace, when its target has one.
    fn current_name(&self, editor: &SlotEditor) -> Option<String> {
        let (_, argument) = self.rename_target(editor)?;
        let slot = editor
            .slots()
            .iter()
            .find(|slot| slot.spec.name == argument)?;
        let value = slot.value.as_ref()?;
        self.domain()
            .rows(slot.spec.kind)?
            .into_iter()
            .find(|row| &row.value == value)
            .filter(|row| row.custom_name)
            .map(|row| row.label.clone())
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
        if !editor.spec().id.as_str().starts_with("rename_") {
            return None;
        }
        let slot = editor
            .slots()
            .iter()
            .find(|slot| is_identity(slot.spec.kind))?;
        let label =
            self.domain().label(slot.spec.kind, slot.value.as_ref()?)?;
        Some((label, slot.spec.name))
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
            ids::TEXT_DELETE_BACKWARD
            | ids::TEXT_DELETE_FORWARD
            | ids::TEXT_DELETE_WORD_BACKWARD
            | ids::TEXT_DELETE_LINE_START
            | ids::TEXT_MOVE_LEFT
            | ids::TEXT_MOVE_RIGHT
            | ids::TEXT_MOVE_WORD_LEFT
            | ids::TEXT_MOVE_WORD_RIGHT
            | ids::TEXT_LINE_START
            | ids::TEXT_LINE_END
            | ids::TEXT_SELECT_ALL
            | ids::TEXT_COPY
            | ids::TEXT_PASTE => {
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
        self.keymap
            .shortcuts(spec.id, &self.target.contexts, args)
            .iter()
            .filter(|binding| args.is_some() || binding.args.is_empty())
            .take(2)
            .map(|binding| binding.key.clone())
            .collect()
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
                "slots command={} requested={} active={} picker={} selected={:?} chips={} pending={}",
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
                self.pending.is_some(),
            ),
        };
        format!(
            "{stage} input={:?} diagnostic={:?}",
            self.input.read(cx).text(),
            self.diagnostic
        )
    }

    /// Scrim click: close regardless of stage.
    fn close_from_scrim(&mut self, cx: &mut Context<'_, Self>) {
        let query = match &self.stage {
            Stage::Search(search) if !search.query.is_empty() => {
                Some(search.query.clone())
            }
            _ => None,
        };
        cx.emit(PaletteEvent::Cancel { query });
    }
}

/// The panel's distance from the top of a window `height` points tall. Centred
/// placement centres the panel at its maximum height, so the input line stays
/// put while the result list grows and shrinks beneath it.
fn panel_top(placement: PalettePlacement, height: f32) -> f32 {
    match placement {
        PalettePlacement::Top => PANEL_TOP_INSET,
        PalettePlacement::Center => {
            ((height - PANEL_MAX_HEIGHT) / 2.0).max(PANEL_TOP_INSET)
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

impl EventEmitter<PaletteEvent> for CommandPalette {}

// ---- presentation ---------------------------------------------------------

#[derive(Clone, Copy)]
struct Swatch {
    fg: Hsla,
    bg: Hsla,
    muted: Hsla,
    dim: Hsla,
    line: Hsla,
    selection: Hsla,
    accent: Hsla,
    chip: Hsla,
}

impl Swatch {
    fn new(colors: PaletteColors) -> Self {
        Self {
            fg: colors.foreground,
            bg: colors.background,
            muted: colors.foreground.opacity(0.62),
            dim: colors.foreground.opacity(0.38),
            line: colors.foreground.opacity(0.1),
            selection: colors.selection.opacity(0.35),
            accent: colors.accent,
            chip: colors.foreground.opacity(0.1),
        }
    }
}

fn key_cap(key: &str, swatch: Swatch) -> impl IntoElement {
    div()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(4.0))
        .bg(swatch.chip)
        .border_1()
        .border_color(swatch.line)
        .text_size(px(10.5))
        .text_color(swatch.fg)
        .child(key.to_owned())
}

fn tag(text: String, swatch: Swatch, dashed: bool) -> impl IntoElement {
    div()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(4.0))
        .when(dashed, |tag| {
            tag.border_1().border_dashed().border_color(swatch.dim)
        })
        .when(!dashed, |tag| tag.bg(swatch.chip))
        .text_size(px(10.0))
        .text_color(swatch.muted)
        .child(text)
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

fn highlighted_title(
    title: &str,
    indices: &[u32],
    swatch: Swatch,
) -> gpui::AnyElement {
    let mut element = div().flex().text_size(px(13.5));
    if indices.is_empty() {
        return element.child(title.to_owned()).into_any_element();
    }
    for (index, ch) in title.char_indices() {
        let matched = indices.iter().any(|i| *i as usize == index);
        element = element.child(if matched {
            div()
                .text_color(swatch.accent)
                .font_weight(FontWeight::SEMIBOLD)
                .child(ch.to_string())
        } else {
            div().child(ch.to_string())
        });
    }
    element.into_any_element()
}

/// Chips for the slot line, the picker list, bands beneath the line, and
/// footer hints.
type SlotRender = (
    Vec<gpui::AnyElement>,
    gpui::Stateful<gpui::Div>,
    Vec<gpui::AnyElement>,
    Vec<(String, String)>,
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
        cx: &mut Context<'_, Self>,
    ) -> (
        gpui::Stateful<gpui::Div>,
        Vec<gpui::AnyElement>,
        Vec<(String, String)>,
    ) {
        let mut list = div()
            .id("palette-results")
            .flex()
            .flex_col()
            .p(px(LIST_PADDING))
            .max_h(px(LIST_MAX_HEIGHT))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(
                |palette, _: &ScrollWheelEvent, _, cx| {
                    palette.show_scrollbar();
                    cx.notify();
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
            } else if slots::runs_without_prompt(spec, &self.domain_for(spec))
                == Some(true)
            {
                Some(("⇥ options".to_owned(), true))
            } else {
                Some(("↩ prompts".to_owned(), false))
            };
            let recent = search.query.is_empty()
                && self.history.recency(spec.id).is_some();
            let mut meta = div().flex().items_center().gap(px(6.0));
            if recent {
                meta = meta.child(tag("recent".to_owned(), swatch, false));
            }
            if let Some((text, dashed)) = badge {
                meta = meta.child(tag(text, swatch, dashed));
            }
            for key in self.shortcut_keys(spec, None) {
                meta = meta.child(key_cap(&key, swatch));
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
                        .child(highlighted_title(
                            spec.title,
                            &matched.title_indices,
                            swatch,
                        ))
                        .child(
                            div()
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
        if self.pending.is_some() {
            below.push(band("Loading targets…".to_owned(), swatch, false));
        }
        let selected_spec = search.selected().map(|matched| matched.spec);
        let has_args = selected_spec.is_some_and(|spec| !spec.args.is_empty());
        let runs = selected_spec.is_some_and(|spec| {
            spec.args.is_empty()
                || slots::runs_without_prompt(spec, &self.domain_for(spec))
                    == Some(true)
        });
        let mut hints = vec![
            ("↑↓".to_owned(), "navigate".to_owned()),
            ("↩".to_owned(), if runs { "run" } else { "next" }.to_owned()),
        ];
        if has_args {
            hints.push((
                "⇥".to_owned(),
                if runs { "set arguments" } else { "arguments" }.to_owned(),
            ));
        }
        hints.push(("esc".to_owned(), "close".to_owned()));
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
            .when(editable, gpui::Styled::cursor_pointer)
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
                        .child("⇥"),
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
                .when(!requested, gpui::Styled::cursor_pointer)
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
        if self.pending.is_some() {
            below.push(band("Loading targets…".to_owned(), swatch, false));
        }
        let slot = editor.active();
        let mut list = div()
            .id("palette-results")
            .flex()
            .flex_col()
            .p(px(LIST_PADDING))
            .max_h(px(LIST_MAX_HEIGHT))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(
                |palette, _: &ScrollWheelEvent, _, cx| {
                    palette.show_scrollbar();
                    cx.notify();
                },
            ));
        if is_identity(slot.spec.kind) {
            let selected_index = self.picker_index();
            let domain = self.domain();
            let rows = domain.rows(slot.spec.kind);
            if rows.is_none() {
                list = list.child(
                    div()
                        .p(px(18.0))
                        .text_color(swatch.dim)
                        .child("Loading targets…"),
                );
            } else if self.picker.is_empty() {
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
                let Some(item) =
                    rows.as_ref().and_then(|rows| rows.get(*index).copied())
                else {
                    continue;
                };
                let selected = selected_index == Some(row);
                let hovered = self.hover == Some(row);
                let key = item.position.and_then(|position| {
                    self.shortcut_keys(
                        select_tab?,
                        Some(&[CommandArgument::new(
                            "index",
                            CommandValue::Integer(position),
                        )]),
                    )
                    .into_iter()
                    .next()
                });
                let value = item.value.clone();
                let mut meta = div().flex().items_center().gap(px(6.0));
                if let Some(key) = key {
                    meta = meta.child(key_cap(&key, swatch));
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
                    .on_hover(cx.listener(
                        move |palette, hovering: &bool, _, cx| {
                            palette.set_hover(row, *hovering, cx);
                        },
                    ))
                    .on_click(cx.listener(
                        move |palette, _: &ClickEvent, _, cx| {
                            palette.picker_selected = Some(value.clone());
                            palette.commit_slot(cx);
                            cx.notify();
                        },
                    ))
                    .child(selection_bar(selected, swatch))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .child(item.label.clone()),
                            )
                            .child(
                                div()
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
            || is_identity(slot.spec.kind);
        let mut hints = vec![(
            "↩".to_owned(),
            if runs { "run" } else { "next" }.to_owned(),
        )];
        if editor.slots().len() > 1 {
            hints.push(("⇥".to_owned(), "next argument".to_owned()));
        }
        let can_pop = editor.slots()[..active].iter().any(|slot| {
            matches!(slot.state, SlotState::Committed | SlotState::Prefilled)
        });
        let leave = if requested { "close" } else { "back" };
        hints.push((
            "⌫ on empty".to_owned(),
            if can_pop { "previous" } else { leave }.to_owned(),
        ));
        hints.push(("esc".to_owned(), leave.to_owned()));
        (line, list, below, hints)
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
        let top =
            panel_top(self.placement, f32::from(window.viewport_size().height));
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
                    self.render_search(search, swatch, cx);
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
                    self.render_slots(editor, swatch, cx);
                line = line.children(chips);
                (list, below, hints)
            }
        };

        let mut footer = div()
            .flex()
            .items_center()
            .gap(px(14.0))
            .px(px(12.0))
            .py(px(6.0))
            .border_t_1()
            .border_color(swatch.line)
            .bg(swatch.fg.opacity(0.03))
            .text_size(px(11.5))
            .text_color(swatch.muted);
        for (key, label) in hints {
            footer = footer.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .child(key_cap(&key, swatch))
                    .child(label),
            );
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
            }])
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_mouse_down(
                MouseButton::Left,
                move |_: &MouseDownEvent, window, cx| {
                    input_focus.focus(window);
                    cx.stop_propagation();
                },
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|palette, _: &MouseUpEvent, _, cx| {
                    palette.scrollbar_release(cx);
                    cx.stop_propagation();
                }),
            )
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
                    .on_mouse_move(cx.listener(
                        |palette, event: &MouseMoveEvent, _, cx| {
                            palette.scrollbar_pointer_moved(event.position, cx);
                        },
                    ))
                    .on_hover(cx.listener(|palette, hovering: &bool, _, cx| {
                        if !hovering && palette.scrollbar_hovering {
                            palette.scrollbar_hovering = false;
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(
                            |palette, event: &MouseDownEvent, _, cx| {
                                if palette.scrollbar_press(event.position, cx) {
                                    cx.stop_propagation();
                                }
                            },
                        ),
                    )
                    .child(list)
                    .children(self.scrollbar_geometry().into_iter().flat_map(
                        |geometry| {
                            crate::ui::scrollbar::layers(
                                geometry,
                                self.indicator.opacity,
                                self.scrollbar_expansion.progress,
                                swatch.fg,
                            )
                        },
                    )),
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
            .on_mouse_move(cx.listener(
                |palette, event: &MouseMoveEvent, _, cx| {
                    palette.scrollbar_drag_to(event.position, cx);
                },
            ))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|palette, _: &MouseUpEvent, _, cx| {
                    palette.scrollbar_release(cx);
                    cx.stop_propagation();
                }),
            )
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
mod tests {
    use super::*;
    use huterm_core::Mux;
    use huterm_protocol::{
        CellSize, GridSize, RuntimeId, TerminalCommand, TerminalEngineKind,
    };

    fn target() -> PaletteTarget {
        PaletteTarget {
            session: None,
            workspace: None,
            tab: None,
            terminal: None,
            terminal_view: None,
            contexts: vec![
                KeyContext::parse("Workspace").unwrap(),
                KeyContext::parse("Terminal").unwrap(),
            ],
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
    fn palette_commands_are_hidden_from_a_terminal_context() {
        let target = target();
        let listed: Vec<_> = catalog()
            .iter()
            .filter(|spec| context_matches(spec.context, &target.contexts))
            .map(|spec| spec.id)
            .collect();
        assert!(listed.contains(&ids::RENAME_TAB));
        assert!(listed.contains(&ids::OPEN_COMMAND_PALETTE));
        assert!(!listed.contains(&ids::PALETTE_CONFIRM));
        assert!(!listed.contains(&ids::TEXT_COPY));
        let palette_context = [
            KeyContext::parse("Workspace palette").unwrap(),
            KeyContext::parse("Palette").unwrap(),
        ];
        assert!(context_matches(Some("Palette"), &palette_context));
    }

    #[test]
    fn tabs_follow_activation_order_with_the_active_tab_last() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let first = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
        let second = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
        let third = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
        let hierarchy = PaletteHierarchy::from_snapshot(
            &mux.capture_hierarchy(),
            &HashMap::new(),
            &[third.id, first.id, second.id],
        );
        let order: Vec<_> = hierarchy
            .tabs
            .iter()
            .map(|row| (row.value.clone(), row.position))
            .collect();
        assert_eq!(
            order,
            [
                (CommandValue::Tab(third.id), Some(3)),
                (CommandValue::Tab(first.id), Some(1)),
                (CommandValue::Tab(second.id), Some(2)),
            ]
        );
        mux.close_session(session).unwrap();
    }

    #[test]
    fn window_scoped_commands_list_only_this_windows_tabs() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let other = mux.create_workspace(session, None).unwrap();
        let mine = mux.open_tab(workspace, &terminal_command()).unwrap().tab;
        let theirs = mux.open_tab(other, &terminal_command()).unwrap().tab;
        let hierarchy = PaletteHierarchy::from_snapshot(
            &mux.capture_hierarchy(),
            &HashMap::new(),
            &[],
        );
        let target = PaletteTarget {
            session: Some(session),
            workspace: Some(workspace),
            tab: Some(mine.id),
            ..target()
        };
        let window_only = DomainView {
            hierarchy: Some(&hierarchy),
            profiles: &[],
            target: &target,
            window_only: true,
        };
        assert_eq!(
            window_only.values(ArgumentKind::Tab),
            Some(vec![CommandValue::Tab(mine.id)])
        );
        let runtime = DomainView {
            window_only: false,
            ..window_only
        };
        assert_eq!(
            runtime.values(ArgumentKind::Tab).map(|values| values.len()),
            Some(2)
        );
        assert!(
            runtime
                .label(ArgumentKind::Tab, &CommandValue::Tab(theirs.id))
                .is_some()
        );
        mux.close_session(session).unwrap();
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
            &[],
        );
        assert_eq!(hierarchy.sessions[0].label, hierarchy.sessions[1].label);
        assert_ne!(hierarchy.sessions[0].value, hierarchy.sessions[1].value);
        assert!(
            hierarchy
                .workspaces
                .iter()
                .find(
                    |row| row.value == CommandValue::Workspace(first_workspace)
                )
                .is_some_and(|row| row.custom_name)
        );

        let rename =
            huterm_protocol::lookup(ids::RENAME_WORKSPACE.as_str()).unwrap();
        let target = PaletteTarget {
            session: Some(first_session),
            workspace: Some(first_workspace),
            ..target()
        };
        let domain = DomainView {
            hierarchy: Some(&hierarchy),
            profiles: &[],
            target: &target,
            window_only: false,
        };
        let mut editor = SlotEditor::new(rename, &[], &domain, false);
        editor.set_text("chosen");
        editor.next_slot(None, &domain).unwrap();
        assert_eq!(editor.active().spec.name, "workspace");
        let commit = editor
            .commit(Some(CommandValue::Workspace(second_workspace)), &domain)
            .unwrap();
        let Commit::Run(invocation) = commit else {
            panic!("picking the workspace runs the rename");
        };
        assert_eq!(invocation.workspace("workspace"), Some(second_workspace));
        assert_eq!(invocation.text("name"), Some("chosen"));
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
        let hierarchy = PaletteHierarchy::from_snapshot(
            &mux.capture_hierarchy(),
            &HashMap::new(),
            &[],
        );

        let rename = huterm_protocol::lookup(ids::RENAME_TAB.as_str()).unwrap();
        let target = PaletteTarget {
            session: Some(session),
            workspace: Some(workspace),
            tab: Some(second.id),
            ..target()
        };
        let domain = DomainView {
            hierarchy: Some(&hierarchy),
            profiles: &[],
            target: &target,
            window_only: false,
        };
        let mut editor = SlotEditor::new(rename, &[], &domain, false);
        editor.set_text("chosen");
        let Commit::Run(invocation) = editor.commit(None, &domain).unwrap()
        else {
            panic!("one Enter runs the rename with the prefilled tab");
        };
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

    #[test]
    fn quake_profiles_form_a_strict_domain_with_a_default() {
        let profiles = profile_rows(vec![
            QuakeProfileRow {
                name: "default".into(),
                detail: "top".into(),
            },
            QuakeProfileRow {
                name: "logs".into(),
                detail: "right".into(),
            },
        ]);
        let target = target();
        let domain = DomainView {
            hierarchy: None,
            profiles: &profiles,
            target: &target,
            window_only: false,
        };
        let toggle =
            huterm_protocol::lookup(ids::TOGGLE_QUAKE.as_str()).unwrap();
        assert_eq!(slots::runs_without_prompt(toggle, &domain), Some(true));
        let editor = SlotEditor::new(toggle, &[], &domain, false);
        assert_eq!(editor.slots()[0].state, SlotState::Prefilled);
        assert_eq!(
            editor.invocation().unwrap().text("profile"),
            Some("default")
        );
        let mut picking = SlotEditor::new(toggle, &[], &domain, false);
        picking.edit(0);
        let Commit::Run(invocation) = picking
            .commit(Some(CommandValue::Text("logs".into())), &domain)
            .unwrap()
        else {
            panic!("picking a profile runs the command");
        };
        assert_eq!(invocation.text("profile"), Some("logs"));
    }

    #[test]
    fn owned_history_ranks_recent_commands_first() {
        let mut recent = RecentCommands::default();
        recent.record(ids::RENAME_TAB);
        let frequency = CommandFrequency::default();
        let history = OwnedHistory::capture(&HistoryView {
            recent: &recent,
            frequency: &frequency,
        });
        assert_eq!(history.recency(ids::RENAME_TAB), Some(0));
        assert_eq!(history.recency(ids::NEW_TAB), None);
        let runtime = RuntimeId::new(3);
        let _ = TabId::in_runtime(runtime, 1);
        let mut engine = CommandSearch::new();
        let ranked = engine.rank("", catalog().iter(), &history);
        assert_eq!(ranked[0].spec.id, ids::RENAME_TAB);
    }
}
