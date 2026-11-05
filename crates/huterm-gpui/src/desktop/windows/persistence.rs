//! The persistence owner: the one object that orders every write of a
//! socket scope's saved state.
//!
//! [`Persistence`] is shared between the UI thread and the structural
//! workers through [`DesktopRuntime`](super::DesktopRuntime), because the
//! Quit write runs inside `commit_close_with`'s capture callback on a
//! worker. Checkpoints, removals, and marker clears go to one writer thread
//! in the order they were queued. The final write waits for that queue to
//! drain, then runs on the caller's thread, and nothing is written after
//! it. Every write takes the slot lock and checks the state under it, so a
//! queued checkpoint that loses the race against Final or Off writes
//! nothing.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use huterm_protocol::RuntimeCapture;
use huterm_state::{DesktopLayout, SaveError, Store};

/// How long after the last change a checkpoint is written.
pub(super) const CHECKPOINT_DELAY: Duration = Duration::from_secs(1);

/// Where persistence stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum State {
    /// Setting off, lock not held, newer version, or bad socket name.
    /// Rejects writes.
    Off,
    /// A restore is in progress. Rejects writes.
    Bootstrapping,
    /// Accepts checkpoints.
    Running,
    /// Took the Quit write. Rejects everything after it.
    Final,
}

/// Where saves land: the scope's [`Store`], or a test double.
pub(super) trait Backend: Send {
    /// Writes both files.
    fn save(
        &mut self,
        capture: &RuntimeCapture,
        layout: &DesktopLayout,
    ) -> Result<(), SaveError>;
    /// Deletes the saved files a save may replace.
    fn remove_saved(&mut self) -> io::Result<()>;
    /// Creates the crash-loop marker.
    fn mark_restore_attempt(&mut self) -> io::Result<()>;
    /// Removes the crash-loop marker.
    fn clear_restore_attempt(&mut self) -> io::Result<()>;
}

impl Backend for Store {
    fn save(
        &mut self,
        capture: &RuntimeCapture,
        layout: &DesktopLayout,
    ) -> Result<(), SaveError> {
        Store::save(self, capture, layout).map(|_| ())
    }

    fn remove_saved(&mut self) -> io::Result<()> {
        Store::remove_saved(self)
    }

    fn mark_restore_attempt(&mut self) -> io::Result<()> {
        Store::mark_restore_attempt(self)
    }

    fn clear_restore_attempt(&mut self) -> io::Result<()> {
        Store::clear_restore_attempt(self)
    }
}

/// A capture and layout written together.
type Saved = (RuntimeCapture, DesktopLayout);

/// What a write found.
#[derive(Debug)]
pub(super) enum Written {
    /// The files now hold it.
    Saved,
    /// They already held it.
    Unchanged,
    /// The state rejected it.
    Rejected,
    /// A checkpoint whose capture started before one already written.
    Superseded,
    /// The save failed and was reported; the previous files stay.
    Failed,
}

struct Slot {
    backend: Box<dyn Backend>,
    /// The last capture and layout written, so an unchanged one is skipped.
    last: Option<Saved>,
    /// The sequence of the newest checkpoint written; an older one that
    /// reaches the writer later is dropped.
    newest: u64,
}

struct Shared {
    /// Never held across I/O, so the UI thread can read it at any time.
    state: Mutex<State>,
    /// `None` when the scope has no store. Held across every write.
    slot: Mutex<Option<Slot>>,
    /// Whether a failure was already reported this run.
    reported: AtomicBool,
    /// Receives the first write failure of the run.
    failure: Box<dyn Fn(String) + Send + Sync>,
}

impl Shared {
    fn state(&self) -> State {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn slot(&self) -> MutexGuard<'_, Option<Slot>> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes `saved` while the state is `allowed`, under the slot lock. A
    /// checkpoint's `sequence` drops it when a capture that started later
    /// was already written; the final write has none.
    fn write(
        &self,
        saved: Saved,
        allowed: State,
        sequence: Option<u64>,
    ) -> Written {
        let mut slot = self.slot();
        if self.state() != allowed {
            return Written::Rejected;
        }
        let Some(slot) = slot.as_mut() else {
            return Written::Rejected;
        };
        if sequence.is_some_and(|sequence| sequence < slot.newest) {
            return Written::Superseded;
        }
        let sequence = sequence.unwrap_or(slot.newest);
        if slot.last.as_ref() == Some(&saved) {
            slot.newest = sequence;
            return Written::Unchanged;
        }
        match slot.backend.save(&saved.0, &saved.1) {
            Ok(()) => {
                slot.last = Some(saved);
                slot.newest = sequence;
                Written::Saved
            }
            Err(error) => {
                let reason = error.to_string();
                eprintln!("Huterm could not save sessions: {reason}");
                if !self.reported.swap(true, Ordering::AcqRel) {
                    (self.failure)(reason.clone());
                }
                Written::Failed
            }
        }
    }

    /// Runs `job` against the backend, whatever the state.
    fn with_backend(
        &self,
        what: &str,
        job: impl FnOnce(&mut Slot) -> io::Result<()>,
    ) {
        if let Some(slot) = self.slot().as_mut()
            && let Err(error) = job(slot)
        {
            eprintln!("Huterm saved state {what} failed: {error}");
        }
    }

    fn remove_saved(&self) {
        self.with_backend("removal", |slot| {
            slot.last = None;
            slot.backend.remove_saved()
        });
    }

    fn clear_marker(&self) {
        self.with_backend("marker removal", |slot| {
            slot.backend.clear_restore_attempt()
        });
    }
}

enum Job {
    /// A checkpoint and the sequence its capture started at.
    Save(u64, Box<Saved>),
    Remove,
    ClearMarker,
    /// Answers once every earlier job is handled.
    Barrier(mpsc::Sender<()>),
    /// Stops the writer until the sender drops, without the slot lock.
    #[cfg(test)]
    Pause(mpsc::Receiver<()>),
}

/// The persistence owner. See the module documentation.
pub(super) struct Persistence {
    shared: Arc<Shared>,
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    /// No usable store, one whose runtime file is newer, or a restore that
    /// failed: the setting cannot turn writes on for the rest of the run.
    blocked: AtomicBool,
    /// `[general] restore_sessions` as last applied while a restore runs;
    /// the restore applies it when it settles.
    enabled: AtomicBool,
    /// The last checkpoint sequence handed out.
    sequence: AtomicU64,
    /// Jobs queued so far, so tests can see a job reach the queue.
    #[cfg(test)]
    sent: std::sync::atomic::AtomicUsize,
}

impl std::fmt::Debug for Persistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Persistence")
            .field("state", &self.state())
            .field("blocked", &self.blocked())
            .finish_non_exhaustive()
    }
}

impl Default for Persistence {
    /// No store: off for the whole run.
    fn default() -> Self {
        Self::new(None, State::Off, true, |_| {})
    }
}

impl Persistence {
    /// An owner over `backend` starting in `state`. With `blocked` set, or
    /// no backend, it never moves to `Running`. `failure` receives the
    /// run's first write failure, from whichever thread wrote.
    pub(super) fn new(
        backend: Option<Box<dyn Backend>>,
        state: State,
        blocked: bool,
        failure: impl Fn(String) + Send + Sync + 'static,
    ) -> Self {
        let blocked = blocked || backend.is_none();
        let state = if blocked && state != State::Bootstrapping {
            State::Off
        } else {
            state
        };
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(state),
                slot: Mutex::new(backend.map(|backend| Slot {
                    backend,
                    last: None,
                    newest: 0,
                })),
                reported: AtomicBool::new(false),
                failure: Box::new(failure),
            }),
            jobs: Mutex::new(None),
            blocked: AtomicBool::new(blocked),
            enabled: AtomicBool::new(true),
            sequence: AtomicU64::new(0),
            #[cfg(test)]
            sent: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub(super) fn state(&self) -> State {
        self.shared.state()
    }

    fn blocked(&self) -> bool {
        self.blocked.load(Ordering::Acquire)
    }

    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Moves from `from` to `to`; returns whether it did.
    fn transition(&self, from: &[State], to: State) -> bool {
        let mut state = self.lock_state();
        if from.contains(&state) {
            *state = to;
            true
        } else {
            false
        }
    }

    /// Queues `job` for the writer thread, starting it on first use.
    fn send(&self, job: Job) {
        let mut jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
        let sender = jobs.get_or_insert_with(|| {
            let (sender, receiver) = mpsc::channel::<Job>();
            let shared = Arc::clone(&self.shared);
            std::thread::Builder::new()
                .name("huterm-persistence".into())
                .spawn(move || {
                    while let Ok(job) = receiver.recv() {
                        match job {
                            Job::Save(sequence, saved) => {
                                shared.write(
                                    *saved,
                                    State::Running,
                                    Some(sequence),
                                );
                            }
                            Job::Remove => shared.remove_saved(),
                            Job::ClearMarker => shared.clear_marker(),
                            Job::Barrier(done) => {
                                let _ = done.send(());
                            }
                            #[cfg(test)]
                            Job::Pause(resume) => {
                                let _ = resume.recv();
                            }
                        }
                    }
                })
                .expect("cannot start the persistence writer");
            sender
        });
        #[cfg(test)]
        self.sent.fetch_add(1, Ordering::AcqRel);
        // The writer only stops when this sender drops.
        let _ = sender.send(job);
    }

    /// Starts a checkpoint capture: returns the sequence it writes under.
    /// Take it before capturing, so captures that finish out of order
    /// still land in the order they started.
    pub(super) fn begin_capture(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Queues a checkpoint; it writes nothing unless the state is still
    /// `Running` when the writer reaches it. Returns whether it was queued.
    pub(super) fn checkpoint(
        &self,
        sequence: u64,
        capture: RuntimeCapture,
        layout: DesktopLayout,
    ) -> bool {
        if self.state() != State::Running {
            return false;
        }
        self.send(Job::Save(sequence, Box::new((capture, layout))));
        true
    }

    /// Takes the Quit write, once: moves to `Final`, waits until the writer
    /// handled every job queued before, so a removal or marker clear lands
    /// first, and writes `saved()` synchronously when the state was
    /// `Running`. From `Bootstrapping` it removes only the crash-loop
    /// marker, so the next launch restores the same state. Returns `None`
    /// when an earlier call already took it.
    pub(super) fn finish(
        &self,
        saved: impl FnOnce() -> Saved,
    ) -> Option<(Written, Duration)> {
        let previous = {
            let mut state = self.lock_state();
            if *state == State::Final {
                return None;
            }
            std::mem::replace(&mut *state, State::Final)
        };
        let started = Instant::now();
        // Checkpoints queued behind it find `Final` and write nothing.
        self.flush();
        let written = match previous {
            State::Running => {
                // `Final` also lets this write through `write`'s check.
                self.shared.write(saved(), State::Final, None)
            }
            State::Bootstrapping => {
                self.shared.clear_marker();
                Written::Rejected
            }
            State::Off | State::Final => Written::Rejected,
        };
        Some((written, started.elapsed()))
    }

    /// Writes the crash-loop marker while a restore runs. On a worker.
    pub(super) fn mark_restore_attempt(&self) {
        let mut slot = self.shared.slot();
        if self.state() != State::Bootstrapping {
            return;
        }
        if let Some(slot) = slot.as_mut()
            && let Err(error) = slot.backend.mark_restore_attempt()
        {
            eprintln!("Huterm restore marker failed: {error}");
        }
    }

    /// Ends a restore: the writer removes the marker, then checkpoints
    /// start, or, when the setting turned off during the restore, the
    /// writer deletes the saved files. Both jobs are queued before the
    /// state moves, so a Quit that follows waits for them. Returns whether
    /// persistence is now running.
    pub(super) fn settle(&self) -> bool {
        if self.state() != State::Bootstrapping {
            return false;
        }
        let blocked = self.blocked();
        let next = if blocked || !self.enabled.load(Ordering::Acquire) {
            State::Off
        } else {
            State::Running
        };
        self.send(Job::ClearMarker);
        if next == State::Off && !blocked {
            self.send(Job::Remove);
        }
        self.transition(&[State::Bootstrapping], next) && next == State::Running
    }

    /// Applies `[general] restore_sessions`. Off stops checkpoints and has
    /// the writer delete the saved files after every job queued before;
    /// on starts checkpoints. During a restore it only records the value,
    /// which the restore applies when it settles. Returns whether the
    /// caller should submit a checkpoint now.
    pub(super) fn set_enabled(&self, enabled: bool) -> bool {
        let mut state = self.lock_state();
        match *state {
            State::Bootstrapping => {
                self.enabled.store(enabled, Ordering::Release);
                false
            }
            State::Off if enabled && !self.blocked() => {
                *state = State::Running;
                true
            }
            State::Running if !enabled => {
                *state = State::Off;
                drop(state);
                self.send(Job::Remove);
                false
            }
            State::Off | State::Running | State::Final => false,
        }
    }

    /// Stops saving for the rest of the run without touching the files,
    /// as when a restore failed and its saved state must survive. Like a
    /// blocked store, the setting cannot turn saving on again. The
    /// crash-loop marker stays, so the next launch sets the files aside.
    pub(super) fn suspend(&self) {
        self.blocked.store(true, Ordering::Release);
        self.transition(&[State::Bootstrapping, State::Running], State::Off);
    }

    /// Waits until the writer has handled every job queued so far. Returns
    /// at once when nothing was ever queued.
    pub(super) fn flush(&self) {
        let jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(sender) = jobs.clone() else {
            return;
        };
        drop(jobs);
        let (done, finished) = mpsc::channel();
        if sender.send(Job::Barrier(done)).is_ok() {
            #[cfg(test)]
            self.sent.fetch_add(1, Ordering::AcqRel);
            let _ = finished.recv();
        }
    }

    /// Stops the writer before its next job until the returned sender
    /// drops, without holding the slot lock.
    #[cfg(test)]
    pub(super) fn pause(&self) -> mpsc::Sender<()> {
        let (resume, paused) = mpsc::channel();
        self.send(Job::Pause(paused));
        resume
    }

    /// How many jobs were queued so far.
    #[cfg(test)]
    pub(super) fn sent(&self) -> usize {
        self.sent.load(Ordering::Acquire)
    }
}

/// A backend that keeps every save in memory and counts its other calls,
/// for desktop tests.
#[cfg(test)]
#[derive(Clone, Default)]
pub(super) struct Recorder(Arc<Mutex<Recorded>>);

#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(super) struct Recorded {
    pub(super) saved: Vec<Saved>,
    pub(super) removed: usize,
    pub(super) marked: usize,
    pub(super) cleared: usize,
}

#[cfg(test)]
impl Recorder {
    /// Every capture and layout saved so far, oldest first.
    pub(super) fn saved(&self) -> Vec<Saved> {
        self.recorded().saved
    }

    pub(super) fn recorded(&self) -> Recorded {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn with(&self, f: impl FnOnce(&mut Recorded)) {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// An owner over this recorder starting in `state`.
    pub(super) fn owner(&self, state: State) -> Persistence {
        Persistence::new(Some(Box::new(self.clone())), state, false, |_| {})
    }
}

#[cfg(test)]
impl Backend for Recorder {
    fn save(
        &mut self,
        capture: &RuntimeCapture,
        layout: &DesktopLayout,
    ) -> Result<(), SaveError> {
        self.with(|recorded| {
            recorded.saved.push((capture.clone(), layout.clone()));
        });
        Ok(())
    }

    fn remove_saved(&mut self) -> io::Result<()> {
        self.with(|recorded| recorded.removed += 1);
        Ok(())
    }

    fn mark_restore_attempt(&mut self) -> io::Result<()> {
        self.with(|recorded| recorded.marked += 1);
        Ok(())
    }

    fn clear_restore_attempt(&mut self) -> io::Result<()> {
        self.with(|recorded| recorded.cleared += 1);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
