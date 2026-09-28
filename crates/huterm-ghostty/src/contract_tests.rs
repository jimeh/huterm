//! One test per libghostty-vt behavior that Huterm relies on. A failure
//! after a pin bump names the assumption that changed.

use crate::native::{self, ProbeEvent};
use crate::test_alloc;
use crate::{
    CellContent, CellWidth, ClipboardLocation, ClipboardWrite,
    ClipboardWriteResult, ColorScheme, DeviceAttributes, Dirty, Effect, Error,
    Fill, Host, Mode, MouseProbe, Options, Point, ProbedFormat, ProbedTracking,
    RenderState, Rgb, Screen, Scroll, Terminal,
};

/// A recorded clipboard write: location, (MIME, data) pairs, and name.
type ClipboardRecord = (ClipboardLocation, Vec<(Vec<u8>, Vec<u8>)>, Vec<u8>);

/// Records every host call and answers with configured results.
#[derive(Debug, Default)]
struct Recorder {
    clipboard: Vec<ClipboardRecord>,
    clipboard_result: Option<ClipboardWriteResult>,
    backgrounds: Vec<Rgb>,
    panic_on_clipboard: bool,
}

impl Host for Recorder {
    fn clipboard_write(
        &mut self,
        request: &ClipboardWrite<'_>,
    ) -> ClipboardWriteResult {
        assert!(!self.panic_on_clipboard, "host clipboard callback panicked");
        self.clipboard.push((
            request.location,
            request
                .contents
                .iter()
                .map(|content| (content.mime.to_vec(), content.data.to_vec()))
                .collect(),
            request.name.to_vec(),
        ));
        self.clipboard_result
            .unwrap_or(ClipboardWriteResult::Success)
    }

    fn color_scheme(&mut self, background: Rgb) -> Option<ColorScheme> {
        self.backgrounds.push(background);
        Some(if background.red > 0x80 {
            ColorScheme::Light
        } else {
            ColorScheme::Dark
        })
    }
}

fn options() -> Options {
    Options {
        columns: 8,
        rows: 3,
        cell_width: 9,
        cell_height: 17,
        device_attributes: Some(DeviceAttributes {
            conformance_level: DeviceAttributes::VT220,
            features: vec![DeviceAttributes::FEATURE_ANSI_COLOR],
            device_type: DeviceAttributes::DEVICE_TYPE_VT220,
            firmware_version: 0,
            rom_cartridge: 0,
            unit_id: 0,
        }),
        xtversion: Some("Huterm".to_owned()),
    }
}

fn terminal() -> Terminal<Recorder> {
    Terminal::new(options(), Recorder::default()).unwrap()
}

fn effects<H>(terminal: &mut Terminal<H>) -> Vec<Effect> {
    let mut effects = Vec::new();
    terminal.take_effects(&mut effects);
    effects
}

fn replies<H>(terminal: &mut Terminal<H>) -> Vec<Vec<u8>> {
    effects(terminal)
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::PtyWrite(bytes) => Some(bytes),
            _ => None,
        })
        .collect()
}

fn written<H>(terminal: &mut Terminal<H>, bytes: &[u8]) -> Vec<Vec<u8>> {
    terminal.write(bytes).unwrap();
    replies(terminal)
}

#[test]
fn linked_library_uses_the_requested_optimization() {
    let expected = match env!("HUTERM_GHOSTTY_BUILT_OPTIMIZE") {
        "Debug" => crate::Optimize::Debug,
        "ReleaseSafe" => crate::Optimize::ReleaseSafe,
        "ReleaseSmall" => crate::Optimize::ReleaseSmall,
        _ => crate::Optimize::ReleaseFast,
    };
    assert_eq!(crate::optimize().unwrap(), expected);
}

#[test]
fn empty_formatter_output_is_null_with_zero_length() {
    let terminal = terminal();
    let output = terminal
        .format_plain(Point::screen(0, 0), Point::screen(7, 2))
        .unwrap();
    assert!(output.is_null());
    assert!(output.as_bytes().is_empty());
}

#[test]
fn ghostty_alloc_of_zero_bytes_returns_null() {
    assert!(native::alloc_bytes(None, 0).is_null());
    let bytes = native::alloc_bytes(None, 16);
    assert!(!bytes.is_null());
    assert_eq!(bytes.as_bytes().len(), 16);
}

#[test]
fn formatter_joins_soft_wraps_and_trims_trailing_whitespace() {
    let mut terminal = terminal();
    terminal.write(b"abcdefghij\r\nx   ").unwrap();
    let output = terminal
        .format_plain(Point::screen(0, 0), Point::screen(7, 2))
        .unwrap();
    assert_eq!(output.as_bytes(), b"abcdefghij\nx");
}

#[test]
fn ground_state_covers_escape_osc_and_utf8_fragments() {
    for (partial, rest) in [
        (&b"\x1b"[..], &b"[m"[..]),
        (b"\x1b[3", b"1m"),
        (b"\x1b]2;title", b"\x07"),
        (b"\x1bP", b"q\x1b\\"),
        (b"\xe2\x98", b"\x83"),
    ] {
        let mut terminal = terminal();
        assert!(terminal.is_ground().unwrap());
        terminal.write(partial).unwrap();
        assert!(!terminal.is_ground().unwrap(), "{partial:?}");
        terminal.write(rest).unwrap();
        assert!(terminal.is_ground().unwrap(), "{partial:?}{rest:?}");
    }
}

#[test]
fn title_changes_notify_and_read_back_after_the_write() {
    let mut terminal = terminal();
    assert!(terminal.title().unwrap().is_empty());
    terminal
        .write(b"\x1b]2;first\x07\x1b]0;second\x07")
        .unwrap();
    assert_eq!(
        effects(&mut terminal),
        [Effect::TitleChanged, Effect::TitleChanged]
    );
    assert_eq!(terminal.title().unwrap(), b"second");
    // Non-mutating reads leave the borrowed string intact.
    let title = terminal.title().unwrap().to_vec();
    let _ = terminal.size().unwrap();
    assert_eq!(terminal.title().unwrap(), title.as_slice());
}

#[test]
fn pwd_callbacks_read_the_new_value_inside_the_write() {
    let mut terminal = terminal();
    terminal
        .write(
            b"\x1b]7;file://host/a\x07\x1b]1337;CurrentDir=/b\x07\x1b]9;9;/c\x07\x1b]7;\x07",
        )
        .unwrap();
    assert_eq!(
        effects(&mut terminal),
        [
            Effect::PwdChanged(b"file://host/a".to_vec()),
            Effect::PwdChanged(b"/b".to_vec()),
            Effect::PwdChanged(b"/c".to_vec()),
            Effect::PwdChanged(Vec::new()),
        ]
    );
    assert!(terminal.pwd().unwrap().is_empty());
}

#[test]
fn effects_queue_in_callback_order() {
    let mut terminal = terminal();
    terminal.write(b"\x07\x1b[6n\x07").unwrap();
    assert_eq!(
        effects(&mut terminal),
        [
            Effect::Bell,
            Effect::PtyWrite(b"\x1b[1;1R".to_vec()),
            Effect::Bell,
        ]
    );
}

#[test]
fn device_attributes_and_xtversion_come_from_plain_data() {
    let mut terminal = terminal();
    assert_eq!(
        written(&mut terminal, b"\x1b[c\x1b[>c\x1b[=c\x1b[>q"),
        [
            b"\x1b[?62;22c".to_vec(),
            b"\x1b[>1;0;0c".to_vec(),
            b"\x1bP!|00000000\x1b\\".to_vec(),
            b"\x1bP>|Huterm\x1b\\".to_vec(),
        ]
    );
}

#[test]
fn size_reports_use_callback_geometry_and_stay_silent_without_cells() {
    let mut terminal = terminal();
    assert_eq!(
        written(&mut terminal, b"\x1b[14t\x1b[16t\x1b[18t"),
        [
            b"\x1b[4;51;72t".to_vec(),
            b"\x1b[6;17;9t".to_vec(),
            b"\x1b[8;3;8t".to_vec(),
        ]
    );
    terminal.resize(5, 4, 0, 0).unwrap();
    assert!(written(&mut terminal, b"\x1b[14t\x1b[18t").is_empty());
}

#[test]
fn color_scheme_queries_see_the_background_at_their_position() {
    let mut terminal = terminal();
    terminal
        .set_default_background(Some(Rgb::new(1, 2, 3)))
        .unwrap();
    assert_eq!(
        written(
            &mut terminal,
            b"\x1b[?996n\x1b]11;#ffffff\x07\x1b[?996n\x1b]111\x07\x1b[?996n"
        ),
        [
            b"\x1b[?997;1n".to_vec(),
            b"\x1b[?997;2n".to_vec(),
            b"\x1b[?997;1n".to_vec(),
        ]
    );
    assert_eq!(
        terminal.host_mut().backgrounds,
        [
            Rgb::new(1, 2, 3),
            Rgb::new(255, 255, 255),
            Rgb::new(1, 2, 3)
        ]
    );
}

#[test]
fn osc52_writes_arrive_as_decoded_text_plain_and_empty_clears() {
    let mut terminal = terminal();
    terminal
        .write(b"\x1b]52;c;AP8=\x07\x1b]52;p;YQ==\x07\x1b]52;s;\x07\x1b]1337;Copy=:Yg==\x07")
        .unwrap();
    let text = b"text/plain".to_vec();
    assert_eq!(
        terminal.host_mut().clipboard,
        [
            (
                ClipboardLocation::Standard,
                vec![(text.clone(), vec![0, 0xff])],
                Vec::new()
            ),
            (
                ClipboardLocation::Primary,
                vec![(text.clone(), b"a".to_vec())],
                Vec::new()
            ),
            (ClipboardLocation::Selection, Vec::new(), Vec::new()),
            (
                ClipboardLocation::Standard,
                vec![(text, b"b".to_vec())],
                Vec::new()
            ),
        ]
    );
    // OSC 52 has no acknowledgement.
    assert!(replies(&mut terminal).is_empty());
}

#[test]
fn oversized_osc52_is_dropped_and_the_parser_recovers() {
    let mut terminal = terminal();
    let mut sequence = b"\x1b]52;c;".to_vec();
    sequence.resize(sequence.len() + 8 * 1024 * 1024 + 4, b'Q');
    sequence.extend_from_slice(b"\x07\x1b]2;after\x07\x1b]52;c;YQ==\x07");
    terminal.write(&sequence).unwrap();
    assert_eq!(terminal.title().unwrap(), b"after");
    let writes = &terminal.host_mut().clipboard;
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].1[0].1, b"a");
}

#[test]
fn kitty_clipboard_writes_with_data_fail_before_the_callback() {
    let mut terminal = terminal();
    terminal.disable_extensions().unwrap();
    assert_eq!(
        written(
            &mut terminal,
            b"\x1b]5522;type=write:id=1\x1b\\\x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;R2hvc3Q=\x1b\\\x1b]5522;type=wdata\x1b\\"
        ),
        [b"\x1b]5522;type=write:status=EFBIG:id=1\x1b\\".to_vec()]
    );
    assert!(terminal.host_mut().clipboard.is_empty());
}

#[test]
fn kitty_clipboard_commit_without_data_reaches_the_callback_like_osc52_clear() {
    let mut terminal = terminal();
    terminal.disable_extensions().unwrap();
    terminal.host_mut().clipboard_result = Some(ClipboardWriteResult::Denied);
    assert_eq!(
        written(
            &mut terminal,
            b"\x1b]5522;type=write\x1b\\\x1b]5522;type=wdata\x1b\\"
        ),
        [b"\x1b]5522;type=write:status=EPERM\x1b\\".to_vec()]
    );
    assert_eq!(
        terminal.host_mut().clipboard,
        [(ClipboardLocation::Standard, Vec::new(), Vec::new())]
    );
}

#[test]
fn clipboard_reads_without_a_callback_are_silent() {
    let mut terminal = terminal();
    assert!(written(&mut terminal, b"\x1b]52;c;?\x07").is_empty());
}

#[test]
fn image_and_glyph_protocols_can_be_disabled() {
    use crate::ffi::keys::terminal_option as option;

    const GLYPH: &[u8] = b"\x1b_25a1;s\x1b\\";
    const IMAGE: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";
    let silent = |terminal: &mut Terminal<Recorder>| {
        written(terminal, GLYPH).is_empty()
            && written(terminal, IMAGE).is_empty()
    };
    let mut terminal = terminal();
    assert!(!written(&mut terminal, GLYPH).is_empty());
    assert!(!written(&mut terminal, IMAGE).is_empty());
    terminal.disable_extensions().unwrap();
    assert!(matches!(
        terminal.kitty_image_storage_limit().unwrap(),
        None | Some(0)
    ));
    assert!(silent(&mut terminal));

    // Each setting silences its protocol alone: first with APC buffering
    // restored, then with both protocols re-enabled behind a zero APC
    // limit.
    let native = terminal.native_parts().unwrap();
    native.set::<option::ApcMaxBytes>(None).unwrap();
    assert!(silent(&mut terminal));
    terminal.disable_extensions().unwrap();
    let native = terminal.native_parts().unwrap();
    native.set::<option::GlyphProtocol>(Some(&true)).unwrap();
    native
        .set::<option::KittyImageStorageLimit>(Some(&(1 << 20)))
        .unwrap();
    assert!(silent(&mut terminal));
}

#[test]
fn kitty_keyboard_queries_are_answered_natively() {
    // Huterm drops these replies because it does not implement the
    // protocol's input side.
    let mut terminal = terminal();
    assert_eq!(written(&mut terminal, b"\x1b[?u"), [b"\x1b[?0u".to_vec()]);
}

#[test]
fn default_color_changes_preserve_osc_overrides_even_when_equal() {
    let mut terminal = terminal();
    terminal
        .set_default_foreground(Some(Rgb::new(1, 1, 1)))
        .unwrap();
    let mut palette = terminal.default_palette().unwrap();
    terminal
        .write(b"\x1b]10;#010101\x07\x1b]4;3;#aabbcc\x07")
        .unwrap();
    terminal
        .set_default_foreground(Some(Rgb::new(9, 9, 9)))
        .unwrap();
    assert_eq!(terminal.foreground().unwrap(), Some(Rgb::new(1, 1, 1)));
    assert_eq!(
        terminal.default_foreground().unwrap(),
        Some(Rgb::new(9, 9, 9))
    );
    palette[3] = Rgb::new(0, 0, 0);
    palette[4] = Rgb::new(4, 4, 4);
    terminal.set_default_palette(&palette).unwrap();
    let effective = terminal.palette().unwrap();
    assert_eq!(effective[3], Rgb::new(0xaa, 0xbb, 0xcc));
    assert_eq!(effective[4], Rgb::new(4, 4, 4));
    terminal.write(b"\x1b]110\x07\x1b]104;3\x07").unwrap();
    assert_eq!(terminal.foreground().unwrap(), Some(Rgb::new(9, 9, 9)));
    assert_eq!(terminal.palette().unwrap()[3], Rgb::new(0, 0, 0));
}

#[test]
fn probe_reports_overrides_and_restores_defaults() {
    let mut terminal = terminal();
    terminal
        .set_default_foreground(Some(Rgb::new(5, 6, 7)))
        .unwrap();
    terminal.set_default_cursor(None).unwrap();
    terminal
        .write(b"\x1b]10;#050607\x07\x1b]12;#010203\x07\x1b]4;200;#000000\x07")
        .unwrap();
    let defaults = (
        terminal.default_foreground().unwrap(),
        terminal.default_background().unwrap(),
        terminal.default_cursor_color().unwrap(),
        terminal.default_palette().unwrap(),
    );
    let effective = (
        terminal.foreground().unwrap(),
        terminal.background().unwrap(),
        terminal.cursor_color().unwrap(),
        terminal.palette().unwrap(),
    );
    let overrides = terminal.probe_color_overrides().unwrap();
    assert!(overrides.foreground);
    assert!(!overrides.background);
    assert!(overrides.cursor);
    assert!(overrides.palette[200]);
    assert_eq!(overrides.palette.iter().filter(|&&set| set).count(), 1);
    assert_eq!(
        (
            terminal.default_foreground().unwrap(),
            terminal.default_background().unwrap(),
            terminal.default_cursor_color().unwrap(),
            terminal.default_palette().unwrap(),
        ),
        defaults
    );
    assert_eq!(
        (
            terminal.foreground().unwrap(),
            terminal.background().unwrap(),
            terminal.cursor_color().unwrap(),
            terminal.palette().unwrap(),
        ),
        effective
    );
}

#[test]
fn full_reset_and_ris_keep_osc_color_overrides() {
    for ris in [false, true] {
        let mut terminal = terminal();
        terminal
            .set_default_foreground(Some(Rgb::new(1, 1, 1)))
            .unwrap();
        terminal
            .write(b"\x1b]10;#0a0b0c\x07\x1b]4;1;#010203\x07")
            .unwrap();
        if ris {
            terminal.write(b"\x1bc").unwrap();
        } else {
            terminal.reset().unwrap();
        }
        assert_eq!(
            terminal.foreground().unwrap(),
            Some(Rgb::new(10, 11, 12)),
            "ris={ris}"
        );
        assert_eq!(terminal.palette().unwrap()[1], Rgb::new(1, 2, 3));
    }
}

#[test]
fn reset_clears_modes_screen_and_scrollback() {
    let mut terminal = terminal();
    terminal
        .write(b"\x1b[?2004h1\r\n2\r\n3\r\n4\x1b[?1049h")
        .unwrap();
    assert!(terminal.mode(Mode::BRACKETED_PASTE).unwrap());
    assert_eq!(terminal.screen().unwrap(), Screen::Alternate);
    terminal.reset().unwrap();
    assert!(!terminal.mode(Mode::BRACKETED_PASTE).unwrap());
    assert_eq!(terminal.screen().unwrap(), Screen::Primary);
    assert_eq!(terminal.scrollback_rows().unwrap(), 0);
}

#[test]
fn ed3_erases_scrollback_and_keeps_the_screen() {
    let mut terminal = terminal();
    terminal.write(b"1\r\n2\r\n3\r\n4\r\n5").unwrap();
    assert_eq!(terminal.scrollback_rows().unwrap(), 2);
    terminal.write(b"\x1b[3J").unwrap();
    assert_eq!(terminal.scrollback_rows().unwrap(), 0);
    let text = terminal
        .format_plain(Point::screen(0, 0), Point::screen(7, 2))
        .unwrap();
    assert_eq!(text.as_bytes(), b"3\n4\n5");
}

#[test]
fn scrollbar_offsets_are_top_based_rows() {
    let mut terminal = terminal();
    terminal.write(b"1\r\n2\r\n3\r\n4\r\n5\r\n6").unwrap();
    let live = terminal.scrollbar().unwrap();
    assert_eq!((live.total, live.offset, live.len), (6, 3, 3));
    terminal.scroll(Scroll::Row(1)).unwrap();
    assert_eq!(terminal.scrollbar().unwrap().offset, 1);
    terminal.scroll(Scroll::Delta(-5)).unwrap();
    assert_eq!(terminal.scrollbar().unwrap().offset, 0);
    terminal.scroll(Scroll::Delta(1)).unwrap();
    assert_eq!(terminal.scrollbar().unwrap().offset, 1);
    terminal.scroll(Scroll::Bottom).unwrap();
    assert_eq!(terminal.scrollbar().unwrap(), live);
    assert_eq!(terminal.total_rows().unwrap(), 6);
}

#[test]
fn render_damage_is_partial_per_row_until_cleaned() {
    let mut terminal = terminal();
    let mut render = RenderState::new().unwrap();
    terminal.write(b"a\r\nb\r\nc").unwrap();
    render.update(&mut terminal).unwrap();
    render.clean().unwrap();
    render.update(&mut terminal).unwrap();
    assert_eq!(render.dirty().unwrap(), Dirty::Clean);
    terminal.write(b"\x1b[2;1HX").unwrap();
    render.update(&mut terminal).unwrap();
    assert_eq!(render.dirty().unwrap(), Dirty::Partial);
    let mut dirty = Vec::new();
    let mut rows = render.rows().unwrap();
    while let Some(row) = rows.next() {
        dirty.push(row.dirty().unwrap());
    }
    // The written row and the row the cursor left are dirty.
    assert_eq!(dirty, [false, true, true]);
    // A palette change dirties every row.
    render.clean().unwrap();
    terminal.write(b"\x1b]4;1;#123456\x07").unwrap();
    render.update(&mut terminal).unwrap();
    assert_eq!(render.dirty().unwrap(), Dirty::Full);
}

#[test]
fn render_cells_expose_codepoints_graphemes_width_and_background_cells() {
    let mut terminal = terminal();
    terminal
        .write("a界e\u{301}\x1b[41m\x1b[K".as_bytes())
        .unwrap();
    let mut render = RenderState::new().unwrap();
    render.update(&mut terminal).unwrap();
    let mut rows = render.rows().unwrap();
    let mut row = rows.next().unwrap();
    assert!(row.row().unwrap().has_graphemes().unwrap());
    let mut cells = row.cells().unwrap();
    let mut seen = Vec::new();
    let mut text = Vec::new();
    while cells.next() {
        let cell = cells.cell().unwrap();
        cells.graphemes_utf8(&mut text).unwrap();
        seen.push((
            cell.content().unwrap(),
            cell.width().unwrap(),
            cell.codepoint().unwrap(),
            String::from_utf8(text.clone()).unwrap(),
        ));
    }
    assert_eq!(seen.len(), 8);
    assert_eq!(
        seen[0],
        (
            CellContent::Codepoint,
            CellWidth::Narrow,
            'a'.into(),
            "a".into()
        )
    );
    assert_eq!(
        seen[1],
        (
            CellContent::Codepoint,
            CellWidth::Wide,
            '界'.into(),
            "界".into()
        )
    );
    assert_eq!(seen[2].1, CellWidth::SpacerTail);
    assert_eq!(
        seen[3],
        (
            CellContent::Grapheme,
            CellWidth::Narrow,
            'e'.into(),
            "e\u{301}".into()
        )
    );
    // Erasing with a background color leaves background-only cells.
    assert_eq!(seen[4].0, CellContent::BackgroundPalette);
    assert_eq!(seen[4].2, 0);
    assert!(seen[4].3.is_empty());
}

#[test]
fn unstyled_cells_report_no_styling_and_a_default_style() {
    let mut terminal = terminal();
    terminal.write(b"a\x1b[1;31mb").unwrap();
    let mut render = RenderState::new().unwrap();
    render.update(&mut terminal).unwrap();
    let mut rows = render.rows().unwrap();
    let mut row = rows.next().unwrap();
    assert!(row.row().unwrap().has_styles().unwrap());
    let mut cells = row.cells().unwrap();
    assert!(cells.next());
    assert!(!cells.cell().unwrap().has_styling().unwrap());
    assert_eq!(cells.style().unwrap(), crate::Style::default());
    assert!(cells.next());
    assert!(cells.cell().unwrap().has_styling().unwrap());
    let style = cells.style().unwrap();
    assert!(style.bold);
    assert_eq!(style.foreground, crate::StyleColor::Palette(1));
}

#[test]
fn grid_refs_report_hyperlink_and_grapheme_buffer_sizes() {
    let mut terminal = terminal();
    terminal
        .write("\x1b]8;;https://a.test\x1b\\l\x1b]8;;\x1b\\e\u{301}".as_bytes())
        .unwrap();
    let link = terminal.grid_ref(Point::active(0, 0)).unwrap();
    assert!(link.cell().unwrap().has_hyperlink().unwrap());
    assert_eq!(link.hyperlink_uri(&mut []).unwrap(), Fill::TooSmall(14));
    let mut uri = [0; 14];
    assert_eq!(link.hyperlink_uri(&mut uri).unwrap(), Fill::Written(14));
    assert_eq!(&uri, b"https://a.test");
    let plain = terminal.grid_ref(Point::viewport(1, 0)).unwrap();
    assert_eq!(plain.hyperlink_uri(&mut uri).unwrap(), Fill::Written(0));
    let mut codepoints = [0; 1];
    assert_eq!(plain.graphemes(&mut codepoints).unwrap(), Fill::TooSmall(2));
    let blank = terminal.grid_ref(Point::screen(5, 0)).unwrap();
    assert_eq!(blank.graphemes(&mut codepoints).unwrap(), Fill::Written(0));
    assert!(
        !terminal
            .grid_ref(Point::screen(0, 0))
            .unwrap()
            .row()
            .unwrap()
            .wrapped()
            .unwrap()
    );
    assert_eq!(
        terminal.grid_ref(Point::screen(8, 0)).unwrap_err(),
        Error::InvalidValue
    );
}

#[test]
fn mouse_encoder_reports_synthetic_geometry_cells() {
    let mut terminal = terminal();
    terminal.write(b"\x1b[?1000h\x1b[?1006h").unwrap();
    let mut probe = MouseProbe::new().unwrap();
    probe.sync(&terminal).unwrap();
    assert_eq!(
        probe.report(ProbeEvent::LeftPress).unwrap(),
        b"\x1b[<0;101;101M"
    );
    assert_eq!(probe.report(ProbeEvent::Motion).unwrap(), b"");
}

#[test]
fn mouse_probe_reads_active_modes_without_pty_output() {
    let mut terminal = terminal();
    let mut probe = MouseProbe::new().unwrap();
    for (sequence, expected) in [
        (&b""[..], (ProbedTracking::Disabled, ProbedFormat::Legacy)),
        (
            b"\x1b[?1000h",
            (ProbedTracking::Buttons, ProbedFormat::Legacy),
        ),
        (
            b"\x1b[?1005h",
            (ProbedTracking::Buttons, ProbedFormat::Utf8),
        ),
        (b"\x1b[?1006h", (ProbedTracking::Buttons, ProbedFormat::Sgr)),
        (
            b"\x1b[?1002h",
            (ProbedTracking::ButtonMotion, ProbedFormat::Sgr),
        ),
        (
            b"\x1b[?1003h",
            (ProbedTracking::AllMotion, ProbedFormat::Sgr),
        ),
        // Disabling the inactive format still resets to legacy encoding.
        (
            b"\x1b[?1005l",
            (ProbedTracking::AllMotion, ProbedFormat::Legacy),
        ),
    ] {
        terminal.write(sequence).unwrap();
        assert_eq!(probe.probe(&terminal).unwrap(), expected, "{sequence:?}");
    }
    assert!(effects(&mut terminal).is_empty());
}

#[test]
fn panicking_host_callbacks_poison_the_terminal_without_aborting() {
    let mut terminal = terminal();
    terminal.host_mut().panic_on_clipboard = true;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = terminal.write(b"\x1b]52;c;YQ==\x07\x1b]2;after\x07");
    std::panic::set_hook(previous);
    assert_eq!(result, Err(Error::Poisoned));
    assert!(terminal.is_poisoned());
    assert_eq!(terminal.write(b"x"), Err(Error::Poisoned));
    assert_eq!(terminal.title().unwrap_err(), Error::Poisoned);
    let mut render = RenderState::new().unwrap();
    assert_eq!(render.update(&mut terminal), Err(Error::Poisoned));
}

/// A panic payload whose destructor panics too.
struct PanicsOnDrop;

impl Drop for PanicsOnDrop {
    fn drop(&mut self) {
        panic!("panic payload destructor panicked");
    }
}

/// Panics with a [`PanicsOnDrop`] payload when asked for a color scheme.
struct PanicsWithBadPayload;

impl Host for PanicsWithBadPayload {
    fn color_scheme(&mut self, _background: Rgb) -> Option<ColorScheme> {
        std::panic::panic_any(PanicsOnDrop)
    }
}

#[test]
fn panic_payloads_with_panicking_destructors_still_poison() {
    let mut terminal = Terminal::new(options(), PanicsWithBadPayload).unwrap();
    terminal
        .set_default_background(Some(Rgb::new(1, 2, 3)))
        .unwrap();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    // Without containment, the destructor's panic unwinds into C and the
    // process aborts here.
    let result = terminal.write(b"\x1b[?996n");
    std::panic::set_hook(previous);
    assert_eq!(result, Err(Error::Poisoned));
    assert!(terminal.is_poisoned());
}

#[test]
fn every_native_allocation_is_released() {
    let (counters, allocator) = test_alloc::tracking();
    {
        let mut terminal =
            Terminal::new_in(options(), Recorder::default(), Some(allocator))
                .unwrap();
        terminal.disable_extensions().unwrap();
        terminal.set_scrollback_bytes(Some(1 << 20)).unwrap();
        let mut render = RenderState::new_in(Some(allocator)).unwrap();
        let mut probe = MouseProbe::new_in(Some(allocator)).unwrap();
        for round in 0..20 {
            terminal
                .write(format!("\x1b[3{}mline {round} 界e\u{301}\r\n\x1b]8;;https://a.test/{round}\x07l\x1b]8;;\x07\x1b]2;t{round}\x07\x1b]52;c;YQ==\x07", round % 8).as_bytes())
                .unwrap();
            render.update(&mut terminal).unwrap();
            let mut text = Vec::new();
            let mut rows = render.rows().unwrap();
            while let Some(mut row) = rows.next() {
                let mut cells = row.cells().unwrap();
                while cells.next() {
                    let _ = cells.style().unwrap();
                    cells.graphemes_utf8(&mut text).unwrap();
                }
            }
            render.clean().unwrap();
            let _ = probe.probe(&terminal).unwrap();
            let _ = terminal.probe_color_overrides().unwrap();
            let output = terminal
                .format_plain(Point::screen(0, 0), Point::screen(7, 2))
                .unwrap();
            assert!(!output.as_bytes().is_empty());
        }
        terminal.resize(20, 6, 8, 16).unwrap();
        terminal.reset().unwrap();
        let _ = replies(&mut terminal);
        assert!(counters.live() > 0);
    }
    assert!(counters.total() > 100, "{} allocations", counters.total());
    // The vtable receives log2 alignments (0 for byte alignment), not the
    // byte counts allocator.h describes.
    let alignments = counters.alignment_arguments();
    assert!(alignments.contains(&0), "{alignments:?}");
    assert!(
        alignments.iter().all(|&alignment| alignment <= 4),
        "{alignments:?}"
    );
    assert_eq!(
        (counters.live(), counters.live_bytes()),
        (0, 0),
        "native allocations leaked"
    );
}
