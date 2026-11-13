use super::*;

#[test]
fn kinds_keep_distinct_numbers_and_one_direction_each() {
    for kind in MessageKind::ALL {
        assert_eq!(MessageKind::from_number(kind.number()), Some(kind));
    }
    let mut numbers: Vec<u64> =
        MessageKind::ALL.iter().map(|kind| kind.number()).collect();
    numbers.dedup();
    assert_eq!(
        numbers,
        (0..MessageKind::ALL.len() as u64).collect::<Vec<_>>()
    );
    // The handshake kinds are frozen in every major version.
    assert_eq!(MessageKind::Hello.number(), 0);
    assert_eq!(MessageKind::Reject.number(), 2);
    assert_eq!(MessageKind::from_number(99), None);
}

#[test]
fn every_minor_0_kind_has_a_cap_covering_its_payload() {
    let caps = MessageCaps::for_minor(0);
    for kind in MessageKind::ALL {
        assert!(caps.cap(kind).is_some_and(|cap| cap >= 64), "{kind:?}");
    }
    assert!(caps.cap(MessageKind::Paste).unwrap() > PASTE_PAYLOAD_LIMIT);
    assert!(
        caps.cap(MessageKind::HostEffect).unwrap() > CLIPBOARD_PAYLOAD_LIMIT
    );
    assert!(caps.cap(MessageKind::Input).unwrap() > INPUT_TEXT_PAYLOAD_LIMIT);
    // Text input carries a whole input budget, as a paste does, plus a
    // stamp and a tag, so it is the largest client message.
    assert_eq!(
        caps.largest(Direction::ClientToServer),
        caps.cap(MessageKind::Input).unwrap()
    );
    assert_eq!(
        reassembly_budget(0),
        crate::stream::MAX_OPEN_STREAMS * caps.cap(MessageKind::Input).unwrap()
    );
}

#[test]
fn requests_stay_below_the_largest_client_cap_and_results_cover_a_status() {
    let caps = MessageCaps::for_minor(0);
    let cap = |kind| caps.cap(kind).unwrap();
    // Requests may span frames, but none raises the reassembly budget.
    assert!(cap(MessageKind::Request) < cap(MessageKind::Input));
    assert!(cap(MessageKind::Cancel) < cap(MessageKind::Input));
    assert!(cap(MessageKind::Focus) < cap(MessageKind::Input));
    // A subscription's result carries a whole status, so results may span
    // frames; selection text travels as a large result instead.
    assert!(cap(MessageKind::Result) > cap(MessageKind::ViewerStatus));
    assert!(cap(MessageKind::LargeResult) > SELECTION_PAYLOAD_LIMIT);
    assert!(
        cap(MessageKind::Result)
            < cap(MessageKind::HierarchySnapshot)
                .min(cap(MessageKind::LargeResult))
    );
    assert_eq!(MessageKind::Request.direction(), Direction::ClientToServer);
    assert_eq!(MessageKind::Cancel.direction(), Direction::ClientToServer);
    assert_eq!(MessageKind::Result.direction(), Direction::ServerToClient);
}
