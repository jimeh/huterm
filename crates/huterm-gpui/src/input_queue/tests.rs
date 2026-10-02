use super::*;
use crate::mouse::MouseState;
use huterm_protocol::{Modifiers, MouseButton, MouseInput, MousePosition};

const STAMP: InputStamp = InputStamp {
    scrolls: 0,
    geometry: None,
};

fn queued(input: TerminalInput) -> Queued {
    Queued::Input(input, STAMP)
}

fn busy(queued: Queued) -> Result<(), Refused> {
    Err(Refused {
        error: RuntimeError::Busy,
        queued,
    })
}

fn mouse(action: MouseAction, column: u32) -> TerminalInput {
    TerminalInput::Mouse(MouseInput {
        action,
        position: MousePosition { column, row: 0 },
        modifiers: Modifiers::default(),
    })
}

fn drain(queue: &mut InputQueue) -> Vec<Queued> {
    let mut sent = Vec::new();
    queue
        .retry(|queued| {
            sent.push(queued);
            Ok(())
        })
        .unwrap();
    sent
}

#[test]
fn file_drop_paste_admission_is_atomic_at_full_and_closed_boundaries() {
    let mut queue = InputQueue::default();
    let paste = crate::file_drop::format_paths(
        &["/tmp/a b".into()],
        PENDING_INPUT_BYTE_CAPACITY,
    )
    .unwrap();
    queue
        .enqueue(
            TerminalInput::Text("x".repeat(PENDING_INPUT_BYTE_CAPACITY)),
            STAMP,
            false,
            busy,
        )
        .unwrap();
    assert_eq!(
        queue
            .enqueue(
                TerminalInput::Paste(paste.clone()),
                STAMP,
                false,
                |_| panic!("full drop reached runtime")
            )
            .unwrap(),
        Admission::Full
    );
    assert_eq!(queue.entries.len(), 1);
    assert_eq!(drain(&mut queue).len(), 1);
    let mut sent = Vec::new();
    assert_eq!(
        queue
            .enqueue(
                TerminalInput::Paste(paste.clone()),
                STAMP,
                false,
                |queued| {
                    sent.push(queued);
                    Ok(())
                }
            )
            .unwrap(),
        Admission::Accepted
    );
    assert_eq!(sent, [queued(TerminalInput::Paste(paste.clone()))]);
    queue.close();
    assert_eq!(
        queue
            .enqueue(TerminalInput::Paste(paste), STAMP, false, |_| panic!(
                "closed drop reached runtime"
            ))
            .unwrap(),
        Admission::Closed
    );
}

#[test]
fn meta_prefix_counts_toward_queue_capacity() {
    let mut queue = InputQueue::default();
    let full = TerminalInput::Character {
        text: "x".repeat(PENDING_INPUT_BYTE_CAPACITY),
        meta: true,
    };
    assert_eq!(
        queue
            .enqueue(full, STAMP, false, |_| panic!(
                "oversized input reached runtime"
            ))
            .unwrap(),
        Admission::Full
    );
    let fits = TerminalInput::Character {
        text: "x".repeat(PENDING_INPUT_BYTE_CAPACITY - 1),
        meta: true,
    };
    assert_eq!(
        queue.enqueue(fits, STAMP, false, busy).unwrap(),
        Admission::Accepted
    );
    assert_eq!(queue.bytes, PENDING_INPUT_BYTE_CAPACITY);
    assert_eq!(
        buffered_input_bytes(&TerminalInput::Character {
            text: "λ".into(),
            meta: true
        }),
        3
    );
}

#[test]
fn queued_characters_retain_policy_stamp_and_order_across_later_input() {
    let mut queue = InputQueue::default();
    let inputs = vec![
        TerminalInput::Text("®".into()),
        TerminalInput::Character {
            text: "r".into(),
            meta: true,
        },
        TerminalInput::Paste("r".into()),
    ];
    for (scrolls, input) in (0..).zip(&inputs) {
        let stamp = InputStamp {
            scrolls,
            geometry: None,
        };
        assert_eq!(
            queue.enqueue(input.clone(), stamp, false, busy).unwrap(),
            Admission::Accepted
        );
    }
    let expected: Vec<_> = (0..)
        .zip(inputs)
        .map(|(scrolls, input)| {
            Queued::Input(
                input,
                InputStamp {
                    scrolls,
                    geometry: None,
                },
            )
        })
        .collect();
    assert_eq!(drain(&mut queue), expected);
    assert_eq!(queue.bytes, 0);
}

#[test]
fn exit_discards_pending_input_but_focus_changes_keep_flowing() {
    let mut queue = InputQueue::default();
    queue
        .enqueue(TerminalInput::Text("queued".into()), STAMP, false, busy)
        .unwrap();
    queue.enqueue_focus(false, busy).unwrap();
    queue.close();
    assert_eq!(drain(&mut queue), [Queued::Focus(false)]);
    for input in [
        TerminalInput::Text("key".into()),
        TerminalInput::Paste("paste".into()),
        mouse(MouseAction::Press(MouseButton::Left), 0),
        mouse(MouseAction::Release(MouseButton::Left), 0),
    ] {
        assert_eq!(
            queue
                .enqueue(input, STAMP, false, |_| panic!(
                    "exited input reached runtime"
                ))
                .unwrap(),
            Admission::Closed
        );
    }
    assert_eq!(queue.bytes, 0);
    let mut sent = Vec::new();
    for focused in [true, false] {
        assert_eq!(
            queue
                .enqueue_focus(focused, |queued| {
                    sent.push(queued);
                    Ok(())
                })
                .unwrap(),
            Admission::Accepted
        );
    }
    assert_eq!(sent, [Queued::Focus(true), Queued::Focus(false)]);
    // A refused focus change survives repeated closing and is retried.
    queue.enqueue_focus(true, busy).unwrap();
    queue.close();
    assert_eq!(drain(&mut queue), [Queued::Focus(true)]);
}

#[test]
fn cancellation_progresses_without_paint_and_releases_precede_focus_out() {
    let mut queue = InputQueue::default();
    let mut state = MouseState::default();
    state.down(MouseButton::Left, true);
    let press = mouse(MouseAction::Press(MouseButton::Left), 0);
    queue.enqueue(press.clone(), STAMP, false, busy).unwrap();
    state.accepted(MouseButton::Left, MousePosition::default());
    queue
        .enqueue(
            mouse(MouseAction::Motion(Some(MouseButton::Left)), 1),
            STAMP,
            false,
            |_| unreachable!(),
        )
        .unwrap();
    queue.cancel_motion();
    for release in state.cancel() {
        queue
            .enqueue(
                TerminalInput::Mouse(release),
                STAMP,
                true,
                |_| unreachable!(),
            )
            .unwrap();
    }
    assert!(state.cancel().is_empty());
    queue.enqueue_focus(false, |_| unreachable!()).unwrap();
    assert_eq!(
        drain(&mut queue),
        [
            queued(press),
            queued(mouse(MouseAction::Release(MouseButton::Left), 0)),
            Queued::Focus(false)
        ]
    );
}

#[test]
fn focus_keeps_its_order_around_input_and_coalesces_only_when_adjacent() {
    let mut queue = InputQueue::default();
    queue.enqueue_focus(true, busy).unwrap();
    queue
        .enqueue(TerminalInput::Text("k".into()), STAMP, false, busy)
        .unwrap();
    queue.enqueue_focus(false, busy).unwrap();
    queue.enqueue_focus(true, busy).unwrap();
    queue.enqueue_focus(false, busy).unwrap();
    assert_eq!(
        drain(&mut queue),
        [
            Queued::Focus(true),
            queued(TerminalInput::Text("k".into())),
            Queued::Focus(false),
        ]
    );
}

#[test]
fn a_refused_focus_change_holds_back_later_input_and_ignores_capacity() {
    let mut queue = InputQueue::default();
    for _ in 0..PENDING_INPUT_CAPACITY {
        queue
            .enqueue(TerminalInput::Text("f".into()), STAMP, false, busy)
            .unwrap();
    }
    assert_eq!(
        queue
            .enqueue(TerminalInput::Text("x".into()), STAMP, false, busy)
            .unwrap(),
        Admission::Full
    );
    assert_eq!(
        queue.enqueue_focus(true, |_| unreachable!()).unwrap(),
        Admission::Accepted,
        "focus is never refused for capacity"
    );
    let mut sent = Vec::new();
    queue
        .retry(|queued| {
            if queued == Queued::Focus(true) {
                return busy(queued);
            }
            sent.push(queued);
            Ok(())
        })
        .unwrap();
    assert_eq!(sent.len(), PENDING_INPUT_CAPACITY);
    queue
        .enqueue(TerminalInput::Text("after".into()), STAMP, false, busy)
        .unwrap();
    assert_eq!(
        drain(&mut queue),
        [
            Queued::Focus(true),
            queued(TerminalInput::Text("after".into()))
        ]
    );
}

#[test]
fn wheel_saturation_discards_unadmitted_steps_without_reordering() {
    let mut queue = InputQueue::default();
    for _ in 0..PENDING_INPUT_CAPACITY - 2 {
        queue
            .enqueue(TerminalInput::Text("f".into()), STAMP, false, busy)
            .unwrap();
    }
    let mut state = MouseState::default();
    let wheel = state.wheel(20.0, 20.0);
    assert_eq!(wheel.len(), 32);
    let mut accepted = 0;
    for direction in wheel {
        accepted += usize::from(
            queue
                .enqueue(
                    mouse(MouseAction::Wheel(direction), 0),
                    STAMP,
                    false,
                    |_| unreachable!(),
                )
                .unwrap()
                == Admission::Accepted,
        );
    }
    assert_eq!(accepted, 2);
    assert_eq!(queue.entries.len(), PENDING_INPUT_CAPACITY);
    assert_eq!(
        queue.entries.back(),
        Some(&queued(mouse(
            MouseAction::Wheel(huterm_protocol::WheelDirection::Up),
            0
        )))
    );
    assert!(state.wheel(0.0, 0.0).is_empty());
}

#[test]
fn coalesced_motion_keeps_the_newer_stamp() {
    let mut queue = InputQueue::default();
    let stamped = |geometry| InputStamp {
        scrolls: 0,
        geometry: Some(geometry),
    };
    for (column, geometry) in [(1, 1), (2, 2)] {
        queue
            .enqueue(
                mouse(MouseAction::Motion(None), column),
                stamped(geometry),
                false,
                |_| panic!("motion must wait for refresh"),
            )
            .unwrap();
    }
    // The newer position was computed against the newer grid; the older
    // stamp would let the runtime discard valid motion as stale.
    assert_eq!(
        drain(&mut queue),
        [Queued::Input(
            mouse(MouseAction::Motion(None), 2),
            stamped(2)
        )]
    );
}

#[test]
fn motion_coalesces_only_within_compatible_fifo_run() {
    let mut queue = InputQueue::default();
    let motion = |column| mouse(MouseAction::Motion(None), column);
    for column in 0..10_000 {
        assert_eq!(
            queue
                .enqueue(motion(column), STAMP, false, |_| panic!(
                    "motion must wait for refresh"
                ))
                .unwrap(),
            Admission::Accepted
        );
    }
    assert_eq!(queue.entries.len(), 1);
    let keyboard = TerminalInput::Text("K".into());
    queue.enqueue(keyboard.clone(), STAMP, false, busy).unwrap();
    queue
        .enqueue(motion(4), STAMP, false, |_| unreachable!())
        .unwrap();
    queue
        .enqueue(
            mouse(MouseAction::Press(MouseButton::Left), 4),
            STAMP,
            false,
            |_| unreachable!(),
        )
        .unwrap();
    queue
        .enqueue(
            mouse(MouseAction::Motion(Some(MouseButton::Left)), 5),
            STAMP,
            false,
            |_| unreachable!(),
        )
        .unwrap();
    let expected = [
        motion(9999),
        keyboard,
        motion(4),
        mouse(MouseAction::Press(MouseButton::Left), 4),
        mouse(MouseAction::Motion(Some(MouseButton::Left)), 5),
    ]
    .map(queued);
    let mut sent = Vec::new();
    queue
        .retry(|queued| {
            sent.push(queued.clone());
            if sent.len() == 2 {
                busy(queued)
            } else {
                Ok(())
            }
        })
        .unwrap();
    assert_eq!(sent, expected[..2]);
    assert_eq!(drain(&mut queue), expected[1..]);
    assert_eq!(queue.bytes, 0);
}

#[test]
fn three_owned_releases_survive_event_saturation_without_allowing_new_cycles() {
    let mut queue = InputQueue::default();
    let mut state = MouseState::default();
    for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
        state.down(button, true);
        assert_eq!(
            queue
                .enqueue(
                    mouse(MouseAction::Press(button), 0),
                    STAMP,
                    false,
                    busy
                )
                .unwrap(),
            Admission::Accepted
        );
        state.accepted(button, MousePosition::default());
    }
    while queue.entries.len() < PENDING_INPUT_CAPACITY {
        queue
            .enqueue(
                TerminalInput::Text("f".into()),
                STAMP,
                false,
                |_| unreachable!(),
            )
            .unwrap();
    }
    for release in state.cancel() {
        queue
            .enqueue(
                TerminalInput::Mouse(release),
                STAMP,
                true,
                |_| unreachable!(),
            )
            .unwrap();
    }
    assert_eq!(queue.entries.len(), PENDING_INPUT_CAPACITY + 3);
    assert!(state.cancel().is_empty());
    for _ in 0..100 {
        state.release(
            MouseButton::Left,
            MousePosition::default(),
            Modifiers::default(),
        );
        state.down(MouseButton::Left, true);
        assert_eq!(
            queue
                .enqueue(
                    mouse(MouseAction::Press(MouseButton::Left), 0),
                    STAMP,
                    false,
                    |_| unreachable!()
                )
                .unwrap(),
            Admission::Full
        );
        assert!(
            state
                .release(
                    MouseButton::Left,
                    MousePosition::default(),
                    Modifiers::default()
                )
                .is_none()
        );
    }
    assert_eq!(queue.entries.len(), PENDING_INPUT_CAPACITY + 3);
    let sent = drain(&mut queue);
    assert!(matches!(
        sent[PENDING_INPUT_CAPACITY],
        Queued::Input(
            TerminalInput::Mouse(MouseInput {
                action: MouseAction::Release(MouseButton::Left),
                ..
            }),
            _
        )
    ));
}

#[test]
fn byte_saturation_counts_overflow_and_disconnect_clears_everything() {
    let mut queue = InputQueue::default();
    let press = mouse(MouseAction::Press(MouseButton::Left), 0);
    let size = buffered_input_bytes(&press);
    queue.enqueue(press, STAMP, false, busy).unwrap();
    queue
        .enqueue(
            TerminalInput::Paste(
                "p".repeat(PENDING_INPUT_BYTE_CAPACITY - size),
            ),
            STAMP,
            false,
            |_| unreachable!(),
        )
        .unwrap();
    assert_eq!(queue.bytes, PENDING_INPUT_BYTE_CAPACITY);
    assert_eq!(
        queue
            .enqueue(
                TerminalInput::Text("x".into()),
                STAMP,
                false,
                |_| unreachable!()
            )
            .unwrap(),
        Admission::Full
    );
    queue
        .enqueue(
            mouse(MouseAction::Release(MouseButton::Left), 0),
            STAMP,
            true,
            |_| unreachable!(),
        )
        .unwrap();
    assert_eq!(queue.bytes, PENDING_INPUT_BYTE_CAPACITY + size);
    assert_eq!(
        queue
            .enqueue(
                TerminalInput::Text("x".into()),
                STAMP,
                false,
                |_| unreachable!()
            )
            .unwrap(),
        Admission::Full
    );
    assert!(queue.retry(busy).is_ok());
    assert_eq!(queue.entries.len(), 3);
    assert!(matches!(
        queue.retry(|queued| Err(Refused {
            error: RuntimeError::Stopped,
            queued,
        })),
        Err(RuntimeError::Stopped)
    ));
    assert!(queue.entries.is_empty());
    assert_eq!(queue.bytes, 0);
    assert!(!queue.motion_run);
}

#[test]
fn immediate_press_and_buffered_release_keep_acceptance_distinct() {
    let mut queue = InputQueue::default();
    assert_eq!(
        queue
            .enqueue(
                mouse(MouseAction::Press(MouseButton::Left), 0),
                STAMP,
                false,
                |_| Ok(())
            )
            .unwrap(),
        Admission::Accepted
    );
    assert!(queue.entries.is_empty());
    assert_eq!(
        queue
            .enqueue(
                mouse(MouseAction::Release(MouseButton::Left), 0),
                STAMP,
                true,
                busy
            )
            .unwrap(),
        Admission::Accepted
    );
    assert_eq!(queue.entries.len(), 1);
}
