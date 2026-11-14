//! Host effects the driver hands to the caller, apart from the event
//! channel.
//!
//! The event channel drops events while it is full and then asks the
//! caller to re-read state, which recovers a projection but not an effect:
//! a dropped clipboard write is gone. Effects therefore wait in a queue of
//! their own, bounded as one desktop's host-effect budget is, at
//! [`EFFECT_BACKLOG_LIMIT`] effects and [`EFFECT_BACKLOG_BYTES`] of text.
//! A caller that falls further behind loses the oldest effects, never the
//! newest, so its clipboard still ends with the last write; each loss is
//! counted. The driver never waits for the caller.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use async_channel::{Receiver, Sender};
use huterm_wire::effect::EffectPayload;

use crate::session::EffectDelivery;

/// The most effects the queue holds for a caller that does not take them.
pub const EFFECT_BACKLOG_LIMIT: usize = 32;

/// The most payload bytes the queue holds for a caller that does not take
/// them: two maximum-size clipboard writes.
pub const EFFECT_BACKLOG_BYTES: usize = 128 << 20;

fn payload_bytes(delivery: &EffectDelivery) -> usize {
    match &delivery.payload {
        EffectPayload::ClipboardWrite { text, .. } => text.len(),
        EffectPayload::Notification { title, body } => title.len() + body.len(),
    }
}

#[derive(Debug, Default)]
struct Backlog {
    queue: VecDeque<EffectDelivery>,
    bytes: usize,
    dropped: u64,
}

/// The queue the driver fills and the handles drain.
#[derive(Debug)]
pub(super) struct Effects {
    backlog: Mutex<Backlog>,
    /// Signalled when an effect arrives; closed when the client shuts down.
    wake: Sender<()>,
    woken: Receiver<()>,
}

impl Effects {
    pub(super) fn new() -> Arc<Self> {
        let (wake, woken) = async_channel::bounded(1);
        Arc::new(Self {
            backlog: Mutex::new(Backlog::default()),
            wake,
            woken,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Backlog> {
        self.backlog.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues `delivery`, dropping the oldest effects while the queue is
    /// past either bound.
    pub(super) fn push(&self, delivery: EffectDelivery) {
        let bytes = payload_bytes(&delivery);
        let mut backlog = self.lock();
        while !backlog.queue.is_empty()
            && (backlog.queue.len() >= EFFECT_BACKLOG_LIMIT
                || backlog.bytes + bytes > EFFECT_BACKLOG_BYTES)
        {
            if let Some(oldest) = backlog.queue.pop_front() {
                backlog.bytes -= payload_bytes(&oldest);
                backlog.dropped += 1;
            }
        }
        backlog.bytes += bytes;
        backlog.queue.push_back(delivery);
        drop(backlog);
        let _ = self.wake.try_send(());
    }

    /// Ends the queue: callers take what remains, then get `None`.
    pub(super) fn close(&self) {
        self.wake.close();
    }

    fn pop(&self) -> Option<EffectDelivery> {
        let mut backlog = self.lock();
        let delivery = backlog.queue.pop_front()?;
        backlog.bytes -= payload_bytes(&delivery);
        let more = !backlog.queue.is_empty();
        drop(backlog);
        // The wake holds one signal for any number of pushes, so a caller
        // that leaves effects behind passes it on to the next waiting one.
        if more {
            let _ = self.wake.try_send(());
        }
        Some(delivery)
    }

    /// The next effect, waiting for one; `None` once the client shut down
    /// and the queue is empty.
    pub(super) async fn next(&self) -> Option<EffectDelivery> {
        loop {
            if let Some(delivery) = self.pop() {
                return Some(delivery);
            }
            if self.woken.recv().await.is_err() {
                return self.pop();
            }
        }
    }

    /// Effects dropped because the caller fell behind.
    pub(super) fn dropped(&self) -> u64 {
        self.lock().dropped
    }
}

#[cfg(test)]
mod tests {
    use huterm_protocol::{
        ClipboardDestination, RuntimeId, TerminalId, ViewerId,
    };

    use super::*;

    fn write(text: String) -> EffectDelivery {
        let runtime = RuntimeId::new(1);
        EffectDelivery {
            viewer: ViewerId::in_runtime(runtime, 1),
            terminal: TerminalId::in_runtime(runtime, 1),
            generation: 1,
            payload: EffectPayload::ClipboardWrite {
                destination: ClipboardDestination::System,
                text,
            },
        }
    }

    fn texts(effects: &Effects) -> Vec<String> {
        std::iter::from_fn(|| effects.pop())
            .map(|delivery| match delivery.payload {
                EffectPayload::ClipboardWrite { text, .. } => text,
                payload @ EffectPayload::Notification { .. } => {
                    panic!("a clipboard write, not {payload:?}")
                }
            })
            .collect()
    }

    #[test]
    fn a_caller_that_falls_behind_loses_the_oldest_effects_by_count_and_bytes()
    {
        let effects = Effects::new();
        for n in 0..=EFFECT_BACKLOG_LIMIT {
            effects.push(write(n.to_string()));
        }
        assert_eq!(effects.dropped(), 1);
        let kept = texts(&effects);
        assert_eq!(kept.len(), EFFECT_BACKLOG_LIMIT);
        assert_eq!(kept.first().map(String::as_str), Some("1"));
        assert_eq!(
            kept.last().map(String::as_str),
            Some(EFFECT_BACKLOG_LIMIT.to_string().as_str())
        );

        let half = EFFECT_BACKLOG_BYTES / 2;
        effects.push(write("a".repeat(half)));
        effects.push(write("b".repeat(half)));
        effects.push(write("c".to_owned()));
        assert_eq!(effects.dropped(), 2);
        let kept = texts(&effects);
        assert_eq!(kept.len(), 2);
        assert!(kept[0].starts_with('b') && kept[1] == "c");
    }

    #[test]
    fn waiting_callers_each_get_an_effect_and_end_after_close() {
        use std::pin::pin;

        use futures_lite::future::{block_on, poll_once};

        let effects = Effects::new();
        let mut first = pin!(effects.next());
        let mut second = pin!(effects.next());
        // Both wait on an empty queue before two effects arrive at once.
        assert!(block_on(poll_once(first.as_mut())).is_none());
        assert!(block_on(poll_once(second.as_mut())).is_none());
        effects.push(write("one".into()));
        effects.push(write("two".into()));
        let one = block_on(first).unwrap();
        // The second caller is ready without another push.
        let two = block_on(poll_once(second.as_mut()))
            .expect("the second caller was left waiting")
            .unwrap();
        let mut got = vec![one, two];
        got.sort_by_key(|delivery| match &delivery.payload {
            EffectPayload::ClipboardWrite { text, .. } => text.clone(),
            EffectPayload::Notification { body, .. } => body.clone(),
        });
        assert_eq!(texts_of(got), ["one", "two"]);

        let mut waiting = pin!(effects.next());
        assert!(block_on(poll_once(waiting.as_mut())).is_none());
        effects.close();
        assert!(block_on(waiting).is_none());
    }

    fn texts_of(deliveries: Vec<EffectDelivery>) -> Vec<String> {
        deliveries
            .into_iter()
            .map(|delivery| match delivery.payload {
                EffectPayload::ClipboardWrite { text, .. } => text,
                payload @ EffectPayload::Notification { .. } => {
                    panic!("a clipboard write, not {payload:?}")
                }
            })
            .collect()
    }
}
