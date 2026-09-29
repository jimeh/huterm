//! The probe that reads a terminal's active mouse mode.

use crate::error::Result;
use crate::native::{
    AllocatorRef, NativeMouseEncoder, NativeMouseEvent, ProbeEvent,
};
use crate::terminal::Terminal;
use crate::types::Fill;

/// Mouse events a terminal's active mode reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbedTracking {
    /// None.
    Disabled,
    /// Presses and releases.
    Buttons,
    /// Also motion with a held button.
    ButtonMotion,
    /// Also motion without a button.
    AllMotion,
}

/// The wire format a terminal's active mode uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbedFormat {
    /// X10 bytes, or any format other than SGR and UTF-8.
    Legacy,
    /// UTF-8 coordinates (mode 1005).
    Utf8,
    /// SGR decimal coordinates (mode 1006).
    Sgr,
}

/// Reads the mouse mode a terminal actually encodes with.
///
/// Ghostty's public mode bits can disagree with its active tracking and
/// format, so this encodes synthetic events against a private encoder
/// synced from the terminal. The output never reaches the PTY. The encoder
/// and events stay private: Ghostty converts geometry and positions with
/// unchecked casts, so only the probe's fixed values may reach it.
///
/// ```compile_fail,E0432
/// use huterm_ghostty::MouseEncoder;
/// ```
///
/// ```compile_fail,E0432
/// use huterm_ghostty::MouseEvent;
/// ```
#[derive(Debug)]
pub struct MouseProbe {
    encoder: NativeMouseEncoder,
    event: NativeMouseEvent,
    buffer: [u8; 64],
}

impl MouseProbe {
    /// Creates a probe with its encoder and event allocated once.
    ///
    /// # Errors
    ///
    /// Fails when allocation fails.
    pub fn new() -> Result<Self> {
        Self::new_in(None)
    }

    pub(crate) fn new_in(allocator: AllocatorRef) -> Result<Self> {
        Ok(Self {
            encoder: NativeMouseEncoder::new(allocator)?,
            event: NativeMouseEvent::new(allocator)?,
            buffer: [0; 64],
        })
    }

    #[cfg(test)]
    pub(crate) const fn encoder_mut(&mut self) -> &mut NativeMouseEncoder {
        &mut self.encoder
    }

    /// Encodes `event`; an empty report means the event is not tracked.
    pub(crate) fn report(&mut self, event: ProbeEvent) -> Result<&[u8]> {
        self.event.set(event);
        match self.encoder.encode(&self.event, &mut self.buffer)? {
            Fill::Written(len) => {
                Ok(&self.buffer[..len.min(self.buffer.len())])
            }
            // Any report proves the event is tracked.
            Fill::TooSmall(_) => Ok(&self.buffer[..]),
        }
    }

    /// Copies tracking and format from `terminal` onto the fixed geometry.
    pub(crate) fn sync<H>(&mut self, terminal: &Terminal<H>) -> Result<()> {
        self.encoder.sync(terminal.native_ref()?.as_ref());
        self.encoder.set_probe_geometry();
        self.encoder.set_track_last_cell(false);
        self.encoder.set_any_button_pressed(false);
        Ok(())
    }

    /// Returns the tracked events and wire format.
    ///
    /// # Errors
    ///
    /// Fails when the terminal is poisoned.
    pub fn probe<H>(
        &mut self,
        terminal: &Terminal<H>,
    ) -> Result<(ProbedTracking, ProbedFormat)> {
        self.sync(terminal)?;
        let tracking = if self.report(ProbeEvent::Motion)?.is_empty() {
            self.encoder.set_any_button_pressed(true);
            if !self.report(ProbeEvent::LeftDrag)?.is_empty() {
                ProbedTracking::ButtonMotion
            } else if terminal.mouse_tracking()? {
                ProbedTracking::Buttons
            } else {
                ProbedTracking::Disabled
            }
        } else {
            ProbedTracking::AllMotion
        };
        self.encoder.track_any_event();
        self.encoder.set_any_button_pressed(false);
        let report = self.report(ProbeEvent::LeftPress)?;
        let format = if report.starts_with(b"\x1b[<") {
            ProbedFormat::Sgr
        } else if report.starts_with(b"\x1b[M") && report.len() > 6 {
            ProbedFormat::Utf8
        } else {
            ProbedFormat::Legacy
        };
        Ok((tracking, format))
    }
}
