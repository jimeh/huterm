//! The wire form of a terminal launch: [`TerminalCommand`] and the values it
//! carries.
//!
//! Paths travel as bytes: on Unix the encoder takes their `OsStr` bytes, and
//! on other targets encoding and decoding require UTF-8. Arguments and
//! environment pairs share one payload limit, [`LAUNCH_PAYLOAD_LIMIT`], so
//! one long argument fits while the whole launch stays bounded. Decoders
//! check every length and count before they allocate, and build values
//! through the protocol's validating constructors.

use std::path::{Path, PathBuf};

use huterm_protocol::{
    CellSize, GridSize, Rgb, ScrollbackLimit, ShellFeature, ShellFeatures,
    ShellIntegration, ShellIntegrationMode, TerminalCommand,
    TerminalPresentation,
};

use crate::codec::{DecodeError, EncodeError, Reader, Writer};

/// The longest program or working directory path, in bytes.
pub const MAX_LAUNCH_PATH_BYTES: usize = 4096;

/// The most arguments one launch carries.
pub const MAX_LAUNCH_ARGUMENTS: usize = 1024;

/// The most environment pairs one launch carries.
pub const MAX_LAUNCH_ENVIRONMENT: usize = 1024;

/// The most bytes of arguments, environment names, and environment values
/// one launch carries together. A sender refuses a larger launch locally.
pub const LAUNCH_PAYLOAD_LIMIT: usize = 256 << 10;

/// The shell feature bits minor version 0 defines.
const SHELL_FEATURE_PATH: u64 = 1;

/// Counts a launch's argument and environment bytes against
/// [`LAUNCH_PAYLOAD_LIMIT`].
#[derive(Debug, Default)]
struct Payload(usize);

impl Payload {
    fn spend(&mut self, bytes: usize) -> bool {
        self.0 += bytes;
        self.0 <= LAUNCH_PAYLOAD_LIMIT
    }
}

fn write_path(
    w: &mut Writer,
    path: &Path,
    field: &'static str,
) -> Result<(), EncodeError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        w.bytes(path.as_os_str().as_bytes(), MAX_LAUNCH_PATH_BYTES, field)
    }
    #[cfg(not(unix))]
    {
        let text = path.to_str().ok_or(EncodeError::Invalid(field))?;
        w.str(text, MAX_LAUNCH_PATH_BYTES, field)
    }
}

fn read_path(
    r: &mut Reader<'_>,
    field: &'static str,
) -> Result<PathBuf, DecodeError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(
            r.bytes(MAX_LAUNCH_PATH_BYTES, field)?,
        )))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(r.str(MAX_LAUNCH_PATH_BYTES, field)?))
    }
}

/// Encodes a grid size: columns, then rows.
pub fn encode_grid(w: &mut Writer, grid: GridSize) {
    let GridSize { columns, rows } = grid;
    w.u16(columns);
    w.u16(rows);
}

/// Decodes a grid size as sent. Core clamps every grid it is given, so the
/// decoder applies no bound of its own.
///
/// # Errors
/// Returns a varint or range error.
pub fn decode_grid(r: &mut Reader<'_>) -> Result<GridSize, DecodeError> {
    Ok(GridSize {
        columns: r.u16()?,
        rows: r.u16()?,
    })
}

/// Encodes a cell size: width, then height.
pub fn encode_cell_size(w: &mut Writer, cell: CellSize) {
    let CellSize { width, height } = cell;
    w.u16(width);
    w.u16(height);
}

/// Decodes a cell size.
///
/// # Errors
/// Returns a varint or range error.
pub fn decode_cell_size(r: &mut Reader<'_>) -> Result<CellSize, DecodeError> {
    Ok(CellSize {
        width: r.u16()?,
        height: r.u16()?,
    })
}

/// Encodes a history limit: its rows, then its optional byte limit.
///
/// # Errors
/// Never fails today; the signature matches the other encoders.
pub fn encode_scrollback(
    w: &mut Writer,
    limit: ScrollbackLimit,
) -> Result<(), EncodeError> {
    w.record(|w| {
        w.u32(limit.rows());
        w.option(limit.bytes(), |w, bytes| {
            w.u64(bytes);
            Ok(())
        })
    })
}

/// Decodes a history limit through [`ScrollbackLimit`]'s constructors.
///
/// # Errors
/// Returns the first decode failure, or [`DecodeError::Invalid`] for rows
/// or bytes past their maximum.
pub fn decode_scrollback(
    r: &mut Reader<'_>,
) -> Result<ScrollbackLimit, DecodeError> {
    r.record(|r| {
        let limit = ScrollbackLimit::new(r.u32()?)
            .ok_or(DecodeError::Invalid("scrollback rows"))?;
        match r.option("scrollback bytes", Reader::u64)? {
            None => Ok(limit),
            Some(bytes) => limit
                .with_bytes(bytes)
                .ok_or(DecodeError::Invalid("scrollback bytes")),
        }
    })
}

fn write_rgb(w: &mut Writer, rgb: Rgb) {
    let Rgb { red, green, blue } = rgb;
    w.fixed(&[red, green, blue]);
}

fn read_rgb(r: &mut Reader<'_>) -> Result<Rgb, DecodeError> {
    let [red, green, blue] = r.fixed()?;
    Ok(Rgb { red, green, blue })
}

pub(crate) fn encode_presentation(
    w: &mut Writer,
    presentation: &TerminalPresentation,
) -> Result<(), EncodeError> {
    let TerminalPresentation {
        foreground,
        background,
        cursor,
        palette,
    } = presentation;
    w.record(|w| {
        write_rgb(w, *foreground);
        write_rgb(w, *background);
        write_rgb(w, *cursor);
        for color in palette {
            write_rgb(w, *color);
        }
        Ok(())
    })
}

pub(crate) fn decode_presentation(
    r: &mut Reader<'_>,
) -> Result<TerminalPresentation, DecodeError> {
    r.record(|r| {
        let foreground = read_rgb(r)?;
        let background = read_rgb(r)?;
        let cursor = read_rgb(r)?;
        let mut palette = [Rgb {
            red: 0,
            green: 0,
            blue: 0,
        }; 256];
        for color in &mut palette {
            *color = read_rgb(r)?;
        }
        Ok(TerminalPresentation {
            foreground,
            background,
            cursor,
            palette,
        })
    })
}

fn encode_shell_integration(
    w: &mut Writer,
    integration: &ShellIntegration,
) -> Result<(), EncodeError> {
    let ShellIntegration { mode, features } = integration;
    let mut bits = 0;
    for feature in features.iter() {
        bits |= match feature {
            ShellFeature::Path => SHELL_FEATURE_PATH,
            // A feature added to the protocol without a wire bit.
            _ => return Err(EncodeError::Unsupported),
        };
    }
    w.record(|w| {
        w.tag(match mode {
            ShellIntegrationMode::Detect => 0,
            ShellIntegrationMode::Off => 1,
        });
        w.u64(bits);
        Ok(())
    })
}

fn decode_shell_integration(
    r: &mut Reader<'_>,
) -> Result<ShellIntegration, DecodeError> {
    r.record(|r| {
        let mode = match r.tag()? {
            0 => ShellIntegrationMode::Detect,
            1 => ShellIntegrationMode::Off,
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "shell integration",
                    tag,
                });
            }
        };
        let bits = r.u64()?;
        if bits & !SHELL_FEATURE_PATH != 0 {
            return Err(DecodeError::Invalid("shell features"));
        }
        let features: ShellFeatures = (bits & SHELL_FEATURE_PATH != 0)
            .then_some(ShellFeature::Path)
            .into_iter()
            .collect();
        Ok(ShellIntegration { mode, features })
    })
}

/// Encodes a terminal launch.
///
/// # Errors
/// Refuses a path, argument list, or environment past its limit, or
/// arguments and environment past [`LAUNCH_PAYLOAD_LIMIT`] together, with
/// [`EncodeError::LimitExceeded`]; a shell feature without a wire bit with
/// [`EncodeError::Unsupported`]; and, off Unix, a path that is not UTF-8
/// with [`EncodeError::Invalid`].
pub fn encode_command(
    w: &mut Writer,
    command: &TerminalCommand,
) -> Result<(), EncodeError> {
    let TerminalCommand {
        program,
        arguments,
        working_directory,
        environment,
        shell_integration,
        grid_size,
        cell_size,
        presentation,
        scrollback,
        scrollback_compression,
    } = command;
    let over = EncodeError::LimitExceeded {
        field: "launch payload",
    };
    let mut payload = Payload::default();
    w.record(|w| {
        write_path(w, program, "program")?;
        w.seq(
            arguments.iter(),
            MAX_LAUNCH_ARGUMENTS,
            "arguments",
            |w, argument| {
                if !payload.spend(argument.len()) {
                    return Err(over);
                }
                w.str(argument, LAUNCH_PAYLOAD_LIMIT, "argument")
            },
        )?;
        write_path(w, working_directory, "working directory")?;
        w.seq(
            environment.iter(),
            MAX_LAUNCH_ENVIRONMENT,
            "environment",
            |w, (name, value)| {
                if !payload.spend(name.len() + value.len()) {
                    return Err(over);
                }
                w.record(|w| {
                    w.str(name, LAUNCH_PAYLOAD_LIMIT, "environment name")?;
                    w.str(value, LAUNCH_PAYLOAD_LIMIT, "environment value")
                })
            },
        )?;
        encode_shell_integration(w, shell_integration)?;
        encode_grid(w, *grid_size);
        encode_cell_size(w, *cell_size);
        encode_presentation(w, presentation)?;
        encode_scrollback(w, *scrollback)?;
        w.bool(*scrollback_compression);
        Ok(())
    })
}

/// Decodes a terminal launch.
///
/// # Errors
/// Returns the first decode failure: a length or count past its limit, or
/// arguments and environment past [`LAUNCH_PAYLOAD_LIMIT`] together, is
/// [`DecodeError::TooLong`].
pub fn decode_command(
    r: &mut Reader<'_>,
) -> Result<TerminalCommand, DecodeError> {
    let over = DecodeError::TooLong {
        field: "launch payload",
    };
    let mut payload = Payload::default();
    r.record(|r| {
        let program = read_path(r, "program")?;
        let arguments = r.seq(MAX_LAUNCH_ARGUMENTS, "arguments", |r| {
            let argument = r.str(LAUNCH_PAYLOAD_LIMIT, "argument")?;
            if !payload.spend(argument.len()) {
                return Err(over);
            }
            Ok(argument.to_owned())
        })?;
        let working_directory = read_path(r, "working directory")?;
        let environment =
            r.seq(MAX_LAUNCH_ENVIRONMENT, "environment", |r| {
                r.record(|r| {
                    let name =
                        r.str(LAUNCH_PAYLOAD_LIMIT, "environment name")?;
                    let value =
                        r.str(LAUNCH_PAYLOAD_LIMIT, "environment value")?;
                    if !payload.spend(name.len() + value.len()) {
                        return Err(over);
                    }
                    Ok((name.to_owned(), value.to_owned()))
                })
            })?;
        Ok(TerminalCommand {
            program,
            arguments,
            working_directory,
            environment,
            shell_integration: decode_shell_integration(r)?,
            grid_size: decode_grid(r)?,
            cell_size: decode_cell_size(r)?,
            presentation: decode_presentation(r)?,
            scrollback: decode_scrollback(r)?,
            scrollback_compression: r.bool()?,
        })
    })
}

#[cfg(test)]
mod tests;
