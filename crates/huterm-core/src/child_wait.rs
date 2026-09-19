//! Blocking child reaping without holding a lock needed by runtime teardown.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use portable_pty::{Child, ChildKiller, ExitStatus};

use crate::wake::Wake;

type OwnedChild = Box<dyn Child + Send + Sync>;

#[derive(Debug, Default)]
struct State {
    result: Mutex<WaitResult>,
    ready: Condvar,
}

#[derive(Debug, Default)]
struct WaitResult {
    status: Option<ExitStatus>,
    error: Option<(std::io::ErrorKind, String)>,
}

#[derive(Debug)]
struct ObservedChild {
    pid: Option<u32>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    state: Arc<State>,
}

/// A failed thread spawn returns the original child to the caller's cleanup path.
pub(crate) fn observe(
    child: OwnedChild,
    wake: Arc<Wake>,
) -> Result<OwnedChild, (std::io::Error, OwnedChild)> {
    let pid = child.process_id();
    observe_with(child, wake, |task| {
        std::thread::Builder::new()
            .name(format!("huterm-child-{}", pid.unwrap_or_default()))
            .spawn(task)
            .map(drop)
    })
}

type WaitTask = Box<dyn FnOnce() + Send>;

fn observe_with(
    child: OwnedChild,
    wake: Arc<Wake>,
    spawn: impl FnOnce(WaitTask) -> std::io::Result<()>,
) -> Result<OwnedChild, (std::io::Error, OwnedChild)> {
    let pid = child.process_id();
    let killer = child.clone_killer();
    let state = Arc::new(State::default());
    let observing = Arc::clone(&state);
    let owner = Arc::new(Mutex::new(Some(child)));
    let worker_owner = Arc::clone(&owner);
    let spawn = spawn(Box::new(move || {
        let Some(mut child) = worker_owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            return;
        };
        // No shared lock is held across wait. Signals and descriptor closure
        // remain available even when the child never voluntarily exits.
        loop {
            match child.wait() {
                Ok(status) => {
                    let mut result = observing
                        .result
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    result.status = Some(status);
                    result.error = None;
                    drop(result);
                    observing.ready.notify_all();
                    wake.notify();
                    return;
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => {
                    observing
                        .result
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .error = Some((error.kind(), error.to_string()));
                    wake.notify();
                    // Error recovery only. Retain ownership until reaped, as
                    // the deferred teardown reaper does for repeated errors.
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }));
    if let Err(error) = spawn {
        let child = owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("failed spawn retains the original child");
        return Err((error, child));
    }
    Ok(Box::new(ObservedChild { pid, killer, state }))
}

impl ChildKiller for ObservedChild {
    fn kill(&mut self) -> std::io::Result<()> {
        self.killer.kill()
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        self.killer.clone_killer()
    }
}

impl Child for ObservedChild {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        let result = self
            .state
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(status) = &result.status {
            return Ok(Some(status.clone()));
        }
        if let Some((kind, message)) = &result.error {
            return Err(std::io::Error::new(*kind, message.clone()));
        }
        Ok(None)
    }
    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        let result = self
            .state
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = self
            .state
            .ready
            .wait_while(result, |result| result.status.is_none())
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(result.status.clone().expect("wait finished after reaping"))
    }
    fn process_id(&self) -> Option<u32> {
        self.pid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[derive(Debug)]
    struct Killer;
    impl ChildKiller for Killer {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(Self)
        }
    }

    #[derive(Debug)]
    struct TestChild {
        entered: mpsc::Sender<()>,
        results: Mutex<mpsc::Receiver<std::io::Result<ExitStatus>>>,
        dropped: mpsc::Sender<()>,
    }
    impl Drop for TestChild {
        fn drop(&mut self) {
            let _ = self.dropped.send(());
        }
    }
    impl ChildKiller for TestChild {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(Killer)
        }
    }
    impl Child for TestChild {
        fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
            Ok(None)
        }
        fn wait(&mut self) -> std::io::Result<ExitStatus> {
            self.entered.send(()).unwrap();
            self.results.lock().unwrap().recv().unwrap()
        }
        fn process_id(&self) -> Option<u32> {
            Some(123)
        }
    }

    #[test]
    fn waiter_retains_child_through_errors_and_publishes_reaped_status() {
        let (entered, waiting) = mpsc::channel();
        let (results, outcomes) = mpsc::channel();
        let (dropped, released) = mpsc::channel();
        let child = Box::new(TestChild {
            entered,
            results: Mutex::new(outcomes),
            dropped,
        });
        let mut observed = observe(child, Arc::new(Wake::default())).unwrap();
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        // Signalling remains callable while the physical waiter blocks.
        observed.kill().unwrap();
        assert!(observed.try_wait().unwrap().is_none());
        results
            .send(Err(std::io::ErrorKind::Interrupted.into()))
            .unwrap();
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        results.send(Err(std::io::Error::other("retry"))).unwrap();
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(observed.try_wait().unwrap_err().to_string(), "retry");
        assert!(released.try_recv().is_err());
        results.send(Ok(ExitStatus::with_exit_code(7))).unwrap();
        let (completed, completion) = mpsc::channel();
        let completion_worker = std::thread::spawn(move || {
            let waited = observed.wait().unwrap().exit_code();
            let cached = observed.try_wait().unwrap().unwrap().exit_code();
            completed.send((waited, cached)).unwrap();
        });
        assert_eq!(
            completion.recv_timeout(Duration::from_secs(2)).unwrap(),
            (7, 7)
        );
        completion_worker.join().unwrap();
        released.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn failed_waiter_spawn_returns_original_child_for_cleanup() {
        let (entered, _waiting) = mpsc::channel();
        let (_results, outcomes) = mpsc::channel();
        let (dropped, released) = mpsc::channel();
        let child = Box::new(TestChild {
            entered,
            results: Mutex::new(outcomes),
            dropped,
        });
        let (_, child) = observe_with(child, Arc::new(Wake::default()), |_| {
            Err(std::io::Error::other("cannot spawn"))
        })
        .unwrap_err();
        assert_eq!(child.process_id(), Some(123));
        assert!(released.try_recv().is_err());
        drop(child);
        released.recv_timeout(Duration::from_secs(2)).unwrap();
    }
}
