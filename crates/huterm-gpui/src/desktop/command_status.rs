//! Shell command marks drawn over a terminal: the failure tint, prompt
//! marks, and margin bars that show how commands ended, and the sticky
//! prompt header's rows, status, and accessible label.
//!
//! Positions come from the snapshot's command blocks and states from the
//! status's command records, matched by sequence. The two arrive apart, so
//! either can be newer; a block whose record the status no longer holds,
//! or does not hold yet, is drawn as unknown.

use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui::{Bounds, Hsla, Pixels, Window, fill, point, px, size};
use huterm_config::{CommandStatusStyle, StickyPromptStyle};
use huterm_protocol::{
    CommandRecord, CommandState, TerminalAppearance, TerminalRow,
    TerminalSnapshot, appearance_for_background,
};

use crate::config::Theme;
use crate::renderer::rgb_color;

/// The fewest grid rows a pane shows the sticky prompt in, so a short pane
/// keeps its rows for output.
pub(super) const STICKY_MIN_ROWS: u16 = 6;

/// The width of a prompt mark or margin bar.
pub(super) const MARK_WIDTH: Pixels = px(3.0);

/// How a finished command ended. Running, abandoned, and unreported
/// commands have none, so they are never shown as succeeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Outcome {
    Succeeded,
    Failed,
}

fn outcome(record: &CommandRecord) -> Option<Outcome> {
    match record.state {
        CommandState::Finished { exit_code: Some(0) } => {
            Some(Outcome::Succeeded)
        }
        CommandState::Finished { exit_code: Some(_) } => Some(Outcome::Failed),
        CommandState::Finished { exit_code: None }
        | CommandState::Running
        | CommandState::Abandoned => None,
    }
}

/// The record numbered `sequence`; `records` is oldest first.
pub(super) fn record(
    records: &[CommandRecord],
    sequence: u64,
) -> Option<&CommandRecord> {
    records
        .binary_search_by_key(&sequence, |record| record.sequence)
        .ok()
        .and_then(|index| records.get(index))
}

/// What a viewport draws for its blocks, in viewport rows.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Marks {
    /// Rows with the failure tint behind them.
    pub(super) tints: Vec<Range<usize>>,
    /// Rows with a failure bar in the left margin.
    pub(super) bars: Vec<Range<usize>>,
    /// Prompt rows with a mark in the left margin, and how they ended.
    pub(super) prompts: Vec<(Range<usize>, Outcome)>,
}

impl Marks {
    pub(super) fn is_empty(&self) -> bool {
        self.tints.is_empty() && self.bars.is_empty() && self.prompts.is_empty()
    }
}

/// The viewport rows from `first` down to `last`, both counted from the
/// live bottom, clipped to the viewport.
fn viewport_rows(
    snapshot: &TerminalSnapshot,
    first: usize,
    last: usize,
) -> Option<Range<usize>> {
    let rows = usize::from(snapshot.size.rows);
    let top = snapshot.viewport.bottom_offset + rows.checked_sub(1)?;
    let start = top.saturating_sub(first);
    let end = (top.checked_sub(last)? + 1).min(rows);
    (start < end).then_some(start..end)
}

/// The marks `style` draws for the snapshot's blocks.
pub(super) fn marks(
    snapshot: &TerminalSnapshot,
    records: &[CommandRecord],
    style: CommandStatusStyle,
) -> Marks {
    let mut marks = Marks::default();
    if style == CommandStatusStyle::Off {
        return marks;
    }
    for block in snapshot.commands.iter() {
        let Some(outcome) = record(records, block.sequence).and_then(outcome)
        else {
            continue;
        };
        let rows = viewport_rows(snapshot, block.prompt, block.end);
        match style {
            CommandStatusStyle::Tint | CommandStatusStyle::TintMarks
                if outcome == Outcome::Failed =>
            {
                marks.tints.extend(rows);
            }
            CommandStatusStyle::Bar if outcome == Outcome::Failed => {
                marks.bars.extend(rows);
            }
            _ => {}
        }
        if style == CommandStatusStyle::TintMarks {
            let prompt_end = block
                .output
                .map_or(block.end, |output| output.saturating_add(1));
            if let Some(rows) =
                viewport_rows(snapshot, block.prompt, prompt_end)
            {
                marks.prompts.push((rows, outcome));
            }
        }
    }
    marks
}

/// The colors marks use: the theme's ANSI red and green, and a tint alpha
/// that stays faint on light themes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MarkColors {
    pub(super) failed: Hsla,
    pub(super) succeeded: Hsla,
    pub(super) tint: Hsla,
}

impl MarkColors {
    pub(super) fn new(theme: &Theme) -> Self {
        let failed = rgb_color(theme.ansi[1]);
        let alpha = match appearance_for_background(theme.background) {
            TerminalAppearance::Dark => 0.10,
            TerminalAppearance::Light => 0.07,
        };
        Self {
            failed,
            succeeded: rgb_color(theme.ansi[2]),
            tint: failed.opacity(alpha),
        }
    }
}

/// Paints `marks` across `pane`, whose grid rows start at `grid_top`. The
/// tint spans the pane's width and the marks sit at its left edge, in the
/// padding. Paint it after the pane's background and before the grid, so
/// cell backgrounds and the selection draw over the tint.
pub(super) fn paint_marks(
    marks: &Marks,
    pane: Bounds<Pixels>,
    grid_top: Pixels,
    cell_height: Pixels,
    colors: MarkColors,
    window: &mut Window,
) {
    let band = |rows: &Range<usize>, left: Pixels, width: Pixels| {
        #[expect(
            clippy::cast_precision_loss,
            reason = "viewport rows are far below f32's exact range"
        )]
        Bounds::new(
            point(left, grid_top + cell_height * rows.start as f32),
            size(width, cell_height * rows.len() as f32),
        )
    };
    for rows in &marks.tints {
        window.paint_quad(fill(
            band(rows, pane.left(), pane.size.width),
            colors.tint,
        ));
    }
    for rows in &marks.bars {
        window.paint_quad(fill(
            band(rows, pane.left(), MARK_WIDTH),
            colors.failed,
        ));
    }
    for (rows, outcome) in &marks.prompts {
        let color = match outcome {
            Outcome::Succeeded => colors.succeeded,
            Outcome::Failed => colors.failed,
        };
        window.paint_quad(fill(band(rows, pane.left(), MARK_WIDTH), color));
    }
}

/// The sticky prompt a pane shows: the rows to draw and the record they
/// belong to.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Header {
    pub(super) sequence: u64,
    pub(super) rows: Vec<Arc<TerminalRow>>,
    /// The row among `rows` that holds the command line.
    pub(super) input: usize,
}

/// The sticky prompt `style` shows for `snapshot`, if any.
pub(super) fn header(
    snapshot: &TerminalSnapshot,
    style: StickyPromptStyle,
) -> Option<Header> {
    let sticky = snapshot.sticky_prompt.as_ref()?;
    if snapshot.size.rows < STICKY_MIN_ROWS || snapshot.modes.alternate_screen {
        return None;
    }
    let input = sticky.rows.get(sticky.input)?;
    let (rows, input) = match style {
        StickyPromptStyle::Off => return None,
        StickyPromptStyle::Status | StickyPromptStyle::Command => {
            (vec![Arc::clone(input)], 0)
        }
        StickyPromptStyle::Prompt => (sticky.rows.clone(), sticky.input),
    };
    Some(Header {
        sequence: sticky.sequence,
        rows,
        input,
    })
}

/// How a command stands, as the header's status shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Chip {
    pub(super) text: String,
    /// The same, in words, for assistive technology.
    pub(super) spoken: String,
    pub(super) outcome: Option<Outcome>,
    /// While the command runs, when its elapsed time next changes.
    pub(super) next_tick: Option<Duration>,
}

/// The status of `record` at `now`, or `None` for a command that ended
/// without reporting.
pub(super) fn chip(record: &CommandRecord, now: SystemTime) -> Option<Chip> {
    match record.state {
        CommandState::Running => {
            let elapsed =
                now.duration_since(record.started_at).unwrap_or_default();
            let clock = clock(elapsed);
            Some(Chip {
                text: format!("running \u{b7} {clock}"),
                spoken: format!("running for {clock}"),
                outcome: None,
                next_tick: Some(Duration::from_secs(1).saturating_sub(
                    Duration::from_nanos(u64::from(elapsed.subsec_nanos())),
                )),
            })
        }
        CommandState::Finished { exit_code } => {
            let took = record.duration.map(duration).unwrap_or_default();
            let (text, spoken) = match exit_code {
                Some(0) => {
                    (format!("\u{2713} {took}"), format!("succeeded in {took}"))
                }
                Some(code) => (
                    format!("\u{2717} exit {code} \u{b7} {took}"),
                    format!("failed with exit status {code} after {took}"),
                ),
                None => (took.clone(), format!("finished in {took}")),
            };
            Some(Chip {
                text,
                spoken,
                outcome: outcome(record),
                next_tick: None,
            })
        }
        CommandState::Abandoned => None,
    }
}

/// Elapsed time as `m:ss`, or `h:mm:ss` from an hour.
fn clock(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let (hours, minutes, seconds) =
        (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// A finished command's duration: tenths of a second below a minute, then
/// a clock.
fn duration(took: Duration) -> String {
    if took < Duration::from_secs(60) {
        format!("{:.1}s", took.as_secs_f64())
    } else {
        clock(took)
    }
}

/// The text of a row, without trailing blanks.
pub(super) fn row_text(row: &TerminalRow) -> String {
    row.cells
        .iter()
        .filter(|cell| !cell.style.wide_spacer)
        .map(|cell| cell.text.displayed())
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// The header's accessible name: what pressing it does, the command line,
/// and how the command stands.
pub(super) fn label(header: &Header, chip: Option<&Chip>) -> String {
    let line = header
        .rows
        .get(header.input)
        .map(|row| row_text(row))
        .unwrap_or_default();
    match chip {
        Some(chip) => format!("Scroll to prompt: {line}, {}", chip.spoken),
        None => format!("Scroll to prompt: {line}"),
    }
}

#[cfg(test)]
mod tests;
