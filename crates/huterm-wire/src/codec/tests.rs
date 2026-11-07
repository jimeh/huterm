use huterm_protocol::{SessionId, TabId};
use proptest::prelude::*;

use super::*;

fn decoded<'a, T>(
    bytes: &'a [u8],
    read: impl FnOnce(&mut Reader<'a>) -> Result<T, DecodeError>,
) -> Result<T, DecodeError> {
    let mut reader = Reader::new(bytes);
    let value = read(&mut reader)?;
    reader.finish()?;
    Ok(value)
}

#[test]
fn varints_are_canonical_and_fit_their_width() {
    for (value, bytes) in [
        (0, &[0x00][..]),
        (127, &[0x7f]),
        (128, &[0x80, 0x01]),
        (300, &[0xac, 0x02]),
        (
            u64::MAX,
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01],
        ),
    ] {
        let mut w = Writer::new();
        w.u64(value);
        assert_eq!(w.into_bytes(), bytes);
        assert_eq!(decoded(bytes, Reader::u64), Ok(value));
        assert_eq!(varint_len(value), bytes.len());
    }
    // A zero continuation byte, an eleventh byte, and a tenth byte past
    // 64 bits are all overlong.
    for overlong in [
        &[0x80, 0x00][..],
        &[0xff, 0x80, 0x00],
        &[0x80; 11],
        &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02],
    ] {
        assert_eq!(
            decoded(overlong, Reader::u64),
            Err(DecodeError::OverlongVarint),
            "{overlong:?}"
        );
    }
    assert_eq!(decoded(&[0x80], Reader::u64), Err(DecodeError::Truncated));
    assert_eq!(
        decoded(&[0x80, 0x80, 0x04], Reader::u16),
        Err(DecodeError::OutOfRange)
    );
    assert_eq!(decoded(&[0xff, 0xff, 0x03], Reader::u16), Ok(u16::MAX));
}

#[test]
fn booleans_strings_options_and_sequences_reject_malformed_bytes() {
    assert_eq!(
        decoded(&[2], Reader::bool),
        Err(DecodeError::InvalidBool(2))
    );
    assert_eq!(decoded(&[], Reader::bool), Err(DecodeError::Truncated));
    assert_eq!(
        decoded(&[2, 0xc3, 0x28], |r| r.string(8, "name")),
        Err(DecodeError::InvalidUtf8)
    );
    // The length is checked against the field's maximum before any byte
    // is taken, even when the input could not back it.
    assert_eq!(
        decoded(&[0xff, 0xff, 0x03], |r| r.string(8, "name")),
        Err(DecodeError::TooLong { field: "name" })
    );
    assert_eq!(
        decoded(&[3, b'a'], |r| r.string(8, "name")),
        Err(DecodeError::Truncated)
    );
    assert_eq!(
        decoded(&[2], |r| r.optional_string(8, "name")),
        Err(DecodeError::UnknownTag {
            field: "name",
            tag: 2
        })
    );
    assert_eq!(
        decoded(&[0xff, 0xff, 0xff, 0x7f], |r| r.seq(
            4,
            "items",
            Reader::u64
        )),
        Err(DecodeError::TooLong { field: "items" })
    );
    assert_eq!(
        decoded(&[0, 1], Reader::bool),
        Err(DecodeError::TrailingBytes)
    );
    let mut w = Writer::new();
    assert_eq!(
        w.str("toolong", 3, "name"),
        Err(EncodeError::LimitExceeded { field: "name" })
    );
    assert_eq!(
        w.seq([1, 2, 3].iter(), 2, "items", |w, item| {
            w.u64(*item);
            Ok(())
        }),
        Err(EncodeError::LimitExceeded { field: "items" })
    );
    assert!(w.is_empty(), "a refused value writes nothing");
}

/// A record of one field, then a second field a newer version appended.
fn appended_record(newer: bool) -> Vec<u8> {
    let mut w = Writer::new();
    w.record(|w| {
        w.u64(7);
        if newer {
            w.str("new", 8, "appended")?;
        }
        Ok(())
    })
    .unwrap();
    w.u64(9);
    w.into_bytes()
}

#[test]
fn records_skip_appended_fields_and_default_missing_later_ones() {
    // An older decoder skips the appended field and reads on.
    for newer in [false, true] {
        let bytes = appended_record(newer);
        assert_eq!(
            decoded(&bytes, |r| Ok((r.record(Reader::u64)?, r.u64()?))),
            Ok((7, 9))
        );
    }
    // A newer decoder takes the default from an older record.
    let newer_decode = |bytes: &[u8]| {
        decoded(bytes, |r| {
            let fields = r.record(|r| {
                let first = r.u64()?;
                let appended =
                    r.appended(String::new(), |r| r.string(8, "appended"))?;
                Ok((first, appended))
            })?;
            Ok((fields, r.u64()?))
        })
    };
    assert_eq!(
        newer_decode(&appended_record(false)),
        Ok(((7, String::new()), 9))
    );
    assert_eq!(
        newer_decode(&appended_record(true)),
        Ok(((7, "new".into()), 9))
    );
    // A record cut short before a field of its own version is truncated.
    assert_eq!(
        decoded(&[0x00, 0x09], |r| Ok((r.record(Reader::u64)?, r.u64()?))),
        Err(DecodeError::Truncated)
    );
    // A record longer than its input is truncated too.
    assert_eq!(
        decoded(&[0x05, 0x01], |r| r.record(Reader::u64)),
        Err(DecodeError::Truncated)
    );
}

#[test]
fn identities_travel_bare_in_the_connection_scope_and_never_leave_it() {
    let own = IdScope::new(RuntimeId::new(4));
    let mut w = Writer::new();
    w.id(own, SessionId::in_runtime(RuntimeId::new(4), 300))
        .unwrap();
    assert_eq!(
        w.id(own, TabId::in_runtime(RuntimeId::new(5), 1)),
        Err(EncodeError::StaleTarget)
    );
    assert_eq!(
        w.id(own, TabId::new(1)),
        Err(EncodeError::StaleTarget),
        "an unscoped identity is not the connection's either"
    );
    let bytes = w.into_bytes();
    assert_eq!(bytes, [0xac, 0x02]);
    let other = IdScope::new(RuntimeId::new(8));
    assert_eq!(
        decoded(&bytes, |r| r.id::<SessionId>(other)),
        Ok(SessionId::in_runtime(RuntimeId::new(8), 300))
    );
}

proptest! {
    #[test]
    fn unsigned_and_signed_varints_round_trip(value: u64, signed: i64) {
        let mut w = Writer::new();
        w.u64(value);
        w.i64(signed);
        let bytes = w.into_bytes();
        prop_assert_eq!(decoded(&bytes, |r| Ok((r.u64()?, r.i64()?))), Ok((value, signed)));
    }

    #[test]
    fn strings_round_trip_and_arbitrary_bytes_never_panic(
        text in ".{0,40}",
        noise in proptest::collection::vec(any::<u8>(), 0..64),
    ) {
        let mut w = Writer::new();
        w.str(&text, 256, "text").unwrap();
        let bytes = w.into_bytes();
        prop_assert_eq!(decoded(&bytes, |r| r.string(256, "text")), Ok(text));
        let mut reader = Reader::new(&noise);
        let _ = reader.record(|r| {
            let _ = r.optional_string(16, "text")?;
            r.seq(8, "items", Reader::i64)
        });
    }
}
