//! Huterm's terminal engine on `huterm-ghostty`.
//!
//! Terminal policy lives here: which replies reach the PTY, clipboard
//! admission, directory normalization, and snapshot construction with row
//! reuse. Everything that depends on Ghostty's C API lives in
//! `huterm-ghostty`.

use std::cell::{Cell as StdCell, RefCell};
use std::sync::Arc;
use std::time::Instant;

use huterm_ghostty::{
    self as ghostty, Boundaries, CellContent, CellWidth, ClipboardLocation,
    ClipboardWrite, ClipboardWriteResult, ColorOverrides, ColorScheme,
    CursorStyle, DeviceAttributes, Dirty, Fill, Host, Mode, MouseProbe, Point,
    ProbedFormat, ProbedTracking, RenderState, RowRef, RowSemanticPrompt,
    Screen, Scroll, StyleColor,
};
use huterm_protocol::{
    BufferPoint, BufferRange, Cell, CellColor, CellSize, CellStyle, CellText,
    CommandBlock, CommandRecord, Cursor, CursorShape, GridSize, LinkLookup,
    MouseEncoding, MouseTracking, Rgb, RowPrompt, ScrollCommand,
    ScrollbackLimit, SelectionUnit, StickyPrompt, TerminalAppearance,
    TerminalId, TerminalModes, TerminalPresentation, TerminalProgress,
    TerminalRow, TerminalSnapshot, Viewport, appearance_for_background,
};

use super::escape_hint::EscapeHint;
use super::links::{LinkBuffer, MAX_LINK_BYTES};
use super::records::{self, CommandLog};
use super::row_matcher::{RowMatcher, viewport_shift};
use super::{DirectoryUpdate, EngineEffect, normalize_directory};
use crate::host_effects::{
    HostEffectAdmission, HostEffectSink, TERMINAL_BYTE_LIMIT,
};
use crate::notifications::{self, NotificationLimiter};
use crate::terminal::RuntimeError;

/// Page allocation allowed, without an explicit byte limit, per configured
/// history row and per screen row, since Ghostty's byte limit counts the
/// screen's pages too. Pages that grow for styles, graphemes, and
/// hyperlinks are bounded only by bytes; plain rows up to about 400 columns
/// fit, so the row limit applies first for them.
const SCROLLBACK_BYTES_PER_ROW: usize = 4 * 1024;
/// Page allocation allowed beyond the rows, for whole partly filled pages,
/// which would otherwise cut small limits on wide screens short.
const SCROLLBACK_BYTES_HEADROOM: usize = 4 * 1024 * 1024;
/// ED 3: erase scrollback only.
const CLEAR_HISTORY: &[u8] = b"\x1b[3J";
/// Longest grapheme cluster a link scan reads from one cell.
const LINK_GRAPHEME_CODEPOINTS: usize = 256;

impl From<ghostty::Error> for RuntimeError {
    fn from(error: ghostty::Error) -> Self {
        Self::Engine(format!("ghostty: {error}"))
    }
}

/// Maps a progress report to the effect that records it; a state this
/// binding does not know changes nothing.
fn progress_effect(report: ghostty::Progress) -> Option<EngineEffect> {
    use ghostty::ProgressState;
    Some(EngineEffect::Progress(match report.state {
        ProgressState::Remove => None,
        ProgressState::Set => {
            Some(TerminalProgress::Determinate(report.percent.unwrap_or(0)))
        }
        ProgressState::Indeterminate => Some(TerminalProgress::Indeterminate),
        ProgressState::Pause => Some(TerminalProgress::Paused(report.percent)),
        ProgressState::Error => Some(TerminalProgress::Error(report.percent)),
        _ => return None,
    }))
}

/// Clipboard and color-scheme policy consulted during writes.
#[derive(Debug, Default)]
struct EngineHost {
    sink: Option<HostEffectSink>,
}

impl Host for EngineHost {
    fn clipboard_write(
        &mut self,
        request: &ClipboardWrite<'_>,
    ) -> ClipboardWriteResult {
        // OSC 52, OSC 1337 Copy, and Kitty OSC 5522 writes share one
        // policy. Kitty names, passwords, and grants change nothing: Huterm
        // never prompts, so the policy alone answers.
        if request.location != ClipboardLocation::Standard {
            return ClipboardWriteResult::Unsupported;
        }
        let text = if request.contents.is_empty() {
            // No representations: clear the clipboard.
            ""
        } else {
            let Some(content) = request
                .contents
                .iter()
                .find(|content| is_plain_text(content.mime))
            else {
                return ClipboardWriteResult::Unsupported;
            };
            match std::str::from_utf8(content.data) {
                Ok(text) => text,
                Err(_) => return ClipboardWriteResult::InvalidData,
            }
        };
        let Some(sink) = &self.sink else {
            // No client has attached yet; allow and drop the write.
            return ClipboardWriteResult::Success;
        };
        match sink.admit_borrowed(text) {
            HostEffectAdmission::Accepted => ClipboardWriteResult::Success,
            HostEffectAdmission::Full => ClipboardWriteResult::Busy,
            HostEffectAdmission::NoRecipient
            | HostEffectAdmission::Denied
            | HostEffectAdmission::Closed => ClipboardWriteResult::Denied,
        }
    }

    fn color_scheme(
        &mut self,
        background: ghostty::Rgb,
    ) -> Option<ColorScheme> {
        Some(match appearance_for_background(rgb(background)) {
            TerminalAppearance::Light => ColorScheme::Light,
            TerminalAppearance::Dark => ColorScheme::Dark,
        })
    }
}

/// Whether a clipboard representation is UTF-8 plain text: `text/plain`
/// in any case, with any parameters, whose `charset`, if present, is
/// `utf-8` or `utf8`, as a token or quoted string. Whitespace around `;`
/// and `=` is ignored, unknown parameters are ignored, and malformed
/// parameters (an unterminated quote, text after a closing quote, or a
/// `charset` without a value) reject the representation.
fn is_plain_text(mime: &[u8]) -> bool {
    std::str::from_utf8(mime)
        .ok()
        .and_then(plain_text_utf8)
        .unwrap_or(false)
}

/// `Some(true)` for UTF-8 plain text, `None` when a parameter is malformed.
fn plain_text_utf8(mime: &str) -> Option<bool> {
    let parts = mime_parts(mime)?;
    let (essence, parameters) = parts.split_first()?;
    if !essence.trim().eq_ignore_ascii_case("text/plain") {
        return Some(false);
    }
    for parameter in parameters {
        let Some((name, value)) = parameter.split_once('=') else {
            // A parameter without a value is named by its first word.
            let name = parameter.split_whitespace().next().unwrap_or_default();
            if name.eq_ignore_ascii_case("charset") {
                return None;
            }
            continue;
        };
        let value = parameter_value(value)?;
        if name.trim().eq_ignore_ascii_case("charset")
            && !value.eq_ignore_ascii_case("utf-8")
            && !value.eq_ignore_ascii_case("utf8")
        {
            return Some(false);
        }
    }
    Some(true)
}

/// Splits a MIME type at the semicolons outside quoted strings; `None`
/// when a quoted string is unterminated.
fn mime_parts(mime: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in mime.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&mime[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if quoted {
        return None;
    }
    parts.push(&mime[start..]);
    Some(parts)
}

/// A parameter value: a token, or a quoted string with backslash escapes
/// and nothing after its closing quote. `None` when malformed.
fn parameter_value(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let Some(quoted) = raw.strip_prefix('"') else {
        return Some(raw.to_owned());
    };
    let mut value = String::new();
    let mut characters = quoted.chars();
    loop {
        match characters.next()? {
            '\\' => value.push(characters.next()?),
            '"' => break,
            character => value.push(character),
        }
    }
    characters.as_str().trim().is_empty().then_some(value)
}

const fn rgb(color: ghostty::Rgb) -> Rgb {
    Rgb {
        red: color.red,
        green: color.green,
        blue: color.blue,
    }
}

const fn native_rgb(color: Rgb) -> ghostty::Rgb {
    ghostty::Rgb::new(color.red, color.green, color.blue)
}

/// Effective colors a snapshot resolves cells against.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Colors {
    foreground: Option<ghostty::Rgb>,
    background: Option<ghostty::Rgb>,
    palette: ghostty::Palette,
}

/// Graphics stays disabled. Mask each native keyboard reply in callback order,
/// since a single output chunk can query several different stack values.
fn supported_reply(bytes: Vec<u8>) -> Option<Vec<u8>> {
    if bytes.starts_with(b"\x1b_G") {
        return None;
    }
    if bytes.starts_with(b"\x1b[?") && bytes.ends_with(b"u") {
        let flags = std::str::from_utf8(&bytes[3..bytes.len() - 1])
            .ok()?
            .parse::<u8>()
            .ok()?;
        return Some(format!("\x1b[?{}u", flags & 5).into_bytes());
    }
    Some(bytes)
}

#[derive(Debug)]
pub(super) struct TerminalEngine {
    terminal: ghostty::Terminal<EngineHost>,
    render: RenderState,
    keyboard: ghostty::KeyEncoder,
    mouse: RefCell<MouseProbe>,
    id: TerminalId,
    size: GridSize,
    cell: CellSize,
    presentation: TerminalPresentation,
    scrollback: HistoryLimit,
    generation: u64,
    modes: StdCell<Option<(u64, TerminalModes)>>,
    hint: EscapeHint,
    overrides: ColorOverrides,
    overrides_stale: bool,
    pending_clear: bool,
    retained: Vec<Arc<TerminalRow>>,
    retained_viewport: (usize, usize),
    retained_colors: Option<Colors>,
    /// An OSC 133 mark arrived since the last snapshot. Ghostty sets a
    /// row's prompt mark without dirtying it, so the next snapshot checks
    /// clean rows against the live grid.
    prompt_marks_stale: bool,
    scratch: TerminalRow,
    grapheme: Vec<u8>,
    effects: Vec<ghostty::Effect>,
    commands: CommandLog,
    /// The last snapshot's sticky prompt, whose rows a new snapshot reuses
    /// when they read the same.
    retained_sticky: Option<StickyPrompt>,
    server_hostname: Option<String>,
    notifications: NotificationLimiter,
    #[cfg(test)]
    stats: super::SnapshotStats,
}

impl TerminalEngine {
    pub(super) fn new(
        id: TerminalId,
        size: GridSize,
        cell: CellSize,
        presentation: TerminalPresentation,
    ) -> Result<Self, RuntimeError> {
        let options = ghostty::Options {
            columns: size.columns,
            rows: size.rows,
            cell_width: u32::from(cell.width),
            cell_height: u32::from(cell.height),
            device_attributes: Some(DeviceAttributes {
                conformance_level: DeviceAttributes::VT220,
                features: vec![DeviceAttributes::FEATURE_ANSI_COLOR],
                device_type: DeviceAttributes::DEVICE_TYPE_VT220,
                firmware_version: 0,
                rom_cartridge: 0,
                unit_id: 0,
            }),
            xtversion: Some("Huterm".to_owned()),
        };
        let mut terminal =
            ghostty::Terminal::new(options, EngineHost::default())?;
        let scrollback = HistoryLimit::from(ScrollbackLimit::DEFAULT);
        apply_scrollback(&mut terminal, scrollback, size.rows)?;
        terminal.disable_apc_protocols()?;
        // A larger Kitty write could never fit the terminal's host-effect
        // budget, so Ghostty answers it with EFBIG before asking the host.
        terminal.set_clipboard_write_limit(Some(TERMINAL_BYTE_LIMIT))?;
        apply_presentation(&mut terminal, &presentation)?;
        Ok(Self {
            terminal,
            render: RenderState::new()?,
            keyboard: ghostty::KeyEncoder::new()?,
            mouse: RefCell::new(MouseProbe::new()?),
            id,
            size,
            cell,
            presentation,
            scrollback,
            generation: 0,
            modes: StdCell::new(None),
            hint: EscapeHint::Ground,
            overrides: ColorOverrides::default(),
            overrides_stale: false,
            pending_clear: false,
            retained: Vec::new(),
            retained_viewport: (0, 0),
            retained_colors: None,
            prompt_marks_stale: false,
            scratch: TerminalRow::default(),
            grapheme: Vec::new(),
            effects: Vec::new(),
            commands: CommandLog::default(),
            retained_sticky: None,
            server_hostname: server_hostname(),
            notifications: NotificationLimiter::default(),
            #[cfg(test)]
            stats: super::SnapshotStats::default(),
        })
    }

    /// Offers a program's notification to one eligible client, if its text
    /// is presentable and the terminal is within its rate limit. A
    /// notification no client takes is dropped, never kept for later.
    fn notify(&mut self, title: &[u8], body: &[u8], now: Instant) {
        let Some(notification) = notifications::normalize(title, body) else {
            return;
        };
        if !self.notifications.admit(now) {
            return;
        }
        if let Some(sink) = &self.terminal.host_mut().sink {
            let _ = sink.admit_notification(notification);
        }
    }

    pub(super) fn set_host_effect_sink(&mut self, sink: HostEffectSink) {
        self.terminal.host_mut().sink = Some(sink);
    }

    pub(super) fn update_presentation(
        &mut self,
        presentation: TerminalPresentation,
    ) -> Result<(), RuntimeError> {
        apply_presentation(&mut self.terminal, &presentation)?;
        self.presentation = presentation;
        self.retained_colors = None;
        self.modes.set(None);
        Ok(())
    }

    #[cfg(test)]
    pub(super) const fn presentation(&self) -> &TerminalPresentation {
        &self.presentation
    }

    #[cfg(test)]
    pub(super) const fn cell_size(&self) -> CellSize {
        self.cell
    }

    /// Rows between the viewport bottom and the live bottom, and history
    /// rows.
    pub(super) fn viewport_state(
        &self,
    ) -> Result<(usize, usize), RuntimeError> {
        Ok((self.viewport_offset()?, self.terminal.scrollback_rows()?))
    }

    /// Rows between the viewport's bottom and live output, from the
    /// scrollbar alone; typing checks it on every keystroke.
    pub(super) fn viewport_offset(&self) -> Result<usize, RuntimeError> {
        let bar = self.terminal.scrollbar()?;
        let bottom =
            bar.total.saturating_sub(bar.offset.saturating_add(bar.len));
        Ok(usize::try_from(bottom).unwrap_or(usize::MAX))
    }

    pub(super) fn process(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.generation = self.generation.saturating_add(1);
        if self.hint.observe(bytes) {
            self.overrides_stale = true;
        }
        self.write_output(bytes)?;
        self.apply_pending_clear()?;
        self.drain_effects()
    }

    /// Writes PTY output, stopping after each OSC so a command marker is
    /// handled with the cursor and screen it was parsed at. Ghostty
    /// dispatches an OSC on the byte its `OSC_END` stop reports, BEL or the
    /// ESC of ST. The other effects stay queued in order for
    /// [`Self::drain_effects`].
    fn write_output(&mut self, mut bytes: &[u8]) -> Result<(), RuntimeError> {
        while !bytes.is_empty() {
            let stop = self.terminal.write_until(bytes, Boundaries::OSC_END)?;
            bytes = &bytes[stop.consumed..];
            if stop.reached == Boundaries::NONE {
                break;
            }
            let start = self.effects.len();
            self.terminal.take_effects(&mut self.effects);
            for effect in &self.effects[start..] {
                if let ghostty::Effect::SemanticPrompt(event) = effect {
                    self.prompt_marks_stale = true;
                    self.commands.handle(&mut self.terminal, event)?;
                }
            }
        }
        Ok(())
    }

    /// Removes records whose rows left history and returns the record list
    /// if it changed since the last call. Call it after each operation.
    pub(super) fn command_update(&mut self) -> Option<Arc<[CommandRecord]>> {
        self.commands.publish(&self.terminal)
    }

    /// Ends a running record whose command can no longer report.
    pub(super) fn abandon_running_command(&mut self) {
        self.commands.abandon_running();
    }

    #[cfg(test)]
    pub(super) fn commands(&self) -> Arc<[CommandRecord]> {
        self.commands.records()
    }

    #[cfg(test)]
    pub(super) fn anchored_text(&self, sequence: u64) -> Option<String> {
        self.commands.anchored_text(&self.terminal, sequence)
    }

    pub(super) fn clear_history(
        &mut self,
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.generation = self.generation.saturating_add(1);
        self.pending_clear = true;
        self.apply_pending_clear()?;
        self.drain_effects()
    }

    /// Injects ED 3 only at ground, so it never joins an unfinished
    /// sequence or codepoint from the PTY.
    fn apply_pending_clear(&mut self) -> Result<(), RuntimeError> {
        if self.pending_clear && self.terminal.is_ground()? {
            self.pending_clear = false;
            if self.hint.observe(CLEAR_HISTORY) {
                self.overrides_stale = true;
            }
            self.terminal.write(CLEAR_HISTORY)?;
        }
        Ok(())
    }

    pub(super) fn reset(&mut self) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.generation = self.generation.saturating_add(1);
        self.pending_clear = false;
        self.terminal.reset()?;
        self.overrides_stale = true;
        let mut effects = self.drain_effects()?;
        // The C reset reports no progress removal, unlike a parsed RIS.
        effects.push(EngineEffect::Progress(None));
        Ok(effects)
    }

    pub(super) fn resize(
        &mut self,
        size: GridSize,
        cell: CellSize,
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.generation = self.generation.saturating_add(1);
        // A derived byte allowance follows the screen's height. During the
        // resize it allows for the taller of both screens, so neither prunes
        // history the row limit would keep. Ghostty enforces bytes only when
        // they are set or a page is allocated, not after a reflow, so the
        // allowance is set again afterward, explicit or not.
        let allowance =
            |screen_rows| scrollback_bytes(self.scrollback, screen_rows);
        let during = allowance(self.size.rows.max(size.rows));
        if during > 0 {
            self.terminal.set_scrollback_bytes(Some(during))?;
        }
        self.terminal.resize(
            size.columns,
            size.rows,
            u32::from(cell.width),
            u32::from(cell.height),
        )?;
        let after = allowance(size.rows);
        if after > 0 {
            self.terminal.set_scrollback_bytes(Some(after))?;
        }
        self.size = size;
        self.cell = cell;
        self.drain_effects()
    }

    pub(super) const fn size(&self) -> GridSize {
        self.size
    }

    pub(super) const fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub(super) const fn last_snapshot_stats(&self) -> super::SnapshotStats {
        self.stats
    }

    /// Applies a history limit, which evicts the oldest rows at once when
    /// history no longer fits. Returns whether it evicted rows that viewers
    /// can see. Eviction is measured, not inferred from the limits: Ghostty
    /// can evict even under a higher byte limit, because pages that grew
    /// for graphemes or styles may already exceed it. Only the primary
    /// screen has history, and pruning it while the alternate screen shows
    /// changes nothing on screen; leaving the alternate screen is output,
    /// which advances the generation anyway.
    pub(super) fn set_scrollback(
        &mut self,
        limit: ScrollbackLimit,
    ) -> Result<bool, RuntimeError> {
        self.set_history(limit.into())
    }

    /// Applies a row limit above [`ScrollbackLimit::MAX`], with the derived
    /// byte allowance, so benchmarks can measure larger histories.
    #[cfg(test)]
    pub(super) fn set_history_rows(
        &mut self,
        rows: u32,
    ) -> Result<bool, RuntimeError> {
        self.set_history(HistoryLimit { rows, bytes: None })
    }

    fn set_history(
        &mut self,
        limit: HistoryLimit,
    ) -> Result<bool, RuntimeError> {
        if limit == self.scrollback {
            return Ok(false);
        }
        let primary = self.terminal.screen()? == Screen::Primary;
        let before = self.terminal.scrollback_rows()?;
        apply_scrollback(&mut self.terminal, limit, self.size.rows)?;
        self.scrollback = limit;
        let evicted = primary && self.terminal.scrollback_rows()? < before;
        if evicted {
            self.generation = self.generation.saturating_add(1);
        }
        Ok(evicted)
    }

    fn drain_effects(&mut self) -> Result<Vec<EngineEffect>, RuntimeError> {
        let mut queued = std::mem::take(&mut self.effects);
        self.terminal.take_effects(&mut queued);
        let mut effects = Vec::with_capacity(queued.len());
        let mut title = false;
        for effect in queued.drain(..) {
            match effect {
                ghostty::Effect::PtyWrite(bytes) => {
                    if let Some(bytes) = supported_reply(bytes) {
                        effects.push(EngineEffect::PtyWrite(bytes));
                    }
                }
                ghostty::Effect::Bell => effects.push(EngineEffect::Bell),
                ghostty::Effect::TitleChanged => title = true,
                ghostty::Effect::PwdChanged(bytes) => {
                    let Ok(reported) = std::str::from_utf8(&bytes) else {
                        continue;
                    };
                    match normalize_directory(
                        reported,
                        self.server_hostname.as_deref(),
                    ) {
                        DirectoryUpdate::Ignore => {}
                        DirectoryUpdate::Clear => {
                            effects.push(EngineEffect::Directory(None));
                        }
                        DirectoryUpdate::Set(directory) => {
                            effects
                                .push(EngineEffect::Directory(Some(directory)));
                        }
                    }
                }
                ghostty::Effect::DesktopNotification(request) => {
                    self.notify(&request.title, &request.body, Instant::now());
                }
                ghostty::Effect::Progress(report) => {
                    effects.extend(progress_effect(report));
                }
                // Semantic prompts were handled at the stop that parsed
                // them; see `write_output`.
                _ => {}
            }
        }
        // Keep the buffer's allocation for the next write.
        self.effects = queued;
        if title {
            let title = String::from_utf8_lossy(self.terminal.title()?);
            effects.push(EngineEffect::Title(title.into_owned()));
        }
        Ok(effects)
    }

    pub(super) fn encode_input(
        &mut self,
        input: &huterm_protocol::TerminalInput,
    ) -> Result<Vec<u8>, RuntimeError> {
        use huterm_protocol::TerminalInput;
        let flags = self.terminal.kitty_keyboard_flags()? & 5;
        if flags & 1 != 0
            && matches!(
                input,
                TerminalInput::Key { .. }
                    | TerminalInput::Character { key: Some(_), .. }
            )
        {
            self.keyboard.sync(&self.terminal)?;
            self.keyboard.set_kitty_flags(flags);
            return Ok(crate::input::encode_kitty_input(
                input,
                &mut self.keyboard,
            )?);
        }
        Ok(crate::input::encode_input(input, self.modes()?, self.size))
    }

    pub(super) fn modes(&self) -> Result<TerminalModes, RuntimeError> {
        if let Some((generation, modes)) = self.modes.get()
            && generation == self.generation
        {
            return Ok(modes);
        }
        let (tracking, encoding) =
            self.mouse.borrow_mut().probe(&self.terminal)?;
        let modes = TerminalModes {
            kitty_keyboard: huterm_protocol::KittyKeyboardFlags::from_bits(
                self.terminal.kitty_keyboard_flags()?,
            ),
            application_cursor: self.terminal.mode(Mode::CURSOR_KEYS)?,
            alternate_screen: self.terminal.screen()? == Screen::Alternate,
            bracketed_paste: self.terminal.mode(Mode::BRACKETED_PASTE)?,
            focus_reporting: self.terminal.mode(Mode::FOCUS_EVENT)?,
            mouse_tracking: match tracking {
                ProbedTracking::Disabled => MouseTracking::Disabled,
                ProbedTracking::Buttons => MouseTracking::Buttons,
                ProbedTracking::ButtonMotion => MouseTracking::ButtonMotion,
                ProbedTracking::AllMotion => MouseTracking::AllMotion,
            },
            mouse_encoding: match encoding {
                ProbedFormat::Legacy => MouseEncoding::Legacy,
                ProbedFormat::Utf8 => MouseEncoding::Utf8,
                ProbedFormat::Sgr => MouseEncoding::Sgr,
            },
        };
        self.modes.set(Some((self.generation, modes)));
        Ok(modes)
    }

    pub(super) fn scroll(
        &mut self,
        scroll: ScrollCommand,
    ) -> Result<(), RuntimeError> {
        let scroll =
            match scroll {
                ScrollCommand::Relative(rows) => {
                    let delta = rows.saturating_neg();
                    Scroll::Delta(isize::try_from(delta).unwrap_or(
                        if delta < 0 { isize::MIN } else { isize::MAX },
                    ))
                }
                ScrollCommand::Absolute(offset) => Scroll::Row(
                    self.terminal.scrollback_rows()?.saturating_sub(offset),
                ),
                ScrollCommand::Live => Scroll::Bottom,
                ScrollCommand::Prompt(prompts) => {
                    Scroll::Prompt(isize::try_from(prompts).unwrap_or(
                        if prompts < 0 { isize::MIN } else { isize::MAX },
                    ))
                }
            };
        Ok(self.terminal.scroll(scroll)?)
    }

    fn colors(&self) -> Result<Colors, RuntimeError> {
        Ok(Colors {
            foreground: self.terminal.foreground()?,
            background: self.terminal.background()?,
            palette: self.terminal.palette()?,
        })
    }

    pub(super) fn snapshot(
        &mut self,
    ) -> Result<TerminalSnapshot, RuntimeError> {
        if self.overrides_stale {
            self.overrides = self.terminal.probe_color_overrides()?;
            self.overrides_stale = false;
        }
        let modes = self.modes()?;
        let viewport = self.viewport_state()?;
        let colors = self.colors()?;
        self.render.update(&mut self.terminal)?;

        let rows = usize::from(self.size.rows);
        let columns = usize::from(self.size.columns);
        // A failed snapshot leaves nothing retained, forcing a full rebuild.
        let previous = std::mem::take(&mut self.retained);
        let full = self.retained_colors.as_ref() != Some(&colors)
            || self.render.dirty()? == Dirty::Full
            || previous.len() != rows
            || previous
                .first()
                .is_some_and(|row| row.cells.len() != columns);
        let cursor = self.render.cursor()?;
        let cursor_color = if self.overrides.cursor {
            self.render.colors()?.cursor.map(rgb)
        } else {
            None
        };

        let built = self.build_rows(full, &previous, viewport, &colors)?;
        let (commands, sticky_prompt) = if modes.alternate_screen {
            (Arc::from([]), None)
        } else {
            self.command_layout(viewport.0, &colors)
        };
        self.retained_sticky.clone_from(&sticky_prompt);
        // Consume damage only once the retained rows are coherent.
        self.render.clean()?;
        self.prompt_marks_stale = false;
        self.retained.clone_from(&built);
        self.retained_viewport = viewport;
        let default_background = colors
            .background
            .filter(|_| self.overrides.background)
            .map(rgb);
        self.retained_colors = Some(colors);
        Ok(TerminalSnapshot {
            terminal_id: self.id,
            generation: self.generation,
            // The runtime stamps both revisions when it publishes.
            revision: 0,
            geometry_revision: 0,
            size: self.size,
            rows: built,
            cursor: snapshot_cursor(cursor),
            modes,
            viewport: Viewport {
                bottom_offset: viewport.0,
            },
            history_size: viewport.1,
            cursor_color,
            default_background,
            commands,
            sticky_prompt,
        })
    }

    /// The command blocks in the viewport, `offset` rows above the live
    /// bottom, and the sticky prompt. A grid read that fails leaves both
    /// out rather than failing the snapshot.
    fn command_layout(
        &mut self,
        offset: usize,
        colors: &Colors,
    ) -> (Arc<[CommandBlock]>, Option<StickyPrompt>) {
        let Ok(view) = self.layout_view(offset) else {
            return (Arc::from([]), None);
        };
        let Ok(layout) = self.commands.layout(&self.terminal, view) else {
            return (Arc::from([]), None);
        };
        let sticky = layout
            .sticky
            .and_then(|rows| self.sticky_prompt(view, rows, colors).ok());
        (Arc::from(layout.blocks), sticky)
    }

    fn layout_view(
        &self,
        offset: usize,
    ) -> Result<records::View, RuntimeError> {
        let total = self.terminal.total_rows()?;
        let rows = usize::from(self.size.rows);
        let (_, cursor) = self.terminal.cursor_position()?;
        Ok(records::View {
            total,
            rows,
            columns: self.size.columns,
            top: total.saturating_sub(rows).saturating_sub(offset),
            cursor: total.saturating_sub(rows) + usize::from(cursor),
        })
    }

    /// Extracts a sticky prompt's rows, reusing the last snapshot's `Arc`
    /// for each row that reads the same.
    fn sticky_prompt(
        &mut self,
        view: records::View,
        rows: records::StickyRows,
        colors: &Colors,
    ) -> Result<StickyPrompt, RuntimeError> {
        let resolver = Resolver {
            overrides: &self.overrides,
            colors,
        };
        let previous = self
            .retained_sticky
            .as_ref()
            .map_or(&[][..], |sticky| sticky.rows.as_slice());
        let mut extracted = Vec::with_capacity(rows.count);
        for row in rows.first..rows.first + rows.count {
            let line = view.row(&self.terminal, row)?;
            let mut out = TerminalRow::default();
            extract_ref_row(&line, &mut out, &resolver, view.columns)?;
            extracted.push(
                previous
                    .iter()
                    .find(|retained| ***retained == out)
                    .map_or_else(|| Arc::new(out), Arc::clone),
            );
        }
        Ok(StickyPrompt {
            sequence: rows.sequence,
            rows: extracted,
            input: rows.input - rows.first,
        })
    }

    /// Builds viewport rows from the updated render state. Clean rows keep
    /// their retained `Arc` unless their live flags changed, which they
    /// read after an OSC 133 mark and below a row this build extracted or
    /// refreshed; extracted rows reuse any identical retained row.
    fn build_rows(
        &mut self,
        full: bool,
        previous: &[Arc<TerminalRow>],
        viewport: (usize, usize),
        colors: &Colors,
    ) -> Result<Vec<Arc<TerminalRow>>, RuntimeError> {
        let columns = usize::from(self.size.columns);
        let resolver = Resolver {
            overrides: &self.overrides,
            colors,
        };
        let mut matcher = RowMatcher::new(
            previous,
            viewport_shift(self.retained_viewport, viewport),
        );
        let mut built = Vec::with_capacity(usize::from(self.size.rows));
        let (mut extracted, mut allocated, mut reused) = (0, 0, 0);
        // Whether the row above was extracted or refreshed. Ghostty's
        // `cursorResetWrap` clears the continuation of the row below the one
        // it rewrites without dirtying it.
        let mut above_changed = false;
        let mut rows = self.render.rows()?;
        while let Some(mut row) = rows.next() {
            let index = built.len();
            if !full
                && !row.dirty()?
                && let Some(retained) = previous.get(index)
            {
                if (self.prompt_marks_stale || above_changed)
                    && let Some(refreshed) =
                        refresh_flags(&self.terminal, retained, index)?
                {
                    built.push(Arc::new(refreshed));
                    allocated += 1;
                    above_changed = true;
                } else {
                    built.push(Arc::clone(retained));
                    reused += 1;
                    above_changed = false;
                }
                continue;
            }
            above_changed = true;
            extract_row(
                &mut row,
                &mut self.scratch,
                &mut self.grapheme,
                &resolver,
                columns,
            )?;
            extracted += 1;
            if let Some(found) = matcher.find(index, &self.scratch) {
                built.push(Arc::clone(found));
                reused += 1;
            } else {
                built.push(Arc::new(std::mem::take(&mut self.scratch)));
                allocated += 1;
            }
        }
        #[cfg(test)]
        {
            self.stats = super::SnapshotStats {
                extracted,
                allocated,
                reused,
            };
        }
        #[cfg(not(test))]
        let _ = (extracted, allocated, reused);
        Ok(built)
    }

    /// One bounded incremental compression step over cold history.
    pub(super) fn compress_step(
        &mut self,
    ) -> Result<huterm_ghostty::Compression, RuntimeError> {
        Ok(self.terminal.compress_step()?)
    }

    /// The token that changes when history may hold new compression work.
    pub(super) fn compression_activity(&self) -> Result<u64, RuntimeError> {
        Ok(self.terminal.compression_activity()?)
    }

    #[cfg(test)]
    pub(super) fn memory_usage(
        &self,
    ) -> Result<huterm_ghostty::MemoryUsage, RuntimeError> {
        Ok(self.terminal.memory_usage()?)
    }

    pub(super) fn link_reader(&self) -> Result<GhosttyLinks<'_>, RuntimeError> {
        let total = self.terminal.total_rows()?;
        let (bottom, _) = self.viewport_state()?;
        let top = total
            .checked_sub(usize::from(self.size.rows) + bottom)
            .ok_or(RuntimeError::Invariant("viewport extends above history"))?;
        Ok(GhosttyLinks {
            terminal: &self.terminal,
            total,
            top,
            last_column: self.size.columns.saturating_sub(1),
            rows: StdCell::new([None; 2]),
            codepoints: [0; LINK_GRAPHEME_CODEPOINTS],
        })
    }

    pub(super) fn extract_text(
        &self,
        generation: u64,
        range: BufferRange,
    ) -> Result<Option<String>, RuntimeError> {
        if generation != self.generation
            || BufferRange::ordered(range.start, range.end) != range
        {
            return Ok(None);
        }
        let total = self.terminal.total_rows()?;
        let point = |point: BufferPoint| {
            if point.column >= self.size.columns {
                return None;
            }
            let row = total
                .checked_sub(1)?
                .checked_sub(point.rows_from_live_bottom)?;
            Some(Point::screen(point.column, u32::try_from(row).ok()?))
        };
        let (Some(start), Some(end)) = (point(range.start), point(range.end))
        else {
            return Ok(None);
        };
        let text = self.terminal.format_plain(start, end)?;
        Ok(Some(String::from_utf8_lossy(text.as_bytes()).into_owned()))
    }

    /// Starts a trimmed line at its logical start when only blank cells
    /// precede its text, so a copied line keeps its indentation, even
    /// indentation that fills whole soft-wrapped rows. Ghostty trims it;
    /// other terminals keep it. Text before the line, such as the prompt
    /// shell integration marks off from its input, still bounds it. The walk
    /// back over blank wrapped rows stops after [`INDENT_ROWS`] rows.
    fn keep_indentation(
        &self,
        line: BufferRange,
        last_row: usize,
    ) -> BufferRange {
        let start = line.start;
        let Some(row) = last_row.checked_sub(start.rows_from_live_bottom)
        else {
            return line;
        };
        let Ok(mut reader) = self.link_reader() else {
            return line;
        };
        let mut cell = String::new();
        let mut blank = |reader: &mut GhosttyLinks<'_>, row, columns| {
            (0..columns).all(|column| {
                cell.clear();
                reader.push_cell(row, column, &mut cell).is_ok()
                    && cell.chars().all(char::is_whitespace)
            })
        };
        if !blank(&mut reader, row, start.column) {
            return line;
        }
        let mut first = row;
        for _ in 0..INDENT_ROWS {
            let Some(previous) = first.checked_sub(1) else {
                break;
            };
            if !reader.wrapped(previous).unwrap_or(false) {
                break;
            }
            if !blank(&mut reader, previous, self.size.columns) {
                return line;
            }
            first = previous;
        }
        BufferRange {
            start: BufferPoint {
                rows_from_live_bottom: last_row - first,
                column: 0,
            },
            ..line
        }
    }

    /// The range a multi-click selection from `anchor` to `head` covers,
    /// following Ghostty's own gestures. A word selection joins the word
    /// nearest each end in the direction of the other, and a selection that
    /// starts on a link takes the whole link instead of the anchor's word,
    /// so a URL never splits at its colon. A line selection joins the lines
    /// under both ends, keeping their indentation; a blank line
    /// counts as its whole row once the drag leaves it. `None` for a stale
    /// generation, a point outside the buffer, or nothing to select, such
    /// as a press on unwritten cells.
    pub(super) fn select_units(
        &self,
        generation: u64,
        anchor: BufferPoint,
        head: BufferPoint,
        unit: SelectionUnit,
    ) -> Result<Option<BufferRange>, RuntimeError> {
        if generation != self.generation {
            return Ok(None);
        }
        let total = self.terminal.total_rows()?;
        let last_row = total
            .checked_sub(1)
            .ok_or(RuntimeError::Invariant("terminal has no rows"))?;
        let columns = self.size.columns;
        let screen_row = |point: BufferPoint| {
            if point.column >= columns {
                return None;
            }
            last_row.checked_sub(point.rows_from_live_bottom)
        };
        let (Some(anchor_row), Some(head_row)) =
            (screen_row(anchor), screen_row(head))
        else {
            return Ok(None);
        };
        let screen = |row: usize, column: u16| {
            u32::try_from(row)
                .map(|row| Point::screen(column, row))
                .map_err(|_| RuntimeError::Invariant("row exceeds u32"))
        };
        let buffer_point = |point: Point| BufferPoint {
            rows_from_live_bottom: last_row
                .saturating_sub(usize::try_from(point.y).unwrap_or(usize::MAX)),
            column: point.x,
        };
        let range = |ends: Option<(Point, Point)>| {
            ends.map(|(start, end)| {
                BufferRange::ordered(buffer_point(start), buffer_point(end))
            })
        };
        let anchor_point = screen(anchor_row, anchor.column)?;
        let head_point = screen(head_row, head.column)?;
        let (anchor_side, head_side) = match unit {
            SelectionUnit::Word => {
                let link = self.link_reader().ok().and_then(|mut reader| {
                    super::links::bounds(
                        &mut reader,
                        columns,
                        anchor_row,
                        anchor.column,
                    )
                });
                let anchor_side = match link {
                    Some(((start_row, start), (end_row, end))) => {
                        Some(BufferRange::ordered(
                            buffer_point(screen(start_row, start)?),
                            buffer_point(screen(end_row, end)?),
                        ))
                    }
                    None => range(
                        self.terminal
                            .select_word_between(anchor_point, head_point)?,
                    ),
                };
                let head_side = range(
                    self.terminal
                        .select_word_between(head_point, anchor_point)?,
                );
                (anchor_side, head_side)
            }
            SelectionUnit::Line => {
                let pressed = anchor == head;
                let line = |row: usize, point: Point| {
                    let trimmed = range(self.terminal.select_line(point)?)
                        .map(|line| self.keep_indentation(line, last_row));
                    // Once the drag leaves the pressed cell, a blank line
                    // still counts. Its physical row stands in for the
                    // logical line; blank soft-wrapped lines are rare.
                    Ok::<_, RuntimeError>(trimmed.or_else(|| {
                        (!pressed).then(|| {
                            let start = BufferPoint {
                                rows_from_live_bottom: last_row - row,
                                column: 0,
                            };
                            BufferRange::ordered(
                                start,
                                BufferPoint {
                                    column: columns.saturating_sub(1),
                                    ..start
                                },
                            )
                        })
                    }))
                };
                (line(anchor_row, anchor_point)?, line(head_row, head_point)?)
            }
        };
        Ok(anchor_side
            .zip(head_side)
            .map(|(anchor, head)| anchor.cover(head)))
    }
}

/// The most blank soft-wrapped rows a line selection walks back over to
/// keep its indentation.
const INDENT_ROWS: usize = 64;

/// The row and byte limits the engine applies. A validated
/// [`ScrollbackLimit`] supplies them, except in benchmarks, which measure
/// histories above its maximum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HistoryLimit {
    rows: u32,
    bytes: Option<u64>,
}

impl From<ScrollbackLimit> for HistoryLimit {
    fn from(limit: ScrollbackLimit) -> Self {
        Self {
            rows: limit.rows(),
            bytes: limit.bytes(),
        }
    }
}

/// Maps a history limit onto Ghostty's line and byte limits, which apply
/// together: whichever is reached first prunes. Ghostty keeps at least one
/// page of rows under any line limit, so only a zero byte limit keeps no
/// history.
fn apply_scrollback(
    terminal: &mut ghostty::Terminal<EngineHost>,
    limit: HistoryLimit,
    screen_rows: u16,
) -> Result<(), RuntimeError> {
    let bytes = scrollback_bytes(limit, screen_rows);
    if bytes == 0 {
        terminal.set_scrollback_bytes(Some(0))?;
        terminal.set_scrollback_lines(None)?;
    } else {
        terminal.set_scrollback_lines(Some(
            usize::try_from(limit.rows).unwrap_or(usize::MAX),
        ))?;
        terminal.set_scrollback_bytes(Some(bytes))?;
    }
    Ok(())
}

/// The byte allowance for `limit` on a screen of `screen_rows`: zero when
/// either limit keeps no history, the explicit byte limit when one is set,
/// and otherwise an allowance derived from the rows.
fn scrollback_bytes(limit: HistoryLimit, screen_rows: u16) -> usize {
    let rows = usize::try_from(limit.rows).unwrap_or(usize::MAX);
    if rows == 0 {
        return 0;
    }
    limit.bytes.map_or_else(
        || {
            rows.saturating_add(usize::from(screen_rows))
                .saturating_mul(SCROLLBACK_BYTES_PER_ROW)
                .saturating_add(SCROLLBACK_BYTES_HEADROOM)
        },
        |bytes| usize::try_from(bytes).unwrap_or(usize::MAX),
    )
}

fn snapshot_cursor(cursor: ghostty::Cursor) -> Option<Cursor> {
    let (column, row) = cursor.position?;
    let shape = if cursor.visible {
        match cursor.style {
            CursorStyle::Bar => CursorShape::Beam,
            CursorStyle::Underline => CursorShape::Underline,
            _ => CursorShape::Block,
        }
    } else {
        CursorShape::Hidden
    };
    Some(Cursor { row, column, shape })
}

fn apply_presentation(
    terminal: &mut ghostty::Terminal<EngineHost>,
    presentation: &TerminalPresentation,
) -> Result<(), RuntimeError> {
    terminal
        .set_default_foreground(Some(native_rgb(presentation.foreground)))?;
    terminal
        .set_default_background(Some(native_rgb(presentation.background)))?;
    terminal.set_default_cursor(Some(native_rgb(presentation.cursor)))?;
    terminal.set_default_palette(&presentation.palette.map(native_rgb))?;
    Ok(())
}

#[cfg(unix)]
fn server_hostname() -> Option<String> {
    nix::unistd::gethostname().ok()?.into_string().ok()
}

#[cfg(not(unix))]
fn server_hostname() -> Option<String> {
    None
}

/// Maps native cell colors to snapshot colors. Default and palette colors
/// stay symbolic unless an OSC sequence overrode them.
struct Resolver<'a> {
    overrides: &'a ColorOverrides,
    colors: &'a Colors,
}

impl Resolver<'_> {
    fn color(
        &self,
        color: StyleColor,
        default: CellColor,
        default_override: Option<ghostty::Rgb>,
    ) -> CellColor {
        match color {
            StyleColor::None => default_override
                .map_or(default, |color| CellColor::Rgb(rgb(color))),
            StyleColor::Rgb(color) => CellColor::Rgb(rgb(color)),
            StyleColor::Palette(index) => {
                if self.overrides.palette[usize::from(index)] {
                    CellColor::Rgb(rgb(self.colors.palette[usize::from(index)]))
                } else {
                    CellColor::Indexed(index)
                }
            }
        }
    }

    fn foreground(&self, color: StyleColor) -> CellColor {
        let default =
            self.colors.foreground.filter(|_| self.overrides.foreground);
        self.color(color, CellColor::DefaultForeground, default)
    }

    fn background(&self, color: StyleColor) -> CellColor {
        let default =
            self.colors.background.filter(|_| self.overrides.background);
        self.color(color, CellColor::DefaultBackground, default)
    }
}

/// A copy of a clean row with the live grid's wrap and prompt flags, when
/// they differ from the retained row's. The render state copies a row's
/// flags only when it rebuilds a dirty row.
fn refresh_flags(
    terminal: &ghostty::Terminal<EngineHost>,
    retained: &TerminalRow,
    index: usize,
) -> Result<Option<TerminalRow>, RuntimeError> {
    let y = u32::try_from(index)
        .map_err(|_| RuntimeError::Invariant("viewport row out of range"))?;
    let live = terminal.grid_ref(Point::viewport(0, y))?.row()?;
    let wrapped = live.wrapped()?;
    let wrap_continuation = live.wrap_continuation()?;
    let prompt = row_prompt(live.semantic_prompt()?);
    Ok((wrapped != retained.wrapped
        || wrap_continuation != retained.wrap_continuation
        || prompt != retained.prompt)
        .then(|| TerminalRow {
            cells: retained.cells.clone(),
            wrapped,
            wrap_continuation,
            prompt,
        }))
}

fn row_prompt(mark: RowSemanticPrompt) -> RowPrompt {
    match mark {
        RowSemanticPrompt::Prompt => RowPrompt::Primary,
        RowSemanticPrompt::Continuation => RowPrompt::Continuation,
        _ => RowPrompt::None,
    }
}

/// A cell's grapheme cluster as cell text. A cluster past
/// [`MAX_CELL_TEXT_BYTES`] is cut here, the one place rows are extracted, so
/// every client sees the same bounded text. The pinned Ghostty stores at
/// most 64 codepoints after a cluster's first, so this guards a pin that
/// raises that bound.
///
/// [`MAX_CELL_TEXT_BYTES`]: huterm_protocol::MAX_CELL_TEXT_BYTES
pub(super) fn grapheme_text(grapheme: &[u8]) -> CellText {
    if grapheme.is_empty() {
        CellText::EMPTY
    } else {
        CellText::truncated(&String::from_utf8_lossy(grapheme))
    }
}

/// Reads one row's cells and flags into `out`, reusing its allocation.
fn extract_row(
    row: &mut ghostty::RenderRow<'_>,
    out: &mut TerminalRow,
    grapheme: &mut Vec<u8>,
    resolver: &Resolver<'_>,
    columns: usize,
) -> Result<(), RuntimeError> {
    let hints = row.row()?;
    out.wrapped = hints.wrapped()?;
    out.wrap_continuation = hints.wrap_continuation()?;
    out.prompt = row_prompt(hints.semantic_prompt()?);
    let out = &mut out.cells;
    out.clear();
    out.try_reserve_exact(columns).map_err(|error| {
        RuntimeError::Engine(format!("snapshot row allocation failed: {error}"))
    })?;
    // Row flags may report false positives but never false negatives, so
    // unstyled and grapheme-free rows skip per-cell queries.
    let graphemes = hints.has_graphemes()?;
    let styled = hints.has_styles()?;
    let mut cells = row.cells()?;
    while cells.next() {
        let cell = cells.cell()?;
        let codepoint = cell.codepoint()?;
        let content = if graphemes || codepoint == 0 {
            cell.content()?
        } else {
            CellContent::Codepoint
        };
        let width = cell.width()?;
        // The style getter returns the default for unstyled cells, so one
        // call replaces a `has_styling` check plus a style read.
        let style = if styled {
            cells.style()?
        } else {
            ghostty::Style::default()
        };
        let (text, background) = match content {
            CellContent::Codepoint if codepoint != 0 => (
                char::from_u32(codepoint).map_or(
                    CellText::new("\u{fffd}"),
                    |character| {
                        CellText::new(character.encode_utf8(&mut [0; 4]))
                    },
                ),
                style.background,
            ),
            // Ghostty's formatter treats a cell as text when its codepoint
            // is not zero, so a written space is text and these are not.
            CellContent::Grapheme => {
                cells.graphemes_utf8(grapheme)?;
                let text = grapheme_text(grapheme);
                (text, style.background)
            }
            CellContent::BackgroundPalette => (
                CellText::EMPTY,
                StyleColor::Palette(cell.background_palette()?),
            ),
            CellContent::BackgroundRgb => {
                (CellText::EMPTY, StyleColor::Rgb(cell.background_rgb()?))
            }
            _ => (CellText::EMPTY, style.background),
        };
        let mut foreground = resolver.foreground(style.foreground);
        let mut background = resolver.background(background);
        if style.inverse {
            std::mem::swap(&mut foreground, &mut background);
        }
        out.push(Cell {
            text,
            foreground,
            background,
            style: CellStyle {
                bold: style.bold,
                dim: style.faint,
                italic: style.italic,
                underline: style.underline,
                strikeout: style.strikethrough,
                hidden: style.invisible,
                wide: width == CellWidth::Wide,
                wide_spacer: width.is_spacer(),
            },
        });
    }
    Ok(())
}

/// Reads one row's cells and flags through a grid reference, as
/// [`extract_row`] reads them from the render state, for rows outside the
/// viewport. A row narrower than `columns`, which incomplete reflow can
/// leave, is padded with empty cells.
fn extract_ref_row(
    row: &RowRef<'_>,
    out: &mut TerminalRow,
    resolver: &Resolver<'_>,
    columns: u16,
) -> Result<(), RuntimeError> {
    let hints = row.row()?;
    out.wrapped = hints.wrapped()?;
    out.wrap_continuation = hints.wrap_continuation()?;
    out.prompt = row_prompt(hints.semantic_prompt()?);
    out.cells.clear();
    let mut codepoints = [0_u32; LINK_GRAPHEME_CODEPOINTS];
    let mut grapheme = String::new();
    for column in 0..columns {
        let Ok(reference) = row.cell(column) else {
            out.cells.push(Cell {
                text: CellText::EMPTY,
                foreground: CellColor::DefaultForeground,
                background: CellColor::DefaultBackground,
                style: CellStyle::default(),
            });
            continue;
        };
        let cell = reference.cell()?;
        let style = if cell.has_styling()? {
            reference.style()?
        } else {
            ghostty::Style::default()
        };
        let (text, background) = match cell.content()? {
            CellContent::BackgroundPalette => (
                CellText::EMPTY,
                StyleColor::Palette(cell.background_palette()?),
            ),
            CellContent::BackgroundRgb => {
                (CellText::EMPTY, StyleColor::Rgb(cell.background_rgb()?))
            }
            _ => {
                let text = match reference.graphemes(&mut codepoints)? {
                    Fill::Written(len) if len > 0 => {
                        grapheme.clear();
                        grapheme.extend(codepoints[..len].iter().map(
                            |&codepoint| {
                                char::from_u32(codepoint).unwrap_or('\u{fffd}')
                            },
                        ));
                        grapheme_text(grapheme.as_bytes())
                    }
                    _ => CellText::EMPTY,
                };
                (text, style.background)
            }
        };
        let width = cell.width()?;
        let mut foreground = resolver.foreground(style.foreground);
        let mut background = resolver.background(background);
        if style.inverse {
            std::mem::swap(&mut foreground, &mut background);
        }
        out.cells.push(Cell {
            text,
            foreground,
            background,
            style: CellStyle {
                bold: style.bold,
                dim: style.faint,
                italic: style.italic,
                underline: style.underline,
                strikeout: style.strikethrough,
                hidden: style.invisible,
                wide: width == CellWidth::Wide,
                wide_spacer: width.is_spacer(),
            },
        });
    }
    Ok(())
}

/// Reads the grid for link resolution. Rows at or below the viewport top
/// use viewport points; Ghostty resolves screen points by walking history
/// pages, so only rows above the viewport pay that cost. Each row resolves
/// once, at the last column, and its cells and wrap flag are read through
/// that one reference.
pub(super) struct GhosttyLinks<'a> {
    terminal: &'a ghostty::Terminal<EngineHost>,
    total: usize,
    top: usize,
    last_column: u16,
    /// The lookup's first row and the row it read last. A scan reads
    /// outward from its first row in each direction, so these two hold
    /// every row it returns to.
    rows: StdCell<[Option<(usize, ghostty::RowRef<'a>)>; 2]>,
    /// Grapheme scratch shared by every cell a lookup visits.
    codepoints: [u32; LINK_GRAPHEME_CODEPOINTS],
}

impl<'a> GhosttyLinks<'a> {
    fn row_ref(&self, row: usize) -> Result<ghostty::RowRef<'a>, LinkLookup> {
        let [first, last] = self.rows.get();
        if let Some((_, cached)) = [first, last]
            .into_iter()
            .flatten()
            .find(|cached| cached.0 == row)
        {
            return Ok(cached);
        }
        let point = if row >= self.top {
            Point::viewport(
                self.last_column,
                u32::try_from(row - self.top)
                    .map_err(|_| LinkLookup::Unavailable)?,
            )
        } else {
            Point::screen(
                self.last_column,
                u32::try_from(row).map_err(|_| LinkLookup::Unavailable)?,
            )
        };
        let resolved = self
            .terminal
            .row_ref(point)
            .map_err(|_| LinkLookup::Unavailable)?;
        self.rows
            .set([first.or(Some((row, resolved))), Some((row, resolved))]);
        Ok(resolved)
    }

    fn grid_ref(
        &self,
        row: usize,
        column: u16,
    ) -> Result<ghostty::GridRef<'a>, LinkLookup> {
        self.row_ref(row)?
            .cell(column)
            .map_err(|_| LinkLookup::Unavailable)
    }
}

fn unavailable(_: ghostty::Error) -> LinkLookup {
    LinkLookup::Unavailable
}

impl LinkBuffer for GhosttyLinks<'_> {
    fn total_rows(&self) -> Result<usize, LinkLookup> {
        Ok(self.total)
    }

    fn wrapped(&self, row: usize) -> Result<bool, LinkLookup> {
        self.row_ref(row)?
            .row()
            .and_then(ghostty::Row::wrapped)
            .map_err(unavailable)
    }

    fn push_cell(
        &mut self,
        row: usize,
        column: u16,
        text: &mut String,
    ) -> Result<(), LinkLookup> {
        let cell = self.grid_ref(row, column)?;
        let codepoints = &mut self.codepoints;
        match cell.graphemes(codepoints).map_err(unavailable)? {
            Fill::Written(0) => {
                let spacer = cell
                    .cell()
                    .and_then(ghostty::Cell::width)
                    .map_err(unavailable)?
                    .is_spacer();
                if !spacer {
                    text.push(' ');
                }
            }
            Fill::Written(len) => {
                let codepoints = &codepoints[..len.min(codepoints.len())];
                text.extend(codepoints.iter().map(|&codepoint| {
                    char::from_u32(codepoint).unwrap_or('\u{fffd}')
                }));
            }
            Fill::TooSmall(_) => return Err(LinkLookup::ScanLimit),
        }
        Ok(())
    }

    fn hyperlink(
        &self,
        row: usize,
        column: u16,
    ) -> Result<Option<String>, LinkLookup> {
        let cell = self.grid_ref(row, column)?;
        if !cell
            .cell()
            .and_then(ghostty::Cell::has_hyperlink)
            .map_err(unavailable)?
        {
            return Ok(None);
        }
        let len = match cell.hyperlink_uri(&mut []).map_err(unavailable)? {
            Fill::Written(_) => return Ok(None),
            Fill::TooSmall(len) if len > MAX_LINK_BYTES => {
                return Err(LinkLookup::ScanLimit);
            }
            Fill::TooSmall(len) => len,
        };
        let mut uri = vec![0; len];
        match cell.hyperlink_uri(&mut uri).map_err(unavailable)? {
            Fill::Written(written) => {
                uri.truncate(written);
                String::from_utf8(uri)
                    .map(Some)
                    .map_err(|_| LinkLookup::Unavailable)
            }
            Fill::TooSmall(_) => Err(LinkLookup::ScanLimit),
        }
    }

    fn same_hyperlink(
        &self,
        row: usize,
        column: u16,
        destination: &str,
        scratch: &mut [u8],
    ) -> Result<bool, LinkLookup> {
        let cell = self.grid_ref(row, column)?;
        if !cell
            .cell()
            .and_then(ghostty::Cell::has_hyperlink)
            .map_err(unavailable)?
        {
            return Ok(false);
        }
        Ok(match cell.hyperlink_uri(scratch).map_err(unavailable)? {
            Fill::Written(len) => {
                scratch.get(..len) == Some(destination.as_bytes())
            }
            Fill::TooSmall(_) => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything a color operation can change, read without the hint.
    fn observe(
        engine: &mut TerminalEngine,
    ) -> (
        Colors,
        Colors,
        Option<ghostty::Rgb>,
        Option<ghostty::Rgb>,
        ColorOverrides,
    ) {
        let terminal = &mut engine.terminal;
        let effective = Colors {
            foreground: terminal.foreground().unwrap(),
            background: terminal.background().unwrap(),
            palette: terminal.palette().unwrap(),
        };
        let defaults = Colors {
            foreground: terminal.default_foreground().unwrap(),
            background: terminal.default_background().unwrap(),
            palette: terminal.default_palette().unwrap(),
        };
        let cursor = terminal.cursor_color().unwrap();
        let default_cursor = terminal.default_cursor_color().unwrap();
        let overrides = terminal.probe_color_overrides().unwrap();
        (effective, defaults, cursor, default_cursor, overrides)
    }

    fn engine() -> TerminalEngine {
        TerminalEngine::new(
            TerminalId::new(1),
            GridSize::clamped(8, 3),
            CellSize {
                width: 8,
                height: 16,
            },
            TerminalPresentation::default(),
        )
        .unwrap()
    }

    fn pty_writes(effects: Vec<EngineEffect>) -> Vec<Vec<u8>> {
        effects
            .into_iter()
            .filter_map(|effect| match effect {
                EngineEffect::PtyWrite(bytes) => Some(bytes),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn kitty_clipboard_writes_are_capped_at_the_terminal_budget() {
        assert_eq!(
            engine().terminal.clipboard_write_limit().unwrap(),
            crate::host_effects::TERMINAL_BYTE_LIMIT
        );
        // terminal.h documents Ghostty's default as the protocol's minimum,
        // so the host budget must not fall below it.
        let fresh = ghostty::Terminal::new(
            ghostty::Options {
                columns: 8,
                rows: 3,
                cell_width: 0,
                cell_height: 0,
                device_attributes: None,
                xtversion: None,
            },
            (),
        )
        .unwrap();
        assert!(
            crate::host_effects::TERMINAL_BYTE_LIMIT
                >= fresh.clipboard_write_limit().unwrap(),
            "the terminal budget is below the Kitty protocol's minimum"
        );
    }

    #[test]
    fn plain_text_mime_types_allow_parameters_but_only_utf8_charsets() {
        let wrong = [
            ("text/plain", true),
            ("TEXT/PLAIN", true),
            ("text/plain;charset=utf-8", true),
            ("text/plain; charset=UTF-8", true),
            ("text/plain ;charset = utf8", true),
            ("text/plain;charset=\"utf-8\"", true),
            ("text/plain; format=flowed", true),
            ("text/plain; format=flowed; charset=UTF8", true),
            ("text/plain;", true),
            // Semicolons and escaped quotes inside quoted values do not
            // split parameters.
            ("text/plain; x=\"a;charset=q\"; charset=utf-8", true),
            ("text/plain; x=\"a\\\";charset=q\"; charset=utf-8", true),
            ("text/plain; x=\"a;b\"", true),
            ("text/plain; charset=\"utf\\-8\"", true),
            // Unknown parameters without a value stay ignored.
            ("text/plain; flowed", true),
            ("text/plain; charset=latin1", false),
            // Malformed: unterminated quotes, text after a closing quote,
            // and a charset without a value.
            ("text/plain; x=\"a;charset=utf-8", false),
            ("text/plain; charset=\"utf-8", false),
            ("text/plain; charset=\"utf-8\\\"", false),
            ("text/plain; charset=\"utf-8\"x", false),
            ("text/plain;charset", false),
            ("text/plain; charset utf-8", false),
            ("text/plain; charset=", false),
            ("text/plain; charset=\"\"", false),
            ("text/plain; charset=utf-8; charset=latin1", false),
            ("text/plainx", false),
            ("text/html", false),
            ("text / plain", false),
            ("", false),
        ]
        .into_iter()
        .filter(|(mime, expected)| is_plain_text(mime.as_bytes()) != *expected)
        .collect::<Vec<_>>();
        assert!(wrong.is_empty(), "misclassified: {wrong:?}");
        assert!(!is_plain_text(&[0xff, b'/', b'x']));
    }

    #[test]
    fn in_band_size_reports_carry_the_resized_grid_and_cells() {
        let mut engine = engine();
        engine.process(b"\x1b[?2048h").unwrap();
        let effects = engine
            .resize(
                GridSize::clamped(10, 4),
                CellSize {
                    width: 7,
                    height: 15,
                },
            )
            .unwrap();
        // CSI 48 ; rows ; columns ; height px ; width px t
        assert_eq!(pty_writes(effects), [b"\x1b[48;4;10;60;70t".to_vec()]);
    }

    #[test]
    fn kitty_keyboard_replies_are_masked_and_graphics_stays_disabled() {
        let mut engine = engine();
        let replies = pty_writes(
            engine
                .process(
                    b"\x1b[?u\x1b[>31u\x1b[?u\x1b[>2u\x1b[?u\x1b[<u\x1b[?u\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[5n",
                )
                .unwrap(),
        );
        // The status report proves replies still flow.
        assert_eq!(
            replies,
            [
                b"\x1b[?0u".to_vec(),
                b"\x1b[?5u".to_vec(),
                b"\x1b[?0u".to_vec(),
                b"\x1b[?5u".to_vec(),
                b"\x1b[0n".to_vec()
            ]
        );
        // Graphics replies are already disabled natively; the filter is a
        // second line of defense.
        assert!(supported_reply(b"\x1b_Gi=31;OK\x1b\\".to_vec()).is_none());
    }

    #[test]
    fn escape_hint_flags_every_observed_color_change() {
        let mut seed = 0x9e37_79b9_u32;
        let mut random = move |bound: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            usize::try_from(seed).unwrap() % bound
        };
        let fixed: &[&[u8]] = &[
            b"text ",
            "界╝".as_bytes(),
            b"\x1b",
            b"\x1b[",
            b"\x1b]",
            b"\x1bP",
            b"\x1bPq",
            b"\x1b_G",
            b"\x1b(",
            b"\x1bc",
            // DECSTR resets the palette.
            b"\x1b[!p",
            b"!",
            b"p",
            &[0x90],
            &[0x98],
            &[0x9b],
            &[0x9c],
            &[0x9d],
            &[0xc2, 0x9d],
            b"0",
            b"1",
            b"4",
            b"10",
            b"11",
            b"104",
            b"110",
            b";",
            b"\x07",
            b"\x1b\\",
            b"\x18",
            b"\x1a",
            b"\x01",
        ];
        let mut changes = 0;
        for stream in 0..200 {
            let mut bytes = Vec::new();
            for _ in 0..40 {
                if random(4) == 0 {
                    let value = random(0x100_0000);
                    let color = match random(8) {
                        0 => format!("\x1b]4;{};#{value:06x}\x07", random(256)),
                        1 => format!("\x1b]10;#{value:06x}\x1b\\"),
                        2 => format!("\x1b]11;#{value:06x}\x07"),
                        3 => format!("\x1b]12;#{value:06x}\x07"),
                        4 => format!("\x1b]104;{}\x07", random(256)),
                        5 => "\x1b]110\x1b\\\x1b]111\x07".to_owned(),
                        6 => format!("\x1b]21;foreground=#{value:06x}\x07"),
                        _ => "\x1b]112\x18\x1b]104\x1a".to_owned(),
                    };
                    bytes.extend_from_slice(color.as_bytes());
                } else {
                    bytes.extend_from_slice(fixed[random(fixed.len())]);
                }
            }
            let mut engine = TerminalEngine::new(
                TerminalId::new(1),
                GridSize::clamped(8, 3),
                CellSize {
                    width: 8,
                    height: 16,
                },
                TerminalPresentation::default(),
            )
            .unwrap();
            let mut previous = observe(&mut engine);
            let mut offset = 0;
            while offset < bytes.len() {
                let end = (offset + 1 + random(8)).min(bytes.len());
                let chunk = &bytes[offset..end];
                engine.overrides_stale = false;
                engine.process(chunk).unwrap();
                let flagged = engine.overrides_stale;
                let current = observe(&mut engine);
                if current != previous {
                    changes += 1;
                    assert!(
                        flagged,
                        "stream {stream}: unflagged color change from {:?} in {:?}",
                        String::from_utf8_lossy(chunk),
                        String::from_utf8_lossy(&bytes),
                    );
                }
                previous = current;
                offset = end;
            }
        }
        assert!(changes > 100, "only {changes} color changes observed");
    }
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one exact-byte matrix pins the native encoder under each supported flag set"
    )]
    fn kitty_exact_bytes_follow_effective_flags_and_keep_legacy_text() {
        use TerminalKey::*;
        use huterm_protocol::{
            CharacterKey, Modifiers, TerminalInput, TerminalKey,
        };
        let mut engine = engine();
        for flags in [1, 5, 31] {
            engine
                .process(format!("\x1b[={flags}u").as_bytes())
                .unwrap();
            let alternates = flags & 4 != 0;
            for (key, modifiers, expected) in [
                (Escape, Modifiers::default(), "\x1b[27u"),
                (Tab, Modifiers::default(), "\t"),
                (Enter, Modifiers::default(), "\r"),
                (Backspace, Modifiers::default(), "\x7f"),
                (
                    Tab,
                    Modifiers {
                        shift: true,
                        ..Default::default()
                    },
                    "\x1b[9;2u",
                ),
                (
                    Enter,
                    Modifiers {
                        shift: true,
                        ..Default::default()
                    },
                    "\x1b[13;2u",
                ),
                (
                    Backspace,
                    Modifiers {
                        control: true,
                        ..Default::default()
                    },
                    "\x1b[127;5u",
                ),
                (
                    Left,
                    Modifiers {
                        control: true,
                        ..Default::default()
                    },
                    "\x1b[1;5D",
                ),
                (F1, Modifiers::default(), "\x1b[P"),
                (F2, Modifiers::default(), "\x1b[Q"),
                (F3, Modifiers::default(), "\x1b[13~"),
                (F4, Modifiers::default(), "\x1b[S"),
                (Insert, Modifiers::default(), "\x1b[2~"),
            ] {
                assert_eq!(
                    engine
                        .encode_input(&TerminalInput::Key { key, modifiers })
                        .unwrap(),
                    expected.as_bytes(),
                    "flags={flags} {key:?}"
                );
            }
            for (unshifted, text, base, control, alt, shift, expected) in [
                ('a', "a", 'a', true, false, false, "\x1b[97;5u"),
                (
                    'a',
                    "A",
                    'a',
                    true,
                    false,
                    true,
                    if alternates {
                        "\x1b[97:65;6u"
                    } else {
                        "\x1b[97;6u"
                    },
                ),
                (
                    ';',
                    ":",
                    ';',
                    true,
                    false,
                    true,
                    if alternates {
                        "\x1b[59:58;6u"
                    } else {
                        "\x1b[59;6u"
                    },
                ),
                ('a', "a", 'a', false, true, false, "\x1b[97;3u"),
                ('1', "1", '1', true, false, false, "\x1b[49;5u"),
                (
                    'ф',
                    "ф",
                    'a',
                    true,
                    false,
                    false,
                    if alternates {
                        "\x1b[1092::97;5u"
                    } else {
                        "\x1b[1092;5u"
                    },
                ),
                ('a', "A", 'a', false, false, true, "A"),
                (' ', " ", ' ', true, false, false, "\x1b[32;5u"),
            ] {
                let input = TerminalInput::Character {
                    text: text.into(),
                    meta: alt,
                    key: Some(Box::new(CharacterKey {
                        unshifted,
                        text: text.into(),
                        base_layout: Some(base),
                        modifiers: Modifiers {
                            control,
                            alt,
                            shift,
                        },
                    })),
                };
                assert_eq!(
                    engine.encode_input(&input).unwrap(),
                    expected.as_bytes(),
                    "flags={flags} {input:?}"
                );
            }
            for input in [
                TerminalInput::Text("é😀".into()),
                TerminalInput::Paste("é😀".into()),
            ] {
                assert_eq!(
                    engine.encode_input(&input).unwrap(),
                    "é😀".as_bytes()
                );
            }
            engine.process(b"\x1b[?2004h").unwrap();
            assert_eq!(
                engine
                    .encode_input(&TerminalInput::Paste("a\x1b[201~b".into()))
                    .unwrap(),
                b"\x1b[200~a[201~b\x1b[201~"
            );
            engine.process(b"\x1b[?2004l").unwrap();
        }
        for flags in [0, 2, 8, 16, 26] {
            engine
                .process(format!("\x1b[={flags}u").as_bytes())
                .unwrap();
            assert_eq!(engine.modes().unwrap().kitty_keyboard.bits(), 0);
            assert_eq!(
                engine
                    .encode_input(&TerminalInput::Key {
                        key: Escape,
                        modifiers: Modifiers::default()
                    })
                    .unwrap(),
                b"\x1b"
            );
            let input = TerminalInput::Character {
                text: ":".into(),
                meta: true,
                key: Some(Box::new(CharacterKey {
                    unshifted: ';',
                    text: ":".into(),
                    base_layout: Some(';'),
                    modifiers: Modifiers {
                        control: true,
                        alt: true,
                        shift: true,
                    },
                })),
            };
            assert_eq!(
                engine.encode_input(&input).unwrap(),
                b"\x1b:",
                "a stale Kitty-only routing hint uses the legacy fallback"
            );
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one matrix covers legacy routing across flags, keys, modifiers, and cursor modes"
    )]
    fn flags_without_disambiguation_use_the_full_legacy_matrix() {
        use huterm_protocol::{
            CharacterKey, Modifiers, TerminalInput, TerminalKey::*,
        };
        let mut engine = engine();
        for flags in [0, 4, 6, 12] {
            engine
                .process(format!("\x1b[={flags}u").as_bytes())
                .unwrap();
            assert_eq!(
                engine.modes().unwrap().kitty_keyboard.bits(),
                flags & 5
            );
            for application_cursor in [false, true] {
                engine
                    .process(if application_cursor {
                        b"\x1b[?1h"
                    } else {
                        b"\x1b[?1l"
                    })
                    .unwrap();
                for (key, normal, application) in [
                    (Escape, "\x1b", "\x1b"),
                    (Enter, "\r", "\r"),
                    (Tab, "\t", "\t"),
                    (Backspace, "\x7f", "\x7f"),
                    (Up, "\x1b[A", "\x1bOA"),
                    (Down, "\x1b[B", "\x1bOB"),
                    (Right, "\x1b[C", "\x1bOC"),
                    (Left, "\x1b[D", "\x1bOD"),
                    (Home, "\x1b[H", "\x1bOH"),
                    (End, "\x1b[F", "\x1bOF"),
                    (PageUp, "\x1b[5~", "\x1b[5~"),
                    (PageDown, "\x1b[6~", "\x1b[6~"),
                    (Delete, "\x1b[3~", "\x1b[3~"),
                    (Insert, "\x1b[2~", "\x1b[2~"),
                    (F1, "\x1bOP", "\x1bOP"),
                    (F2, "\x1bOQ", "\x1bOQ"),
                    (F3, "\x1bOR", "\x1bOR"),
                    (F4, "\x1bOS", "\x1bOS"),
                    (F5, "\x1b[15~", "\x1b[15~"),
                    (F6, "\x1b[17~", "\x1b[17~"),
                    (F7, "\x1b[18~", "\x1b[18~"),
                    (F8, "\x1b[19~", "\x1b[19~"),
                    (F9, "\x1b[20~", "\x1b[20~"),
                    (F10, "\x1b[21~", "\x1b[21~"),
                    (F11, "\x1b[23~", "\x1b[23~"),
                    (F12, "\x1b[24~", "\x1b[24~"),
                ] {
                    for modifiers in [
                        Modifiers::default(),
                        Modifiers {
                            shift: true,
                            ..Default::default()
                        },
                        Modifiers {
                            control: true,
                            ..Default::default()
                        },
                        Modifiers {
                            alt: true,
                            ..Default::default()
                        },
                    ] {
                        let plain = if key == Tab && modifiers.shift {
                            "\x1b[Z"
                        } else if application_cursor {
                            application
                        } else {
                            normal
                        };
                        let expected = if modifiers.alt && key != Escape {
                            format!("\x1b{plain}")
                        } else {
                            plain.into()
                        };
                        assert_eq!(
                            engine
                                .encode_input(&TerminalInput::Key {
                                    key,
                                    modifiers
                                })
                                .unwrap(),
                            expected.as_bytes(),
                            "flags={flags} {key:?} {modifiers:?} app={application_cursor}"
                        );
                    }
                }
                for (unshifted, layout, text, control, alt, shift, expected) in [
                    ('a', "a", "\x01", true, false, false, "\x01"),
                    ('a', "A", "\x01", true, false, true, "\x01"),
                    (';', ":", ":", true, false, true, ":"),
                    ('a', "a", "a", false, true, false, "\x1ba"),
                    ('1', "1", "1", true, false, false, "1"),
                    ('ф', "ф", "\x01", true, false, false, "\x01"),
                    ('a', "A", "A", false, false, true, "A"),
                    (' ', " ", "\0", true, false, false, "\0"),
                ] {
                    let input = TerminalInput::Character {
                        text: text.into(),
                        meta: alt,
                        key: Some(Box::new(CharacterKey {
                            unshifted,
                            text: layout.into(),
                            base_layout: Some('a'),
                            modifiers: Modifiers {
                                control,
                                alt,
                                shift,
                            },
                        })),
                    };
                    assert_eq!(
                        engine.encode_input(&input).unwrap(),
                        expected.as_bytes(),
                        "flags={flags} {input:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn kitty_disambiguation_arrows_ignore_application_cursor_mode() {
        use huterm_protocol::{Modifiers, TerminalInput, TerminalKey::*};
        let mut engine = engine();
        engine.process(b"\x1b[>1u\x1b[?1h").unwrap();
        assert!(engine.modes().unwrap().application_cursor);
        // Kitty specifies CSI arrows even with DECCKM set. Ghostty agrees.
        for (key, bytes) in [
            (Up, b"\x1b[A"),
            (Down, b"\x1b[B"),
            (Right, b"\x1b[C"),
            (Left, b"\x1b[D"),
        ] {
            assert_eq!(
                engine
                    .encode_input(&TerminalInput::Key {
                        key,
                        modifiers: Modifiers::default()
                    })
                    .unwrap(),
                bytes
            );
        }
    }

    #[test]
    fn malformed_keyboard_replies_fail_closed() {
        for reply in [
            b"\x1b[?u".as_slice(),
            b"\x1b[?256u",
            b"\x1b[?1;4u",
            b"\x1b[?nopeu",
            b"\x1b[?\xffu",
        ] {
            assert!(supported_reply(reply.to_vec()).is_none(), "{reply:?}");
        }
        assert_eq!(
            supported_reply(b"\x1b[?4u".to_vec()),
            Some(b"\x1b[?4u".to_vec())
        );
        assert_eq!(
            supported_reply(b"\x1b[0n".to_vec()),
            Some(b"\x1b[0n".to_vec())
        );
    }

    #[test]
    fn kitty_alternate_screen_restores_each_screens_stack() {
        let mut engine = engine();
        for (sequence, flags) in [
            ("\x1b[>5u", 5),
            ("\x1b[?1049h", 0),
            ("\x1b[>1u", 1),
            ("\x1b[?1049l", 5),
        ] {
            engine.process(sequence.as_bytes()).unwrap();
            assert_eq!(engine.modes().unwrap().kitty_keyboard.bits(), flags);
            assert_eq!(
                pty_writes(engine.process(b"\x1b[?u").unwrap()),
                [format!("\x1b[?{flags}u").into_bytes()]
            );
        }
    }
}
