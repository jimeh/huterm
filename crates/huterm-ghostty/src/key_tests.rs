//! Checks each generated key's Rust type against what the library actually
//! writes or reads, independently of the header prose the generator parsed.
//!
//! Getters run twice into an output prefilled with 0x00 and then 0xFF and
//! followed by guard bytes: every byte of the Rust type must be written and
//! no guard byte may change. Setters read a value followed by guard bytes,
//! and the library's behavior must match the value alone.

use crate::ffi::{self, keys};
use crate::native::key_probe::{self, Written};
use crate::native::{NativeRowCells, NativeRowIterator, ProbeEvent};
use crate::{
    Effect, Fill, MouseProbe, Options, Point, RenderState, Rgb, Terminal,
};

fn options() -> Options {
    Options {
        columns: 8,
        rows: 3,
        cell_width: 9,
        cell_height: 17,
        device_attributes: None,
        xtversion: None,
    }
}

/// Asserts that a getter wrote every declared byte of its output, including
/// the union member its tag selects, and nothing after it. Padding is never
/// read.
fn check(written: &Written, marker: &str) {
    assert!(!written.overran, "{marker} wrote past its output");
    assert!(
        written.untouched.is_empty(),
        "{marker} left declared bytes {:?} of {} unwritten",
        written.untouched,
        written.type_name
    );
}

/// Checks one getter key and records its marker.
macro_rules! exact {
    ($probed:ident, $marker:ident, $written:expr) => {{
        check(&$written.unwrap(), stringify!($marker));
        $probed.push(stringify!($marker));
    }};
}

/// Asserts that `probed` names the marker of every key in `keys`.
fn covers(mut probed: Vec<&str>, keys: &[&str]) {
    let mut expected: Vec<String> = keys
        .iter()
        .map(|key| {
            key.split('_')
                .map(|word| word[..1].to_owned() + &word[1..].to_lowercase())
                .collect()
        })
        .collect();
    expected.sort();
    probed.sort_unstable();
    assert_eq!(probed, expected);
}

fn replies<H>(terminal: &mut Terminal<H>, bytes: &[u8]) -> Vec<Vec<u8>> {
    terminal.write(bytes).unwrap();
    let mut effects = Vec::new();
    terminal.take_effects(&mut effects);
    effects
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::PtyWrite(bytes) => Some(bytes),
            _ => None,
        })
        .collect()
}

/// A terminal with every color set, a title, a directory, and styled,
/// background-only, and grapheme cells.
///
/// The render-state probes read `GhosttyRenderStateColors.cursor` and
/// `GhosttyRenderStateCursor.viewport_*`, which render.h defines only when
/// their `*_has_value` flags are set, so the fixture needs a default cursor
/// color and a cursor inside the viewport.
fn populated() -> Terminal<()> {
    let mut terminal = Terminal::new(options(), ()).unwrap();
    terminal
        .set_default_foreground(Some(Rgb::new(1, 2, 3)))
        .unwrap();
    terminal
        .set_default_background(Some(Rgb::new(4, 5, 6)))
        .unwrap();
    terminal
        .set_default_cursor(Some(Rgb::new(7, 8, 9)))
        .unwrap();
    terminal
        .write(
            "\x1b]2;title\x07\x1b]7;file:///tmp\x07\x1b[1;31me\u{301}\x1b[m\
             \x1b]8;;https://a.test\x07l\x1b]8;;\x07\r\n\x1b[41m\x1b[K\r\n\
             \x1b[48;2;1;2;3m\x1b[K\x1b[m"
                .as_bytes(),
        )
        .unwrap();
    let mut render = RenderState::new().unwrap();
    render.update(&mut terminal).unwrap();
    assert!(
        render.colors().unwrap().cursor.is_some(),
        "fixture precondition: set a default cursor color, or the render \
         colors' cursor bytes are undefined"
    );
    assert!(
        render.cursor().unwrap().position.is_some(),
        "fixture precondition: keep the cursor inside the viewport, or the \
         render cursor's position bytes are undefined"
    );
    terminal
}

#[test]
fn terminal_data_keys_write_exactly_their_output_type() {
    use keys::terminal_data as key;

    let terminal = populated();
    let t = terminal.native_ref().unwrap().as_ref();
    let rgb = ffi::GhosttyColorRgb::default();
    let palette = [rgb; 256];
    let string = ffi::GhosttyString::default();
    let mut probed = Vec::new();
    macro_rules! get {
        ($($marker:ident: $blank:expr),* $(,)?) => {$(
            exact!(
                probed,
                $marker,
                key_probe::terminal::<key::$marker>(t, &$blank, 0)
            );
        )*};
    }
    get!(
        Cols: 0,
        Rows: 0,
        ActiveScreen: 0,
        Scrollbar: ffi::GhosttyTerminalScrollbar::default(),
        MouseTracking: false,
        Title: string,
        Pwd: string,
        TotalRows: 0,
        ScrollbackRows: 0,
        ColorForeground: rgb,
        ColorBackground: rgb,
        ColorCursor: rgb,
        ColorPalette: palette,
        ColorForegroundDefault: rgb,
        ColorBackgroundDefault: rgb,
        ColorCursorDefault: rgb,
        ColorPaletteDefault: palette,
        KittyImageStorageLimit: 0,
        ScrollbackMaxBytes: 0,
        VtGround: false,
        ClipboardWriteMaxBytes: 0,
    );
    // The mode key reads the mode from its output and writes the value.
    let mode = ffi::GhosttyTerminalModeConfig {
        mode: ffi::mode(25, false),
        value: false,
    };
    let keep = std::mem::offset_of!(ffi::GhosttyTerminalModeConfig, value);
    exact!(
        probed,
        Mode,
        key_probe::terminal::<key::Mode>(t, &mode, keep)
    );
    covers(probed, key::KEYS);
}

#[test]
fn render_keys_write_exactly_their_output_type() {
    use keys::{
        render_cell_data as cell, render_row_data as row,
        render_state_data as state,
    };

    let mut terminal = populated();
    let mut render = RenderState::new().unwrap();
    render.update(&mut terminal).unwrap();
    let native = render.native();
    let sized_keep = size_of::<usize>();
    let mut states = Vec::new();
    exact!(
        states,
        Dirty,
        key_probe::render_state::<state::Dirty>(native, &0, 0)
    );
    exact!(
        states,
        Cursor,
        key_probe::render_state::<state::Cursor>(
            native,
            &ffi::sized(),
            sized_keep
        )
    );
    exact!(
        states,
        Colors,
        key_probe::render_state::<state::Colors>(
            native,
            &ffi::sized(),
            sized_keep
        )
    );
    let mut iterator = NativeRowIterator::new(None).unwrap();
    exact!(
        states,
        RowIterator,
        key_probe::row_iterator(native, &mut iterator)
    );
    covers(states, state::KEYS);

    let mut rows = render.rows().unwrap();
    let mut row = rows.next().unwrap();
    let mut row_keys = Vec::new();
    exact!(
        row_keys,
        Dirty,
        key_probe::render_row::<row::Dirty>(row.cursor(), &false, 0)
    );
    exact!(
        row_keys,
        Raw,
        key_probe::render_row::<row::Raw>(row.cursor(), &0, 0)
    );
    let mut cells = NativeRowCells::new(None).unwrap();
    exact!(
        row_keys,
        Cells,
        key_probe::row_cells(row.cursor(), &mut cells)
    );
    covers(row_keys, row::KEYS);

    let mut cells = row.cells().unwrap();
    assert!(cells.next());
    let mut cell_keys = Vec::new();
    exact!(
        cell_keys,
        Raw,
        key_probe::render_cell::<cell::Raw>(cells.cursor(), &0, 0)
    );
    exact!(
        cell_keys,
        Style,
        key_probe::render_cell::<cell::Style>(
            cells.cursor(),
            &ffi::sized(),
            sized_keep
        )
    );
    // The UTF-8 key writes through a caller buffer; its wrapper has its own
    // bounds test.
    cell_keys.push("GraphemesUtf8");
    covers(cell_keys, cell::KEYS);
}

#[test]
fn grapheme_utf8_writes_stay_inside_the_caller_buffer() {
    let mut terminal = populated();
    let mut render = RenderState::new().unwrap();
    render.update(&mut terminal).unwrap();
    let mut rows = render.rows().unwrap();
    let mut row = rows.next().unwrap();
    let mut cells = row.cells().unwrap();
    assert!(cells.next());
    let cursor = cells.cursor();
    // "e\u{301}" is three UTF-8 bytes.
    let mut buffer = [0xAA_u8; 8];
    assert_eq!(cursor.graphemes_utf8(&mut []).unwrap(), Fill::TooSmall(3));
    assert_eq!(
        cursor.graphemes_utf8(&mut buffer[..2]).unwrap(),
        Fill::TooSmall(3)
    );
    assert_eq!(buffer, [0xAA; 8], "a short buffer is left untouched");
    assert_eq!(
        cursor.graphemes_utf8(&mut buffer[..3]).unwrap(),
        Fill::Written(3)
    );
    assert_eq!(&buffer[..3], "e\u{301}".as_bytes());
    assert_eq!(buffer[3..], [0xAA; 5]);
}

#[test]
fn screen_cell_and_row_keys_write_exactly_their_output_type() {
    use keys::{cell_data as cell, row_data as row};

    let terminal = populated();
    let at = |x, y| {
        terminal
            .grid_ref(Point::viewport(x, y))
            .unwrap()
            .cell()
            .unwrap()
    };
    let (text, palette, rgb) = (at(0, 0), at(0, 1), at(0, 2));
    let mut cells = Vec::new();
    exact!(
        cells,
        Codepoint,
        key_probe::cell::<cell::Codepoint>(text, &0)
    );
    exact!(
        cells,
        ContentTag,
        key_probe::cell::<cell::ContentTag>(text, &0)
    );
    exact!(cells, Wide, key_probe::cell::<cell::Wide>(text, &0));
    exact!(
        cells,
        HasText,
        key_probe::cell::<cell::HasText>(text, &false)
    );
    exact!(
        cells,
        HasStyling,
        key_probe::cell::<cell::HasStyling>(text, &false)
    );
    exact!(
        cells,
        HasHyperlink,
        key_probe::cell::<cell::HasHyperlink>(at(1, 0), &false)
    );
    exact!(
        cells,
        ColorPalette,
        key_probe::cell::<cell::ColorPalette>(palette, &0)
    );
    exact!(
        cells,
        ColorRgb,
        key_probe::cell::<cell::ColorRgb>(
            rgb,
            &ffi::GhosttyColorRgb::default()
        )
    );
    covers(cells, cell::KEYS);

    let value = terminal
        .grid_ref(Point::viewport(0, 0))
        .unwrap()
        .row()
        .unwrap();
    let mut rows = Vec::new();
    exact!(rows, Wrap, key_probe::row::<row::Wrap>(value, &false));
    exact!(
        rows,
        Grapheme,
        key_probe::row::<row::Grapheme>(value, &false)
    );
    exact!(rows, Styled, key_probe::row::<row::Styled>(value, &false));
    covers(rows, row::KEYS);

    let mut builds = Vec::new();
    exact!(
        builds,
        Optimize,
        key_probe::build_info::<keys::build_info::Optimize>(&0)
    );
    covers(builds, keys::build_info::KEYS);
}

#[test]
fn terminal_option_keys_read_exactly_their_value_type() {
    use keys::{terminal_data as data, terminal_option as option};

    let mut terminal = Terminal::new(options(), ()).unwrap();
    let mut probed = Vec::new();
    // Callback and userdata values are passed as the pointer itself; the
    // callback contract tests exercise each one.
    probed.extend([
        "Userdata",
        "WritePty",
        "Bell",
        "Xtversion",
        "TitleChanged",
        "Size",
        "ColorScheme",
        "DeviceAttributes",
        "PwdChanged",
        "ClipboardWrite",
    ]);

    // Values read back exactly despite 0xFF bytes after them.
    let native = terminal.native_parts().unwrap();
    let rgb = ffi::GhosttyColorRgb { r: 1, g: 2, b: 3 };
    macro_rules! round_trip {
        ($marker:ident, $value:expr, $blank:expr) => {{
            let value = $value;
            key_probe::set_terminal::<option::$marker>(native, &value, 0xFF)
                .unwrap();
            let mut read = $blank;
            native.as_ref().get::<data::$marker>(&mut read).unwrap();
            assert_eq!(
                format!("{read:?}"),
                format!("{value:?}"),
                stringify!($marker)
            );
            probed.push(stringify!($marker));
        }};
    }
    let blank = ffi::GhosttyColorRgb::default();
    round_trip!(ColorForeground, rgb, blank);
    round_trip!(ColorBackground, rgb, blank);
    round_trip!(ColorCursor, rgb, blank);
    let mut palette = [rgb; 256];
    palette[255] = ffi::GhosttyColorRgb { r: 9, g: 8, b: 7 };
    round_trip!(ColorPalette, palette, [blank; 256]);
    round_trip!(KittyImageStorageLimit, 0x0000_0001_0000_0010_u64, 0);
    round_trip!(ScrollbackMaxBytes, 0x0000_0001_0000_0100_usize, 0);

    // Values without getters: 0xFF guard bytes would turn a zero value
    // nonzero if the library read past it.
    let glyph_query: &[u8] = b"\x1b_25a1;s\x1b\\";
    let native = terminal.native_parts().unwrap();
    key_probe::set_terminal::<option::GlyphProtocol>(native, &false, 0xFF)
        .unwrap();
    assert!(replies(&mut terminal, glyph_query).is_empty());
    let native = terminal.native_parts().unwrap();
    key_probe::set_terminal::<option::GlyphProtocol>(native, &true, 0x00)
        .unwrap();
    assert!(!replies(&mut terminal, glyph_query).is_empty());
    probed.push("GlyphProtocol");

    let native = terminal.native_parts().unwrap();
    key_probe::set_terminal::<option::ApcMaxBytes>(native, &0, 0xFF).unwrap();
    assert!(replies(&mut terminal, glyph_query).is_empty());
    // A 32-bit read would see zero here.
    let native = terminal.native_parts().unwrap();
    key_probe::set_terminal::<option::ApcMaxBytes>(native, &(1 << 32), 0x00)
        .unwrap();
    assert!(!replies(&mut terminal, glyph_query).is_empty());
    probed.push("ApcMaxBytes");

    let kitty_write: &[u8] = b"\x1b]5522;type=write\x1b\\\x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;YQ==\x1b\\\x1b]5522;type=wdata\x1b\\";
    let too_big: &[u8] = b"\x1b]5522;type=write:status=EFBIG\x1b\\";
    let native = terminal.native_parts().unwrap();
    key_probe::set_terminal::<option::ClipboardWriteMaxBytes>(native, &0, 0xFF)
        .unwrap();
    assert_eq!(replies(&mut terminal, kitty_write), [too_big.to_vec()]);
    // A 32-bit read would see zero here.
    let native = terminal.native_parts().unwrap();
    key_probe::set_terminal::<option::ClipboardWriteMaxBytes>(
        native,
        &(1 << 32),
        0x00,
    )
    .unwrap();
    assert!(!replies(&mut terminal, kitty_write).contains(&too_big.to_vec()));
    probed.push("ClipboardWriteMaxBytes");

    covers(probed, option::KEYS);
}

#[test]
fn mouse_option_keys_read_exactly_their_value_type() {
    use keys::mouse_encoder_option as option;

    let mut terminal = Terminal::new(options(), ()).unwrap();
    let mut probe = MouseProbe::new().unwrap();
    let mut probed = Vec::new();

    // A geometry unlike the probe's shows the guarded value was read.
    terminal.write(b"\x1b[?1000h\x1b[?1006h").unwrap();
    probe.sync(&terminal).unwrap();
    let mut size: ffi::GhosttyMouseEncoderSize = ffi::sized();
    size.screen_width = 400;
    size.screen_height = 400;
    size.cell_width = 2;
    size.cell_height = 2;
    key_probe::set_mouse::<option::Size>(probe.encoder_mut(), &size, 0xFF);
    assert_eq!(
        probe.report(ProbeEvent::LeftPress).unwrap(),
        b"\x1b[<0;51;51M"
    );
    probed.push("Size");

    key_probe::set_mouse::<option::Event>(
        probe.encoder_mut(),
        &ffi::GHOSTTY_MOUSE_TRACKING_NONE,
        0xFF,
    );
    assert_eq!(probe.report(ProbeEvent::LeftPress).unwrap(), b"");
    key_probe::set_mouse::<option::Event>(
        probe.encoder_mut(),
        &ffi::GHOSTTY_MOUSE_TRACKING_ANY,
        0xFF,
    );
    assert!(!probe.report(ProbeEvent::Motion).unwrap().is_empty());
    probed.push("Event");

    // The pressed flag only matters outside the surface, so shrink it below
    // the probe position.
    terminal.write(b"\x1b[?1002h").unwrap();
    probe.sync(&terminal).unwrap();
    size.screen_width = 50;
    size.screen_height = 50;
    size.cell_width = 1;
    size.cell_height = 1;
    key_probe::set_mouse::<option::Size>(probe.encoder_mut(), &size, 0x00);
    key_probe::set_mouse::<option::AnyButtonPressed>(
        probe.encoder_mut(),
        &false,
        0xFF,
    );
    assert_eq!(probe.report(ProbeEvent::LeftDrag).unwrap(), b"");
    key_probe::set_mouse::<option::AnyButtonPressed>(
        probe.encoder_mut(),
        &true,
        0x00,
    );
    assert!(!probe.report(ProbeEvent::LeftDrag).unwrap().is_empty());
    probed.push("AnyButtonPressed");

    terminal.write(b"\x1b[?1003h").unwrap();
    probe.sync(&terminal).unwrap();
    key_probe::set_mouse::<option::TrackLastCell>(
        probe.encoder_mut(),
        &false,
        0xFF,
    );
    assert!(!probe.report(ProbeEvent::Motion).unwrap().is_empty());
    assert!(!probe.report(ProbeEvent::Motion).unwrap().is_empty());
    key_probe::set_mouse::<option::TrackLastCell>(
        probe.encoder_mut(),
        &true,
        0x00,
    );
    assert!(!probe.report(ProbeEvent::Motion).unwrap().is_empty());
    assert_eq!(probe.report(ProbeEvent::Motion).unwrap(), b"");
    probed.push("TrackLastCell");
    covers(probed, option::KEYS);
}
