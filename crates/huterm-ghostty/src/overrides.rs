//! Detects which colors an OSC sequence overrode.
//!
//! The C API reports effective and default colors but no override mask, and
//! an override equal to its default is still an override. Changing a
//! default moves the effective color only where no override applies, so the
//! probe changes each default, compares effective colors, and restores the
//! default.

use crate::error::Result;
use crate::terminal::Terminal;
use crate::types::{Palette, Rgb};

/// Which colors carry an OSC override.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColorOverrides {
    /// OSC 10 foreground.
    pub foreground: bool,
    /// OSC 11 background.
    pub background: bool,
    /// OSC 12 cursor.
    pub cursor: bool,
    /// OSC 4 palette entries.
    pub palette: [bool; 256],
}

impl Default for ColorOverrides {
    fn default() -> Self {
        Self {
            foreground: false,
            background: false,
            cursor: false,
            palette: [false; 256],
        }
    }
}

/// The same color with the red channel's lowest bit flipped.
const fn flipped(color: Rgb) -> Rgb {
    Rgb {
        red: color.red ^ 1,
        ..color
    }
}

type Get<H> = fn(&Terminal<H>) -> Result<Option<Rgb>>;
type Set<H> = fn(&mut Terminal<H>, Option<Rgb>) -> Result<()>;

impl<H> Terminal<H> {
    /// Reports current OSC overrides. Every default is restored, even when a
    /// read fails; the writes can mark the whole viewport dirty.
    ///
    /// # Errors
    ///
    /// Fails when a read or write fails; defaults are restored first.
    pub fn probe_color_overrides(&mut self) -> Result<ColorOverrides> {
        Ok(ColorOverrides {
            foreground: self.probe_color(
                Self::default_foreground,
                Self::foreground,
                Self::set_default_foreground,
            )?,
            background: self.probe_color(
                Self::default_background,
                Self::background,
                Self::set_default_background,
            )?,
            cursor: self.probe_color(
                Self::default_cursor_color,
                Self::cursor_color,
                Self::set_default_cursor,
            )?,
            palette: self.probe_palette()?,
        })
    }

    fn probe_color(
        &mut self,
        default: Get<H>,
        effective: Get<H>,
        set: Set<H>,
    ) -> Result<bool> {
        let original = default(self)?;
        let before = effective(self)?;
        let probe = flipped(original.unwrap_or_default());
        set(self, Some(probe))?;
        let after = effective(self);
        set(self, original)?;
        Ok(after? == before)
    }

    fn probe_palette(&mut self) -> Result<[bool; 256]> {
        let original = self.default_palette()?;
        let before = self.palette()?;
        let probe: Palette = original.map(flipped);
        self.set_default_palette(&probe)?;
        let after = self.palette();
        self.set_default_palette(&original)?;
        let after = after?;
        Ok(std::array::from_fn(|index| after[index] == before[index]))
    }
}
