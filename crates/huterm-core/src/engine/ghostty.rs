//! Huterm's terminal engine on `huterm-ghostty`.
//!
//! Terminal policy lives here: which replies reach the PTY, clipboard
//! admission, directory normalization, and snapshot construction with row
//! reuse. Everything that depends on Ghostty's C API lives in
//! `huterm-ghostty`.

use std::cell::{Cell as StdCell, RefCell};
use std::sync::Arc;

use huterm_ghostty::{
    self as ghostty, CellContent, CellWidth, ClipboardLocation, ClipboardWrite,
    ClipboardWriteResult, ColorOverrides, ColorScheme, CursorStyle,
    DeviceAttributes, Dirty, Fill, Host, Mode, MouseProbe, Point, ProbedFormat,
    ProbedTracking, RenderState, Screen, Scroll, StyleColor,
};
use huterm_protocol::{
    BufferPoint, BufferRange, Cell, CellColor, CellSize, CellStyle, CellText,
    Cursor, CursorShape, GridSize, LinkLookup, MouseEncoding, MouseTracking,
    Rgb, ScrollCommand, TerminalAppearance, TerminalId, TerminalModes,
    TerminalPresentation, TerminalRow, TerminalSnapshot, Viewport,
    appearance_for_background,
};

use super::escape_hint::EscapeHint;
use super::links::{LinkBuffer, MAX_LINK_BYTES};
use super::row_matcher::{RowMatcher, viewport_shift};
use super::{DirectoryUpdate, EngineEffect, normalize_directory};
use crate::host_effects::{
    HostEffectAdmission, HostEffectSink, TERMINAL_BYTE_LIMIT,
};
use crate::terminal::RuntimeError;

/// History budget; Ghostty prunes whole pages, so retention is approximate.
const SCROLLBACK_BYTES: usize = 16 * 1024 * 1024;
/// ED 3: erase scrollback only.
const CLEAR_HISTORY: &[u8] = b"\x1b[3J";
/// Longest grapheme cluster a link scan reads from one cell.
const LINK_GRAPHEME_CODEPOINTS: usize = 256;

impl From<ghostty::Error> for RuntimeError {
    fn from(error: ghostty::Error) -> Self {
        Self::Engine(format!("ghostty: {error}"))
    }
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
            HostEffectAdmission::Full | HostEffectAdmission::Contended => {
                ClipboardWriteResult::Busy
            }
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
/// `utf-8` or `utf8`, optionally quoted. Whitespace around `;` and `=` is
/// ignored. Other representations in the same write are ignored.
fn is_plain_text(mime: &[u8]) -> bool {
    let Ok(mime) = std::str::from_utf8(mime) else {
        return false;
    };
    let mut parts = mime.split(';');
    let essence = parts.next().unwrap_or_default().trim();
    essence.eq_ignore_ascii_case("text/plain")
        && parts.all(|parameter| {
            let Some((name, value)) = parameter.split_once('=') else {
                return true;
            };
            if !name.trim().eq_ignore_ascii_case("charset") {
                return true;
            }
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(value);
            value.eq_ignore_ascii_case("utf-8")
                || value.eq_ignore_ascii_case("utf8")
        })
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

/// Replies Huterm must not send: Kitty graphics responses and Kitty
/// keyboard flags, whose protocols it does not implement.
fn unsupported_reply(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x1b_G")
        || (bytes.starts_with(b"\x1b[?") && bytes.ends_with(b"u"))
}

#[derive(Debug)]
pub(super) struct TerminalEngine {
    terminal: ghostty::Terminal<EngineHost>,
    render: RenderState,
    mouse: RefCell<MouseProbe>,
    id: TerminalId,
    size: GridSize,
    cell: CellSize,
    presentation: TerminalPresentation,
    generation: u64,
    modes: StdCell<Option<(u64, TerminalModes)>>,
    hint: EscapeHint,
    overrides: ColorOverrides,
    overrides_stale: bool,
    pending_clear: bool,
    retained: Vec<Arc<TerminalRow>>,
    retained_viewport: (usize, usize),
    retained_colors: Option<Colors>,
    scratch: Vec<Cell>,
    grapheme: Vec<u8>,
    effects: Vec<ghostty::Effect>,
    server_hostname: Option<String>,
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
        terminal.set_scrollback_bytes(Some(SCROLLBACK_BYTES))?;
        terminal.disable_apc_protocols()?;
        // A larger Kitty write could never fit the terminal's host-effect
        // budget, so Ghostty answers it with EFBIG before asking the host.
        terminal.set_clipboard_write_limit(Some(TERMINAL_BYTE_LIMIT))?;
        apply_presentation(&mut terminal, &presentation)?;
        Ok(Self {
            terminal,
            render: RenderState::new()?,
            mouse: RefCell::new(MouseProbe::new()?),
            id,
            size,
            cell,
            presentation,
            generation: 0,
            modes: StdCell::new(None),
            hint: EscapeHint::Ground,
            overrides: ColorOverrides::default(),
            overrides_stale: false,
            pending_clear: false,
            retained: Vec::new(),
            retained_viewport: (0, 0),
            retained_colors: None,
            scratch: Vec::new(),
            grapheme: Vec::new(),
            effects: Vec::new(),
            server_hostname: server_hostname(),
            #[cfg(test)]
            stats: super::SnapshotStats::default(),
        })
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
        let bar = self.terminal.scrollbar()?;
        let bottom =
            bar.total.saturating_sub(bar.offset.saturating_add(bar.len));
        Ok((
            usize::try_from(bottom).unwrap_or(usize::MAX),
            self.terminal.scrollback_rows()?,
        ))
    }

    pub(super) fn process(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.generation = self.generation.saturating_add(1);
        if self.hint.observe(bytes) {
            self.overrides_stale = true;
        }
        self.terminal.write(bytes)?;
        self.apply_pending_clear()?;
        self.drain_effects()
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
        self.drain_effects()
    }

    pub(super) fn resize(
        &mut self,
        size: GridSize,
        cell: CellSize,
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.generation = self.generation.saturating_add(1);
        self.terminal.resize(
            size.columns,
            size.rows,
            u32::from(cell.width),
            u32::from(cell.height),
        )?;
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

    #[cfg(test)]
    pub(super) fn set_scrollback_limit(
        &mut self,
        bytes: usize,
    ) -> Result<(), RuntimeError> {
        Ok(self.terminal.set_scrollback_bytes(Some(bytes))?)
    }

    fn drain_effects(&mut self) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.terminal.take_effects(&mut self.effects);
        let mut effects = Vec::with_capacity(self.effects.len());
        let mut title = false;
        for effect in self.effects.drain(..) {
            match effect {
                ghostty::Effect::PtyWrite(bytes) => {
                    if !unsupported_reply(&bytes) {
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
                _ => {}
            }
        }
        if title {
            let title = String::from_utf8_lossy(self.terminal.title()?);
            effects.push(EngineEffect::Title(title.into_owned()));
        }
        Ok(effects)
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
        // Consume damage only once the retained rows are coherent.
        self.render.clean()?;
        self.retained.clone_from(&built);
        self.retained_viewport = viewport;
        self.retained_colors = Some(colors);
        Ok(TerminalSnapshot {
            terminal_id: self.id,
            generation: self.generation,
            size: self.size,
            rows: built,
            cursor: snapshot_cursor(cursor),
            modes,
            viewport: Viewport {
                bottom_offset: viewport.0,
            },
            history_size: viewport.1,
            cursor_color,
        })
    }

    /// Builds viewport rows from the updated render state. Clean rows keep
    /// their retained `Arc`; extracted rows reuse any identical retained
    /// row.
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
        let mut rows = self.render.rows()?;
        while let Some(mut row) = rows.next() {
            let index = built.len();
            if !full
                && !row.dirty()?
                && let Some(retained) = previous.get(index)
            {
                built.push(Arc::clone(retained));
                reused += 1;
                continue;
            }
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
                built.push(Arc::new(TerminalRow {
                    cells: std::mem::take(&mut self.scratch),
                }));
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

/// Reads one row's cells into `out`, reusing its allocation.
fn extract_row(
    row: &mut ghostty::RenderRow<'_>,
    out: &mut Vec<Cell>,
    grapheme: &mut Vec<u8>,
    resolver: &Resolver<'_>,
    columns: usize,
) -> Result<(), RuntimeError> {
    out.clear();
    out.try_reserve_exact(columns).map_err(|error| {
        RuntimeError::Engine(format!("snapshot row allocation failed: {error}"))
    })?;
    let hints = row.row()?;
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
            CellContent::Grapheme => {
                cells.graphemes_utf8(grapheme)?;
                let text = if grapheme.is_empty() {
                    CellText::BLANK
                } else {
                    CellText::new(&String::from_utf8_lossy(grapheme))
                };
                (text, style.background)
            }
            CellContent::BackgroundPalette => (
                CellText::BLANK,
                StyleColor::Palette(cell.background_palette()?),
            ),
            CellContent::BackgroundRgb => {
                (CellText::BLANK, StyleColor::Rgb(cell.background_rgb()?))
            }
            _ => (CellText::BLANK, style.background),
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

/// Reads the grid for link resolution. Rows at or below the viewport top
/// use viewport points; Ghostty resolves screen points by walking history
/// from its first page, so only rows above the viewport pay that cost.
pub(super) struct GhosttyLinks<'a> {
    terminal: &'a ghostty::Terminal<EngineHost>,
    total: usize,
    top: usize,
    /// Grapheme scratch shared by every cell a lookup visits.
    codepoints: [u32; LINK_GRAPHEME_CODEPOINTS],
}

impl<'a> GhosttyLinks<'a> {
    fn grid_ref(
        &self,
        row: usize,
        column: u16,
    ) -> Result<ghostty::GridRef<'a>, LinkLookup> {
        let point = if row >= self.top {
            Point::viewport(
                column,
                u32::try_from(row - self.top)
                    .map_err(|_| LinkLookup::Unavailable)?,
            )
        } else {
            Point::screen(
                column,
                u32::try_from(row).map_err(|_| LinkLookup::Unavailable)?,
            )
        };
        self.terminal
            .grid_ref(point)
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
        self.grid_ref(row, 0)?
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
    }

    #[test]
    fn plain_text_mime_types_allow_parameters_but_only_utf8_charsets() {
        for (mime, expected) in [
            ("text/plain", true),
            ("TEXT/PLAIN", true),
            ("text/plain;charset=utf-8", true),
            ("text/plain; charset=UTF-8", true),
            ("text/plain ;charset = utf8", true),
            ("text/plain;charset=\"utf-8\"", true),
            ("text/plain; format=flowed", true),
            ("text/plain; format=flowed; charset=UTF8", true),
            ("text/plain;", true),
            ("text/plain; charset=latin1", false),
            ("text/plain; charset=\"\"", false),
            ("text/plain; charset=utf-8; charset=latin1", false),
            ("text/plainx", false),
            ("text/html", false),
            ("text / plain", false),
            ("", false),
        ] {
            assert_eq!(is_plain_text(mime.as_bytes()), expected, "{mime:?}");
        }
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
    fn kitty_keyboard_and_graphics_replies_never_reach_the_pty() {
        let mut engine = engine();
        let replies = pty_writes(
            engine
                .process(
                    b"\x1b[?u\x1b[>1u\x1b[?u\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[5n",
                )
                .unwrap(),
        );
        // The status report proves replies still flow.
        assert_eq!(replies, [b"\x1b[0n".to_vec()]);
        // Graphics replies are already disabled natively; the filter is a
        // second line of defense.
        assert!(unsupported_reply(b"\x1b_Gi=31;OK\x1b\\"));
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
}
