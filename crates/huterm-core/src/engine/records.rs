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
    self as ghostty, Point, PromptKind, Screen, SemanticPrompt,
    SemanticPromptKind, Terminal, TrackedRef,
};
use huterm_protocol::{COMMAND_RECORD_CAPACITY, CommandRecord, CommandState};

/// Longest command line a record keeps, in UTF-8 bytes. The pinned parser
/// already rejects an OSC 133 whose text after `133;` exceeds 2,048 bytes;
/// this guards against a pin that raises that limit.
const COMMAND_TEXT_LIMIT: usize = 4096;

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
