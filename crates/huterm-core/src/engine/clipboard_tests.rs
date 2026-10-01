use super::*;
use huterm_protocol::HostEffect;

fn engine_with_recipient()
-> (TerminalEngine, crate::host_effects::HostEffectRecipient) {
    let terminal_id = TerminalId::new(1);
    let (sink, recipient) = crate::host_effects::test_fixture(terminal_id);
    let mut engine = TerminalEngine::new(
        terminal_id,
        GridSize::clamped(8, 3),
        CellSize {
            width: 8,
            height: 16,
        },
        TerminalPresentation::default(),
    )
    .unwrap();
    engine.set_host_effect_sink(sink);
    (engine, recipient)
}

fn drain_text(
    recipient: &crate::host_effects::HostEffectRecipient,
) -> Vec<String> {
    let mut text = Vec::new();
    while let Some(pending) = recipient.try_next() {
        match pending.effect() {
            HostEffect::ClipboardWrite(write) => {
                text.push(write.text().to_owned());
            }
            _ => panic!("unexpected host effect"),
        }
    }
    text
}

fn with_engine(
    mut test: impl FnMut(
        &mut TerminalEngine,
        &crate::host_effects::HostEffectRecipient,
    ),
) {
    let (mut engine, recipient) = engine_with_recipient();
    test(&mut engine, &recipient);
}

#[test]
fn osc52_writes_preserve_order_unicode_empty_and_nul() {
    with_engine(|engine, recipient| {
        engine
            .process(
                b"\x1b]52;;dG11eA==\x07\x1b]52;c;\x07\x1b]52;c;YQBi\x1b\\\x1b]52;c;5LiW55WM8J+Zgg==\x07",
            )
            .unwrap();
        assert_eq!(drain_text(recipient), ["tmux", "", "a\0b", "世界🙂"]);
    });
}

#[test]
fn osc52_bel_and_st_complete_at_every_read_split() {
    for terminator in ["\x07", "\x1b\\"] {
        let sequence = format!("\x1b]52;c;5LiW55WM8J+Zgg=={terminator}");
        for split in 0..=sequence.len() {
            with_engine(|engine, recipient| {
                engine.process(&sequence.as_bytes()[..split]).unwrap();
                engine.process(&sequence.as_bytes()[split..]).unwrap();
                assert_eq!(
                    drain_text(recipient),
                    ["世界🙂"],
                    "split={split} terminator={terminator:?}"
                );
            });
        }
        with_engine(|engine, recipient| {
            for byte in sequence.as_bytes() {
                engine.process(std::slice::from_ref(byte)).unwrap();
            }
            assert_eq!(
                drain_text(recipient),
                ["世界🙂"],
                "byte-at-a-time terminator={terminator:?}"
            );
        });
    }
}

#[test]
fn osc52_reads_selection_and_invalid_utf8_are_ignored() {
    with_engine(|engine, recipient| {
        let effects = engine
            .process(
                b"\x1b]52;c;?\x07\x1b]52;p;c2VsZWN0aW9u\x07\x1b]52;c;/w==\x07",
            )
            .unwrap();
        assert!(recipient.try_next().is_none());
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, EngineEffect::PtyWrite(_))),
            "Ghostty replied to a clipboard read"
        );
    });
}

#[test]
fn stalled_recipient_keeps_only_the_terminal_effect_budget() {
    with_engine(|engine, recipient| {
        let mut input = Vec::new();
        for _ in 0..100 {
            input.extend_from_slice(b"\x1b]52;c;dmFsdWU=\x07");
        }
        input.extend_from_slice(b"\x1b]2;still-running\x07");
        let effects = engine.process(&input).unwrap();
        assert_eq!(drain_text(recipient).len(), 8);
        assert!(
            effects.iter().any(|effect| {
                matches!(effect, EngineEffect::Title(title) if title == "still-running")
            }),
            "Ghostty did not process later non-clipboard output"
        );
    });
}

#[test]
fn ghostty_keeps_osc1337_copy_support() {
    let (mut engine, recipient) = engine_with_recipient();
    engine.process(b"\x1b]1337;Copy=:aHV0ZXJt\x07").unwrap();
    assert_eq!(drain_text(&recipient), ["huterm"]);
}

#[test]
fn osc52_preserves_upstream_selector_and_malformed_input_behavior() {
    for (name, sequence, expected) in [
        ("malformed", b"\x1b]52;c;!!!!\x07".as_slice(), None),
        ("extra", b"\x1b]52;c;Zm9v;extra\x07", None),
        ("selector-list", b"\x1b]52;c,p;Zm9v\x07", None),
        ("unknown-selector", b"\x1b]52;x;Zm9v\x07", Some("foo")),
    ] {
        with_engine(|engine, recipient| {
            engine.process(sequence).unwrap();
            assert_eq!(
                drain_text(recipient),
                expected.into_iter().collect::<Vec<_>>(),
                "{name}"
            );
        });
    }
}

#[test]
fn osc52_can_and_sub_finish_the_valid_prefix_once() {
    for cancellation in ['\x18', '\x1a'] {
        with_engine(|engine, recipient| {
            let sequence =
                format!("\x1b]52;c;Zm9v{cancellation}\x07\x1b]52;c;YmFy\x07");
            engine.process(sequence.as_bytes()).unwrap();
            assert_eq!(drain_text(recipient), ["foo", "bar"]);
        });
    }
}

/// Standard base64, as OSC 5522 carries payloads and metadata values.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let word = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |word, (index, &byte)| {
                word | u32::from(byte) << (16 - 8 * index)
            });
        for index in 0..4 {
            out.push(if index <= chunk.len() {
                char::from(ALPHABET[(word >> (18 - 6 * index)) as usize & 63])
            } else {
                '='
            });
        }
    }
    out
}

/// A complete Kitty clipboard write: the opening packet with
/// `metadata`, one `wdata` packet per representation, and the commit.
fn kitty_write(metadata: &str, contents: &[(&str, &[u8])]) -> Vec<u8> {
    use std::fmt::Write as _;

    let mut out = format!("\x1b]5522;type=write{metadata}\x1b\\");
    for (mime, data) in contents {
        let _ = write!(
            out,
            "\x1b]5522;type=wdata:mime={};{}\x1b\\",
            base64(mime.as_bytes()),
            base64(data)
        );
    }
    out.push_str("\x1b]5522;type=wdata\x1b\\");
    out.into_bytes()
}

/// The Kitty status a write received.
fn status(effects: Vec<EngineEffect>) -> String {
    let replies: Vec<_> = effects
        .into_iter()
        .filter_map(|effect| match effect {
            EngineEffect::PtyWrite(bytes) => {
                Some(String::from_utf8(bytes).unwrap())
            }
            _ => None,
        })
        .collect();
    match replies.as_slice() {
        [reply] => reply
            .strip_prefix("\x1b]5522;type=write:status=")
            .and_then(|reply| reply.strip_suffix("\x1b\\"))
            .unwrap_or_else(|| panic!("unexpected reply {reply:?}"))
            .to_owned(),
        other => panic!("expected one Kitty reply, got {other:?}"),
    }
}

fn with_policy(
    clipboard_allowed: bool,
    mut test: impl FnMut(
        &mut TerminalEngine,
        &crate::host_effects::HostEffectRecipient,
    ),
) {
    let terminal_id = TerminalId::new(1);
    let (sink, recipient) = crate::host_effects::test_fixture_with_clipboard(
        terminal_id,
        clipboard_allowed,
    );
    let mut engine = TerminalEngine::new(
        terminal_id,
        GridSize::clamped(8, 3),
        CellSize {
            width: 8,
            height: 16,
        },
        TerminalPresentation::default(),
    )
    .unwrap();
    engine.set_host_effect_sink(sink);
    test(&mut engine, &recipient);
}

#[test]
fn kitty_text_writes_follow_the_clipboard_policy() {
    let write = kitty_write("", &[("text/plain", "héllo".as_bytes())]);
    // OSC 52 carries the same text and follows the same policy, but has
    // no acknowledgement.
    let osc52 = b"\x1b]52;c;aMOpbGxv\x07";
    with_policy(true, |engine, recipient| {
        assert_eq!(status(engine.process(&write).unwrap()), "DONE");
        assert!(engine.process(osc52).unwrap().is_empty());
        assert_eq!(drain_text(recipient), ["héllo", "héllo"]);
    });
    with_policy(false, |engine, recipient| {
        assert_eq!(status(engine.process(&write).unwrap()), "EPERM");
        assert!(engine.process(osc52).unwrap().is_empty());
        assert!(drain_text(recipient).is_empty());
    });
}

#[test]
fn kitty_writes_admit_only_the_first_text_representation() {
    with_engine(|engine, recipient| {
        let write = kitty_write(
            "",
            &[
                ("text/html", b"<b>bold</b>"),
                ("text/plain; charset=latin1", b"latin"),
                ("text/plain; x=\"a;charset=q\"; charset=utf-8", b"bold"),
                ("TEXT/PLAIN ; format=flowed; charset=\"UTF-8\"", b"later"),
                ("text/plain", b"second"),
                ("image/png", &[0x89, b'P', b'N', b'G']),
            ],
        );
        assert_eq!(status(engine.process(&write).unwrap()), "DONE");
        assert_eq!(drain_text(recipient), ["bold"]);
    });
}

#[test]
fn kitty_writes_without_usable_text_are_refused() {
    with_engine(|engine, recipient| {
        let image = kitty_write("", &[("image/png", b"png")]);
        assert_eq!(status(engine.process(&image).unwrap()), "ENOSYS");
        // Parameters other than the UTF-8 charset are not plain text.
        let latin1 =
            kitty_write("", &[("text/plain;charset=iso-8859-1", b"caf\xe9")]);
        assert_eq!(status(engine.process(&latin1).unwrap()), "ENOSYS");
        let invalid = kitty_write("", &[("text/plain", &[0xff, 0xfe])]);
        assert_eq!(status(engine.process(&invalid).unwrap()), "EINVAL");
        let primary =
            kitty_write(":loc=primary", &[("text/plain", b"primary")]);
        assert_eq!(status(engine.process(&primary).unwrap()), "ENOSYS");
        assert!(drain_text(recipient).is_empty());
    });
}

#[test]
fn kitty_named_and_passworded_writes_match_anonymous_ones() {
    let name = format!(":name={}", base64(b"editor"));
    let password = format!("{name}:pw={}", base64(b"secret"));
    for (clipboard_allowed, expected) in [(true, "DONE"), (false, "EPERM")] {
        // The engine never reads `granted`, so remembered grants cannot
        // change its answer; huterm-ghostty's contract test
        // `kitty_clipboard_writes_deliver_every_representation_name_and_password`
        // shows that replies never record one.
        with_policy(clipboard_allowed, |engine, recipient| {
            for metadata in ["", name.as_str(), password.as_str()] {
                let write = kitty_write(metadata, &[("text/plain", b"same")]);
                assert_eq!(
                    status(engine.process(&write).unwrap()),
                    expected,
                    "{metadata}"
                );
            }
            let admitted = drain_text(recipient);
            assert_eq!(admitted.len(), if clipboard_allowed { 3 } else { 0 });
            assert!(admitted.iter().all(|text| text == "same"));
        });
    }
}

#[test]
fn kitty_writes_to_a_full_queue_are_busy_until_it_drains() {
    with_engine(|engine, recipient| {
        let limit = crate::host_effects::TERMINAL_EFFECT_LIMIT;
        let fill: Vec<_> =
            (0..limit).map(|index| format!("fill-{index}")).collect();
        for text in &fill {
            let write = kitty_write("", &[("text/plain", text.as_bytes())]);
            assert_eq!(status(engine.process(&write).unwrap()), "DONE");
        }
        // Nobody consumes the queue, so the next write is refused as busy.
        let overflow = kitty_write("", &[("text/plain", b"overflow")]);
        assert_eq!(status(engine.process(&overflow).unwrap()), "EBUSY");
        assert_eq!(drain_text(recipient), fill);
        let after = kitty_write("", &[("text/plain", b"after")]);
        assert_eq!(status(engine.process(&after).unwrap()), "DONE");
        assert_eq!(drain_text(recipient), ["after"]);
    });
}

/// Streams a Kitty write of `len` bytes of `a` as `text/plain` in
/// bounded packets and returns every reply.
fn stream_kitty_text(engine: &mut TerminalEngine, len: usize) -> Vec<String> {
    // Each "YWFh" encodes "aaa", so unpadded packets concatenate into
    // one base64 stream.
    const UNITS: usize = 64 * 1024;
    let packet = |payload: &str| {
        format!("\x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;{payload}\x1b\\")
    };
    let mut replies = Vec::new();
    let mut send = |engine: &mut TerminalEngine, bytes: &[u8]| {
        for effect in engine.process(bytes).unwrap() {
            if let EngineEffect::PtyWrite(reply) = effect {
                replies.push(String::from_utf8(reply).unwrap());
            }
        }
    };
    send(engine, b"\x1b]5522;type=write\x1b\\");
    let full = packet(&"YWFh".repeat(UNITS));
    let mut remaining = len;
    while remaining >= 3 * UNITS {
        send(engine, full.as_bytes());
        remaining -= 3 * UNITS;
    }
    let mut tail = "YWFh".repeat(remaining / 3);
    tail.push_str(["", "YQ==", "YWE="][remaining % 3]);
    if !tail.is_empty() {
        send(engine, packet(&tail).as_bytes());
    }
    send(engine, b"\x1b]5522;type=wdata\x1b\\");
    replies
}

#[test]
fn kitty_writes_over_the_terminal_budget_fail_before_the_host() {
    with_engine(|engine, recipient| {
        let limit = crate::host_effects::TERMINAL_BYTE_LIMIT;
        assert_eq!(
            stream_kitty_text(engine, limit + 1),
            ["\x1b]5522;type=write:status=EFBIG\x1b\\"]
        );
        assert!(drain_text(recipient).is_empty());
    });
}

#[test]
fn kitty_writes_filling_the_terminal_budget_are_admitted() {
    with_engine(|engine, recipient| {
        let limit = crate::host_effects::TERMINAL_BYTE_LIMIT;
        assert_eq!(
            stream_kitty_text(engine, limit),
            ["\x1b]5522;type=write:status=DONE\x1b\\"]
        );
        // Inspect the queued text in place rather than copying it.
        let pending = recipient.try_next().expect("the write is queued");
        let HostEffect::ClipboardWrite(write) = pending.effect() else {
            panic!("unexpected host effect");
        };
        assert_eq!(write.text().len(), limit);
        assert!(write.text().bytes().all(|byte| byte == b'a'));
        drop(pending);
        assert!(recipient.try_next().is_none());
    });
}

#[test]
fn kitty_empty_commit_clears_and_echoes_the_id() {
    with_engine(|engine, recipient| {
        let effects = engine
            .process(
                b"\x1b]5522;type=write:id=clear-1\x07\x1b]5522;type=wdata\x07",
            )
            .unwrap();
        let replies: Vec<_> = effects
            .into_iter()
            .filter_map(|effect| match effect {
                EngineEffect::PtyWrite(bytes) => Some(bytes),
                _ => None,
            })
            .collect();
        // The reply echoes the id and the commit's BEL terminator.
        assert_eq!(
            replies,
            [b"\x1b]5522;type=write:status=DONE:id=clear-1\x07".to_vec()]
        );
        assert_eq!(drain_text(recipient), [""]);
    });
}
