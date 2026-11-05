use super::*;

/// What the fake backend saw, in order.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Event {
    /// A save of the capture with this watermark.
    Saved(u64),
    Removed,
    Marked,
    Cleared,
}

/// A held save's start signal and its release.
type Hold = (mpsc::Sender<()>, mpsc::Receiver<()>);

/// A backend that records its calls and can hold its next save before
/// "renaming", so a test can act while the write is in flight.
#[derive(Clone, Default)]
struct Fake {
    events: Arc<Mutex<Vec<Event>>>,
    /// Signals when the held save starts, then waits for a release.
    hold: Arc<Mutex<Option<Hold>>>,
    /// Fails every save with this reason.
    fail: Arc<Mutex<Option<String>>>,
}

impl Fake {
    fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }

    fn record(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }

    /// Holds the next save; returns its start signal and its release.
    fn hold_next(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (started, entered) = mpsc::channel();
        let (release, released) = mpsc::channel();
        *self.hold.lock().unwrap() = Some((started, released));
        (entered, release)
    }
}

impl Backend for Fake {
    fn save(
        &mut self,
        capture: &RuntimeCapture,
        _: &DesktopLayout,
    ) -> Result<(), SaveError> {
        if let Some((started, release)) = self.hold.lock().unwrap().take() {
            started.send(()).unwrap();
            release.recv().unwrap();
        }
        if let Some(reason) = self.fail.lock().unwrap().clone() {
            return Err(SaveError::Io(io::Error::other(reason)));
        }
        self.record(Event::Saved(capture.next_id));
        Ok(())
    }

    fn remove_saved(&mut self) -> io::Result<()> {
        self.record(Event::Removed);
        Ok(())
    }

    fn mark_restore_attempt(&mut self) -> io::Result<()> {
        self.record(Event::Marked);
        Ok(())
    }

    fn clear_restore_attempt(&mut self) -> io::Result<()> {
        self.record(Event::Cleared);
        Ok(())
    }
}

/// A capture told apart by its watermark.
fn saved(next_id: u64) -> Saved {
    (
        RuntimeCapture {
            socket_name: "default".into(),
            next_id,
            session_ordinal: 0,
            workspace_ordinal: 0,
            sessions: Vec::new(),
            terminals: Vec::new(),
        },
        DesktopLayout::default(),
    )
}

fn owner(fake: &Fake, state: State) -> Persistence {
    Persistence::new(Some(Box::new(fake.clone())), state, false, |_| {})
}

#[test]
fn quit_waits_for_a_checkpoint_in_flight_and_writes_after_it() {
    let fake = Fake::default();
    let persistence = Arc::new(owner(&fake, State::Running));
    let (entered, release) = fake.hold_next();
    let (capture, layout) = saved(1);
    assert!(persistence.checkpoint(capture, layout));
    entered.recv().unwrap();
    // Queued behind the held write; it must not land after Quit's.
    let (capture, layout) = saved(3);
    assert!(persistence.checkpoint(capture, layout));
    let quitting = Arc::clone(&persistence);
    let quit = std::thread::spawn(move || quitting.finish(|| saved(2)));
    // Quit has moved to Final and now waits on the held write.
    let deadline = Instant::now() + Duration::from_secs(10);
    while persistence.state() != State::Final {
        assert!(Instant::now() < deadline, "Quit never started");
        std::thread::yield_now();
    }
    assert!(!quit.is_finished(), "Quit waits for the write in flight");
    release.send(()).unwrap();
    let (written, _) = quit.join().unwrap().unwrap();
    assert!(matches!(written, Written::Saved), "{written:?}");
    persistence.flush();
    assert_eq!(fake.events(), [Event::Saved(1), Event::Saved(2)]);
    assert_eq!(persistence.state(), State::Final);
}

#[test]
fn only_the_first_termination_writes() {
    let fake = Fake::default();
    let persistence = owner(&fake, State::Running);
    assert!(persistence.finish(|| saved(1)).is_some());
    assert!(
        persistence
            .finish(|| panic!("a later termination captures nothing"))
            .is_none()
    );
    let (capture, layout) = saved(2);
    assert!(!persistence.checkpoint(capture, layout));
    assert!(!persistence.set_enabled(true));
    persistence.flush();
    assert_eq!(fake.events(), [Event::Saved(1)]);
}

#[test]
fn turning_off_deletes_after_the_write_in_flight_and_on_writes_again() {
    let fake = Fake::default();
    let persistence = owner(&fake, State::Running);
    let (entered, release) = fake.hold_next();
    let (capture, layout) = saved(1);
    persistence.checkpoint(capture, layout);
    entered.recv().unwrap();
    assert!(!persistence.set_enabled(false));
    assert_eq!(persistence.state(), State::Off);
    let (capture, layout) = saved(2);
    assert!(!persistence.checkpoint(capture, layout));
    release.send(()).unwrap();
    persistence.flush();
    assert_eq!(fake.events(), [Event::Saved(1), Event::Removed]);
    // Turning it on asks for a checkpoint, which writes even an unchanged
    // capture, since the removal emptied the files.
    assert!(persistence.set_enabled(true));
    let (capture, layout) = saved(1);
    assert!(persistence.checkpoint(capture, layout));
    persistence.flush();
    assert_eq!(
        fake.events(),
        [Event::Saved(1), Event::Removed, Event::Saved(1)]
    );
}

#[test]
fn an_unchanged_capture_is_not_written_again() {
    let fake = Fake::default();
    let persistence = owner(&fake, State::Running);
    for next_id in [1, 1, 2] {
        let (capture, layout) = saved(next_id);
        persistence.checkpoint(capture, layout);
    }
    persistence.flush();
    let (written, _) = persistence.finish(|| saved(2)).unwrap();
    assert!(matches!(written, Written::Unchanged), "{written:?}");
    assert_eq!(fake.events(), [Event::Saved(1), Event::Saved(2)]);
}

#[test]
fn quit_during_a_restore_only_removes_the_marker() {
    let fake = Fake::default();
    let persistence = owner(&fake, State::Bootstrapping);
    let (capture, layout) = saved(1);
    assert!(!persistence.checkpoint(capture, layout));
    persistence.mark_restore_attempt();
    persistence
        .finish(|| panic!("a restore in progress captures nothing"))
        .unwrap();
    // A spawn that finishes after Quit writes no marker again, and a
    // settle that loses the race starts nothing.
    persistence.mark_restore_attempt();
    assert!(!persistence.settle());
    persistence.flush();
    assert_eq!(fake.events(), [Event::Marked, Event::Cleared]);
}

#[test]
fn settling_a_restore_clears_the_marker_and_starts_checkpoints() {
    let fake = Fake::default();
    let persistence = owner(&fake, State::Bootstrapping);
    persistence.mark_restore_attempt();
    assert!(persistence.settle());
    let (capture, layout) = saved(1);
    assert!(persistence.checkpoint(capture, layout));
    persistence.flush();
    assert_eq!(
        fake.events(),
        [Event::Marked, Event::Cleared, Event::Saved(1)]
    );
    // A blocked owner never runs, even after a restore.
    let blocked = Persistence::new(
        Some(Box::new(fake.clone())),
        State::Bootstrapping,
        true,
        |_| {},
    );
    assert!(!blocked.settle());
    assert!(!blocked.set_enabled(true));
    assert_eq!(blocked.state(), State::Off);
}

#[test]
fn a_failed_write_is_reported_once_and_retried_at_the_next_change() {
    let fake = Fake::default();
    let (report, reports) = mpsc::channel();
    let persistence = Persistence::new(
        Some(Box::new(fake.clone())),
        State::Running,
        false,
        move |reason| report.send(reason).unwrap(),
    );
    *fake.fail.lock().unwrap() = Some("disk full".into());
    for _ in 0..2 {
        let (capture, layout) = saved(1);
        persistence.checkpoint(capture, layout);
    }
    persistence.flush();
    assert!(reports.recv().unwrap().contains("disk full"));
    assert!(reports.try_recv().is_err());
    *fake.fail.lock().unwrap() = None;
    let (capture, layout) = saved(1);
    persistence.checkpoint(capture, layout);
    persistence.flush();
    assert_eq!(fake.events(), [Event::Saved(1)]);
}
