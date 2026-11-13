//! Terminal updates: a viewer's rows and metadata, sent as deltas against
//! the last update the server sent that viewer.
//!
//! The server keeps, per viewer, the snapshot it last encoded. A row whose
//! `Arc` is identical to one of that baseline's rows travels as
//! [`RowOp::Keep`] naming the baseline row's position, so pointer identity
//! never crosses the wire; any other row travels in full, as runs of cells
//! that share one entry of the update's style table. Every update carries
//! every non-row field. Updates are numbered per viewer from 1, and a delta
//! names the update it builds on, so a client applies it only to that
//! baseline and detects a missing, repeated, or reordered delta at once.
//!
//! [`UpdateEncoder`] is the server's side and [`ViewerView`] the client's.
//! Both are generic over the rows they hold, so the test-only minor version
//! reuses them with a cell style flag that minor version 0 lacks.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

use huterm_protocol::{
    Cell, CellColor, CellStyle, CellText, Cursor, CursorShape, GridSize,
    KittyKeyboardFlags, MAX_CELL_TEXT_BYTES, MAX_GRID_CELLS, MouseEncoding,
    MouseTracking, Rgb, RowPrompt, TerminalId, TerminalModes, TerminalRow,
    TerminalSnapshot, ViewerId, Viewport,
};

use crate::codec::{
    DecodeError, EncodeError, IdScope, Reader, Writer, varint_len,
};

/// How many deltas pass between the digests deltas carry. Every complete
/// snapshot carries one too.
pub const UPDATE_DIGEST_INTERVAL: u64 = 64;

/// The style flag bits minor version 0 defines, one per [`CellStyle`]
/// field in declaration order.
pub const STYLE_FLAGS: u64 = 0xff;

/// The style flag bits a peer at `minor` understands. A decoder rejects any
/// other bit, so a sender projects a newer flag onto an older peer.
#[must_use]
pub const fn style_flags(minor: u16) -> u64 {
    let _ = minor;
    STYLE_FLAGS
}

/// A viewport's non-row state: everything an update carries besides rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewMetadata {
    /// Content generation.
    pub generation: u64,
    /// Publication revision.
    pub revision: u64,
    /// Geometry revision; a change makes the next update complete.
    pub geometry_revision: u64,
    /// Grid size: `size.rows` rows of `size.columns` cells.
    pub size: GridSize,
    /// Cursor, when it falls within the viewport.
    pub cursor: Option<Cursor>,
    /// Emulator modes.
    pub modes: TerminalModes,
    /// Shared viewport.
    pub viewport: Viewport,
    /// Maximum valid scroll offset.
    pub history_size: usize,
    /// Cursor color override.
    pub cursor_color: Option<Rgb>,
    /// Default background override.
    pub default_background: Option<Rgb>,
}

impl ViewMetadata {
    /// The non-row state of `snapshot`.
    #[must_use]
    pub fn of(snapshot: &TerminalSnapshot) -> Self {
        let TerminalSnapshot {
            terminal_id: _,
            generation,
            revision,
            geometry_revision,
            size,
            rows: _,
            cursor,
            modes,
            viewport,
            history_size,
            cursor_color,
            default_background,
        } = snapshot;
        Self {
            generation: *generation,
            revision: *revision,
            geometry_revision: *geometry_revision,
            size: *size,
            cursor: *cursor,
            modes: *modes,
            viewport: *viewport,
            history_size: *history_size,
            cursor_color: *cursor_color,
            default_background: *default_background,
        }
    }

    /// A snapshot of `terminal` with this state and `rows`.
    #[must_use]
    pub fn snapshot(
        self,
        terminal: TerminalId,
        rows: Vec<Arc<TerminalRow>>,
    ) -> TerminalSnapshot {
        let Self {
            generation,
            revision,
            geometry_revision,
            size,
            cursor,
            modes,
            viewport,
            history_size,
            cursor_color,
            default_background,
        } = self;
        TerminalSnapshot {
            terminal_id: terminal,
            generation,
            revision,
            geometry_revision,
            size,
            rows,
            cursor,
            modes,
            viewport,
            history_size,
            cursor_color,
            default_background,
        }
    }
}

/// One style-table entry: the colors and style flags a run's cells share.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StyleEntry {
    /// Foreground color.
    pub foreground: CellColor,
    /// Background color.
    pub background: CellColor,
    /// Style flags: [`STYLE_FLAGS`] at minor version 0, plus any bits a
    /// later minor version defines.
    pub flags: u64,
}

/// Consecutive cells of one row that share a style-table entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Run {
    /// The style-table entry.
    pub style: u32,
    /// Whether the cells are wide characters in `style`, each followed by
    /// its spacer: a cell with no text, `style`'s colors, and the spacer
    /// flag in place of the wide flag. `style` holds the wide flag, and each
    /// character is no text or one scalar other than U+0000.
    pub wide: bool,
    /// Each cell's text, at most [`MAX_CELL_TEXT_BYTES`] bytes; a wide run's
    /// spacers included.
    pub texts: Vec<CellText>,
}

/// A row sent in full.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullRow {
    /// The row soft-wraps into the next.
    pub wrapped: bool,
    /// The row continues a soft-wrapped row.
    pub wrap_continuation: bool,
    /// The row's prompt mark.
    pub prompt: RowPrompt,
    /// The row's cells as runs, filling exactly the grid's columns.
    pub runs: Vec<Run>,
}

/// How one row of an update is built.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RowOp {
    /// The baseline's row at this position, unchanged.
    Keep(u16),
    /// A row in full.
    Full(FullRow),
}

/// A viewer's rows and metadata, complete or as a delta.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalUpdate {
    /// The viewer.
    pub viewer: ViewerId,
    /// The update's number among the viewer's updates, from 1.
    pub seq: u64,
    /// The update this one builds on; `None` for a complete snapshot,
    /// whose rows are all in full.
    pub baseline: Option<u64>,
    /// Whether the update spent one of the viewer's credits.
    pub credited: bool,
    /// The digest of the resulting rows and metadata at the negotiated
    /// minor version, on every complete snapshot and every
    /// [`UPDATE_DIGEST_INTERVAL`]th delta.
    pub digest: Option<u64>,
    /// Everything besides rows.
    pub metadata: ViewMetadata,
    /// The styles full rows use.
    pub styles: Vec<StyleEntry>,
    /// Exactly `metadata.size.rows` rows.
    pub rows: Vec<RowOp>,
}

/// A row an update is built from or applied into.
pub trait UpdateRow {
    /// The row's cells and flags.
    fn row(&self) -> &TerminalRow;

    /// The style flags of the cell at `column`, projected onto a peer at
    /// `minor`.
    fn flags(&self, column: usize, minor: u16) -> u64;
}

/// A row the server encodes, identified by its allocation so an unchanged
/// row travels as [`RowOp::Keep`].
pub trait SourceRow: UpdateRow {
    /// The row's identity: equal for the same allocation while both are
    /// alive.
    fn identity(&self) -> usize;
}

/// A row a client builds from a full row.
pub trait ViewRow: UpdateRow + Clone {
    /// Builds the row, resolving each run's style through `styles`.
    fn build(row: &FullRow, styles: &[StyleEntry]) -> Self;
}

/// What a viewer shows at one publication: the state an update encodes.
pub trait UpdateSource {
    /// The rows.
    type Row: SourceRow;
    /// Everything besides rows.
    fn metadata(&self) -> ViewMetadata;
    /// The rows, `metadata().size.rows` of them.
    fn rows(&self) -> &[Self::Row];
}

/// The flag bits of a protocol cell style.
#[must_use]
pub fn flags_of(style: CellStyle) -> u64 {
    let CellStyle {
        bold,
        dim,
        italic,
        underline,
        strikeout,
        hidden,
        wide,
        wide_spacer,
    } = style;
    [
        bold,
        dim,
        italic,
        underline,
        strikeout,
        hidden,
        wide,
        wide_spacer,
    ]
    .into_iter()
    .enumerate()
    .fold(0, |flags, (bit, set)| flags | (u64::from(set) << bit))
}

/// The protocol cell style of minor version 0's flag bits.
#[must_use]
pub fn style_of(flags: u64) -> CellStyle {
    let bit = |index: u32| flags & (1 << index) != 0;
    CellStyle {
        bold: bit(0),
        dim: bit(1),
        italic: bit(2),
        underline: bit(3),
        strikeout: bit(4),
        hidden: bit(5),
        wide: bit(6),
        wide_spacer: bit(7),
    }
}

impl UpdateRow for Arc<TerminalRow> {
    fn row(&self) -> &TerminalRow {
        self
    }

    fn flags(&self, column: usize, _minor: u16) -> u64 {
        self.cells
            .get(column)
            .map_or(0, |cell| flags_of(cell.style))
    }
}

impl SourceRow for Arc<TerminalRow> {
    fn identity(&self) -> usize {
        Arc::as_ptr(self).addr()
    }
}

impl ViewRow for Arc<TerminalRow> {
    fn build(row: &FullRow, styles: &[StyleEntry]) -> Self {
        Arc::new(build_row(row, styles))
    }
}

impl UpdateSource for Arc<TerminalSnapshot> {
    type Row = Arc<TerminalRow>;

    fn metadata(&self) -> ViewMetadata {
        ViewMetadata::of(self)
    }

    fn rows(&self) -> &[Self::Row] {
        &self.rows
    }
}

/// Builds a protocol row from a full row, resolving each run's style. The
/// decoder has already checked every style index.
#[must_use]
pub fn build_row(row: &FullRow, styles: &[StyleEntry]) -> TerminalRow {
    let FullRow {
        wrapped,
        wrap_continuation,
        prompt,
        runs,
    } = row;
    let mut cells =
        Vec::with_capacity(runs.iter().map(|run| run.texts.len()).sum());
    for run in runs {
        let Some(style) = styles.get(run.style as usize) else {
            continue;
        };
        let flags = style_of(style.flags);
        let spacer = style_of(spacer_flags(style.flags));
        cells.extend(run.texts.iter().enumerate().map(|(index, text)| Cell {
            text: text.clone(),
            foreground: style.foreground,
            background: style.background,
            style: if run.wide && index % 2 == 1 {
                spacer
            } else {
                flags
            },
        }));
    }
    TerminalRow {
        cells,
        wrapped: *wrapped,
        wrap_continuation: *wrap_continuation,
        prompt: *prompt,
    }
}

/// A 64-bit hash over a sequence of 64-bit words, with `MurmurHash64A`'s
/// word step and finalizer (Austin Appleby, public domain). Every step is a
/// bijection of the state for a fixed word and of the word for a fixed
/// state, so two sequences of equal length that differ in one word always
/// hash differently. Words are built arithmetically, never from memory, so
/// the digest is the same on every platform and byte order.
struct WordHash(u64);

impl WordHash {
    const MULTIPLIER: u64 = 0xc6a4_a793_5bd1_e995;
    const SHIFT: u32 = 47;
    const SEED: u64 = 0x9e37_79b9_7f4a_7c15;

    fn new() -> Self {
        Self(Self::SEED)
    }

    fn word(&mut self, word: u64) {
        let mut mixed = word.wrapping_mul(Self::MULTIPLIER);
        mixed ^= mixed >> Self::SHIFT;
        mixed = mixed.wrapping_mul(Self::MULTIPLIER);
        self.0 = (self.0 ^ mixed).wrapping_mul(Self::MULTIPLIER);
    }

    fn finish(self) -> u64 {
        let mut hash = self.0;
        hash ^= hash >> Self::SHIFT;
        hash = hash.wrapping_mul(Self::MULTIPLIER);
        hash ^ hash >> Self::SHIFT
    }
}

/// Up to 8 bytes as a little-endian word, zero-padded.
fn le_word(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .rev()
        .fold(0, |word, byte| word << 8 | u64::from(*byte))
}

/// An optional RGB color as 25 bits: a presence bit above the color.
fn rgb_word(rgb: Option<Rgb>) -> u64 {
    rgb.map_or(0, |Rgb { red, green, blue }| {
        1 << 24 | u64::from(red) << 16 | u64::from(green) << 8 | u64::from(blue)
    })
}

/// Cell text up to this many bytes shares the word that holds its length.
const PACKED_TEXT_BYTES: usize = 6;

/// Style flag bits that share the word that holds a cell's colors; a flag
/// above them adds a word.
const PACKED_FLAG_BITS: u32 = 14;

/// The digest of a viewport's rows and metadata as a peer at `minor` holds
/// them, over a canonical sequence of words that is independent of the wire
/// codec. It covers exactly the fields of `minor`, so a projected view and
/// its digest always agree.
///
/// The metadata takes nine words. Each row takes a word with its cell count,
/// wrap flags, and prompt mark, then each cell's words: its colors and low
/// style flags; its text length, with the text when it is at most
/// [`PACKED_TEXT_BYTES`] bytes, as every single scalar is; any longer text
/// as zero-padded little-endian words; and the flags above
/// [`PACKED_FLAG_BITS`] when there are any, which the length word marks.
/// Every count and length precedes what it counts, so distinct views give
/// distinct word sequences.
#[must_use]
pub fn view_digest<R: UpdateRow>(
    metadata: &ViewMetadata,
    rows: &[R],
    minor: u16,
) -> u64 {
    let ViewMetadata {
        generation,
        revision,
        geometry_revision,
        size,
        cursor,
        modes,
        viewport,
        history_size,
        cursor_color,
        default_background,
    } = metadata;
    let mut hash = WordHash::new();
    hash.word(*generation);
    hash.word(*revision);
    hash.word(*geometry_revision);
    hash.word(
        u64::from(size.columns)
            | u64::from(size.rows) << 16
            | (rows.len() as u64) << 32,
    );
    hash.word(cursor.map_or(0, |Cursor { row, column, shape }| {
        1 | u64::from(row) << 8
            | u64::from(column) << 24
            | u64::from(shape_tag(shape)) << 40
    }));
    let (kitty, flags, tracking, encoding) = mode_parts(*modes);
    hash.word(
        u64::from(kitty)
            | u64::from(flags) << 8
            | u64::from(tracking) << 16
            | u64::from(encoding) << 24,
    );
    hash.word(viewport.bottom_offset as u64);
    hash.word(*history_size as u64);
    hash.word(rgb_word(*cursor_color) | rgb_word(*default_background) << 32);
    for row in rows {
        let TerminalRow {
            cells,
            wrapped,
            wrap_continuation,
            prompt,
        } = row.row();
        hash.word(
            u64::from(*wrapped)
                | u64::from(*wrap_continuation) << 1
                | u64::from(prompt_tag(*prompt)) << 2
                | (cells.len() as u64) << 8,
        );
        for (column, cell) in cells.iter().enumerate() {
            let flags = row.flags(column, minor);
            let high_flags = flags >> PACKED_FLAG_BITS;
            hash.word(
                u64::from(color_key(cell.foreground))
                    | u64::from(color_key(cell.background)) << 25
                    | (flags & ((1 << PACKED_FLAG_BITS) - 1)) << 50,
            );
            let text = cell.text.as_bytes();
            // Cell text is far below 2^15 bytes (`MAX_CELL_TEXT_BYTES`).
            let length = text.len() as u64 | u64::from(high_flags != 0) << 15;
            if text.len() <= PACKED_TEXT_BYTES {
                hash.word(length | le_word(text) << 16);
            } else {
                hash.word(length);
                for chunk in text.chunks(8) {
                    hash.word(le_word(chunk));
                }
            }
            if high_flags != 0 {
                hash.word(high_flags);
            }
        }
    }
    hash.finish()
}

fn shape_tag(shape: CursorShape) -> u8 {
    match shape {
        CursorShape::Block => 0,
        CursorShape::Underline => 1,
        CursorShape::Beam => 2,
        CursorShape::Hidden => 3,
    }
}

fn prompt_tag(prompt: RowPrompt) -> u8 {
    match prompt {
        RowPrompt::None => 0,
        RowPrompt::Primary => 1,
        RowPrompt::Continuation => 2,
    }
}

/// A mode set's Kitty flags, its boolean modes as bits, and its mouse
/// tracking and encoding tags.
fn mode_parts(modes: TerminalModes) -> (u8, u8, u8, u8) {
    let TerminalModes {
        kitty_keyboard,
        application_cursor,
        alternate_screen,
        bracketed_paste,
        focus_reporting,
        mouse_tracking,
        mouse_encoding,
    } = modes;
    let flags = u8::from(application_cursor)
        | u8::from(alternate_screen) << 1
        | u8::from(bracketed_paste) << 2
        | u8::from(focus_reporting) << 3;
    let tracking = match mouse_tracking {
        MouseTracking::Disabled => 0,
        MouseTracking::Buttons => 1,
        MouseTracking::ButtonMotion => 2,
        MouseTracking::AllMotion => 3,
    };
    let encoding = match mouse_encoding {
        MouseEncoding::Legacy => 0,
        MouseEncoding::Utf8 => 1,
        MouseEncoding::Sgr => 2,
    };
    (kitty_keyboard.bits(), flags, tracking, encoding)
}

/// A cell color's canonical 25-bit form, which keys the style table and
/// the view digest.
fn color_key(color: CellColor) -> u32 {
    match color {
        CellColor::DefaultForeground => 0,
        CellColor::DefaultBackground => 1,
        CellColor::Cursor => 2,
        CellColor::Indexed(index) => 0x100 | u32::from(index),
        CellColor::Rgb(Rgb { red, green, blue }) => {
            0x0100_0000
                | u32::from(red) << 16
                | u32::from(green) << 8
                | u32::from(blue)
        }
    }
}

/// Whether a cell's text travels in a packed run: no text, or exactly one
/// scalar other than U+0000, which a packed run uses for no text.
fn packs(text: &CellText) -> bool {
    text.is_empty() || text.as_char().is_some_and(|scalar| scalar != '\0')
}

/// Minor version 0's wide-character flag.
const WIDE_FLAG: u64 = 1 << 6;

/// Minor version 0's wide-character spacer flag.
const SPACER_FLAG: u64 = 1 << 7;

/// The flags of the spacer that follows a wide character with `flags`.
const fn spacer_flags(flags: u64) -> u64 {
    flags & !WIDE_FLAG | SPACER_FLAG
}

/// Whether a style can head a wide run: it has the wide flag and not the
/// spacer flag.
const fn heads_wide(flags: u64) -> bool {
    flags & WIDE_FLAG != 0 && flags & SPACER_FLAG == 0
}

/// Whether a wide run's texts alternate a character that packs and a spacer
/// with no text.
fn wide_texts(texts: &[CellText]) -> bool {
    texts.len().is_multiple_of(2)
        && texts
            .chunks(2)
            .all(|pair| packs(&pair[0]) && pair[1].is_empty())
}

/// The fewest identical cells that travel as a fill run. A fill costs about
/// four bytes, a packed cell one or more, and splitting a run costs a run
/// header, so shorter repeats stay in their run.
const FILL_CELLS: usize = 8;

// `push_runs` finds repeats by comparing cells 4 apart.
const _: () = assert!(FILL_CELLS >= 8);

/// How a run's cells travel, the low bits of its header.
const FORM_TEXTS: u64 = 0;
const FORM_PACKED: u64 = 1;
const FORM_FILL: u64 = 2;
const FORM_WIDE: u64 = 3;

/// What the row builder groups cells into runs by.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Kind {
    /// A cell whose text does not pack.
    Texts,
    /// A cell whose text packs.
    Packed,
    /// A wide character and its spacer.
    Wide,
}

fn run(style: u32, wide: bool, cells: &[Cell]) -> Run {
    Run {
        style,
        wide,
        texts: cells.iter().map(|cell| cell.text.clone()).collect(),
    }
}

/// Pushes the runs of `cells`, which share a style and kind. Repeats of at
/// least [`FILL_CELLS`] packed cells become runs of their own, which travel
/// as fills.
fn push_runs(runs: &mut Vec<Run>, style: u32, kind: Kind, cells: &[Cell]) {
    if kind != Kind::Packed {
        runs.push(run(style, kind == Kind::Wide, cells));
        return;
    }
    // Any 8 consecutive cells hold a multiple of 4 and the cell 4 after it,
    // so comparing those pairs finds every repeat long enough to fill while
    // reading a quarter of the cells of text that never repeats.
    let mut pending = 0;
    let mut probe = 0;
    while probe + 4 < cells.len() {
        let text = &cells[probe].text;
        if cells[probe + 4].text != *text {
            probe += 4;
            continue;
        }
        let start = probe
            - cells[pending..probe]
                .iter()
                .rev()
                .take_while(|cell| cell.text == *text)
                .count();
        let end = probe
            + cells[probe..]
                .iter()
                .take_while(|cell| cell.text == *text)
                .count();
        if end - start >= FILL_CELLS {
            if pending < start {
                runs.push(run(style, false, &cells[pending..start]));
            }
            runs.push(run(style, false, &cells[start..end]));
            pending = end;
        }
        probe = end.next_multiple_of(4).max(probe + 4);
    }
    if pending < cells.len() {
        runs.push(run(style, false, &cells[pending..]));
    }
}

/// A style-table key: a cell's two color keys in one word, and its flags.
type StyleKey = (u64, u64);

/// Hashes the encoder's map keys, row identities and [`StyleKey`]s, with
/// [`WordHash`]'s word step, which costs less than the standard library's
/// `SipHash`. No peer chooses these keys, and neither the order of a map nor
/// the hash reaches the wire: the style table's order is the order styles
/// first appear.
#[derive(Default)]
struct KeyHasher(u64);

/// A map keyed through [`KeyHasher`].
type KeyMap<K, V> = HashMap<K, V, BuildHasherDefault<KeyHasher>>;

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            self.write_u64(le_word(chunk));
        }
    }

    fn write_u64(&mut self, word: u64) {
        let mut hash = WordHash(self.0);
        hash.word(word);
        self.0 = hash.0;
    }

    fn write_usize(&mut self, word: usize) {
        self.write_u64(word as u64);
    }
}

/// Builds the style table and full rows of an update.
struct RowBuilder {
    minor: u16,
    columns: usize,
    styles: Vec<StyleEntry>,
    index: KeyMap<StyleKey, u32>,
    /// The last style looked up, which most cells repeat.
    last: Option<(StyleKey, u32)>,
}

impl RowBuilder {
    fn style(&mut self, cell: &Cell, flags: u64) -> u32 {
        let key = (
            u64::from(color_key(cell.foreground))
                | u64::from(color_key(cell.background)) << 32,
            flags,
        );
        if let Some((last, style)) = self.last
            && last == key
        {
            return style;
        }
        let style = *self.index.entry(key).or_insert_with(|| {
            self.styles.push(StyleEntry {
                foreground: cell.foreground,
                background: cell.background,
                flags,
            });
            u32::try_from(self.styles.len() - 1).unwrap_or(u32::MAX)
        });
        self.last = Some((key, style));
        style
    }

    fn full<R: UpdateRow>(&mut self, row: &R) -> Result<FullRow, EncodeError> {
        let TerminalRow {
            cells,
            wrapped,
            wrap_continuation,
            prompt,
        } = row.row();
        if cells.len() != self.columns {
            return Err(EncodeError::Invalid("row width"));
        }
        // A run ends where the style or the kind of its cells changes, so
        // each run's texts are copied once, at their exact size, and a run's
        // form never meets a cell it cannot hold.
        let mut runs: Vec<Run> = Vec::new();
        let mut open: Option<(usize, u32, Kind)> = None;
        let mut column = 0;
        while let Some(cell) = cells.get(column) {
            if cell.text.len() > MAX_CELL_TEXT_BYTES {
                return Err(EncodeError::LimitExceeded { field: "cell text" });
            }
            let flags = row.flags(column, self.minor);
            let style = self.style(cell, flags);
            let packs = packs(&cell.text);
            let wide = packs
                && heads_wide(flags)
                && cells.get(column + 1).is_some_and(|spacer| {
                    spacer.text.is_empty()
                        && spacer.foreground == cell.foreground
                        && spacer.background == cell.background
                        && row.flags(column + 1, self.minor)
                            == spacer_flags(flags)
                });
            let kind = match (wide, packs) {
                (true, _) => Kind::Wide,
                (false, true) => Kind::Packed,
                (false, false) => Kind::Texts,
            };
            if open.is_none_or(|(_, open_style, open_kind)| {
                open_style != style || open_kind != kind
            }) {
                if let Some((start, style, kind)) = open {
                    push_runs(&mut runs, style, kind, &cells[start..column]);
                }
                open = Some((column, style, kind));
            }
            column += if wide { 2 } else { 1 };
        }
        if let Some((start, style, kind)) = open {
            push_runs(&mut runs, style, kind, &cells[start..]);
        }
        Ok(FullRow {
            wrapped: *wrapped,
            wrap_continuation: *wrap_continuation,
            prompt: *prompt,
            runs,
        })
    }
}

/// The server's side of one viewer's updates: the update numbers it sent
/// and the snapshot it last encoded, which the next delta builds on.
#[derive(Debug)]
pub struct UpdateEncoder<S = Arc<TerminalSnapshot>> {
    viewer: ViewerId,
    minor: u16,
    seq: u64,
    baseline: Option<S>,
    deltas: u64,
}

impl<S: UpdateSource + Clone> UpdateEncoder<S> {
    /// An encoder for `viewer` on a connection at negotiated `minor`, whose
    /// first update is complete.
    #[must_use]
    pub fn new(viewer: ViewerId, minor: u16) -> Self {
        Self {
            viewer,
            minor,
            seq: 0,
            baseline: None,
            deltas: 0,
        }
    }

    /// The number of the last update built; zero before the first.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Drops the baseline, so the next update is a complete snapshot, as
    /// `ResyncViewer` and a stopped terminal's final update require.
    pub fn resync(&mut self) {
        self.baseline = None;
    }

    /// Builds the next update from `current`, which becomes the baseline of
    /// the update after it. The update is complete when there is no
    /// baseline or the geometry changed since it.
    ///
    /// # Errors
    /// Refuses a snapshot whose grid is empty or past [`MAX_GRID_CELLS`],
    /// whose row count or widths do not match its grid, or whose cell text
    /// is past [`MAX_CELL_TEXT_BYTES`]; nothing advances.
    pub fn next(
        &mut self,
        current: &S,
        credited: bool,
    ) -> Result<TerminalUpdate, EncodeError> {
        let metadata = current.metadata();
        let size = metadata.size;
        if size.columns == 0 || size.rows == 0 || size.cells() > MAX_GRID_CELLS
        {
            return Err(EncodeError::LimitExceeded { field: "grid size" });
        }
        let rows = current.rows();
        if rows.len() != usize::from(size.rows) {
            return Err(EncodeError::Invalid("row count"));
        }
        let baseline = self.baseline.as_ref().filter(|baseline| {
            let previous = baseline.metadata();
            previous.geometry_revision == metadata.geometry_revision
                && previous.size == size
        });
        let keep: KeyMap<usize, u16> = baseline
            .map(|baseline| {
                let mut keep = KeyMap::default();
                for (index, row) in baseline.rows().iter().enumerate() {
                    keep.entry(row.identity())
                        .or_insert(u16::try_from(index).unwrap_or(u16::MAX));
                }
                keep
            })
            .unwrap_or_default();
        let mut builder = RowBuilder {
            minor: self.minor,
            columns: usize::from(size.columns),
            styles: Vec::new(),
            index: HashMap::default(),
            last: None,
        };
        let mut ops = Vec::with_capacity(rows.len());
        for row in rows {
            ops.push(match keep.get(&row.identity()) {
                Some(index) => RowOp::Keep(*index),
                None => RowOp::Full(builder.full(row)?),
            });
        }
        let complete = baseline.is_none();
        let digest = if complete || self.deltas + 1 == UPDATE_DIGEST_INTERVAL {
            Some(view_digest(&metadata, rows, self.minor))
        } else {
            None
        };
        self.deltas = if digest.is_some() { 0 } else { self.deltas + 1 };
        let update = TerminalUpdate {
            viewer: self.viewer,
            seq: self.seq + 1,
            baseline: (!complete).then_some(self.seq),
            credited,
            digest,
            metadata,
            styles: builder.styles,
            rows: ops,
        };
        self.seq += 1;
        self.baseline = Some(current.clone());
        Ok(update)
    }
}

/// What a client does after disposing of an update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateOutcome {
    /// The view shows the update.
    Applied,
    /// The update was discarded: it is older than the view, or a delta
    /// arrived while the view waits for a complete snapshot. Nothing to
    /// send but any credit.
    Stale,
    /// The update was a delta on a baseline the view does not hold, or its
    /// digest differed from the view's. Send one `ResyncViewer`; further
    /// deltas are discarded until a complete snapshot arrives.
    Resync,
}

/// How a client disposed of one update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Disposal {
    /// What happened to the update.
    pub outcome: UpdateOutcome,
    /// Whether to return one credit, naming [`ViewerView::applied`]: the
    /// update spent one, whether it was applied or discarded.
    pub return_credit: bool,
}

/// Why a client cannot use an update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewError {
    /// The update breaks the contract against the view it applies to, such
    /// as a `Keep` past the baseline's rows: a protocol violation that ends
    /// the connection.
    Invalid(&'static str),
    /// A complete snapshot's digest differs from the view it produced.
    /// Another snapshot would differ the same way, so the client closes the
    /// viewer as a contract fault and sends no resync.
    ContractFault,
}

/// The client's side of one viewer's updates: the view the applied updates
/// built and the number of the last one.
#[derive(Clone, Debug)]
pub struct ViewerView<R = Arc<TerminalRow>> {
    minor: u16,
    view: Option<(ViewMetadata, Vec<R>)>,
    applied: u64,
    awaiting_resync: bool,
}

impl<R: ViewRow> ViewerView<R> {
    /// An empty view on a connection at negotiated `minor`.
    #[must_use]
    pub fn new(minor: u16) -> Self {
        Self {
            minor,
            view: None,
            applied: 0,
            awaiting_resync: false,
        }
    }

    /// The number of the last update applied; zero before the first.
    #[must_use]
    pub fn applied(&self) -> u64 {
        self.applied
    }

    /// The view's metadata and rows, once an update was applied.
    #[must_use]
    pub fn view(&self) -> Option<(&ViewMetadata, &[R])> {
        self.view
            .as_ref()
            .map(|(metadata, rows)| (metadata, rows.as_slice()))
    }

    /// The digest of the view at the connection's minor version.
    #[must_use]
    pub fn digest(&self) -> Option<u64> {
        self.view
            .as_ref()
            .map(|(metadata, rows)| view_digest(metadata, rows, self.minor))
    }

    /// Applies `update` if it builds on the view, and says what to send.
    ///
    /// # Errors
    /// Returns [`ViewError::Invalid`] for an update that cannot apply to
    /// the view it names, and [`ViewError::ContractFault`] for a complete
    /// snapshot whose digest differs from the view it produced.
    pub fn apply(
        &mut self,
        update: &TerminalUpdate,
    ) -> Result<Disposal, ViewError> {
        let disposal = |outcome| Disposal {
            outcome,
            return_credit: update.credited,
        };
        if update.seq <= self.applied {
            return Ok(disposal(UpdateOutcome::Stale));
        }
        let baseline = match update.baseline {
            None if update.digest.is_none() => {
                return Err(ViewError::Invalid("complete snapshot digest"));
            }
            None => None,
            Some(_) if self.awaiting_resync => {
                return Ok(disposal(UpdateOutcome::Stale));
            }
            Some(baseline) => match &self.view {
                Some((metadata, rows)) if baseline == self.applied => {
                    if metadata.size != update.metadata.size {
                        return Err(ViewError::Invalid("delta grid size"));
                    }
                    Some(rows.as_slice())
                }
                _ => {
                    self.awaiting_resync = true;
                    return Ok(disposal(UpdateOutcome::Resync));
                }
            },
        };
        let mut rows = Vec::with_capacity(update.rows.len());
        for op in &update.rows {
            rows.push(match op {
                RowOp::Keep(index) => baseline
                    .and_then(|baseline| baseline.get(usize::from(*index)))
                    .cloned()
                    .ok_or(ViewError::Invalid("keep index"))?,
                RowOp::Full(row) => R::build(row, &update.styles),
            });
        }
        // The digest is computed only where the update carries one, so
        // steady state pays nothing for it.
        let matches = update.digest.is_none_or(|expected| {
            expected == view_digest(&update.metadata, &rows, self.minor)
        });
        self.view = Some((update.metadata, rows));
        self.applied = update.seq;
        if update.baseline.is_none() {
            if !matches {
                return Err(ViewError::ContractFault);
            }
            self.awaiting_resync = false;
            return Ok(disposal(UpdateOutcome::Applied));
        }
        if matches {
            Ok(disposal(UpdateOutcome::Applied))
        } else {
            self.awaiting_resync = true;
            Ok(disposal(UpdateOutcome::Resync))
        }
    }
}

impl ViewerView<Arc<TerminalRow>> {
    /// The view as a snapshot of `terminal`, once an update was applied.
    #[must_use]
    pub fn snapshot(&self, terminal: TerminalId) -> Option<TerminalSnapshot> {
        self.view
            .as_ref()
            .map(|(metadata, rows)| metadata.snapshot(terminal, rows.clone()))
    }
}

fn write_color(w: &mut Writer, color: CellColor) {
    match color {
        CellColor::DefaultForeground => w.tag(0),
        CellColor::DefaultBackground => w.tag(1),
        CellColor::Cursor => w.tag(2),
        CellColor::Indexed(index) => {
            w.tag(3);
            w.fixed(&[index]);
        }
        CellColor::Rgb(rgb) => {
            w.tag(4);
            write_rgb(w, rgb);
        }
    }
}

fn read_color(r: &mut Reader<'_>) -> Result<CellColor, DecodeError> {
    Ok(match r.tag()? {
        0 => CellColor::DefaultForeground,
        1 => CellColor::DefaultBackground,
        2 => CellColor::Cursor,
        3 => CellColor::Indexed(r.fixed::<1>()?[0]),
        4 => CellColor::Rgb(read_rgb(r)?),
        tag => {
            return Err(DecodeError::UnknownTag {
                field: "cell color",
                tag,
            });
        }
    })
}

fn write_rgb(w: &mut Writer, Rgb { red, green, blue }: Rgb) {
    w.fixed(&[red, green, blue]);
}

fn read_rgb(r: &mut Reader<'_>) -> Result<Rgb, DecodeError> {
    let [red, green, blue] = r.fixed()?;
    Ok(Rgb { red, green, blue })
}

fn read_shape(r: &mut Reader<'_>) -> Result<CursorShape, DecodeError> {
    Ok(match r.tag()? {
        0 => CursorShape::Block,
        1 => CursorShape::Underline,
        2 => CursorShape::Beam,
        3 => CursorShape::Hidden,
        tag => {
            return Err(DecodeError::UnknownTag {
                field: "cursor shape",
                tag,
            });
        }
    })
}

fn read_prompt(r: &mut Reader<'_>) -> Result<RowPrompt, DecodeError> {
    Ok(match r.tag()? {
        0 => RowPrompt::None,
        1 => RowPrompt::Primary,
        2 => RowPrompt::Continuation,
        tag => {
            return Err(DecodeError::UnknownTag {
                field: "row prompt",
                tag,
            });
        }
    })
}

fn write_modes(
    w: &mut Writer,
    modes: TerminalModes,
) -> Result<(), EncodeError> {
    let (kitty, flags, tracking, encoding) = mode_parts(modes);
    w.record(|w| {
        w.u64(u64::from(kitty));
        w.u64(u64::from(flags));
        w.tag(u64::from(tracking));
        w.tag(u64::from(encoding));
        Ok(())
    })
}

fn read_modes(r: &mut Reader<'_>) -> Result<TerminalModes, DecodeError> {
    r.record(|r| {
        let kitty =
            u8::try_from(r.u64()?).map_err(|_| DecodeError::OutOfRange)?;
        let kitty_keyboard = KittyKeyboardFlags::from_bits(kitty);
        if kitty_keyboard.bits() != kitty {
            return Err(DecodeError::Invalid("Kitty keyboard flags"));
        }
        let flags = r.u64()?;
        if flags & !0xf != 0 {
            return Err(DecodeError::Invalid("mode flags"));
        }
        let mouse_tracking = match r.tag()? {
            0 => MouseTracking::Disabled,
            1 => MouseTracking::Buttons,
            2 => MouseTracking::ButtonMotion,
            3 => MouseTracking::AllMotion,
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "mouse tracking",
                    tag,
                });
            }
        };
        let mouse_encoding = match r.tag()? {
            0 => MouseEncoding::Legacy,
            1 => MouseEncoding::Utf8,
            2 => MouseEncoding::Sgr,
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "mouse encoding",
                    tag,
                });
            }
        };
        Ok(TerminalModes {
            kitty_keyboard,
            application_cursor: flags & 1 != 0,
            alternate_screen: flags & 2 != 0,
            bracketed_paste: flags & 4 != 0,
            focus_reporting: flags & 8 != 0,
            mouse_tracking,
            mouse_encoding,
        })
    })
}

/// Encodes an update's record in `scope`. The grid comes first, so a
/// receiver derives the update's cap before buffering the rest.
///
/// # Errors
/// Refuses a style flag outside `flags`, which the peer's minor version
/// lacks, with [`EncodeError::Unsupported`]; an identity of another scope
/// with [`EncodeError::StaleTarget`]; and a malformed update with
/// [`EncodeError::Invalid`] or [`EncodeError::LimitExceeded`].
pub fn encode_update(
    w: &mut Writer,
    scope: IdScope,
    update: &TerminalUpdate,
    flags: u64,
) -> Result<(), EncodeError> {
    let TerminalUpdate {
        seq,
        baseline,
        digest,
        metadata,
        styles,
        rows,
        ..
    } = update;
    let size = metadata.size;
    if size.columns == 0 || size.rows == 0 || size.cells() > MAX_GRID_CELLS {
        return Err(EncodeError::LimitExceeded { field: "grid size" });
    }
    if rows.len() != usize::from(size.rows) {
        return Err(EncodeError::Invalid("row count"));
    }
    if styles.iter().any(|style| style.flags & !flags != 0) {
        return Err(EncodeError::Unsupported);
    }
    // The decoder's structural checks, so a sender refuses locally what its
    // peer would treat as a protocol violation.
    if *seq == 0 || baseline.is_some_and(|baseline| baseline >= *seq) {
        return Err(EncodeError::Invalid("update sequence"));
    }
    if baseline.is_none() && digest.is_none() {
        return Err(EncodeError::Invalid("complete snapshot digest"));
    }
    for op in rows {
        match op {
            RowOp::Keep(index) if baseline.is_none() || *index >= size.rows => {
                return Err(EncodeError::Invalid("keep index"));
            }
            RowOp::Full(row)
                if row.runs.iter().any(|run| {
                    run.texts.is_empty()
                        || styles.get(run.style as usize).is_none_or(|style| {
                            run.wide
                                && !(heads_wide(style.flags)
                                    && wide_texts(&run.texts))
                        })
                }) =>
            {
                return Err(EncodeError::Invalid("run"));
            }
            _ => {}
        }
    }
    write_update(w, scope, update)
}

/// Writes an update's record without the structural checks, so tests can
/// build the malformed records a decoder must reject.
fn write_update(
    w: &mut Writer,
    scope: IdScope,
    update: &TerminalUpdate,
) -> Result<(), EncodeError> {
    let TerminalUpdate {
        viewer,
        seq,
        baseline,
        credited,
        digest,
        metadata,
        styles,
        rows,
    } = update;
    let ViewMetadata {
        generation,
        revision,
        geometry_revision,
        size,
        cursor,
        modes,
        viewport,
        history_size,
        cursor_color,
        default_background,
    } = metadata;
    w.record(|w| {
        w.u16(size.columns);
        w.u16(size.rows);
        w.id(scope, *viewer)?;
        w.u64(*seq);
        w.option(*baseline, |w, baseline| {
            w.u64(baseline);
            Ok(())
        })?;
        w.bool(*credited);
        w.option(*digest, |w, digest| {
            w.fixed(&digest.to_le_bytes());
            Ok(())
        })?;
        w.u64(*generation);
        w.u64(*revision);
        w.u64(*geometry_revision);
        w.option(*cursor, |w, Cursor { row, column, shape }| {
            w.record(|w| {
                w.u16(row);
                w.u16(column);
                w.tag(u64::from(shape_tag(shape)));
                Ok(())
            })
        })?;
        write_modes(w, *modes)?;
        w.usize(viewport.bottom_offset);
        w.usize(*history_size);
        w.option(*cursor_color, |w, rgb| {
            write_rgb(w, rgb);
            Ok(())
        })?;
        w.option(*default_background, |w, rgb| {
            write_rgb(w, rgb);
            Ok(())
        })?;
        w.seq(styles.iter(), size.cells(), "style table", write_style)?;
        let ops = wire_ops(rows);
        w.seq(ops.iter(), usize::from(size.rows), "rows", |w, op| {
            write_op(w, op, usize::from(size.columns))
        })
    })
}

fn write_style(w: &mut Writer, style: &StyleEntry) -> Result<(), EncodeError> {
    let StyleEntry {
        foreground,
        background,
        flags,
    } = style;
    w.record(|w| {
        write_color(w, *foreground);
        write_color(w, *background);
        w.u64(*flags);
        Ok(())
    })
}

/// A row operation as it travels: consecutive kept rows become one range.
enum WireOp<'a> {
    /// `count` baseline rows from `start`, in order.
    Keep { start: u16, count: u16 },
    /// A row in full.
    Full(&'a FullRow),
}

fn wire_ops(rows: &[RowOp]) -> Vec<WireOp<'_>> {
    let mut ops: Vec<WireOp<'_>> = Vec::new();
    for op in rows {
        match (op, ops.last_mut()) {
            (RowOp::Keep(index), Some(WireOp::Keep { start, count }))
                if u32::from(*start) + u32::from(*count)
                    == u32::from(*index) =>
            {
                *count += 1;
            }
            (RowOp::Keep(index), _) => ops.push(WireOp::Keep {
                start: *index,
                count: 1,
            }),
            (RowOp::Full(row), _) => ops.push(WireOp::Full(row)),
        }
    }
    ops
}

fn write_op(
    w: &mut Writer,
    op: &WireOp<'_>,
    columns: usize,
) -> Result<(), EncodeError> {
    match op {
        WireOp::Keep { start, count } => {
            w.tag(0);
            w.u16(*start);
            w.u16(*count);
            Ok(())
        }
        WireOp::Full(row) => {
            w.tag(1);
            encode_row(w, row, columns)
        }
    }
}

fn encode_row(
    w: &mut Writer,
    row: &FullRow,
    columns: usize,
) -> Result<(), EncodeError> {
    let FullRow {
        wrapped,
        wrap_continuation,
        prompt,
        runs,
    } = row;
    if runs.iter().map(|run| run.texts.len()).sum::<usize>() != columns {
        return Err(EncodeError::Invalid("row width"));
    }
    w.record(|w| {
        w.bool(*wrapped);
        w.bool(*wrap_continuation);
        w.tag(u64::from(prompt_tag(*prompt)));
        w.seq(runs.iter(), columns, "runs", |w, run| {
            write_run(w, run, columns)
        })
    })
}

/// Writes a run behind a header of its style index and form: a wide run
/// as its characters, packed; at least [`FILL_CELLS`] identical cells that
/// pack as a count and one packed cell; other cells that all pack as one
/// packed string; and any other run as one length-prefixed text per cell.
fn write_run(
    w: &mut Writer,
    run: &Run,
    columns: usize,
) -> Result<(), EncodeError> {
    let header = u64::from(run.style) << 2;
    if run.wide {
        w.u64(header | FORM_WIDE);
        let characters = run.texts.iter().step_by(2);
        return if write_packed(w, characters.len(), characters)? {
            Ok(())
        } else {
            Err(EncodeError::Invalid("run"))
        };
    }
    if let [first, rest @ ..] = run.texts.as_slice()
        && run.texts.len() >= FILL_CELLS
        && packs(first)
        && rest.iter().all(|text| text == first)
    {
        w.u64(header | FORM_FILL);
        w.usize(run.texts.len());
        write_packed(w, 1, [first])?;
        return Ok(());
    }
    let start = w.len();
    w.u64(header | FORM_PACKED);
    if write_packed(w, run.texts.len(), &run.texts)? {
        return Ok(());
    }
    w.truncate(start);
    w.u64(header | FORM_TEXTS);
    w.seq(run.texts.iter(), columns, "run", |w, text| {
        w.str(text, MAX_CELL_TEXT_BYTES, "cell text")
    })
}

/// Writes `count` texts as one length-prefixed UTF-8 string, one scalar per
/// cell and U+0000 for no text. Returns `false`, with the bytes written so
/// far left for the caller to discard, at a text that does not pack.
fn write_packed<'a>(
    w: &mut Writer,
    count: usize,
    texts: impl IntoIterator<Item = &'a CellText>,
) -> Result<bool, EncodeError> {
    let mut packed = true;
    // A cell takes at least a byte, so the length rarely needs more room
    // than the cell count does.
    w.length_prefixed(varint_len(count as u64), |w| {
        for text in texts {
            match text.as_bytes() {
                [] => w.fixed(&[0]),
                ascii @ [1..=0x7f] => w.fixed(ascii),
                bytes if packs(text) => w.fixed(bytes),
                _ => {
                    packed = false;
                    break;
                }
            }
        }
        Ok(())
    })?;
    Ok(packed)
}

/// Reads a packed string of at most `cells` scalars, passing each cell's
/// text to `push`.
fn read_packed(
    r: &mut Reader<'_>,
    cells: usize,
    mut push: impl FnMut(CellText),
) -> Result<(), DecodeError> {
    let packed = r.str(cells * 4, "run")?;
    for (index, scalar) in packed.chars().enumerate() {
        if index == cells {
            return Err(DecodeError::TooLong { field: "run" });
        }
        push(if scalar == '\0' {
            CellText::EMPTY
        } else {
            CellText::from(scalar)
        });
    }
    Ok(())
}

/// Reads an update record's grid without consuming the rest, so a receiver
/// can derive its cap from a prefix of the message.
///
/// # Errors
/// Returns [`DecodeError::Truncated`] when the prefix ends first.
pub fn read_grid(r: &mut Reader<'_>) -> Result<GridSize, DecodeError> {
    Ok(GridSize {
        columns: r.u16()?,
        rows: r.u16()?,
    })
}

/// Decodes an update record in `scope`, accepting only the style flag bits
/// in `flags`. It checks the grid against [`MAX_GRID_CELLS`], that there are
/// exactly `size.rows` rows of exactly `size.columns` cells, that every run
/// holds cells and names a style in the table, that a complete snapshot
/// keeps no row, and every text against [`MAX_CELL_TEXT_BYTES`].
///
/// # Errors
/// Returns the first check that fails; each is a protocol violation.
pub fn decode_update(
    r: &mut Reader<'_>,
    scope: IdScope,
    flags: u64,
) -> Result<TerminalUpdate, DecodeError> {
    r.record(|r| {
        let size = read_grid(r)?;
        if size.columns == 0 || size.rows == 0 || size.cells() > MAX_GRID_CELLS
        {
            return Err(DecodeError::Invalid("grid size"));
        }
        let viewer = r.id(scope)?;
        let seq = r.u64()?;
        let baseline = r.option("update baseline", Reader::u64)?;
        if seq == 0 || baseline.is_some_and(|baseline| baseline >= seq) {
            return Err(DecodeError::Invalid("update sequence"));
        }
        let credited = r.bool()?;
        let digest =
            r.option("update digest", |r| Ok(u64::from_le_bytes(r.fixed()?)))?;
        if baseline.is_none() && digest.is_none() {
            return Err(DecodeError::Invalid("complete snapshot digest"));
        }
        let generation = r.u64()?;
        let revision = r.u64()?;
        let geometry_revision = r.u64()?;
        let cursor = r.option("cursor", |r| {
            r.record(|r| {
                Ok(Cursor {
                    row: r.u16()?,
                    column: r.u16()?,
                    shape: read_shape(r)?,
                })
            })
        })?;
        let modes = read_modes(r)?;
        let viewport = Viewport {
            bottom_offset: r.usize()?,
        };
        let history_size = r.usize()?;
        let cursor_color = r.option("cursor color", read_rgb)?;
        let default_background = r.option("default background", read_rgb)?;
        let styles = r.seq(size.cells(), "style table", |r| {
            r.record(|r| {
                let style = StyleEntry {
                    foreground: read_color(r)?,
                    background: read_color(r)?,
                    flags: r.u64()?,
                };
                if style.flags & !flags != 0 {
                    return Err(DecodeError::Invalid("style flags"));
                }
                Ok(style)
            })
        })?;
        let columns = usize::from(size.columns);
        let rows = decode_ops(r, size, baseline.is_some(), columns, &styles)?;
        if rows.len() != usize::from(size.rows) {
            return Err(DecodeError::Invalid("row count"));
        }
        Ok(TerminalUpdate {
            viewer,
            seq,
            baseline,
            credited,
            digest,
            metadata: ViewMetadata {
                generation,
                revision,
                geometry_revision,
                size,
                cursor,
                modes,
                viewport,
                history_size,
                cursor_color,
                default_background,
            },
            styles,
            rows,
        })
    })
}

/// Reads an update's row operations, expanding each range of kept rows,
/// which a complete snapshot never holds.
fn decode_ops(
    r: &mut Reader<'_>,
    size: GridSize,
    delta: bool,
    columns: usize,
    styles: &[StyleEntry],
) -> Result<Vec<RowOp>, DecodeError> {
    let total = usize::from(size.rows);
    let count = r.usize()?;
    if count > total {
        return Err(DecodeError::TooLong { field: "rows" });
    }
    let mut rows = Vec::with_capacity(total.min(r.remaining()));
    for _ in 0..count {
        match r.tag()? {
            0 => {
                let start = r.u16()?;
                let kept = r.u16()?;
                let end = usize::from(start) + usize::from(kept);
                if !delta || kept == 0 || end > total {
                    return Err(DecodeError::Invalid("keep index"));
                }
                // Checked per operation, so repeated ranges never build
                // more rows than the grid holds.
                if rows.len() + usize::from(kept) > total {
                    return Err(DecodeError::TooLong { field: "rows" });
                }
                rows.extend((start..).take(usize::from(kept)).map(RowOp::Keep));
            }
            1 if rows.len() == total => {
                return Err(DecodeError::TooLong { field: "rows" });
            }
            1 => rows.push(RowOp::Full(decode_row(r, columns, styles)?)),
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "row op",
                    tag,
                });
            }
        }
    }
    Ok(rows)
}

fn decode_row(
    r: &mut Reader<'_>,
    columns: usize,
    styles: &[StyleEntry],
) -> Result<FullRow, DecodeError> {
    r.record(|r| {
        let wrapped = r.bool()?;
        let wrap_continuation = r.bool()?;
        let prompt = read_prompt(r)?;
        let mut cells = 0_usize;
        let runs = r.seq(columns, "runs", |r| {
            let header = r.u64()?;
            let style = u32::try_from(header >> 2)
                .ok()
                .filter(|style| (*style as usize) < styles.len())
                .ok_or(DecodeError::Invalid("style index"))?;
            let remaining = columns - cells;
            let mut texts = Vec::new();
            let wide = header & 3 == FORM_WIDE;
            match header & 3 {
                FORM_TEXTS => {
                    texts = r.seq(remaining, "run", |r| {
                        Ok(CellText::new(
                            r.str(MAX_CELL_TEXT_BYTES, "cell text")?,
                        ))
                    })?;
                }
                FORM_PACKED => {
                    texts.reserve(r.peek().len().min(remaining));
                    read_packed(r, remaining, |text| texts.push(text))?;
                }
                FORM_FILL => {
                    let count = r.usize()?;
                    if count > remaining {
                        return Err(DecodeError::TooLong { field: "run" });
                    }
                    let mut fill = None;
                    read_packed(r, 1, |text| fill = Some(text))?;
                    let fill = fill.ok_or(DecodeError::Invalid("fill text"))?;
                    texts = vec![fill; count];
                }
                _ => {
                    if !heads_wide(styles[style as usize].flags) {
                        return Err(DecodeError::Invalid("wide run style"));
                    }
                    texts.reserve(r.peek().len().min(remaining));
                    read_packed(r, remaining / 2, |text| {
                        texts.push(text);
                        texts.push(CellText::EMPTY);
                    })?;
                }
            }
            if texts.is_empty() {
                return Err(DecodeError::Invalid("empty run"));
            }
            cells += texts.len();
            Ok(Run { style, wide, texts })
        })?;
        if cells != columns {
            return Err(DecodeError::Invalid("row width"));
        }
        Ok(FullRow {
            wrapped,
            wrap_continuation,
            prompt,
            runs,
        })
    })
}

#[cfg(test)]
mod tests;
