use huterm_protocol::{CommandValue, catalog, ids};

use super::*;
use crate::vectors;

#[test]
fn an_unknown_identifier_decodes_with_its_text_for_an_unknown_command_answer() {
    let mut w = Writer::new();
    w.record(|w| {
        w.str("from_the_future", MAX_COMMAND_NAME_BYTES, "command ID")?;
        w.seq(std::iter::once(()), 1, "args", |w, ()| {
            w.record(|w| {
                w.str("depth", MAX_COMMAND_NAME_BYTES, "argument name")?;
                w.tag(1);
                w.record(|w| {
                    w.i64(3);
                    Ok(())
                })
            })
        })
    })
    .unwrap();
    let bytes = w.into_bytes();
    assert_eq!(
        decode_invocation(&mut Reader::new(&bytes), vectors::scope()),
        Ok(DecodedInvocation::Unknown {
            id: "from_the_future".into(),
            args: vec![CommandArgument::new("depth", CommandValue::Integer(3))],
        })
    );
}

#[test]
fn every_catalog_command_fits_the_invocation_limits() {
    for spec in catalog() {
        assert!(
            spec.id.as_str().len() <= MAX_COMMAND_NAME_BYTES,
            "{}",
            spec.id
        );
        assert!(spec.args.len() <= MAX_COMMAND_ARGUMENTS, "{}", spec.id);
        for arg in spec.args {
            assert!(arg.name.len() <= MAX_COMMAND_NAME_BYTES, "{}", spec.id);
        }
    }
    let mut w = Writer::new();
    let long = CommandInvocation::new(
        ids::RENAME_TAB,
        vec![CommandArgument::new(
            "name",
            CommandValue::Text("x".repeat(MAX_ARGUMENT_TEXT_BYTES + 1)),
        )],
    );
    assert_eq!(
        encode_invocation(&mut w, vectors::scope(), &long),
        Err(EncodeError::LimitExceeded {
            field: "argument text"
        })
    );
}

#[test]
fn unknown_value_and_outcome_tags_are_rejected() {
    assert_eq!(
        decode_outcome(&mut Reader::new(&[2])),
        Err(DecodeError::UnknownTag {
            field: "command outcome",
            tag: 2
        })
    );
    let bytes = [0x08, 0x01, b'q', 0x01, 0x04, 0x01, b'a', 0x09, 0x00];
    assert_eq!(
        decode_invocation(&mut Reader::new(&bytes), vectors::scope()),
        Err(DecodeError::UnknownTag {
            field: "command value",
            tag: 9
        })
    );
}
