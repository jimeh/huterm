use super::*;
use crate::host_effects::{HostEffectRecipient, HostEffectSink};
use crate::notifications::NOTIFICATION_BURST;
use huterm_protocol::{HostEffect, HostEffectKind, TerminalProgress};

fn engine(sink: Option<HostEffectSink>) -> TerminalEngine {
    let mut engine = TerminalEngine::new(
        TerminalId::new(1),
        GridSize::clamped(8, 3),
        CellSize {
            width: 8,
            height: 16,
        },
        TerminalPresentation::default(),
    )
    .unwrap();
    if let Some(sink) = sink {
        engine.set_host_effect_sink(sink);
    }
    engine
}

fn notifications(recipient: &HostEffectRecipient) -> Vec<(String, String)> {
    let mut seen = Vec::new();
    while let Some(pending) = recipient.try_next() {
        match pending.effect() {
            HostEffect::Notification(note) => {
                seen.push((note.title().to_owned(), note.body().to_owned()));
            }
            effect => panic!("unexpected host effect {effect:?}"),
        }
    }
    seen
}

fn progress(effects: Vec<EngineEffect>) -> Vec<Option<TerminalProgress>> {
    effects
        .into_iter()
        .filter_map(|effect| match effect {
            EngineEffect::Progress(progress) => Some(progress),
            _ => None,
        })
        .collect()
}

#[test]
fn notifications_reach_one_recipient_as_normalized_text() {
    let (sink, recipient) =
        crate::host_effects::test_fixture(TerminalId::new(1));
    let mut engine = engine(Some(sink));
    // Notifications produce no engine effect: they leave as host effects.
    // Ghostty drops C0 bytes such as a tab inside an OSC, and an ESC ends
    // the payload; Huterm turns the C1 control that reaches it into a space
    // and removes the right-to-left override.
    assert!(
        engine
            .process(
                "\x1b]9;Build\tdone\u{85}now \u{202E}ok\x07\x1b]777;notify;CI;\x1b]8;;x\x07"
                    .as_bytes()
            )
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        notifications(&recipient),
        [
            (String::new(), "Builddone now ok".to_owned()),
            ("CI".to_owned(), String::new()),
        ]
    );
    // A malformed ConEmu command and blank text are dropped.
    engine.process(b"\x1b]9;4;9\x07\x1b]9; \x07").unwrap();
    assert_eq!(notifications(&recipient), []);
}

#[test]
fn notifications_over_the_rate_limit_are_dropped_not_deferred() {
    let (sink, recipient) =
        crate::host_effects::test_fixture(TerminalId::new(1));
    let mut engine = engine(Some(sink));
    let burst = usize::try_from(NOTIFICATION_BURST).unwrap();
    let mut flood = Vec::new();
    for index in 0..burst * 3 {
        flood.extend_from_slice(format!("\x1b]9;n{index}\x07").as_bytes());
    }
    engine.process(&flood).unwrap();
    let delivered = notifications(&recipient);
    assert_eq!(delivered.len(), burst);
    assert_eq!(delivered[0].1, "n0");
    assert_eq!(delivered[burst - 1].1, format!("n{}", burst - 1));
}

#[test]
fn notifications_without_an_eligible_recipient_are_not_replayed() {
    let sink = HostEffectSink::new(TerminalId::new(1));
    let mut engine = engine(Some(sink.clone()));
    engine.process(b"\x1b]9;nobody\x07").unwrap();
    let (_, late) = {
        let process = crate::host_effects::DesktopHostEffectClient::new();
        let recipient = sink
            .register(
                None,
                &process,
                crate::host_effects::HostEffectRecipientOptions::local_desktop(
                    crate::host_effects::HostEffectPermissions::ALL,
                ),
                crate::host_effects::RecipientLink::default(),
            )
            .unwrap();
        (process, recipient)
    };
    assert_eq!(notifications(&late), [], "a later client gets no replay");
    // A client that denies notifications is not selected; clipboard
    // permission does not imply notification permission.
    late.set_allowed(HostEffectKind::Notification, false);
    engine.process(b"\x1b]9;denied\x07").unwrap();
    late.set_allowed(HostEffectKind::Notification, true);
    assert_eq!(notifications(&late), []);
    engine.process(b"\x1b]9;allowed\x07").unwrap();
    assert_eq!(
        notifications(&late),
        [(String::new(), "allowed".to_owned())]
    );
}

#[test]
fn progress_reports_become_status_values_and_a_full_reset_clears_them() {
    use TerminalProgress::{Determinate, Error, Indeterminate, Paused};
    let mut engine = engine(None);
    assert_eq!(
        progress(
            engine
                .process(
                    b"\x1b]9;4;1;42\x07\x1b]9;4;3\x07\x1b]9;4;4\x07\
                      \x1b]9;4;2;7\x07\x1b]9;4;0\x07"
                )
                .unwrap()
        ),
        [
            Some(Determinate(42)),
            Some(Indeterminate),
            Some(Paused(None)),
            Some(Error(Some(7))),
            None,
        ]
    );
    // Split across writes, the report arrives with its terminator.
    assert_eq!(progress(engine.process(b"\x1b]9;4;1;").unwrap()), []);
    assert_eq!(
        progress(engine.process(b"5\x1b\\").unwrap()),
        [Some(Determinate(5))]
    );
    assert_eq!(progress(engine.process(b"\x1bc").unwrap()), [None]);
}
