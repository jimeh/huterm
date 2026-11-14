//! Command records built from the OSC 133 events Ghostty reports.
//!
//! Each record holds one Ghostty tracked reference at its prompt, so it
//! survives reflow and leaves when its rows leave history. Events are
//! handled at the output position that sent them, before the next byte is
//! parsed, so the cursor and active screen they read are the ones the
//! marker saw.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use huterm_ghostty::{
    self as ghostty, CellSemantic, Point, PointSpace, PromptKind, RowRef,
    RowSemanticPrompt, Screen, SemanticPrompt, SemanticPromptKind, Terminal,
    TrackedRef,
};
use huterm_protocol::{
    COMMAND_RECORD_CAPACITY, CommandBlock, CommandRecord, CommandState,
    MAX_STATUS_TEXT_BYTES, STICKY_PROMPT_ROWS,
};

/// Longest command line a record keeps, in UTF-8 bytes: the status text
/// limit, so every status stays encodable. The pinned parser already rejects
/// an OSC 133 whose text after `133;` exceeds 2,048 bytes; this guards
/// against a pin that raises that limit.
const COMMAND_TEXT_LIMIT: usize = MAX_STATUS_TEXT_BYTES;

/// Most rows a prompt and its command line span before a block's output.
const PROMPT_GROUP_LIMIT: usize = 16;

/// Most trailing rows a block's end trims, so a block that ends in many
/// blank rows costs a bounded scan.
const TRIM_LIMIT: usize = 64;

/// The viewport and grid a layout is computed for, in screen rows counted
/// from the top of history.
#[derive(Clone, Copy, Debug)]
pub(super) struct View {
    /// Rows in history and the active area.
    pub(super) total: usize,
    /// Viewport rows.
    pub(super) rows: usize,
    /// Columns.
    pub(super) columns: u16,
    /// The viewport's first row.
    pub(super) top: usize,
    /// The cursor's row.
    pub(super) cursor: usize,
}

impl View {
    /// The first row of the active area.
    const fn active(&self) -> usize {
        self.total.saturating_sub(self.rows)
    }

    /// Rows above the live bottom, as snapshots count them.
    const fn rows_above_live(&self, row: usize) -> usize {
        self.total.saturating_sub(1).saturating_sub(row)
    }

    /// Resolves `row`, through the cheapest coordinate space that holds it.
    /// Screen points walk history from its nearer end.
    pub(super) fn row<'a, H>(
        &self,
        terminal: &'a Terminal<H>,
        row: usize,
    ) -> Result<RowRef<'a>, ghostty::Error> {
        let last = self.columns.saturating_sub(1);
        let point = if row >= self.active() {
            Point::active(last, offset(row - self.active())?)
        } else if row >= self.top && row < self.top + self.rows {
            Point::viewport(last, offset(row - self.top)?)
        } else {
            Point::screen(last, offset(row)?)
        };
        terminal.row_ref(point)
    }
}

fn offset(row: usize) -> Result<u32, ghostty::Error> {
    u32::try_from(row).map_err(|_| ghostty::Error::InvalidValue)
}

/// Command blocks positioned for one snapshot.
#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct Layout {
    /// Blocks that reach into the viewport, in row order.
    pub(super) blocks: Vec<CommandBlock>,
    /// The prompt rows of the block whose output fills the viewport's top
    /// row while its prompt is above the viewport.
    pub(super) sticky: Option<StickyRows>,
}

/// Where a sticky prompt's rows are, in screen rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct StickyRows {
    pub(super) sequence: u64,
    /// The first row the header shows.
    pub(super) first: usize,
    /// How many rows it shows, at most [`STICKY_PROMPT_ROWS`].
    pub(super) count: usize,
    /// The row holding the command line, within those rows.
    pub(super) input: usize,
}

/// A record's prompt row, in screen rows, once validated.
#[derive(Clone, Copy, Debug)]
struct Start {
    sequence: u64,
    row: usize,
}

/// A tracked reference that marks where a command's block starts.
#[derive(Debug)]
struct Anchor(TrackedRef);

impl Anchor {
    /// An anchor at the cursor, or `None` when Ghostty cannot allocate one.
    fn at_cursor<H>(terminal: &mut Terminal<H>) -> Option<Self> {
        let (column, row) = terminal.cursor_position().ok()?;
        let reference =
            terminal.track(Point::active(column, row.into())).ok()?;
        #[cfg(test)]
        live::created();
        Some(Self(reference))
    }
}

#[cfg(test)]
impl Drop for Anchor {
    fn drop(&mut self) {
        live::dropped();
    }
}

#[derive(Debug)]
struct Entry {
    record: CommandRecord,
    /// `None` when Ghostty could not allocate a reference; eviction then
    /// leaves the record to the count cap.
    anchor: Option<Anchor>,
    started: Instant,
    /// The record left `Running` since the last publication, which must
    /// show that final state before eviction may remove it.
    settled: bool,
}

impl Entry {
    fn settle(&mut self, state: CommandState) {
        self.record.state = state;
        self.record.finished_at = Some(SystemTime::now());
        self.record.duration = Some(self.started.elapsed());
        self.settled = true;
    }

    fn running(&self) -> bool {
        self.record.state == CommandState::Running
    }
}

/// A terminal's command records and the prompt anchor waiting for a command.
#[derive(Debug, Default)]
pub(super) struct CommandLog {
    entries: VecDeque<Entry>,
    /// Set by a primary prompt start, taken by the next output start.
    pending: Option<Anchor>,
    last_sequence: u64,
    changed: bool,
}

impl CommandLog {
    /// Applies one event at the position that reported it.
    pub(super) fn handle<H>(
        &mut self,
        terminal: &mut Terminal<H>,
        event: &SemanticPrompt,
    ) -> Result<(), ghostty::Error> {
        match event.kind {
            SemanticPromptKind::PromptStart
                if event.prompt == PromptKind::Primary =>
            {
                if terminal.screen()? == Screen::Alternate {
                    return Ok(());
                }
                self.abandon_running();
                // Free the old anchor before taking a new one, so a redraw
                // never holds two.
                self.pending = None;
                self.pending = Anchor::at_cursor(terminal);
            }
            SemanticPromptKind::OutputStart => {
                if terminal.screen()? == Screen::Alternate {
                    return Ok(());
                }
                self.start(terminal, &event.command);
            }
            SemanticPromptKind::CommandEnd => {
                if let Some(newest) =
                    self.entries.back_mut().filter(|entry| entry.running())
                {
                    newest.settle(CommandState::Finished {
                        exit_code: event.exit_code,
                    });
                    self.changed = true;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn start<H>(&mut self, terminal: &mut Terminal<H>, command: &[u8]) {
        self.abandon_running();
        let pending = self
            .pending
            .take()
            .filter(|anchor| terminal.has_value(&anchor.0));
        if self.entries.len() == COMMAND_RECORD_CAPACITY {
            self.entries.pop_front();
        }
        let anchor = pending.or_else(|| Anchor::at_cursor(terminal));
        self.last_sequence = self.last_sequence.saturating_add(1);
        self.entries.push_back(Entry {
            record: CommandRecord {
                sequence: self.last_sequence,
                text: command_text(command),
                state: CommandState::Running,
                started_at: SystemTime::now(),
                finished_at: None,
                duration: None,
            },
            anchor,
            started: Instant::now(),
            settled: false,
        });
        self.changed = true;
    }

    /// Ends a running record without a report. Only the newest record can
    /// be running.
    pub(super) fn abandon_running(&mut self) {
        if let Some(newest) =
            self.entries.back_mut().filter(|entry| entry.running())
        {
            newest.settle(CommandState::Abandoned);
            self.changed = true;
        }
    }

    /// Removes records whose anchor lost its value, except the newest and
    /// those that left `Running` since the last publication, and returns
    /// the list when anything changed since then.
    pub(super) fn publish<H>(
        &mut self,
        terminal: &Terminal<H>,
    ) -> Option<Arc<[CommandRecord]>> {
        let newest = self.last_sequence;
        let before = self.entries.len();
        // Command order need not match row order, so check every record.
        self.entries.retain(|entry| {
            entry.record.sequence == newest
                || entry.settled
                || entry
                    .anchor
                    .as_ref()
                    .is_none_or(|anchor| terminal.has_value(&anchor.0))
        });
        self.changed |= self.entries.len() != before;
        for entry in &mut self.entries {
            entry.settled = false;
        }
        std::mem::take(&mut self.changed).then(|| self.records())
    }

    pub(super) fn records(&self) -> Arc<[CommandRecord]> {
        self.entries
            .iter()
            .map(|entry| entry.record.clone())
            .collect()
    }

    /// Positions the blocks that reach into `view` and the sticky prompt.
    ///
    /// Every record with a value is read once in viewport space, which
    /// walks only from the viewport's top page and reports no value for a
    /// record above the viewport; at live output that is a page or two per
    /// record. Records are ordered by row, not sequence: cursor addressing
    /// can draw a newer prompt above an older one. A record is used only
    /// when its anchor sits on a primary prompt row, and of records on one
    /// row only the newest is: in-place erasure can move an older anchor
    /// onto a newer prompt. The records above the viewport are placed in
    /// screen space, and the lowest validated one owns the viewport's top
    /// row.
    pub(super) fn layout<H>(
        &self,
        terminal: &Terminal<H>,
        view: View,
    ) -> Result<Layout, ghostty::Error> {
        let mut above = Vec::new();
        let mut placed: Vec<Start> = Vec::new();
        for entry in &self.entries {
            let Some(anchor) = entry
                .anchor
                .as_ref()
                .filter(|anchor| terminal.has_value(&anchor.0))
            else {
                continue;
            };
            let point = terminal
                .tracked_point(&anchor.0, PointSpace::Viewport)
                .ok()
                .flatten()
                .and_then(|point| usize::try_from(point.y).ok());
            match point {
                Some(row) => placed.push(Start {
                    sequence: entry.record.sequence,
                    row: view.top + row,
                }),
                None => above.push(entry),
            }
        }
        // Newest first within a row, so the dedup keeps the newest.
        placed.sort_by_key(|start| {
            (start.row, std::cmp::Reverse(start.sequence))
        });
        placed.dedup_by_key(|start| start.row);
        let bottom = view.top + view.rows;
        let mut starts = Vec::new();
        let mut below = None;
        for start in placed {
            if !is_prompt_start(terminal, view, start.row)? {
                continue;
            }
            if start.row < bottom {
                starts.push(start);
            } else {
                below = Some(start.row);
                break;
            }
        }

        let owner = owner_start(terminal, view, &above)?.filter(|owner| {
            starts.first().is_none_or(|first| owner.row < first.row)
        });
        let pending = self
            .pending
            .as_ref()
            .and_then(|anchor| screen_row(terminal, view, &anchor.0));
        let all: Vec<Start> = owner.into_iter().chain(starts).collect();
        let mut layout = Layout::default();
        for (index, start) in all.iter().enumerate() {
            // The next prompt below this one, whichever comes first.
            let bound =
                [all.get(index + 1).map(|next| next.row), below, pending]
                    .into_iter()
                    .flatten()
                    .filter(|&row| row > start.row)
                    .min()
                    .unwrap_or_else(|| view.cursor.max(start.row) + 1);
            let (output, end) = extent(terminal, view, start.row, bound)?;
            let block = CommandBlock {
                sequence: start.sequence,
                prompt: view.rows_above_live(start.row),
                output: output.map(|row| view.rows_above_live(row)),
                end: view.rows_above_live(end),
            };
            if index == 0 && owner.is_some() {
                if end < view.top {
                    continue;
                }
                if let Some(output) =
                    output.filter(|&output| output <= view.top)
                {
                    layout.sticky =
                        Some(sticky_rows(terminal, view, *start, output)?);
                }
            }
            layout.blocks.push(block);
        }
        Ok(layout)
    }

    /// The text of the screen row `sequence` is anchored on, for tests of
    /// where anchors land.
    #[cfg(test)]
    pub(super) fn anchored_text<H>(
        &self,
        terminal: &Terminal<H>,
        sequence: u64,
    ) -> Option<String> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.record.sequence == sequence)?;
        let anchor = entry.anchor.as_ref()?;
        let point = terminal
            .tracked_point(&anchor.0, ghostty::PointSpace::Screen)
            .ok()??;
        let (columns, _) = terminal.size().ok()?;
        let start = Point::screen(0, point.y);
        let end = Point::screen(columns.saturating_sub(1), point.y);
        let text = terminal.format_plain(start, end).ok()?;
        Some(String::from_utf8_lossy(text.as_bytes()).into_owned())
    }
}

/// The validated prompt row nearest above the viewport among `above`, the
/// records above it. Sequence order says nothing here: cursor addressing
/// can draw any number of newer prompts above an older one, so every record
/// is placed in screen space, newest first on a shared row, and validated
/// from the lowest until one passes.
fn owner_start<H>(
    terminal: &Terminal<H>,
    view: View,
    above: &[&Entry],
) -> Result<Option<Start>, ghostty::Error> {
    let mut placed: Vec<Start> = above
        .iter()
        .filter_map(|entry| {
            let anchor = entry.anchor.as_ref()?;
            let point = terminal
                .tracked_point(&anchor.0, PointSpace::Screen)
                .ok()??;
            let row = usize::try_from(point.y).ok()?;
            (row < view.top).then_some(Start {
                sequence: entry.record.sequence,
                row,
            })
        })
        .collect();
    placed.sort_by_key(|start| std::cmp::Reverse((start.row, start.sequence)));
    placed.dedup_by_key(|start| start.row);
    for start in placed {
        if is_prompt_start(terminal, view, start.row)? {
            return Ok(Some(start));
        }
    }
    Ok(None)
}

/// An anchor's screen row, read in the cheapest space that holds it.
fn screen_row<H>(
    terminal: &Terminal<H>,
    view: View,
    reference: &TrackedRef,
) -> Option<usize> {
    let read = |space| {
        terminal
            .tracked_point(reference, space)
            .ok()
            .flatten()
            .and_then(|point| usize::try_from(point.y).ok())
    };
    read(PointSpace::Active)
        .map(|row| view.active() + row)
        .or_else(|| {
            read(PointSpace::Viewport)
                .map(|row| view.top + row)
                .filter(|&row| row < view.active())
        })
        .or_else(|| read(PointSpace::Screen))
}

fn row_mark<H>(
    terminal: &Terminal<H>,
    view: View,
    row: usize,
) -> Result<RowSemanticPrompt, ghostty::Error> {
    view.row(terminal, row)?.row()?.semantic_prompt()
}

fn is_prompt_start<H>(
    terminal: &Terminal<H>,
    view: View,
    row: usize,
) -> Result<bool, ghostty::Error> {
    Ok(row_mark(terminal, view, row)? == RowSemanticPrompt::Prompt)
}

/// A block's first output row, if it has one, and its last row with text,
/// for a block from `start` to before `bound`. Trailing rows without text
/// or with a prompt mark, such as an empty prompt left by Enter or Ctrl-L,
/// are trimmed, at most [`TRIM_LIMIT`] of them.
fn extent<H>(
    terminal: &Terminal<H>,
    view: View,
    start: usize,
    bound: usize,
) -> Result<(Option<usize>, usize), ghostty::Error> {
    let mut row = start + 1;
    while row < bound
        && row - start < PROMPT_GROUP_LIMIT
        && row_mark(terminal, view, row)? == RowSemanticPrompt::Continuation
    {
        row += 1;
    }
    let prompt_end = row - 1;
    if row >= bound {
        return Ok((None, prompt_end));
    }
    let output = row;
    let floor = output.max(bound.saturating_sub(TRIM_LIMIT));
    for candidate in (floor..bound).rev() {
        let line = view.row(terminal, candidate)?;
        if line.row()?.semantic_prompt()? == RowSemanticPrompt::None
            && has_text(&line)?
        {
            return Ok((Some(output), candidate));
        }
    }
    if floor > output {
        // The trim limit ran out before the output's text.
        return Ok((Some(output), bound - 1));
    }
    Ok((None, prompt_end))
}

fn has_text(row: &RowRef<'_>) -> Result<bool, ghostty::Error> {
    for column in 0..=row.last_column() {
        if row.cell(column)?.cell()?.has_text()? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The rows a sticky header shows for `start`'s prompt, which ends before
/// `output`: up to [`STICKY_PROMPT_ROWS`] rows ending at the row that holds
/// the command line, the first with an input cell, or the last prompt row
/// when the shell marked no input.
fn sticky_rows<H>(
    terminal: &Terminal<H>,
    view: View,
    start: Start,
    output: usize,
) -> Result<StickyRows, ghostty::Error> {
    let mut input = output - 1;
    'rows: for row in start.row..output {
        let line = view.row(terminal, row)?;
        for column in 0..=line.last_column() {
            if line.cell(column)?.cell()?.semantic()? == CellSemantic::Input {
                input = row;
                break 'rows;
            }
        }
    }
    // A grid shorter than the most rows a header holds shows fewer, so
    // the sticky prompt never holds more cells than its grid.
    let most = STICKY_PROMPT_ROWS.min(view.rows).max(1);
    let first = start.row.max(input + 1 - most.min(input + 1));
    Ok(StickyRows {
        sequence: start.sequence,
        first,
        count: input - first + 1,
        input,
    })
}

/// The shell-sent command line, without control characters and cut at
/// [`COMMAND_TEXT_LIMIT`] on a character boundary; `None` when empty.
fn command_text(command: &[u8]) -> Option<String> {
    let mut text = String::new();
    for character in String::from_utf8_lossy(command)
        .chars()
        .filter(|character| !character.is_control())
    {
        if text.len() + character.len_utf8() > COMMAND_TEXT_LIMIT {
            break;
        }
        text.push(character);
    }
    (!text.is_empty()).then_some(text)
}

/// Live anchors on this thread, so tests can bound them.
#[cfg(test)]
pub(super) mod live {
    use std::cell::Cell;

    thread_local! {
        static LIVE: Cell<usize> = const { Cell::new(0) };
        static PEAK: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn created() {
        let live = LIVE.get() + 1;
        LIVE.set(live);
        PEAK.set(PEAK.get().max(live));
    }

    pub(super) fn dropped() {
        LIVE.set(LIVE.get() - 1);
    }

    /// Live anchors now, and the most alive at once since the last reset.
    pub(crate) fn counts() -> (usize, usize) {
        (LIVE.get(), PEAK.get())
    }

    /// Restarts the peak from the anchors alive now.
    pub(crate) fn reset_peak() {
        PEAK.set(LIVE.get());
    }
}

#[cfg(test)]
mod tests {
    use super::{COMMAND_TEXT_LIMIT, command_text};

    #[test]
    fn command_text_drops_controls_and_caps_on_a_character_boundary() {
        assert_eq!(
            command_text("a\u{7}\u{1b}\u{7f}\u{9b}b\nc".as_bytes()).as_deref(),
            Some("abc")
        );
        assert_eq!(command_text(b""), None);
        assert_eq!(command_text(b"\x1b\x07"), None);
        // A three-byte character that would cross the limit is left out.
        let long = format!("{}€", "x".repeat(COMMAND_TEXT_LIMIT - 2));
        let text = command_text(long.as_bytes()).unwrap();
        assert_eq!(text.len(), COMMAND_TEXT_LIMIT - 2);
        let exact = "y".repeat(COMMAND_TEXT_LIMIT + 10);
        assert_eq!(
            command_text(exact.as_bytes()).unwrap().len(),
            COMMAND_TEXT_LIMIT
        );
        assert_eq!(command_text(b"\xffok").as_deref(), Some("\u{fffd}ok"));
    }
}
