//! Reloads the configuration when it, or a theme file it uses, changes on
//! disk.
//!
//! A worker thread owns the `notify` watcher and every file access: the
//! `FSEvents` backend blocks while it starts and joins a thread when dropped,
//! so the GPUI thread only exchanges messages with it. The worker watches
//! directories, never files, so editors that save by renaming or by
//! deleting and recreating stay visible, and it reconciles after a quiet
//! period: first the watched directories, then a fingerprint of the files
//! the last load consumed. It asks for a reload only when that fingerprint
//! differs from the last *attempted* reload's, so restoring a good file
//! after a broken save reloads too. With no events it arms no timer and
//! reads nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use notify::event::ModifyKind;
use notify::{Event, EventKind, RecursiveMode, Watcher as _};

use crate::config::{self, Fingerprint, Inputs};

/// How long events must stop before the worker reconciles.
pub(super) const QUIET_PERIOD: Duration = Duration::from_millis(150);

/// Attempts to settle the watched directories when they keep vanishing
/// between resolution and registration.
const TOPOLOGY_ATTEMPTS: usize = 3;

/// What the desktop tells the worker after every load and reload.
#[derive(Clone, Debug)]
pub(super) struct Settings {
    pub(super) enabled: bool,
    /// The configuration file's logical path.
    pub(super) config: PathBuf,
    /// What the last attempted load consumed.
    pub(super) inputs: Inputs,
}

/// What the worker tells the desktop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Report {
    /// Events touched a watched input. `differs` says whether the content
    /// now differs from the last attempted reload; an event during a reload
    /// matters either way. `fingerprint` is the inputs' current one.
    Changed {
        differs: bool,
        fingerprint: Fingerprint,
    },
    /// Acknowledges every configuration and reconciliation.
    Status {
        watching: bool,
        reconciliations: u64,
    },
    /// The watcher could not start, or could not watch a directory that
    /// holds an input; changes there will not reload on their own.
    Unavailable(String),
}

enum Request {
    Configure(Settings),
    Event(notify::Result<Event>),
    #[cfg(test)]
    Probe(mpsc::Sender<Probe>),
    Shutdown,
}

/// The worker's state at one point in its message order, for tests.
#[cfg(test)]
#[derive(Debug)]
struct Probe {
    installed: Vec<PathBuf>,
    timer_armed: bool,
    reconciliations: u64,
}

/// Why a directory could not be watched.
#[derive(Debug)]
pub(super) enum WatchError {
    /// The directory vanished after the topology was resolved.
    NotFound,
    Other(String),
}

/// The notification backend, behind a seam so tests can hold registration
/// at a barrier or run without native events.
pub(super) trait Backend {
    /// Starts a watcher that watches nothing yet.
    fn start(&mut self) -> Result<(), String>;
    /// Drops the watcher and every registration.
    fn stop(&mut self);
    fn watch(&mut self, directory: &Path) -> Result<(), WatchError>;
    fn unwatch(&mut self, directory: &Path);
}

/// The platform watcher: `FSEvents` on macOS, inotify on Linux.
pub(super) struct NotifyBackend {
    events: mpsc::Sender<Request>,
    watcher: Option<notify::RecommendedWatcher>,
}

impl Backend for NotifyBackend {
    fn start(&mut self) -> Result<(), String> {
        let events = self.events.clone();
        let watcher = notify::recommended_watcher(move |event| {
            let _ = events.send(Request::Event(event));
        })
        .map_err(|error| error.to_string())?;
        self.watcher = Some(watcher);
        Ok(())
    }

    fn stop(&mut self) {
        self.watcher = None;
    }

    fn watch(&mut self, directory: &Path) -> Result<(), WatchError> {
        let Some(watcher) = self.watcher.as_mut() else {
            return Err(WatchError::Other("the watcher is not running".into()));
        };
        watcher
            .watch(directory, RecursiveMode::NonRecursive)
            .map_err(|error| match error.kind {
                notify::ErrorKind::PathNotFound => WatchError::NotFound,
                notify::ErrorKind::Io(ref io)
                    if io.kind() == std::io::ErrorKind::NotFound =>
                {
                    WatchError::NotFound
                }
                _ => WatchError::Other(error.to_string()),
            })
    }

    fn unwatch(&mut self, directory: &Path) {
        if let Some(watcher) = self.watcher.as_mut() {
            let _ = watcher.unwatch(directory);
        }
    }
}

/// The paths an event may touch to matter.
#[derive(Debug, Default)]
struct Watched {
    /// Every input file, as given and canonicalized.
    files: BTreeSet<PathBuf>,
    /// The directories that hold an input, as given and canonicalized.
    /// Events on them or their ancestors change what can be watched.
    directories: BTreeSet<PathBuf>,
}

impl Watched {
    fn touches(&self, path: &Path) -> bool {
        self.files.contains(path)
            || self
                .directories
                .iter()
                .any(|directory| directory.starts_with(path))
    }
}

/// Whether `event` may change an input. Reads and opens never do, and
/// hashing opens the files, so reacting to them would loop on inotify.
fn relevant(event: &notify::Result<Event>, watched: &Watched) -> bool {
    let Ok(event) = event else {
        // An error may mean lost events.
        return true;
    };
    if event.need_rescan() {
        return true;
    }
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.paths.is_empty()
        || event.paths.iter().any(|path| watched.touches(path))
}

/// `path` with its nearest existing ancestor canonicalized, so it compares
/// equal to the real paths native events report. A symlink that exists
/// resolves to its target.
fn resolve(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut ancestor = path;
    loop {
        if let Ok(canonical) = ancestor.canonicalize() {
            return rest
                .iter()
                .rev()
                .fold(canonical, |path: PathBuf, name| path.join(name));
        }
        let (Some(parent), Some(name)) =
            (ancestor.parent(), ancestor.file_name())
        else {
            return path.to_owned();
        };
        rest.push(name.to_owned());
        ancestor = parent;
    }
}

/// `path` with its directory resolved but its own name kept, so a symlink
/// compares equal to the path native events report for the link itself.
fn normalize(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => resolve(parent).join(name),
        _ => path.to_owned(),
    }
}

/// The nearest existing directory at or above `directory`, canonicalized.
fn nearest_existing(directory: &Path) -> Option<PathBuf> {
    directory
        .ancestors()
        .find(|ancestor| ancestor.is_dir())
        .and_then(|ancestor| ancestor.canonicalize().ok())
}

/// The directories to watch for `settings`, and the paths events must
/// touch: the configuration's directory, its symlink target's directory,
/// and every theme file's directory, each through its nearest existing
/// ancestor.
fn topology(settings: &Settings) -> (BTreeSet<PathBuf>, Watched) {
    let mut watched = Watched::default();
    let mut files = vec![settings.config.clone()];
    if settings
        .config
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        if let Ok(target) = settings.config.canonicalize() {
            files.push(target);
        } else if let Ok(link) = settings.config.read_link() {
            let parent = settings.config.parent().unwrap_or(Path::new(""));
            files.push(parent.join(link));
        }
    }
    files.extend(settings.inputs.themes.iter().cloned());
    let mut desired = BTreeSet::new();
    for file in files {
        for path in [file.clone(), normalize(&file), resolve(&file)] {
            if let Some(parent) = path.parent() {
                watched.directories.insert(parent.to_owned());
                if let Some(directory) = nearest_existing(parent) {
                    desired.insert(directory);
                }
            }
            watched.files.insert(path);
        }
    }
    (desired, watched)
}

/// A directory's identity: device and inode on Unix, so a directory
/// replaced at the same path differs. `None` when it is missing, and
/// always elsewhere.
#[cfg(unix)]
fn identity(directory: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = directory.metadata().ok()?;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn identity(_: &Path) -> Option<(u64, u64)> {
    None
}

/// The worker's state, driven by messages and the clock its caller passes.
#[expect(
    clippy::struct_excessive_bools,
    reason = "backend running, warning raised, coverage, and pending events are independent facts"
)]
pub(super) struct Worker<B> {
    backend: B,
    report: Box<dyn FnMut(Report) + Send>,
    settings: Option<Settings>,
    watching: bool,
    unavailable: bool,
    /// Whether every directory the topology wants holds a registration.
    covered: bool,
    /// Each registered directory and its identity when it was registered.
    installed: BTreeMap<PathBuf, Option<(u64, u64)>>,
    watched: Watched,
    /// When the next reconciliation runs; `None` while idle.
    deadline: Option<Instant>,
    /// Whether events, not only a registration change, led to the
    /// scheduled reconciliation.
    events_pending: bool,
    reconciliations: u64,
}

impl<B: Backend> Worker<B> {
    pub(super) fn new(
        backend: B,
        report: impl FnMut(Report) + Send + 'static,
    ) -> Self {
        Self {
            backend,
            report: Box::new(report),
            settings: None,
            watching: false,
            unavailable: false,
            covered: false,
            installed: BTreeMap::new(),
            watched: Watched::default(),
            deadline: None,
            events_pending: false,
            reconciliations: 0,
        }
    }

    /// Reports watching only while every wanted registration holds; an
    /// ancestor kept in place of a failed descendant misses edits inside it.
    fn status(&mut self) {
        (self.report)(Report::Status {
            watching: self.watching
                && self.covered
                && !self.installed.is_empty(),
            reconciliations: self.reconciliations,
        });
    }

    /// Starts, stops, or reconciles the watcher to match `settings`.
    pub(super) fn configure(&mut self, settings: Settings, now: Instant) {
        let enabled = settings.enabled;
        self.settings = Some(settings);
        if !enabled {
            if self.watching {
                self.backend.stop();
                self.watching = false;
            }
            self.installed.clear();
            self.covered = false;
            self.watched = Watched::default();
            self.deadline = None;
            self.events_pending = false;
            self.unavailable = false;
            self.status();
            return;
        }
        if !self.watching {
            if let Err(error) = self.backend.start() {
                if !self.unavailable {
                    self.unavailable = true;
                    (self.report)(Report::Unavailable(error));
                }
                self.status();
                return;
            }
            self.watching = true;
            self.unavailable = false;
        }
        self.reconcile(false, now);
    }

    /// Schedules a reconciliation after the quiet period when `event` may
    /// change an input.
    pub(super) fn event(
        &mut self,
        event: &notify::Result<Event>,
        now: Instant,
    ) {
        if !self.watching {
            return;
        }
        let dropped = self.drop_lost_registrations(event);
        if dropped || relevant(event, &self.watched) {
            self.deadline = Some(now + QUIET_PERIOD);
            self.events_pending = true;
        }
    }

    /// Forgets registrations whose directory `event` removed or renamed:
    /// inotify drops a removed directory's watch and keeps following a
    /// renamed one, so neither covers whatever appears at the path next.
    /// The next reconciliation registers the path again. Returns whether
    /// any registration went.
    fn drop_lost_registrations(
        &mut self,
        event: &notify::Result<Event>,
    ) -> bool {
        let Ok(event) = event else {
            return false;
        };
        if !matches!(
            event.kind,
            EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
        ) {
            return false;
        }
        let mut dropped = false;
        for path in &event.paths {
            if self.installed.remove(path).is_some() {
                self.backend.unwatch(path);
                dropped = true;
            }
        }
        dropped
    }

    /// How long until the next reconciliation; `None` while idle.
    pub(super) fn timeout(&self, now: Instant) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(now))
    }

    /// Runs the scheduled reconciliation once its deadline passes.
    pub(super) fn tick(&mut self, now: Instant) {
        if self.deadline.is_some_and(|deadline| deadline <= now) {
            self.deadline = None;
            let events = std::mem::take(&mut self.events_pending);
            self.reconcile(events, now);
        }
    }

    /// Installs the watches the topology requires, then compares the inputs'
    /// fingerprint with the last attempted reload's. A registration change
    /// schedules one catch-up reconciliation for whatever changed while no
    /// watch covered it, including the `FSEvents` stream's restart.
    fn reconcile(&mut self, events: bool, now: Instant) {
        let Some(settings) = self.settings.clone() else {
            return;
        };
        self.reconciliations += 1;
        if self.reconfigure(&settings) {
            self.deadline = Some(now + QUIET_PERIOD);
        }
        // A missing file is an editor's save in progress; the next event
        // reconciles again.
        if let Some(current) =
            config::fingerprint(&settings.config, &settings.inputs.themes)
        {
            let differs = current != settings.inputs.fingerprint;
            if events || differs {
                (self.report)(Report::Changed {
                    differs,
                    fingerprint: current,
                });
            }
        }
        self.status();
    }

    /// Moves the registrations to the current topology. Returns whether any
    /// registration changed. Warns once when a wanted registration fails,
    /// and again only after every wanted registration has held since.
    fn reconfigure(&mut self, settings: &Settings) -> bool {
        let mut changed = false;
        let mut failure = None;
        let mut covered = false;
        for _ in 0..TOPOLOGY_ATTEMPTS {
            // A directory replaced at the same path, or removed, since it
            // was registered is no longer covered by that registration.
            let replaced: Vec<PathBuf> = self
                .installed
                .iter()
                .filter(|(directory, registered)| {
                    identity(directory) != **registered
                })
                .map(|(directory, _)| directory.clone())
                .collect();
            for directory in replaced {
                self.backend.unwatch(&directory);
                self.installed.remove(&directory);
            }
            let (desired, watched) = topology(settings);
            self.watched = watched;
            let mut vanished = false;
            let mut failed = Vec::new();
            for directory in desired
                .iter()
                .filter(|directory| !self.installed.contains_key(*directory))
                .cloned()
                .collect::<Vec<_>>()
            {
                // Read before registering, so a replacement in between
                // shows as a changed identity next time.
                let registered = identity(&directory);
                match self.backend.watch(&directory) {
                    Ok(()) => {
                        self.installed.insert(directory, registered);
                        changed = true;
                    }
                    Err(WatchError::NotFound) => vanished = true,
                    Err(WatchError::Other(error)) => {
                        failure = Some(error);
                        failed.push(directory);
                    }
                }
            }
            covered = desired
                .iter()
                .all(|directory| self.installed.contains_key(directory));
            if vanished {
                // Keep every existing watch and resolve again.
                continue;
            }
            // A registration stays until every watch that replaces its
            // coverage succeeds, so an ancestor outlives a failed
            // descendant.
            for directory in self
                .installed
                .keys()
                .filter(|directory| {
                    !desired.contains(*directory)
                        && !failed
                            .iter()
                            .any(|failed| failed.starts_with(directory))
                })
                .cloned()
                .collect::<Vec<_>>()
            {
                self.backend.unwatch(&directory);
                self.installed.remove(&directory);
                changed = true;
            }
            break;
        }
        self.covered = covered;
        if let Some(error) = failure {
            if !self.unavailable {
                self.unavailable = true;
                (self.report)(Report::Unavailable(error));
            }
        } else if covered {
            self.unavailable = false;
        }
        changed
    }

    fn shutdown(&mut self) {
        if self.watching {
            self.backend.stop();
            self.watching = false;
        }
    }

    #[cfg(test)]
    fn probe(&self) -> Probe {
        Probe {
            installed: self.installed.keys().cloned().collect(),
            timer_armed: self.deadline.is_some(),
            reconciliations: self.reconciliations,
        }
    }
}

/// The desktop's end of the worker. Dropping it stops the worker.
pub(super) struct Handle {
    requests: mpsc::Sender<Request>,
}

impl Handle {
    pub(super) fn configure(&self, settings: Settings) {
        let _ = self.requests.send(Request::Configure(settings));
    }

    #[cfg(test)]
    fn probe(&self) -> Option<Probe> {
        let (sender, receiver) = mpsc::channel();
        self.requests.send(Request::Probe(sender)).ok()?;
        receiver.recv().ok()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.requests.send(Request::Shutdown);
    }
}

/// Starts the worker with the platform watcher.
pub(super) fn spawn(report: impl FnMut(Report) + Send + 'static) -> Handle {
    spawn_with(
        |events| NotifyBackend {
            events,
            watcher: None,
        },
        report,
    )
}

fn spawn_with<B: Backend + 'static>(
    backend: impl FnOnce(mpsc::Sender<Request>) -> B + Send + 'static,
    report: impl FnMut(Report) + Send + 'static,
) -> Handle {
    let (requests, receiver) = mpsc::channel();
    let events = requests.clone();
    thread::Builder::new()
        .name("huterm-config-watch".into())
        .spawn(move || run(Worker::new(backend(events), report), &receiver))
        .expect("spawn the configuration watcher");
    Handle { requests }
}

fn run<B: Backend>(mut worker: Worker<B>, receiver: &mpsc::Receiver<Request>) {
    loop {
        let request = match worker.timeout(Instant::now()) {
            None => match receiver.recv() {
                Ok(request) => Some(request),
                Err(_) => break,
            },
            Some(wait) => match receiver.recv_timeout(wait) {
                Ok(request) => Some(request),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
        };
        let now = Instant::now();
        match request {
            Some(Request::Configure(settings)) => {
                worker.configure(settings, now);
            }
            Some(Request::Event(event)) => worker.event(&event, now),
            #[cfg(test)]
            Some(Request::Probe(reply)) => {
                let _ = reply.send(worker.probe());
            }
            Some(Request::Shutdown) => break,
            None => {}
        }
        worker.tick(Instant::now());
    }
    worker.shutdown();
}

/// Who asked for a reload. Manual reloads always run and report every
/// failure; automatic ones skip a file an editor is replacing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReloadKind {
    Manual,
    Automatic,
}

/// The desktop's reload latch: one reload at a time, and one more after it
/// when something changed or was requested meanwhile.
#[derive(Debug, Default)]
pub(super) struct Reloads {
    reloading: bool,
    follow_up: Option<ReloadKind>,
}

impl Reloads {
    pub(super) fn reloading(&self) -> bool {
        self.reloading
    }

    /// A manual reload. Returns whether to start it now; during another
    /// reload it runs as that reload's follow-up instead.
    pub(super) fn manual(&mut self) -> bool {
        if self.reloading {
            self.follow_up = Some(ReloadKind::Manual);
            return false;
        }
        self.reloading = true;
        true
    }

    /// Handles a watcher report. Returns whether to start a reload now. A
    /// change during a reload marks it dirty before any comparison, so the
    /// reload that follows sees whatever it missed.
    pub(super) fn changed(&mut self, differs: bool) -> bool {
        if self.reloading {
            self.follow_up.get_or_insert(ReloadKind::Automatic);
            return false;
        }
        if differs {
            self.reloading = true;
        }
        differs
    }

    /// Ends the running reload. Returns the follow-up to start, if any.
    pub(super) fn finished(&mut self) -> Option<ReloadKind> {
        let follow_up = self.follow_up.take();
        self.reloading = follow_up.is_some();
        follow_up
    }
}

#[cfg(test)]
mod tests;
