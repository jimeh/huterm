//! The accessibility text model of a terminal view: the rows it displays as
//! AccessKit text runs, its cursor as a hidden caret node, and its selection.
//!
//! The document is the displayed grid, clipped to the painted bounds as
//! [`GridGeometry`] clips painting and hit testing; scrollback is not part of
//! it. Its text is what Copy copies from the whole displayed grid, and a
//! selection inside it selects what Copy copies from that selection. Copy is
//! Ghostty's plain formatter with soft wraps joined and blanks trimmed, and
//! [`Document`] follows it step by step: empty cells and spaces are blanks,
//! written as spaces only once later text on their logical line follows; a
//! line break is written only once text follows it; a row part with no text
//! at all is a line break of its own. Each character sits on the displayed
//! row it is drawn on: a grapheme cell is one character, spacers add none,
//! and a row's line break follows its text. A cursor past its row's text
//! moves that row's line break to the cursor's cell, or stretches the
//! document's last character to it, in geometry only, so the caret's
//! character extents are the cursor's (see [`reach_cursor`]). A row with
//! no characters has no run, because
//! AccessKit's position and line lookups misbehave next to an empty one
//! (AccessKit #836 and #837). A displayed grid whose Copy text is empty
//! would then have no runs, and so no text interface and no caret, so its
//! document is one line break in the cursor's cell, or in the first cell
//! without a displayed cursor. Runs longer than [`RUN_LIMIT`]
//! characters continue in another run on the same line, since word starts
//! are `u8` indices.
//!
//! Geometry is in window physical pixels, as GPUI reports node bounds.
//! Building reads only the snapshot the frame paints and the view's own
//! state; it never calls the runtime.

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::accesskit::{
    Node, NodeId, Rect, Role, TextDirection, TextPosition, TextSelection,
};
use gpui::{A11ySubtreeBuilder, Bounds, Pixels};
use huterm_protocol::{
    BufferPoint, BufferRange, CellText, Cursor, CursorShape, GridSize,
    TerminalRow, TerminalSnapshot, Viewport,
};

use super::grid_geometry::GridGeometry;
use crate::renderer::{GridMetrics, cursor_bounds};

/// The most characters one run holds: AccessKit indexes word starts with
/// `u8`.
pub(super) const RUN_LIMIT: usize = 255;

/// What a view's accessibility model derives from.
#[derive(Clone, Copy, Debug)]
pub(super) struct Inputs<'a> {
    pub(super) snapshot: &'a TerminalSnapshot,
    /// The displayed grid inside the painted bounds, in window coordinates.
    pub(super) geometry: GridGeometry,
    /// The pointer selection's range, if it has one yet.
    pub(super) selection: Option<BufferRange>,
    pub(super) metrics: GridMetrics,
    /// Physical pixels per window point.
    pub(super) scale: f32,
}

/// Which run of which displayed row a node is.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct RunKey {
    pub(super) row: u16,
    pub(super) segment: u16,
}

/// A position in the model: a run and a character index in it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ModelPosition {
    pub(super) run: RunKey,
    pub(super) character_index: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ModelRun {
    pub(super) key: RunKey,
    /// The run's node, without its line links: those name node IDs, which
    /// only the builder of the parent node can derive.
    pub(super) node: Node,
    pub(super) continues: bool,
    pub(super) continued: bool,
}

/// A terminal's text runs, caret, and selection, published together.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Model {
    pub(super) runs: Vec<ModelRun>,
    /// The painted cursor rectangle, clipped to the displayed grid.
    pub(super) caret: Option<Rect>,
    /// The selection, or the caret as a collapsed one: anchor, then focus.
    pub(super) selection: Option<(ModelPosition, ModelPosition)>,
}

/// A synthetic child's identity under its terminal node: a run, or the
/// caret.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum ChildKey {
    Run(RunKey),
    Caret,
}

impl Model {
    /// The child nodes in document order, then the caret, and the
    /// selection naming them, with node IDs from `id`.
    pub(super) fn nodes(
        &self,
        id: impl Fn(ChildKey) -> NodeId,
    ) -> (Vec<(NodeId, Node)>, Option<TextSelection>) {
        let run_id = |key: RunKey| id(ChildKey::Run(key));
        let mut nodes = Vec::with_capacity(self.runs.len() + 1);
        for run in &self.runs {
            let mut node = run.node.clone();
            if run.continued {
                node.set_previous_on_line(run_id(RunKey {
                    segment: run.key.segment - 1,
                    ..run.key
                }));
            }
            if run.continues {
                node.set_next_on_line(run_id(RunKey {
                    segment: run.key.segment + 1,
                    ..run.key
                }));
            }
            nodes.push((run_id(run.key), node));
        }
        if let Some(bounds) = self.caret {
            let mut caret = Node::new(Role::Caret);
            caret.set_hidden();
            caret.set_bounds(bounds);
            nodes.push((id(ChildKey::Caret), caret));
        }
        let selection = self.selection.map(|(anchor, focus)| {
            let position = |position: ModelPosition| TextPosition {
                node: run_id(position.run),
                character_index: position.character_index,
            };
            TextSelection {
                anchor: position(anchor),
                focus: position(focus),
            }
        });
        (nodes, selection)
    }

    /// Adds the runs and the caret as children of the builder's node and
    /// sets its selection.
    pub(super) fn push(&self, builder: &mut A11ySubtreeBuilder<'_>) {
        let (nodes, selection) =
            self.nodes(|key| builder.synthetic_node_id(key));
        for (id, node) in nodes {
            builder.push_child(id, node);
        }
        if let Some(selection) = selection {
            builder.parent_node().set_text_selection(selection);
        }
    }
}

/// The shortest time between two publications while only the snapshot's
/// content changes.
pub(super) const PUBLISH_INTERVAL: Duration = Duration::from_millis(100);

/// Everything but the content a model depends on. A change to any of it
/// publishes at once.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    geometry: GridGeometry,
    selection: Option<BufferRange>,
    metrics: GridMetrics,
    scale: f32,
    viewport: Viewport,
    size: GridSize,
}

impl Layout {
    fn of(inputs: &Inputs<'_>) -> Self {
        Self {
            geometry: inputs.geometry,
            selection: inputs.selection,
            metrics: inputs.metrics,
            scale: inputs.scale,
            viewport: inputs.snapshot.viewport,
            size: inputs.snapshot.size,
        }
    }
}

/// What a frame did with the view's accessibility model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Publication {
    /// The model stands: nothing it depends on changed, or a rebuild came
    /// out the same.
    Unchanged,
    /// The frame publishes a new model.
    Published,
    /// Only content changed, too soon after the last publication; a frame
    /// after this long must publish it.
    Deferred(Duration),
}

/// Decides when a view's accessibility model changes. Output can change a
/// snapshot every frame, and every changed run makes the platform adapters
/// notify assistive technology, so content alone republishes at most once
/// per [`PUBLISH_INTERVAL`]. Selection, geometry, scale, and scroll changes
/// publish at once, and text, caret, and selection always change together
/// because they are one model.
#[derive(Debug, Default)]
pub(super) struct Publisher {
    builder: ModelBuilder,
    published: Rc<Model>,
    /// What the published model was last built from: rebuilding from the
    /// same inputs is skipped.
    basis: Option<(Layout, Arc<TerminalSnapshot>)>,
    published_at: Option<Instant>,
    /// Read only by tests and the macOS accessibility smoke.
    #[cfg(any(test, target_os = "macos"))]
    publications: u64,
}

impl Publisher {
    /// Brings the model up to date with `inputs` at `now`, unless only
    /// content changed within [`PUBLISH_INTERVAL`] of the last publication.
    pub(super) fn update(
        &mut self,
        inputs: &Inputs<'_>,
        snapshot: &Arc<TerminalSnapshot>,
        now: Instant,
    ) -> Publication {
        let layout = Layout::of(inputs);
        if let Some((basis, built)) = &self.basis
            && *basis == layout
        {
            if Arc::ptr_eq(built, snapshot) {
                return Publication::Unchanged;
            }
            if let Some(due) = self
                .published_at
                .map(|at| at + PUBLISH_INTERVAL)
                .filter(|due| now < *due)
            {
                return Publication::Deferred(due - now);
            }
        }
        let model = self.builder.build(inputs);
        self.basis = Some((layout, Arc::clone(snapshot)));
        self.publish(model, now)
    }

    /// Publishes the empty model of a view with no snapshot.
    pub(super) fn clear(&mut self, now: Instant) -> Publication {
        self.basis = None;
        self.publish(Model::default(), now)
    }

    fn publish(&mut self, model: Model, now: Instant) -> Publication {
        if model == *self.published {
            return Publication::Unchanged;
        }
        self.published = Rc::new(model);
        self.published_at = Some(now);
        #[cfg(any(test, target_os = "macos"))]
        {
            self.publications += 1;
        }
        Publication::Published
    }

    /// The model the view's node shows.
    pub(super) fn model(&self) -> Rc<Model> {
        Rc::clone(&self.published)
    }

    /// How many distinct models this view has published.
    #[cfg(any(test, target_os = "macos"))]
    pub(super) fn publications(&self) -> u64 {
        self.publications
    }
}

/// One run's text and its geometry relative to the row's left edge.
#[derive(Debug)]
struct Segment {
    value: Box<str>,
    lengths: Box<[u8]>,
    /// Relative to the segment's own left edge.
    positions: Box<[f32]>,
    widths: Box<[f32]>,
    word_starts: Box<[u8]>,
    /// The segment's left edge relative to the row's.
    x: f32,
    width: f32,
}

/// How Ghostty's plain formatter reads a cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Class {
    /// No text: never written, erased, skipped, or a background alone.
    Empty,
    /// A space, combining marks included. Copy trims it as it trims an
    /// empty cell, but a row part holding one has text.
    Space,
    /// Any other character, a no-break or ideographic space included.
    Text,
}

impl Class {
    fn of(text: &CellText) -> Self {
        if text.is_empty() {
            Self::Empty
        } else if text.as_str().starts_with(' ') {
            Self::Space
        } else {
            Self::Text
        }
    }
}

/// A cell that is not a spacer: one character of its row.
#[derive(Debug)]
struct CellChar {
    column: usize,
    class: Class,
    /// The character in [`RowCells::text`], for a text cell; Copy writes a
    /// blank as a space.
    text: Range<usize>,
    x: f32,
    width: f32,
}

/// What a displayed column's cell is to a selection.
#[derive(Clone, Copy, Debug)]
enum ColumnCell {
    /// Character `index` of the row.
    Char(usize),
    /// A wide character's spacer, which selects character `index`.
    Tail(usize),
    /// A spacer head, left at the end of a row by a wide character that
    /// wrapped onto the next.
    Head,
}

/// A displayed row's cells as the formatter reads them, built once per row
/// identity and geometry.
#[derive(Debug)]
struct RowCells {
    /// Keeps the row alive, so its address names it while cached.
    row: Arc<TerminalRow>,
    chars: Vec<CellChar>,
    text: String,
    columns: Box<[ColumnCell]>,
    /// The first column whose cell has text, spaces included. Copy's text
    /// of a selection ending on the row counts the row only from there.
    first_text: Option<usize>,
}

impl RowCells {
    fn character(&self, index: usize) -> &str {
        let cell = &self.chars[index];
        match cell.class {
            Class::Text => &self.text[cell.text.clone()],
            Class::Empty | Class::Space => " ",
        }
    }
}

/// Which of a row's characters the document holds, in this order.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct Plan {
    /// Blanks of earlier rows that Copy writes only after the line break
    /// of a row with no text between them and the text that follows. They
    /// lead the row, with no width.
    carried: u16,
    /// How many of the row's own characters, from its first.
    own: u16,
    /// Whether a line break follows them.
    newline: bool,
}

/// A displayed row's runs, built once per row identity, geometry, and
/// plan.
#[derive(Debug)]
struct RowText {
    row: Arc<TerminalRow>,
    /// Empty for a row with no characters.
    segments: Vec<Segment>,
    /// Characters in all segments.
    len: usize,
    /// Whether the last character is the row's line break.
    newline: bool,
}

impl RowText {
    fn position(&self, character: usize) -> (u16, usize) {
        let last = self.segments.len().saturating_sub(1);
        let segment = (character / RUN_LIMIT).min(last);
        let index = character - segment * RUN_LIMIT;
        (u16::try_from(segment).unwrap_or(u16::MAX), index)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct RowKey {
    row: usize,
    columns: u16,
    cell_width: u32,
    clip_width: u32,
}

/// Values kept while the rows they were built for stay displayed: a build
/// keeps only what it used.
#[derive(Debug)]
struct Cache<K, V> {
    kept: HashMap<K, Rc<V>>,
    used: HashMap<K, Rc<V>>,
}

impl<K, V> Default for Cache<K, V> {
    fn default() -> Self {
        Self {
            kept: HashMap::new(),
            used: HashMap::new(),
        }
    }
}

impl<K: Copy + Eq + Hash, V> Cache<K, V> {
    fn begin(&mut self) {
        self.kept.extend(self.used.drain());
    }

    /// The value cached for `key` if `valid` accepts it, or a new one.
    fn get(
        &mut self,
        key: K,
        valid: impl Fn(&V) -> bool,
        build: impl FnOnce() -> V,
    ) -> Rc<V> {
        let value = self
            .kept
            .remove(&key)
            .or_else(|| self.used.get(&key).cloned())
            .filter(|value| valid(value))
            .unwrap_or_else(|| Rc::new(build()));
        self.used.insert(key, Rc::clone(&value));
        value
    }

    fn end(&mut self) {
        self.kept.clear();
    }
}

/// Each displayed row's cells and runs, kept while the rows stay displayed.
#[derive(Debug, Default)]
pub(super) struct ModelBuilder {
    cells: Cache<RowKey, RowCells>,
    texts: Cache<(RowKey, Plan), RowText>,
}

impl ModelBuilder {
    /// Builds the model of `inputs`, reusing rows cached since the last
    /// build at any index.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "cell counts are small, positive, and bounded by the grid"
    )]
    pub(super) fn build(&mut self, inputs: &Inputs<'_>) -> Model {
        let snapshot = inputs.snapshot;
        let area = inputs.geometry.visible_area();
        let cell = inputs.geometry.cell_size();
        let scale = inputs.scale;
        let cell_width = f32::from(cell.width) * scale;
        let cell_height = f32::from(cell.height) * scale;
        let area_width = f32::from(area.size.width) * scale;
        let area_height = f32::from(area.size.height) * scale;
        let origin_x = f32::from(area.origin.x) * scale;
        let origin_y = f32::from(area.origin.y) * scale;
        let visible = |extent: f32, cell: f32, count: u16| {
            if cell <= 0.0 || extent <= 0.0 {
                return 0;
            }
            ((extent / cell).ceil() as u32).min(u32::from(count)) as u16
        };
        let columns = visible(area_width, cell_width, snapshot.size.columns);
        let rows = visible(area_height, cell_height, snapshot.size.rows)
            .min(u16::try_from(snapshot.rows.len()).unwrap_or(u16::MAX));

        let (cells, document, mut texts) = self.rows(
            &snapshot.rows[..usize::from(rows)],
            columns,
            cell_width,
            area_width,
        );

        let cursor = snapshot
            .cursor
            .filter(|cursor| cursor.row < rows && cursor.column < columns);
        if document.len == 0 {
            hold_empty_document(
                &mut texts,
                &cells,
                cursor,
                (cell_width, area_width),
            );
        }
        let cursor_cell = cursor.map(|cursor| {
            let x = cell_width * f32::from(cursor.column);
            CursorCell {
                row: usize::from(cursor.row),
                x,
                width: cell_width.min(area_width - x).max(0.0),
                document_end: texts.iter().rposition(|text| text.len > 0)
                    == Some(usize::from(cursor.row)),
            }
        });
        let runs = runs(
            &texts,
            (origin_x, origin_y),
            cell_height,
            origin_y + area_height,
            cursor_cell,
        );

        let caret =
            cursor.and_then(|cursor| caret_bounds(cursor, inputs, area));
        let position = |index| model_position(&texts, index);
        let selection = inputs
            .selection
            .and_then(|range| selection(range, snapshot, &cells, &document))
            .and_then(|(start, end)| Some((position(start)?, position(end)?)))
            .or_else(|| {
                let cursor = cursor?;
                let at = position(document.start(
                    &cells,
                    usize::from(cursor.column),
                    usize::from(cursor.row),
                ))?;
                Some((at, at))
            });
        Model {
            runs,
            caret,
            selection,
        }
    }

    /// The displayed rows' cells, the formatter's pass over them, and their
    /// characters, from the cache where the rows are unchanged.
    fn rows(
        &mut self,
        rows: &[Arc<TerminalRow>],
        columns: u16,
        cell_width: f32,
        clip_width: f32,
    ) -> (Vec<Rc<RowCells>>, Document, Vec<Rc<RowText>>) {
        self.cells.begin();
        self.texts.begin();
        let mut keys = Vec::with_capacity(rows.len());
        let mut cells = Vec::with_capacity(rows.len());
        for row in rows {
            let key = RowKey {
                row: Arc::as_ptr(row) as usize,
                columns,
                cell_width: cell_width.to_bits(),
                clip_width: clip_width.to_bits(),
            };
            keys.push(key);
            cells.push(self.cells.get(
                key,
                |cached| Arc::ptr_eq(&cached.row, row),
                || row_cells(row, columns, cell_width, clip_width),
            ));
        }
        let document = Document::new(&cells, usize::from(columns));
        let texts: Vec<_> = keys
            .iter()
            .zip(&cells)
            .zip(&document.plans)
            .map(|((key, cells), plan)| {
                self.texts.get(
                    (*key, *plan),
                    |cached| Arc::ptr_eq(&cached.row, &cells.row),
                    || row_text(cells, *plan, cell_width, clip_width),
                )
            })
            .collect();
        self.cells.end();
        self.texts.end();
        debug_assert_eq!(
            texts.iter().map(|text| text.len).sum::<usize>(),
            document.len,
            "the runs hold the document"
        );
        (cells, document, texts)
    }
}

/// Gives a displayed grid whose Copy text is empty its one character: a
/// line break in the cursor's cell, or in the first cell without a
/// displayed cursor. Without it the terminal would have no runs, and so no
/// text interface and no caret position. A view that displays no cells
/// keeps none.
fn hold_empty_document(
    texts: &mut [Rc<RowText>],
    cells: &[Rc<RowCells>],
    cursor: Option<Cursor>,
    (cell_width, clip_width): (f32, f32),
) {
    let (row, column) = cursor
        .map_or((0, 0), |cursor| (usize::from(cursor.row), cursor.column));
    let Some(cells) = cells.get(row).filter(|cells| !cells.columns.is_empty())
    else {
        return;
    };
    let x = cell_width * f32::from(column);
    texts[row] = Rc::new(RowText {
        row: Arc::clone(&cells.row),
        segments: segments(&[Character {
            text: "\n",
            x,
            width: cell_width.min(clip_width - x).max(0.0),
        }]),
        len: 1,
        newline: true,
    });
}

/// The cursor's displayed cell, relative to its row's left edge.
#[derive(Clone, Copy, Debug)]
struct CursorCell {
    row: usize,
    x: f32,
    /// One cell, clipped to the displayed grid.
    width: f32,
    /// Whether the cursor's row holds the document's last character.
    document_end: bool,
}

/// The rows' runs as nodes, placed from the grid's `origin` down to
/// `bottom`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "row and run counts are small and bounded by the grid"
)]
fn runs(
    texts: &[Rc<RowText>],
    (origin_x, origin_y): (f32, f32),
    cell_height: f32,
    bottom: f32,
    cursor: Option<CursorCell>,
) -> Vec<ModelRun> {
    let mut runs = Vec::new();
    for (index, text) in texts.iter().enumerate() {
        let top = origin_y + cell_height * index as f32;
        let row_bottom = (top + cell_height).min(bottom);
        let count = text.segments.len();
        for (segment_index, segment) in text.segments.iter().enumerate() {
            let mut node = Node::new(Role::TextRun);
            let mut positions = Cow::Borrowed(&*segment.positions);
            let mut widths = Cow::Borrowed(&*segment.widths);
            let mut width = segment.width;
            if let Some(cursor) = cursor
                && cursor.row == index
                && segment_index + 1 == count
            {
                width = width.max(reach_cursor(
                    text.newline,
                    cursor,
                    segment.x,
                    positions.to_mut(),
                    widths.to_mut(),
                ));
            }
            let left = f64::from(origin_x + segment.x);
            node.set_bounds(Rect {
                x0: left,
                y0: f64::from(top),
                x1: left + f64::from(width),
                y1: f64::from(row_bottom),
            });
            node.set_value(&*segment.value);
            node.set_character_lengths(&*segment.lengths);
            node.set_character_positions(&*positions);
            node.set_character_widths(&*widths);
            node.set_word_starts(&*segment.word_starts);
            node.set_text_direction(TextDirection::LeftToRight);
            runs.push(ModelRun {
                key: RunKey {
                    row: index as u16,
                    segment: segment_index as u16,
                },
                node,
                continued: segment_index > 0,
                continues: segment_index + 1 < count,
            });
        }
    }
    runs
}

/// Moves the cursor row's last character to a cursor past the row's text,
/// so the character at the caret is drawn where the cursor is: AT-SPI
/// clients read the caret's place from that character's extents, not from
/// the caret node. Text is unchanged, since it must match Copy.
///
/// A line break takes the cursor's cell. A row that ends the document has
/// no line break, and the caret is the document's end, which AccessKit
/// places at the right edge of the last character, with no width, so that
/// character stretches to the cursor's cell. Returns the run's new width,
/// relative to the segment's left edge `x`.
fn reach_cursor(
    newline: bool,
    cursor: CursorCell,
    x: f32,
    positions: &mut [f32],
    widths: &mut [f32],
) -> f32 {
    let last = positions.len() - 1;
    let at = cursor.x - x;
    if newline {
        if positions[last] < at {
            positions[last] = at;
            widths[last] = cursor.width;
        }
    } else if cursor.document_end && positions[last] + widths[last] < at {
        widths[last] = at - positions[last];
    }
    positions[last] + widths[last]
}

/// The painted cursor rectangle, clipped to the displayed grid. A hidden
/// cursor still marks the caret, so it gets its cell.
fn caret_bounds(
    cursor: Cursor,
    inputs: &Inputs<'_>,
    area: Bounds<Pixels>,
) -> Option<Rect> {
    let shape = match cursor.shape {
        CursorShape::Hidden => CursorShape::Block,
        shape => shape,
    };
    let bounds =
        cursor_bounds(area.origin, Cursor { shape, ..cursor }, inputs.metrics)
            .intersect(&area);
    if bounds.size.width <= gpui::px(0.0) || bounds.size.height <= gpui::px(0.0)
    {
        return None;
    }
    let scale = f64::from(inputs.scale);
    Some(Rect {
        x0: f64::from(bounds.origin.x) * scale,
        y0: f64::from(bounds.origin.y) * scale,
        x1: f64::from(bounds.right()) * scale,
        y1: f64::from(bounds.bottom()) * scale,
    })
}

/// Document character `index` as a run and an index in it: at the start of
/// the row holding it, or at the end of the last row with characters. A
/// document with no characters has no positions.
fn model_position(
    texts: &[Rc<RowText>],
    index: usize,
) -> Option<ModelPosition> {
    let mut start = 0;
    let mut last = None;
    for (row, text) in texts.iter().enumerate() {
        if text.len == 0 {
            continue;
        }
        if index < start + text.len {
            last = Some((row, index - start));
            break;
        }
        last = Some((row, text.len));
        start += text.len;
    }
    let (row, character) = last?;
    let (segment, character_index) = texts[row].position(character);
    Some(ModelPosition {
        run: RunKey {
            row: u16::try_from(row).unwrap_or(u16::MAX),
            segment,
        },
        character_index,
    })
}

/// The displayed grid's text as Copy writes it, and where Copy's text of a
/// selection inside the grid starts and ends in it.
///
/// It runs Ghostty's plain formatter with `unwrap` and `trim`
/// (`formatter.zig`, `PageFormatter.formatWithStateEmit`) over the
/// displayed cells. A row with text writes the line breaks pending before
/// it, then its characters, each blank pending until text on its logical
/// line follows; a row's own line break pends unless it soft-wraps, and a
/// row without its own wrap-continuation flag drops the pending blanks,
/// whatever the row above it says. A row with no text adds a pending line
/// break and nothing else, even when it soft-wraps. Whatever pends at the
/// end is dropped.
#[derive(Debug)]
struct Document {
    plans: Vec<Plan>,
    len: usize,
    columns: usize,
    /// For each row, one slot per displayed column and one after them: the
    /// first character from a source at or after the slot. A selection
    /// starting there starts at it.
    starts: Vec<usize>,
    /// For each displayed cell: the characters Copy writes before a
    /// selection ending there ends.
    ends: Vec<usize>,
}

impl Document {
    fn new(rows: &[Rc<RowCells>], columns: usize) -> Self {
        let slots = columns + 1;
        let mut plans = vec![Plan::default(); rows.len()];
        let mut starts = vec![usize::MAX; rows.len() * slots];
        let mut ends = vec![0; rows.len() * columns];
        let mut len = 0;
        // Rows whose line break is pending, and pending blanks as
        // (row, character).
        let mut lines: Vec<usize> = Vec::new();
        let mut blanks: Vec<(usize, usize)> = Vec::new();
        // The row the last character went to, and whether it was that
        // row's line break.
        let mut last: Option<(usize, bool)> = None;
        // The characters written once each of a row's cells is read.
        let mut written = Vec::with_capacity(columns);
        let mut write = |slot: usize, len: &mut usize| {
            starts[slot] = starts[slot].min(*len);
            *len += 1;
        };
        for (index, cells) in rows.iter().enumerate() {
            let row_start = len;
            let Some(first_text) = cells.first_text else {
                lines.push(index);
                ends[index * columns..(index + 1) * columns].fill(row_start);
                continue;
            };
            for line in lines.drain(..) {
                plans[line].newline = true;
                write(line * slots + columns, &mut len);
                last = Some((line, true));
            }
            if !cells.row.wrapped {
                lines.push(index);
            }
            // Copy reads the row's own flag, even after ED 1 reset the row
            // above, so it no longer wraps.
            if !cells.row.wrap_continuation {
                blanks.clear();
            }
            written.clear();
            for (char_index, cell) in cells.chars.iter().enumerate() {
                if cell.class != Class::Text {
                    blanks.push((index, char_index));
                    written.push(len);
                    continue;
                }
                for (source, source_index) in blanks.drain(..) {
                    // Blanks written after a later row's line break lead
                    // the row after that break.
                    let target = match last {
                        Some((row, closed))
                            if source < row || (source == row && closed) =>
                        {
                            let row = if closed { row + 1 } else { row };
                            debug_assert_eq!(
                                plans[row].own, 0,
                                "carried blanks lead their row"
                            );
                            plans[row].carried += 1;
                            row
                        }
                        _ => {
                            debug_assert_eq!(
                                usize::from(plans[source].own),
                                source_index,
                                "a row's characters are written in order"
                            );
                            plans[source].own = count(source_index + 1);
                            source
                        }
                    };
                    let column = rows[source].chars[source_index].column;
                    write(source * slots + column, &mut len);
                    last = Some((target, false));
                }
                plans[index].own = count(char_index + 1);
                write(index * slots + cell.column, &mut len);
                last = Some((index, false));
                written.push(len);
            }
            let row_end = written.last().copied().unwrap_or(row_start);
            for column in 0..columns {
                ends[index * columns + column] = if column < first_text {
                    row_start
                } else {
                    match cells.columns.get(column) {
                        Some(ColumnCell::Char(at) | ColumnCell::Tail(at)) => {
                            written[*at]
                        }
                        Some(ColumnCell::Head) | None => row_end,
                    }
                };
            }
        }
        let mut next = len;
        for start in starts.iter_mut().rev() {
            next = next.min(*start);
            *start = next;
        }
        Self {
            plans,
            len,
            columns,
            starts,
            ends,
        }
    }

    /// Where Copy's text of a selection starting at displayed cell
    /// (`column`, `row`) starts. Ghostty starts a selection on a spacer at
    /// its wide character, and skips the row of one starting on a spacer
    /// head.
    fn start(&self, rows: &[Rc<RowCells>], column: usize, row: usize) -> usize {
        let slots = self.columns + 1;
        let slot = match rows[row].columns.get(column) {
            None => self.columns,
            Some(ColumnCell::Char(at) | ColumnCell::Tail(at)) => {
                rows[row].chars[*at].column
            }
            Some(ColumnCell::Head) if column == 0 => 0,
            Some(ColumnCell::Head) => {
                return self
                    .starts
                    .get((row + 1) * slots)
                    .map_or(self.len, |start| *start);
            }
        };
        self.starts[row * slots + slot]
    }

    /// Where Copy's text of a selection ending at displayed cell
    /// (`column`, `row`) ends. Ghostty moves an end on a spacer head onto
    /// the wide character that wrapped to the next row.
    fn end(&self, rows: &[Rc<RowCells>], column: usize, row: usize) -> usize {
        let column = column.min(self.columns - 1);
        if let Some(ColumnCell::Head) = rows[row].columns.get(column)
            && row + 1 < rows.len()
        {
            return self.ends[(row + 1) * self.columns];
        }
        self.ends[row * self.columns + column]
    }
}

fn count(characters: usize) -> u16 {
    u16::try_from(characters).unwrap_or(u16::MAX)
}

/// The part of `range` on the displayed rows, as document indices: Copy's
/// text of the range, or of the part a range leaving the displayed rows
/// has on them. A range Copy would copy nothing from is none, so the caret
/// stands in for it.
fn selection(
    range: BufferRange,
    snapshot: &TerminalSnapshot,
    rows: &[Rc<RowCells>],
    document: &Document,
) -> Option<(usize, usize)> {
    if rows.is_empty() || document.columns == 0 {
        return None;
    }
    // Snapshot row `r` is `bottom_offset + (rows - 1 - r)` rows above the
    // live bottom, so rows count down as offsets count up.
    let last_row = snapshot
        .viewport
        .bottom_offset
        .saturating_add(usize::from(snapshot.size.rows).saturating_sub(1));
    let row_of =
        |point: BufferPoint| last_row.checked_sub(point.rows_from_live_bottom);
    let start = match row_of(range.start) {
        Some(row) if row < rows.len() => {
            document.start(rows, usize::from(range.start.column), row)
        }
        Some(_) => return None,
        None => 0,
    };
    let end = match row_of(range.end) {
        None => return None,
        Some(row) if row < rows.len() => {
            document.end(rows, usize::from(range.end.column), row)
        }
        Some(_) => document.len,
    };
    (start < end).then_some((start, end))
}

/// A character that separates words.
fn is_space(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

/// The longest prefix of `text` that AccessKit's `u8` character length can
/// hold. Only a cluster of dozens of combining marks is longer.
fn character(text: &str) -> &str {
    if u8::try_from(text.len()).is_ok() {
        return text;
    }
    let mut end = usize::from(u8::MAX);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[expect(
    clippy::cast_precision_loss,
    reason = "column counts fit an f32 exactly"
)]
fn row_cells(
    row: &Arc<TerminalRow>,
    columns: u16,
    cell_width: f32,
    clip_width: f32,
) -> RowCells {
    let clip = |x: f32, width: f32| width.min(clip_width - x).max(0.0);
    let columns = usize::from(columns).min(row.cells.len());
    let mut chars: Vec<CellChar> = Vec::with_capacity(columns);
    let mut text = String::new();
    let mut kinds = Vec::with_capacity(columns);
    let mut first_text = None;
    for (column, cell) in row.cells.iter().take(columns).enumerate() {
        if cell.style.wide_spacer {
            let tail = column > 0 && row.cells[column - 1].style.wide;
            kinds.push(match chars.len().checked_sub(1) {
                Some(wide) if tail => ColumnCell::Tail(wide),
                _ => ColumnCell::Head,
            });
            continue;
        }
        let class = Class::of(&cell.text);
        if class != Class::Empty && first_text.is_none() {
            first_text = Some(column);
        }
        let start = text.len();
        if class == Class::Text {
            text.push_str(character(cell.text.as_str()));
        }
        let x = cell_width * column as f32;
        let cells = if cell.style.wide { 2.0 } else { 1.0 };
        kinds.push(ColumnCell::Char(chars.len()));
        chars.push(CellChar {
            column,
            class,
            text: start..text.len(),
            x,
            width: clip(x, cell_width * cells),
        });
    }
    RowCells {
        row: Arc::clone(row),
        chars,
        text,
        columns: kinds.into(),
        first_text,
    }
}

/// One character of a row and where it is drawn, relative to the row.
struct Character<'a> {
    text: &'a str,
    x: f32,
    width: f32,
}

fn row_text(
    cells: &RowCells,
    plan: Plan,
    cell_width: f32,
    clip_width: f32,
) -> RowText {
    let mut characters: Vec<Character<'_>> = Vec::with_capacity(
        usize::from(plan.carried) + usize::from(plan.own) + 1,
    );
    characters.extend((0..plan.carried).map(|_| Character {
        text: " ",
        x: 0.0,
        width: 0.0,
    }));
    characters.extend((0..usize::from(plan.own)).map(|index| {
        let cell = &cells.chars[index];
        Character {
            text: cells.character(index),
            x: cell.x,
            width: cell.width,
        }
    }));
    if plan.newline {
        let right = characters
            .last()
            .map_or(0.0, |character| character.x + character.width);
        characters.push(Character {
            text: "\n",
            x: right.min(clip_width),
            width: cell_width.min(clip_width - right).max(0.0),
        });
    }
    RowText {
        row: Arc::clone(&cells.row),
        len: characters.len(),
        segments: segments(&characters),
        newline: plan.newline,
    }
}

/// Splits a row's characters into runs of at most [`RUN_LIMIT`]. A word
/// starts at a character that is not blank after one that is, across runs.
fn segments(characters: &[Character<'_>]) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut previous_blank = true;
    for chunk in characters.chunks(RUN_LIMIT) {
        let x = chunk[0].x;
        let last = &chunk[chunk.len() - 1];
        let mut value = String::new();
        let mut lengths = Vec::with_capacity(chunk.len());
        let mut positions = Vec::with_capacity(chunk.len());
        let mut widths = Vec::with_capacity(chunk.len());
        let mut word_starts = Vec::new();
        for (index, character) in chunk.iter().enumerate() {
            value.push_str(character.text);
            lengths.push(u8::try_from(character.text.len()).unwrap_or(u8::MAX));
            positions.push(character.x - x);
            widths.push(character.width);
            let blank = is_space(character.text);
            if !blank && previous_blank {
                word_starts.push(u8::try_from(index).unwrap_or(u8::MAX));
            }
            previous_blank = blank;
        }
        debug_assert_eq!(
            lengths
                .iter()
                .map(|&length| usize::from(length))
                .sum::<usize>(),
            value.len(),
            "character lengths cover the run's value"
        );
        segments.push(Segment {
            value: value.into(),
            lengths: lengths.into(),
            positions: positions.into(),
            widths: widths.into(),
            word_starts: word_starts.into(),
            x,
            width: last.x + last.width - x,
        });
    }
    debug_assert!(
        segments.iter().all(|segment| {
            !segment.lengths.is_empty()
                && segment.lengths.len() <= RUN_LIMIT
                && segment.lengths.len() == segment.positions.len()
                && segment.lengths.len() == segment.widths.len()
        }),
        "every run holds 1 to {RUN_LIMIT} characters"
    );
    segments
}

#[cfg(test)]
pub(super) mod tests;
