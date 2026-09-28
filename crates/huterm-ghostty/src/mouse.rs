//! Mouse encoding and the probe that reads a terminal's active mouse mode.

use crate::error::Result;
use crate::ffi::{self, keys::mouse_encoder_option as option};
use crate::native::{AllocatorRef, NativeMouseEncoder, NativeMouseEvent};
use crate::terminal::Terminal;
use crate::types::{
    Fill, MouseAction, MouseButton, MouseGeometry, MouseTrackingMode,
};

/// A reusable mouse event.
#[derive(Debug)]
pub struct MouseEvent(NativeMouseEvent);

impl MouseEvent {
    /// Creates a motion event without a button at the origin.
    ///
    /// # Errors
    ///
    /// Fails when allocation fails.
    pub fn new() -> Result<Self> {
        Self::new_in(None)
    }

    pub(crate) fn new_in(allocator: AllocatorRef) -> Result<Self> {
        let mut event = Self(NativeMouseEvent::new(allocator)?);
        event.set(MouseAction::Motion, None, 0.0, 0.0);
        Ok(event)
    }

    /// Sets every field of the event.
    pub fn set(
        &mut self,
        action: MouseAction,
        button: Option<MouseButton>,
        x: f32,
        y: f32,
    ) {
        self.0.set_action(action);
        self.0.set_button(button);
        self.0.set_position(x, y);
    }
}

/// A reusable mouse encoder.
#[derive(Debug)]
pub struct MouseEncoder(NativeMouseEncoder);

impl MouseEncoder {
    /// Creates an encoder with tracking disabled.
    ///
    /// # Errors
    ///
    /// Fails when allocation fails.
    pub fn new() -> Result<Self> {
        Self::new_in(None)
    }

    pub(crate) fn new_in(allocator: AllocatorRef) -> Result<Self> {
        Ok(Self(NativeMouseEncoder::new(allocator)?))
    }

    /// Copies tracking mode and output format from the terminal.
    ///
    /// # Errors
    ///
    /// Fails when the terminal is poisoned.
    pub fn sync<H>(&mut self, terminal: &Terminal<H>) -> Result<()> {
        self.0.sync(terminal.native_ref()?.as_ref());
        Ok(())
    }

    /// Overrides the tracking mode.
    pub fn set_tracking(&mut self, mode: MouseTrackingMode) {
        let mode = match mode {
            MouseTrackingMode::None => ffi::GHOSTTY_MOUSE_TRACKING_NONE,
            MouseTrackingMode::X10 => ffi::GHOSTTY_MOUSE_TRACKING_X10,
            MouseTrackingMode::Normal => ffi::GHOSTTY_MOUSE_TRACKING_NORMAL,
            MouseTrackingMode::Button => ffi::GHOSTTY_MOUSE_TRACKING_BUTTON,
            MouseTrackingMode::Any => ffi::GHOSTTY_MOUSE_TRACKING_ANY,
        };
        self.0.set::<option::Event>(&mode);
    }

    /// Sets the surface geometry, without padding.
    pub fn set_geometry(&mut self, geometry: MouseGeometry) {
        let mut size: ffi::GhosttyMouseEncoderSize = ffi::sized();
        size.screen_width = geometry.screen_width;
        size.screen_height = geometry.screen_height;
        size.cell_width = geometry.cell_width;
        size.cell_height = geometry.cell_height;
        self.0.set::<option::Size>(&size);
    }

    /// Sets whether any button is currently held.
    pub fn set_any_button_pressed(&mut self, pressed: bool) {
        self.0.set::<option::AnyButtonPressed>(&pressed);
    }

    /// Sets whether motion within one cell is reported once.
    pub fn set_track_last_cell(&mut self, track: bool) {
        self.0.set::<option::TrackLastCell>(&track);
    }

    /// Encodes `event` into `buffer`; `Written(0)` means no report.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the event.
    pub fn encode(
        &mut self,
        event: &MouseEvent,
        buffer: &mut [u8],
    ) -> Result<Fill> {
        self.0.encode(&event.0, buffer)
    }
}

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
/// synced from the terminal. The output never reaches the PTY.
#[derive(Debug)]
pub struct MouseProbe {
    encoder: MouseEncoder,
    event: MouseEvent,
    buffer: [u8; 64],
}

impl MouseProbe {
    // A 200x200 surface of 1x1 cells puts (100, 100) at column 100, past the
    // single-byte range so UTF-8 encoding is visible.
    const GEOMETRY: MouseGeometry = MouseGeometry {
        screen_width: 200,
        screen_height: 200,
        cell_width: 1,
        cell_height: 1,
    };
    const POSITION: f32 = 100.0;

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
            encoder: MouseEncoder::new_in(allocator)?,
            event: MouseEvent::new_in(allocator)?,
            buffer: [0; 64],
        })
    }

    fn reports(
        &mut self,
        action: MouseAction,
        button: Option<MouseButton>,
    ) -> Result<&[u8]> {
        self.event
            .set(action, button, Self::POSITION, Self::POSITION);
        match self.encoder.encode(&self.event, &mut self.buffer)? {
            Fill::Written(len) => {
                Ok(&self.buffer[..len.min(self.buffer.len())])
            }
            // Any report proves the event is tracked.
            Fill::TooSmall(_) => Ok(&self.buffer[..]),
        }
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
        self.encoder.sync(terminal)?;
        self.encoder.set_geometry(Self::GEOMETRY);
        self.encoder.set_track_last_cell(false);
        self.encoder.set_any_button_pressed(false);
        let tracking = if self.reports(MouseAction::Motion, None)?.is_empty() {
            self.encoder.set_any_button_pressed(true);
            if !self
                .reports(MouseAction::Motion, Some(MouseButton::Left))?
                .is_empty()
            {
                ProbedTracking::ButtonMotion
            } else if terminal.mouse_tracking()? {
                ProbedTracking::Buttons
            } else {
                ProbedTracking::Disabled
            }
        } else {
            ProbedTracking::AllMotion
        };
        self.encoder.set_tracking(MouseTrackingMode::Any);
        self.encoder.set_any_button_pressed(false);
        let report =
            self.reports(MouseAction::Press, Some(MouseButton::Left))?;
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
