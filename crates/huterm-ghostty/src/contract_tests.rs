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
    /// `(granted, can_remember)` for each clipboard write.
    clipboard_grants: Vec<(bool, bool)>,
}

impl Host for Recorder {
    fn clipboard_write(
        &mut self,
        request: &ClipboardWrite<'_>,
    ) -> ClipboardWriteResult {
        assert!(!self.panic_on_clipboard, "host clipboard callback panicked");
        self.clipboard_grants
            .push((request.granted, request.can_remember));
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
    assert_eq!(output.as_bytes(), b"");
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
    assert_eq!(terminal.title().unwrap(), b"");
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
    assert_eq!(terminal.pwd().unwrap(), b"");
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
    assert_eq!(
        written(&mut terminal, b"\x1b[14t\x1b[18t"),
        Vec::<Vec<u8>>::new()
    );
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
    assert_eq!(replies(&mut terminal), Vec::<Vec<u8>>::new());
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

/// A Kitty write of plain text and HTML, opened with `metadata`.
fn kitty_write(metadata: &str) -> Vec<u8> {
    format!(
        "\x1b]5522;type=write{metadata}\x1b\\\
         \x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;R2hvc3Q=\x1b\\\
         \x1b]5522;type=wdata:mime=dGV4dC9odG1s;PGI+RzwvYj4=\x1b\\\
         \x1b]5522;type=wdata\x1b\\"
    )
    .into_bytes()
}

#[test]
fn kitty_clipboard_writes_deliver_every_representation_name_and_password() {
    let mut terminal = terminal();
    terminal.disable_apc_protocols().unwrap();
    // Ghostty's default Kitty write limit, which Huterm keeps.
    assert_eq!(terminal.clipboard_write_limit().unwrap(), 64 * 1024 * 1024);
    let named = ":name=ZWRpdG9y:pw=cHc=";
    assert_eq!(
        written(&mut terminal, &kitty_write(&format!(":id=a1{named}"))),
        [b"\x1b]5522;type=write:status=DONE:id=a1\x1b\\".to_vec()]
    );
    // A successful reply never records a grant, so the repeat is not granted.
    written(&mut terminal, &kitty_write(named));
    // A password without a name is dropped; `loc=primary` is the only other
    // location.
    written(&mut terminal, &kitty_write(":pw=cHc=:loc=primary"));
    let contents = vec![
        (b"text/plain".to_vec(), b"Ghost".to_vec()),
        (b"text/html".to_vec(), b"<b>G</b>".to_vec()),
    ];
    let host = terminal.host_mut();
    assert_eq!(
        host.clipboard,
        [
            (
                ClipboardLocation::Standard,
                contents.clone(),
                b"editor".to_vec()
            ),
            (
                ClipboardLocation::Standard,
                contents.clone(),
                b"editor".to_vec()
            ),
            (ClipboardLocation::Primary, contents, Vec::new()),
        ]
    );
    assert_eq!(
        host.clipboard_grants,
        [(false, true), (false, true), (false, false)]
    );
}

#[test]
fn kitty_clipboard_repeats_and_aliases_keep_first_arrival_order() {
    let mut terminal = terminal();
    let packet = |metadata: &str, payload: &str| {
        format!("\x1b]5522;{metadata};{payload}\x1b\\")
    };
    let write = [
        "\x1b]5522;type=write\x1b\\".to_owned(),
        // text/plain "A", text/html "H", then text/plain again as "B".
        packet("type=wdata:mime=dGV4dC9wbGFpbg==", "QQ=="),
        packet("type=wdata:mime=dGV4dC9odG1s", "SA=="),
        packet("type=wdata:mime=dGV4dC9wbGFpbg==", "Qg=="),
        // text/plain also names text/html and the new text/x-new.
        packet(
            "type=walias:mime=dGV4dC9wbGFpbg==",
            "dGV4dC9odG1sIHRleHQveC1uZXc=",
        ),
        // An alias of a type that carries no data is dropped.
        packet("type=walias:mime=dGV4dC9taXNzaW5n", "aW1hZ2UvcG5n"),
        "\x1b]5522;type=wdata\x1b\\".to_owned(),
    ]
    .concat();
    assert_eq!(
        written(&mut terminal, write.as_bytes()),
        [b"\x1b]5522;type=write:status=DONE\x1b\\".to_vec()]
    );
    assert_eq!(
        terminal.host_mut().clipboard,
        [(
            ClipboardLocation::Standard,
            vec![
                (b"text/plain".to_vec(), b"B".to_vec()),
                (b"text/html".to_vec(), b"B".to_vec()),
                (b"text/x-new".to_vec(), b"B".to_vec()),
            ],
            Vec::new()
        )]
    );
}

#[test]
fn kitty_clipboard_write_limit_counts_every_representation() {
    let mut terminal = terminal();
    let reply = |status: &str| {
        vec![format!("\x1b]5522;type=write:status={status}\x1b\\").into_bytes()]
    };
    // `kitty_write` carries 5 bytes of text and 8 of HTML.
    for (limit, effective, status) in [
        (Some(13), 13, "DONE"),
        (Some(12), 12, "EFBIG"),
        (Some(0), 0, "EFBIG"),
        (Some(usize::MAX), usize::MAX, "DONE"),
        (None, 64 * 1024 * 1024, "DONE"),
    ] {
        terminal.set_clipboard_write_limit(limit).unwrap();
        assert_eq!(terminal.clipboard_write_limit().unwrap(), effective);
        assert_eq!(
            written(&mut terminal, &kitty_write("")),
            reply(status),
            "{limit:?}"
        );
    }
    // A zero limit still lets an empty commit clear.
    terminal.set_clipboard_write_limit(Some(0)).unwrap();
    assert_eq!(
        written(
            &mut terminal,
            b"\x1b]5522;type=write\x1b\\\x1b]5522;type=wdata\x1b\\"
        ),
        reply("DONE")
    );
    assert_eq!(terminal.host_mut().clipboard.len(), 4);
}

#[test]
fn kitty_clipboard_status_follows_the_host_answer() {
    let mut terminal = terminal();
    terminal.disable_apc_protocols().unwrap();
    for (result, status) in [
        (ClipboardWriteResult::Success, "DONE"),
        (ClipboardWriteResult::Denied, "EPERM"),
        (ClipboardWriteResult::Unsupported, "ENOSYS"),
        (ClipboardWriteResult::Busy, "EBUSY"),
        (ClipboardWriteResult::InvalidData, "EINVAL"),
        (ClipboardWriteResult::IoError, "EIO"),
    ] {
        terminal.host_mut().clipboard_result = Some(result);
        assert_eq!(
            written(&mut terminal, &kitty_write(":id=s")),
            [format!("\x1b]5522;type=write:status={status}:id=s\x1b\\")
                .into_bytes()],
            "{result:?}"
        );
    }
}

#[test]
fn kitty_clipboard_commit_without_data_reaches_the_callback_as_a_clear() {
    let mut terminal = terminal();
    terminal.disable_apc_protocols().unwrap();
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
    assert_eq!(
        written(&mut terminal, b"\x1b]52;c;?\x07"),
        Vec::<Vec<u8>>::new()
    );
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
    assert_ne!(written(&mut terminal, GLYPH), Vec::<Vec<u8>>::new());
    assert_ne!(written(&mut terminal, IMAGE), Vec::<Vec<u8>>::new());
    terminal.disable_apc_protocols().unwrap();
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
    terminal.disable_apc_protocols().unwrap();
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
fn can_and_sub_cancel_a_color_osc() {
    for cancel in [0x18, 0x1a] {
        let mut terminal = terminal();
        terminal
            .set_default_foreground(Some(Rgb::new(1, 1, 1)))
            .unwrap();
        let palette = terminal.palette().unwrap();
        let mut bytes = b"\x1b]10;#0a0b0c".to_vec();
        bytes.push(cancel);
        bytes.extend_from_slice(b"\x1b]4;1;#010203");
        bytes.push(cancel);
        terminal.write(&bytes).unwrap();
        assert_eq!(
            (
                terminal.foreground().unwrap(),
                terminal.palette().unwrap()[1]
            ),
            (Some(Rgb::new(1, 1, 1)), palette[1]),
            "cancel={cancel:#x}"
        );
    }
}

#[test]
fn full_reset_and_ris_reset_the_palette_but_keep_dynamic_colors() {
    let default_palette = terminal().palette().unwrap();
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
        assert_eq!(
            terminal.palette().unwrap()[1],
            default_palette[1],
            "ris={ris}"
        );
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

/// A four-row terminal of `columns` limited only by physical scrollback
/// rows; `None` keeps everything.
fn row_limited(columns: u16, lines: Option<usize>) -> Terminal<Recorder> {
    let mut terminal = Terminal::new(
        Options {
            columns,
            rows: 4,
            ..options()
        },
        Recorder::default(),
    )
    .unwrap();
    // The C default is a 10,000-byte budget, which would bind first.
    terminal.set_scrollback_bytes(None).unwrap();
    terminal.set_scrollback_lines(lines).unwrap();
    terminal
}

fn write_lines<H: Host>(terminal: &mut Terminal<H>, count: usize) {
    for line in 0..count {
        terminal
            .write(format!("line {line}\r\n").as_bytes())
            .unwrap();
    }
}

/// Rows in one page of plain text at this width: under a one-row limit,
/// history grows to a full page and then drops when that page is pruned.
fn page_rows(columns: u16) -> usize {
    let mut terminal = row_limited(columns, Some(1));
    let mut peak = 0;
    loop {
        terminal.write(b"x\r\n").unwrap();
        let history = terminal.scrollback_rows().unwrap();
        if history < peak {
            return peak;
        }
        peak = history;
    }
}

/// Rows between the viewport's bottom and the live bottom.
fn bottom_offset<H>(terminal: &Terminal<H>) -> u64 {
    let bar = terminal.scrollbar().unwrap();
    bar.total - bar.offset - bar.len
}

fn viewport_top_text<H>(terminal: &Terminal<H>) -> Vec<u8> {
    terminal
        .format_plain(Point::viewport(0, 0), Point::viewport(79, 0))
        .unwrap()
        .as_bytes()
        .to_vec()
}

#[test]
fn line_limits_prune_whole_pages_and_keep_at_least_one_page() {
    let page = page_rows(80);
    // The configuration reference quotes this figure.
    assert!(
        (500..=650).contains(&page),
        "80-column page holds {page} rows"
    );
    let wide = page_rows(200);
    assert!(
        (200..=260).contains(&wide),
        "200-column page holds {wide} rows"
    );

    // A larger limit is a ceiling, and whole-page pruning falls short of
    // it by less than one page.
    let limit = page * 3 + page / 2;
    let mut terminal = row_limited(80, Some(limit));
    let (mut peak, mut floor, mut previous) = (0, usize::MAX, 0);
    for _ in 0..limit * 3 {
        terminal.write(b"x\r\n").unwrap();
        let history = terminal.scrollback_rows().unwrap();
        peak = peak.max(history);
        if history < previous || floor != usize::MAX {
            floor = floor.min(history);
        }
        previous = history;
    }
    assert_eq!(peak, limit);
    assert_eq!(floor, limit + 1 - page);
    assert_eq!(terminal.scrollback_lines().unwrap(), Some(limit));
    assert_eq!(terminal.scrollback_bytes().unwrap(), None);
}

#[test]
fn the_first_reached_of_the_line_and_byte_limits_applies() {
    let page = page_rows(80);
    let peak = |terminal: &mut Terminal<Recorder>| {
        let mut peak = 0;
        for _ in 0..page * 8 {
            terminal.write(b"x\r\n").unwrap();
            peak = peak.max(terminal.scrollback_rows().unwrap());
        }
        peak
    };

    // Two mebibytes is about five pages, far below eight pages of lines.
    let mut by_bytes = row_limited(80, Some(page * 8));
    by_bytes.set_scrollback_bytes(Some(2 << 20)).unwrap();
    let kept = peak(&mut by_bytes);
    assert!(kept < page * 6, "kept {kept} rows under the byte limit");

    // A byte limit that is never reached leaves the line limit to bind.
    let mut by_lines = row_limited(80, Some(page * 2));
    by_lines.set_scrollback_bytes(Some(1 << 30)).unwrap();
    assert_eq!(peak(&mut by_lines), page * 2);
}

#[test]
fn lowering_the_line_limit_evicts_at_once_and_fixes_up_the_viewport() {
    let page = page_rows(80);
    let mut terminal = row_limited(80, None);
    write_lines(&mut terminal, page * 4);
    assert!(terminal.scrollback_rows().unwrap() > page * 3);
    terminal
        .scroll(Scroll::Delta(-isize::try_from(page).unwrap()))
        .unwrap();
    let shown = viewport_top_text(&terminal);
    assert_eq!(bottom_offset(&terminal), page as u64);

    // The viewport's page survives: it keeps its rows and its distance
    // from live output.
    terminal.set_scrollback_lines(Some(page * 2)).unwrap();
    let history = terminal.scrollback_rows().unwrap();
    assert!(history <= page * 2 && history > page, "history {history}");
    assert_eq!(bottom_offset(&terminal), page as u64);
    assert_eq!(viewport_top_text(&terminal), shown);

    // Scrolled to the top, the viewport's page is evicted and the
    // viewport moves to the new top.
    terminal.scroll(Scroll::Top).unwrap();
    terminal.set_scrollback_lines(Some(1)).unwrap();
    let history = terminal.scrollback_rows().unwrap();
    assert!(history <= page, "history {history}");
    assert_eq!(bottom_offset(&terminal), history as u64);
}

#[test]
fn zero_bytes_disables_scrollback_until_the_byte_limit_is_lifted() {
    let mut terminal = row_limited(80, Some(1000));
    write_lines(&mut terminal, 20);
    assert_eq!(terminal.scrollback_rows().unwrap(), 17);

    terminal.set_scrollback_bytes(Some(0)).unwrap();
    assert_eq!(terminal.scrollback_rows().unwrap(), 0);
    write_lines(&mut terminal, 20);
    assert_eq!(terminal.scrollback_rows().unwrap(), 0);

    terminal.set_scrollback_bytes(None).unwrap();
    write_lines(&mut terminal, 20);
    assert_eq!(terminal.scrollback_rows().unwrap(), 20);
}

#[test]
fn line_limits_reach_the_primary_screen_and_survive_resets() {
    let page = page_rows(80);
    let mut terminal = row_limited(80, None);
    write_lines(&mut terminal, page * 3);

    // Lowered on the alternate screen, the limit prunes primary history.
    terminal.write(b"\x1b[?1049h").unwrap();
    assert_eq!(terminal.scrollback_rows().unwrap(), 0);
    terminal.set_scrollback_lines(Some(page)).unwrap();
    terminal.write(b"\x1b[?1049l").unwrap();
    assert!(terminal.scrollback_rows().unwrap() <= page);

    // RIS and ED 3 keep both limits.
    terminal.reset().unwrap();
    terminal.write(b"\x1b[3J").unwrap();
    assert_eq!(terminal.scrollback_lines().unwrap(), Some(page));
    assert_eq!(terminal.scrollback_bytes().unwrap(), None);
    write_lines(&mut terminal, page * 3);
    assert!(terminal.scrollback_rows().unwrap() <= page);
}

#[test]
fn narrowing_reflows_history_within_the_line_limit() {
    let page = page_rows(80);
    let limit = page * 2;
    let mut terminal = row_limited(80, Some(limit));
    let line = format!("{}\r\n", "w".repeat(60));
    for _ in 0..limit {
        terminal.write(line.as_bytes()).unwrap();
    }
    assert!(terminal.scrollback_rows().unwrap() <= limit);

    // Each line now wraps onto two rows; the oldest pages go.
    terminal.resize(40, 4, 9, 17).unwrap();
    let history = terminal.scrollback_rows().unwrap();
    assert!(
        history <= limit.max(page_rows(40)),
        "history {history} after narrowing"
    );
    assert!(history > 0);
}

/// The page node holding the active area's row `y`.
fn active_page<H>(terminal: &Terminal<H>, y: u32) -> *mut std::ffi::c_void {
    terminal.grid_ref(Point::active(0, y)).unwrap().position().0
}

/// GHSA-w9q4-5jvv-mcv4: a cursor style or hyperlink is an ID into its page's
/// sets. When ED 22 scrolls the screen into history and the scrollback limit
/// then prunes the cursor's page, the cursor must register both again on the
/// page it moves to; before Ghostty 1.4.0 it kept the freed page's IDs.
/// Huterm's smallest nonzero history limit sets one line. Its zero-row limit
/// sets zero bytes, which still lets ED 22 grow history, and prunes the
/// cursor's page once the screen spans pages, as a 400 by 200 one does.
#[test]
fn scroll_clear_keeps_the_cursor_style_and_link_when_its_page_is_pruned() {
    let limits = [(8, 3, Some(1), None), (400, 200, None, Some(0))];
    let attributes =
        [("\x1b[1m", true), ("\x1b]8;;https://a.test/\x1b\\", false)];
    for (columns, rows, lines, bytes) in limits {
        for (start, styled) in attributes {
            let case = format!(
                "{columns}x{rows}, lines {lines:?}, bytes {bytes:?}, {start:?}"
            );
            let mut terminal = Terminal::new(
                Options {
                    columns,
                    rows,
                    ..options()
                },
                Recorder::default(),
            )
            .unwrap();
            terminal.set_scrollback_bytes(bytes).unwrap();
            terminal.set_scrollback_lines(lines).unwrap();
            // Every row has text, so ED 22 scrolls the whole screen. Write
            // until the screen spans two pages, its top row on the first.
            terminal.write(b"x").unwrap();
            for _ in 1..rows {
                terminal.write(b"\r\nx").unwrap();
            }
            let bottom = u32::from(rows - 1);
            let mut written = 0;
            while active_page(&terminal, 0) == active_page(&terminal, bottom) {
                assert!(written < 100_000, "{case}: never spanned two pages");
                terminal.write(b"\r\nx").unwrap();
                written += 1;
            }
            let first_page = active_page(&terminal, 0);
            let history = terminal.scrollback_rows().unwrap();

            terminal
                .write(format!("\x1b[H{start}\x1b[22Jy").as_bytes())
                .unwrap();
            // Without pruning, history would gain every screen row.
            let pruned = terminal.scrollback_rows().unwrap();
            assert!(
                pruned < history + usize::from(rows),
                "{case}: history went from {history} to {pruned} rows"
            );
            assert_ne!(active_page(&terminal, 0), first_page, "{case}");
            let cell = terminal.grid_ref(Point::active(0, 0)).unwrap();
            let mut text = [0; 1];
            assert_eq!(cell.graphemes(&mut text).unwrap(), Fill::Written(1));
            assert_eq!(text[0], u32::from('y'), "{case}");
            let mut uri = [0; 32];
            let Fill::Written(uri_len) = cell.hyperlink_uri(&mut uri).unwrap()
            else {
                panic!("hyperlink buffer too small");
            };
            let expected_uri: &[u8] =
                if styled { b"" } else { b"https://a.test/" };
            assert_eq!(&uri[..uri_len], expected_uri, "{case}");

            let mut render = RenderState::new().unwrap();
            render.update(&mut terminal).unwrap();
            let mut render_rows = render.rows().unwrap();
            let mut row = render_rows.next().unwrap();
            let mut cells = row.cells().unwrap();
            assert!(cells.next());
            assert_eq!(cells.style().unwrap().bold, styled, "{case}");
        }
    }
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
    assert_eq!(seen[4].3, "");
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
    assert_eq!(effects(&mut terminal), []);
}

#[test]
fn panicking_host_callbacks_poison_the_terminal_without_aborting() {
    let mut terminal = terminal();
    terminal.host_mut().panic_on_clipboard = true;
    // The callback runs on this thread, so the test harness captures its
    // panic message.
    let result = terminal.write(b"\x1b]52;c;YQ==\x07\x1b]2;after\x07");
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

/// Set in the child process that runs the payload-destructor scenario.
const PANIC_CHILD: &str = "HUTERM_GHOSTTY_PANIC_PAYLOAD_CHILD";

#[test]
fn panic_payloads_with_panicking_destructors_still_poison() {
    const NAME: &str = "contract_tests::panic_payloads_with_panicking_destructors_still_poison";
    const POISONED: &str = "panic payload scenario: terminal poisoned";
    if std::env::var_os(PANIC_CHILD).is_some() {
        // Without containment, the destructor's panic unwinds into C and
        // aborts this child process.
        let mut terminal =
            Terminal::new(options(), PanicsWithBadPayload).unwrap();
        terminal
            .set_default_background(Some(Rgb::new(1, 2, 3)))
            .unwrap();
        assert_eq!(terminal.write(b"\x1b[?996n"), Err(Error::Poisoned));
        assert!(terminal.is_poisoned());
        println!("{POISONED}");
        return;
    }
    // A regression aborts the process, so run the scenario in a child and
    // report its outcome under this test's name.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
        .env(PANIC_CHILD, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains(POISONED),
        "the scenario did not poison the terminal and exit cleanly ({}):\n\
         {stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn every_native_allocation_is_released() {
    let (counters, allocator) = test_alloc::tracking();
    {
        let mut terminal =
            Terminal::new_in(options(), Recorder::default(), Some(allocator))
                .unwrap();
        terminal.disable_apc_protocols().unwrap();
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
            assert_ne!(output.as_bytes(), b"");
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
