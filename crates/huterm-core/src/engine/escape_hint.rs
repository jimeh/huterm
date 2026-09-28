//! Finds color operations in PTY output without parsing it a second time.

// A conservative invalidation hint, not a second terminal parser. Native
// Ghostty still interprets every color and reset. This mirrors the pinned
// parser's state transitions (`stream.zig` and `parse_table.zig`) closely
// enough to find color OSC dispatches and RIS across PTY chunks, so ordinary
// text, CSI, and non-color OSC output never trigger palette probing. It may
// flag extra sequences, but it must never miss a color operation: where the
// hint cannot tell two parser states apart, it assumes the one in which more
// bytes can start a sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EscapeHint {
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    /// DCS parameters and intermediates, where C1 bytes still start new
    /// sequences. Also stands in for `dcs_ignore`, which treats C1 bytes as
    /// payload, so that case can only over-flag.
    DcsEntry,
    /// The DCS payload after its final byte. The table makes 0x80..=0xff
    /// payload here, so only ESC, CAN, and SUB exit.
    DcsString,
    /// SOS, PM, and APC strings, which every anywhere transition exits.
    String,
    Osc(OscNumber),
}

/// The leading decimal number of an OSC, accumulated across chunks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct OscNumber {
    value: u16,
    digits: u8,
    complete: bool,
}

impl OscNumber {
    /// Ghostty accepts at most four prefix digits (OSC 3008).
    const MAX_DIGITS: u8 = 4;

    fn push(self, byte: u8) -> Self {
        if self.complete {
            return self;
        }
        if !byte.is_ascii_digit() {
            return Self {
                complete: true,
                ..self
            };
        }
        if self.digits == Self::MAX_DIGITS {
            // Too long for any OSC Ghostty dispatches.
            return Self {
                value: 0,
                digits: Self::MAX_DIGITS + 1,
                complete: true,
            };
        }
        Self {
            value: self.value * 10 + u16::from(byte - b'0'),
            digits: self.digits + 1,
            complete: false,
        }
    }

    /// OSCs that `osc.zig` dispatches to its color and kitty color parsers.
    /// The pinned parser no longer recognizes OSC 105; keeping it only
    /// over-flags.
    fn is_color(self) -> bool {
        (1..=Self::MAX_DIGITS).contains(&self.digits)
            && matches!(
                self.value,
                4 | 5 | 10..=19 | 21 | 104 | 105 | 110..=119
            )
    }
}

impl EscapeHint {
    pub(super) fn observe(&mut self, bytes: &[u8]) -> bool {
        let mut changed = false;
        let mut index = 0;
        while index < bytes.len() {
            // Skip bytes that cannot change the current state. Dense SGR
            // output, such as truecolor per-cell colors, is mostly CSI
            // parameters, and stepping each one dominated parse time.
            let rest = &bytes[index..];
            let next = match *self {
                // Ghostty decodes ground bytes as UTF-8, so only ESC leaves
                // it. Decoded C0 controls execute and decoded C1 controls
                // are ignored without leaving ground.
                Self::Ground => memchr::memchr(0x1b, rest),
                Self::Csi => {
                    rest.iter().position(|byte| !matches!(byte, 0x20..=0x3f))
                }
                Self::DcsString => rest
                    .iter()
                    .position(|byte| matches!(byte, 0x18 | 0x1a | 0x1b)),
                // Strings end only through anywhere transitions.
                Self::String => rest.iter().position(|byte| {
                    matches!(byte, 0x18 | 0x1a | 0x1b | 0x80..=0x9f)
                }),
                Self::Osc(number) if number.complete => rest
                    .iter()
                    .position(|byte| matches!(byte, 0x07 | 0x18 | 0x1a | 0x1b)),
                _ => Some(0),
            };
            let Some(offset) = next else {
                break;
            };
            index += offset;
            changed |= self.step(bytes[index]);
            index += 1;
        }
        changed
    }

    fn step(&mut self, byte: u8) -> bool {
        let (next, changed) = match (*self, byte) {
            // OSC exits dispatch the accumulated command. Other C0 bytes are
            // ignored, and 0x20..=0xff, including C1 values, are payload.
            (Self::Osc(number), 0x07 | 0x18 | 0x1a) => {
                (Self::Ground, number.is_color())
            }
            (Self::Osc(number), 0x1b) => (Self::Escape, number.is_color()),
            (Self::Osc(number), 0x20..=0xff) => {
                (Self::Osc(number.push(byte)), false)
            }
            (_, 0x1b) => (Self::Escape, false),
            (Self::DcsString, 0x18 | 0x1a) => (Self::Ground, false),
            (Self::Ground | Self::Osc(_) | Self::DcsString, _) => {
                (*self, false)
            }
            (Self::Escape, b'c') => (Self::Ground, true),
            // C1 bytes are anywhere transitions in every other non-ground
            // state; the ESC finals that open sequences must precede the
            // generic ESC-final arm below.
            (_, 0x90) | (Self::Escape, b'P') => (Self::DcsEntry, false),
            (_, 0x98 | 0x9e | 0x9f) | (Self::Escape, b'X' | b'^' | b'_') => {
                (Self::String, false)
            }
            (_, 0x9b) | (Self::Escape, b'[') => (Self::Csi, false),
            (_, 0x9d) | (Self::Escape, b']') => {
                (Self::Osc(OscNumber::default()), false)
            }
            (
                _,
                0x18 | 0x1a | 0x80..=0x8f | 0x91..=0x97 | 0x99 | 0x9a | 0x9c,
            )
            | (Self::Escape | Self::EscapeIntermediate, 0x30..=0x7e)
            | (Self::Csi, 0x40..=0x7e) => (Self::Ground, false),
            (Self::DcsEntry, 0x40..=0x7e) => (Self::DcsString, false),
            (Self::Escape | Self::EscapeIntermediate, 0x20..=0x2f) => {
                (Self::EscapeIntermediate, false)
            }
            _ => (*self, false),
        };
        *self = next;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_hint_skipping_matches_bytewise_state_across_chunks() {
        fn bytewise(state: &mut EscapeHint, bytes: &[u8]) -> bool {
            let mut changed = false;
            for &byte in bytes {
                changed |= state.step(byte);
            }
            changed
        }

        let mut fixtures: Vec<Vec<u8>> = [
            &b"plain text without escapes"[..],
            b"a\x1b[31mred\x1b[0m b",
            b"x\x1b]4;1;rgb:aa/bb/cc\x07y",
            b"x\x1b]10;?\x1b\\y\x1bcz",
            b"\x1b[1\x9d4;1;#fff\x9cq\x1b\x1b]11;#000\x18w\x1a",
            // U+271D encodes a 0x9d continuation byte inside ordinary text.
            "cross \u{271d} then \x1b]2;title\x07 done".as_bytes(),
            "\x1b[38;2;1;2;3;48;2;4;5;6m\u{2580}\x1b[0m".as_bytes(),
            b"\x1bPq#0;2;0;0;0\xa0\x01\x9d11;#000\x07",
            b"\x1b]52;c;aGVsbG8gd29ybGQ=\x01\xa0\x9d\x1b\\",
        ]
        .map(<[u8]>::to_vec)
        .into();
        let alphabet = b"ab14;m \x01\xa0\x1b\x9d\x9b\x90]P[c\x07\x18\x1a\x9c\\";
        let mut seed = 0x2545_f491_u32;
        fixtures.push(
            (0..512)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    alphabet[seed as usize % alphabet.len()]
                })
                .collect(),
        );

        for bytes in &fixtures {
            for start in [
                EscapeHint::Ground,
                EscapeHint::Escape,
                EscapeHint::Csi,
                EscapeHint::DcsEntry,
                EscapeHint::DcsString,
                EscapeHint::String,
                EscapeHint::Osc(OscNumber::default()),
                EscapeHint::Osc(OscNumber {
                    value: 11,
                    digits: 2,
                    complete: true,
                }),
            ] {
                for split in 0..=bytes.len() {
                    let (mut fast, mut expected) = (start, start);
                    for chunk in [&bytes[..split], &bytes[split..]] {
                        assert_eq!(
                            fast.observe(chunk),
                            bytewise(&mut expected, chunk),
                            "{bytes:?} from {start:?} split at {split}"
                        );
                        assert_eq!(fast, expected);
                    }
                }
            }
        }
    }

    #[test]
    fn escape_hint_flags_only_color_operations_at_every_split() {
        let cases: &[(&[u8], bool)] = &[
            ("╝帝\x1b[31mtext\x1b[0m".as_bytes(), false),
            (b"\x1b]2;title\x07", false),
            (b"\x1b]7;file:///tmp\x1b\\", false),
            (b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07", false),
            (b"\x1b]133;A\x07", false),
            (b"\x1b]52;c;aGk=\x07", false),
            (b"\x1b]3008;x\x07", false),
            (b"\x1b]10004;#fff\x07", false),
            // Raw C1 bytes are UTF-8 data in the ground state.
            (b"\x9d4;1;#fff\x07", false),
            // Inside an OSC, C1 bytes are payload rather than new sequences.
            (b"\x1b]2;a\x9d4;1;#fff\x07", false),
            // An intermediate turns `]` into an ordinary escape final byte.
            (b"\x1b(]4;1;#fff\x07", false),
            (b"\x1b]4;1;#fff\x07", true),
            (b"\x1b]11;#000\x1b\\", true),
            (b"\x1b]21;foreground=#fff\x1b\\", true),
            (b"\x1b]104\x07", true),
            (b"\x1b]105;0\x07", true),
            (b"\x1b]110\x18", true),
            (b"\x1b]119\x1a", true),
            (b"\x1bc", true),
            // Ignored C0 bytes do not end the OSC number.
            (b"\x1b]1\x010;#fff\x07", true),
            // C1 OSC is recognized once another sequence has started.
            (b"\x1b[1\x9d4;1;#fff\x07", true),
            (b"\x1bP1\x9d11;#000\x07", true),
            (b"\x1b_G\x9d11;#000\x07", true),
            // DCS payload keeps C1 bytes, so only ESC leaves it.
            (b"\x1bPq\x9d11;#000\x07", false),
            (b"\x1bPq\x9c\x1b]11;#000\x07", true),
            (b"\x1bPq\x18\x9d11;#000\x07", false),
        ];
        for &(bytes, expected) in cases {
            for split in 0..=bytes.len() {
                let mut hint = EscapeHint::Ground;
                let flagged = hint.observe(&bytes[..split])
                    | hint.observe(&bytes[split..]);
                assert_eq!(
                    flagged,
                    expected,
                    "{:?} split at {split}",
                    String::from_utf8_lossy(bytes)
                );
            }
        }
    }
}
