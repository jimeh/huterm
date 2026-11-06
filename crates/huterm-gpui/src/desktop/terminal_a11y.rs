//! The accessibility text model of a terminal view: the rows it displays as
//! AccessKit text runs, its cursor as a hidden caret node, and its selection.
//!
//! The document is the displayed grid, clipped to the painted bounds as
//! [`GridGeometry`] clips painting and hit testing; scrollback is not part of
//! it. Each grapheme cell is one character, spacer cells add none, and empty
//! cells are spaces. Rows lose their trailing blanks, as Copy trims them. A
//! hard row end is a `\n` of its own; a soft-wrapped row has none, as Copy
//! joins it with the next. No run is empty, because AccessKit's position and
//! line lookups misbehave next to one (AccessKit #836 and #837): the last row
//! has no `\n`, and an empty last row is one space. Runs longer than
//! [`RUN_LIMIT`] characters continue in another run on the same line, since
//! word starts are `u8` indices.
//!
//! Geometry is in window physical pixels, as GPUI reports node bounds.
//! Building reads only the snapshot the frame paints and the view's own
//! state; it never calls the runtime.

use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::accesskit::{
    Node, NodeId, Rect, Role, TextDirection, TextPosition, TextSelection,
};
use gpui::{A11ySubtreeBuilder, Bounds, Pixels};
use huterm_protocol::{
    BufferPoint, BufferRange, Cursor, CursorShape, GridSize, TerminalRow,
    TerminalSnapshot, Viewport,
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
        self.publications += 1;
        Publication::Published
    }

    /// The model the view's node shows.
    pub(super) fn model(&self) -> Rc<Model> {
        Rc::clone(&self.published)
    }

    /// How many distinct models this view has published.
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

/// A displayed row's characters, built once per row identity and geometry.
#[derive(Debug)]
struct RowText {
    /// Keeps the row alive, so its address names it while cached.
    row: Arc<TerminalRow>,
    segments: Vec<Segment>,
    /// For each displayed column, the index of the character that starts
    /// the column's selection and the one after the character that ends
    /// it, both clamped to `text_len`.
    starts: Box<[u16]>,
    ends: Box<[u16]>,
    /// Characters before the row's `\n`, if it has one.
    text_len: usize,
}

impl RowText {
    fn position(&self, character: usize) -> (u16, usize) {
        let last = self.segments.len() - 1;
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
    last: bool,
}

/// Each displayed row's characters and run-relative geometry, kept while
/// the rows stay displayed.
#[derive(Debug, Default)]
pub(super) struct ModelBuilder {
    rows: HashMap<RowKey, Rc<RowText>>,
    /// Rows the last build used; the next build keeps only those it uses.
    used: HashMap<RowKey, Rc<RowText>>,
}

impl ModelBuilder {
    /// Builds the model of `inputs`, reusing rows cached since the last
    /// build at any index.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
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
        self.rows.extend(self.used.drain());
        let mut texts = Vec::with_capacity(usize::from(rows));
        for index in 0..rows {
            let row = &snapshot.rows[usize::from(index)];
            let key = RowKey {
                row: Arc::as_ptr(row) as usize,
                columns,
                cell_width: cell_width.to_bits(),
                clip_width: area_width.to_bits(),
                last: index + 1 == rows,
            };
            let text = match self.rows.remove(&key) {
                Some(text) if Arc::ptr_eq(&text.row, row) => text,
                _ => Rc::new(row_text(
                    row, columns, cell_width, area_width, key.last,
                )),
            };
            self.used.insert(key, Rc::clone(&text));
            texts.push(text);
        }
        self.rows.clear();

        let mut runs = Vec::new();
        for (index, text) in texts.iter().enumerate() {
            let top = origin_y + cell_height * index as f32;
            let bottom = (top + cell_height).min(origin_y + area_height);
            let count = text.segments.len();
            for (segment_index, segment) in text.segments.iter().enumerate() {
                let mut node = Node::new(Role::TextRun);
                let left = f64::from(origin_x + segment.x);
                node.set_bounds(Rect {
                    x0: left,
                    y0: f64::from(top),
                    x1: left + f64::from(segment.width),
                    y1: f64::from(bottom),
                });
                node.set_value(&*segment.value);
                node.set_character_lengths(&*segment.lengths);
                node.set_character_positions(&*segment.positions);
                node.set_character_widths(&*segment.widths);
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

        let caret = snapshot
            .cursor
            .filter(|cursor| cursor.row < rows && cursor.column < columns)
            .and_then(|cursor| caret_bounds(cursor, inputs, area));
        let selection = inputs
            .selection
            .and_then(|range| selection(range, snapshot, &texts, columns))
            .or_else(|| {
                let cursor = snapshot.cursor.filter(|cursor| {
                    cursor.row < rows && cursor.column < columns
                })?;
                let text = &texts[usize::from(cursor.row)];
                let character =
                    usize::from(text.starts[usize::from(cursor.column)]);
                let position = model_position(cursor.row, text, character);
                Some((position, position))
            });
        Model {
            runs,
            caret,
            selection,
        }
    }
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

fn model_position(row: u16, text: &RowText, character: usize) -> ModelPosition {
    let (segment, character_index) = text.position(character);
    ModelPosition {
        run: RunKey { row, segment },
        character_index,
    }
}

/// The part of `range` on the displayed rows, as model positions.
fn selection(
    range: BufferRange,
    snapshot: &TerminalSnapshot,
    texts: &[Rc<RowText>],
    columns: u16,
) -> Option<(ModelPosition, ModelPosition)> {
    let rows = texts.len();
    // Snapshot row `r` is `bottom_offset + (rows - 1 - r)` rows above the
    // live bottom, so rows count down as offsets count up.
    let last_row = snapshot
        .viewport
        .bottom_offset
        .saturating_add(usize::from(snapshot.size.rows).saturating_sub(1));
    let row_of =
        |point: BufferPoint| last_row.checked_sub(point.rows_from_live_bottom);
    let start = match row_of(range.start) {
        Some(row) if row < rows => {
            let text = &texts[row];
            let character =
                column_character(&text.starts, range.start, columns, text);
            (row, character)
        }
        Some(_) => return None,
        None => (0, 0),
    };
    let end = match row_of(range.end) {
        None => return None,
        Some(row) if row < rows => {
            let text = &texts[row];
            (row, column_character(&text.ends, range.end, columns, text))
        }
        Some(_) => (rows - 1, texts[rows - 1].text_len),
    };
    let position = |(row, character): (usize, usize)| {
        model_position(
            u16::try_from(row).unwrap_or(u16::MAX),
            &texts[row],
            character,
        )
    };
    Some((position(start), position(end)))
}

fn column_character(
    characters: &[u16],
    point: BufferPoint,
    columns: u16,
    text: &RowText,
) -> usize {
    if point.column >= columns {
        return text.text_len;
    }
    usize::from(characters[usize::from(point.column)])
}

fn is_blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

/// The longest prefix of `text` that AccessKit's `u8` character length can
/// hold. Only a cluster of dozens of combining marks is longer.
fn character(text: &str) -> &str {
    if text.is_empty() {
        return " ";
    }
    if u8::try_from(text.len()).is_ok() {
        return text;
    }
    let mut end = usize::from(u8::MAX);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// One character of a row and where it is drawn, relative to the row.
struct Character<'a> {
    text: &'a str,
    x: f32,
    width: f32,
}

#[expect(
    clippy::cast_precision_loss,
    reason = "column counts fit an f32 exactly"
)]
fn row_text(
    row: &Arc<TerminalRow>,
    columns: u16,
    cell_width: f32,
    clip_width: f32,
    last: bool,
) -> RowText {
    let clip = |x: f32, width: f32| width.min(clip_width - x).max(0.0);
    let mut characters: Vec<Character<'_>> = Vec::new();
    let columns = usize::from(columns).min(row.cells.len());
    let mut starts = Vec::with_capacity(columns);
    let mut ends = Vec::with_capacity(columns);
    // Characters up to and including the last that is not blank.
    let mut text_len = 0;
    for (column, cell) in row.cells.iter().take(columns).enumerate() {
        let index = characters.len();
        if cell.style.wide_spacer {
            // The tail of a wide character selects it; a spacer head, left
            // at the last column by a wide character that wrapped, holds
            // none.
            let wide =
                index > 0 && column > 0 && row.cells[column - 1].style.wide;
            if wide {
                starts.push(index - 1);
            } else {
                starts.push(index);
            }
            ends.push(index);
            continue;
        }
        let x = cell_width * column as f32;
        let cells = if cell.style.wide { 2.0 } else { 1.0 };
        let text = character(cell.text.as_str());
        characters.push(Character {
            text,
            x,
            width: clip(x, cell_width * cells),
        });
        if !is_blank(text) {
            text_len = index + 1;
        }
        starts.push(index);
        ends.push(index + 1);
    }
    characters.truncate(text_len);
    let text_end = characters
        .last()
        .map_or(0.0, |character| character.x + character.width);
    if last {
        if characters.is_empty() {
            characters.push(Character {
                text: " ",
                x: 0.0,
                width: clip(0.0, cell_width),
            });
            text_len = 1;
        }
    } else if !row.wrapped {
        characters.push(Character {
            text: "\n",
            x: text_end.min(clip_width),
            width: clip(text_end, cell_width),
        });
    } else if characters.is_empty() {
        // A soft-wrapped row of blanks still needs a character.
        characters.push(Character {
            text: " ",
            x: 0.0,
            width: clip(0.0, cell_width),
        });
        text_len = 1;
    }
    let clamp =
        |index: usize| u16::try_from(index.min(text_len)).unwrap_or(u16::MAX);
    RowText {
        row: Arc::clone(row),
        segments: segments(&characters),
        starts: starts.into_iter().map(clamp).collect(),
        ends: ends.into_iter().map(clamp).collect(),
        text_len,
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
            let blank = is_blank(character.text);
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
        !segments.is_empty()
            && segments.iter().all(|segment| {
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
