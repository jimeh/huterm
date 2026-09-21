//! Bounded terminal event state with a coalescing wake signal.

use std::collections::VecDeque;
use std::sync::mpsc::{SendError, TryRecvError};
use std::sync::{Arc, Mutex, Weak};

use huterm_protocol::TerminalEvent;

type Queue = Mutex<VecDeque<TerminalEvent>>;

#[derive(Clone, Debug)]
pub(crate) struct EventPublisher {
    queue: Weak<Queue>,
    pub(crate) activity: Arc<async_channel::Sender<()>>,
}

#[derive(Debug)]
pub(crate) struct EventReceiver {
    queue: Arc<Queue>,
    publisher: Weak<async_channel::Sender<()>>,
}

impl EventPublisher {
    pub(crate) fn channel() -> (Self, EventReceiver, async_channel::Receiver<()>)
    {
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let (activity, signal) = async_channel::bounded(1);
        let activity = Arc::new(activity);
        (
            Self {
                queue: Arc::downgrade(&queue),
                activity: Arc::clone(&activity),
            },
            EventReceiver {
                queue,
                publisher: Arc::downgrade(&activity),
            },
            signal,
        )
    }

    pub(crate) fn send(
        &self,
        event: TerminalEvent,
    ) -> Result<(), SendError<TerminalEvent>> {
        let Some(queue) = self.queue.upgrade() else {
            return Err(SendError(event));
        };
        let mut queue = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = queue.iter().position(|pending| {
            std::mem::discriminant(pending) == std::mem::discriminant(&event)
        }) {
            // Lifecycle is delivered once with the first failure reason. Titles,
            // metadata, invalidations, and bells describe the latest pending state.
            if matches!(
                event,
                TerminalEvent::Ready(_)
                    | TerminalEvent::Exited { .. }
                    | TerminalEvent::Failed { .. }
            ) {
                return Ok(());
            }
            queue.remove(index);
        }
        queue.push_back(event);
        drop(queue);
        let _ = self.activity.try_send(());
        Ok(())
    }
}

impl EventReceiver {
    pub(crate) fn try_recv(&self) -> Result<TerminalEvent, TryRecvError> {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .ok_or_else(|| {
                if self.publisher.strong_count() == 0 {
                    TryRecvError::Disconnected
                } else {
                    TryRecvError::Empty
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use huterm_protocol::{ExitStatus, TerminalId};

    #[test]
    fn flood_keeps_latest_state_and_lifecycle_in_bounded_space() {
        let (publisher, receiver, signal) = EventPublisher::channel();
        let id = TerminalId::new(1);
        publisher.send(TerminalEvent::Ready(id)).unwrap();
        for index in 0..10_000 {
            publisher
                .send(TerminalEvent::TitleChanged {
                    terminal_id: id,
                    title: index.to_string(),
                })
                .unwrap();
            publisher.send(TerminalEvent::Bell(id)).unwrap();
        }
        publisher
            .send(TerminalEvent::Exited {
                terminal_id: id,
                status: ExitStatus {
                    code: Some(0),
                    success: true,
                },
            })
            .unwrap();
        assert_eq!(signal.len(), 1);
        assert_eq!(receiver.queue.lock().unwrap().len(), 4);
        assert!(matches!(receiver.try_recv(), Ok(TerminalEvent::Ready(_))));
        assert!(
            matches!(receiver.try_recv(), Ok(TerminalEvent::TitleChanged { title, .. }) if title == "9999")
        );
        assert!(matches!(receiver.try_recv(), Ok(TerminalEvent::Bell(_))));
        assert!(matches!(
            receiver.try_recv(),
            Ok(TerminalEvent::Exited { .. })
        ));
    }

    #[test]
    fn closure_preserves_queued_events_and_does_not_require_another_wake() {
        let (publisher, receiver, signal) = EventPublisher::channel();
        publisher
            .send(TerminalEvent::Bell(TerminalId::new(1)))
            .unwrap();
        signal.try_recv().unwrap();
        drop(publisher);
        assert!(matches!(receiver.try_recv(), Ok(TerminalEvent::Bell(_))));
        assert!(matches!(
            receiver.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
        assert!(signal.is_closed());
    }
}
