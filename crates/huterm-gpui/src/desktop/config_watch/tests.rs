use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use notify::event::{AccessKind, CreateKind, Flag, ModifyKind, RemoveKind};

use super::*;

/// A temporary directory, canonicalized, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "huterm-config-watch-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the worker takes events as the backend delivers them"
)]
fn event(kind: EventKind, path: &Path) -> notify::Result<Event> {
    Ok(Event::new(kind).add_path(path.to_owned()))
}

fn modified(path: &Path) -> notify::Result<Event> {
    event(EventKind::Modify(ModifyKind::Any), path)
}

/// The settings a reload of `config` would hand the worker.
fn settings(config: &Path, enabled: bool) -> Settings {
    Settings {
        enabled,
        config: config.to_owned(),
        inputs: crate::config::reload(config).inputs,
    }
}

#[test]
fn the_event_filter_ignores_access_and_accepts_mutations_and_rescans() {
    let scratch = Scratch::new();
    let config = scratch.path("huterm/config.toml");
    let theme = scratch.path("huterm/themes/mine.toml");
    let (_, watched) = topology(&Settings {
        enabled: true,
        config: config.clone(),
        inputs: Inputs {
            themes: vec![theme.clone()],
            fingerprint: crate::config::Fingerprint::default(),
        },
    });
    for kind in [
        EventKind::Access(AccessKind::Read),
        EventKind::Access(AccessKind::Open(notify::event::AccessMode::Any)),
        EventKind::Access(AccessKind::Close(notify::event::AccessMode::Write)),
    ] {
        assert!(!relevant(&event(kind, &config), &watched), "{kind:?}");
    }
    for kind in [
        EventKind::Create(CreateKind::File),
        EventKind::Modify(ModifyKind::Any),
        EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::To)),
        EventKind::Remove(RemoveKind::File),
    ] {
        assert!(relevant(&event(kind, &config), &watched), "{kind:?}");
        assert!(relevant(&event(kind, &theme), &watched), "{kind:?}");
    }
    // Directories on the way to an input change what can be watched.
    assert!(relevant(&modified(&scratch.path("huterm")), &watched));
    assert!(relevant(
        &modified(&scratch.path("huterm/themes")),
        &watched
    ));
    // Unrelated files beside the inputs do not.
    assert!(!relevant(
        &modified(&scratch.path("huterm/.config.toml.swp")),
        &watched
    ));
    assert!(!relevant(&modified(&scratch.path("other.toml")), &watched));
    // A rescan or an error counts even without paths.
    let rescan = Event::new(EventKind::Other).set_flag(Flag::Rescan);
    assert!(relevant(&Ok(rescan), &watched));
    assert!(relevant(&Err(notify::Error::generic("overflow")), &watched));
}

#[test]
fn the_reload_latch_runs_one_follow_up_after_changes_during_a_reload() {
    let mut reloads = Reloads::default();
    // An unchanged report starts nothing; a change starts a reload.
    assert!(!reloads.changed(false));
    assert!(reloads.changed(true));
    // Reports during the reload, equal or not, mark it dirty.
    assert!(!reloads.changed(false));
    assert!(!reloads.changed(true));
    assert_eq!(reloads.finished(), Some(ReloadKind::Automatic));
    assert!(reloads.reloading());
    assert_eq!(reloads.finished(), None, "nothing changed meanwhile");
    assert!(!reloads.reloading());
    // A report whose content matches the last attempt still marks a running
    // reload dirty: that reload may have read something else.
    assert!(reloads.manual());
    assert!(!reloads.changed(false));
    assert_eq!(reloads.finished(), Some(ReloadKind::Automatic));
    assert_eq!(reloads.finished(), None);
    // A manual reload during another runs as its follow-up, still manual.
    assert!(reloads.changed(true));
    assert!(!reloads.manual());
    assert!(!reloads.changed(true));
    assert_eq!(reloads.finished(), Some(ReloadKind::Manual));
    assert_eq!(reloads.finished(), None);
}

/// A backend without native events that records registrations.
#[derive(Clone, Default)]
struct Fake {
    fail_start: bool,
    /// Directories whose registration fails, as a permission or watch
    /// limit failure would.
    refused: Arc<Mutex<BTreeSet<PathBuf>>>,
    watched: Arc<Mutex<BTreeSet<PathBuf>>>,
}

impl Fake {
    /// Drops the registration of `directory` as inotify does when the
    /// directory is removed.
    fn invalidate(&self, directory: &Path) {
        self.watched.lock().unwrap().remove(directory);
    }

    fn watched(&self) -> Vec<PathBuf> {
        self.watched.lock().unwrap().iter().cloned().collect()
    }
}

impl Backend for Fake {
    fn start(&mut self) -> Result<(), String> {
        if self.fail_start {
            Err("no watches left".into())
        } else {
            Ok(())
        }
    }

    fn stop(&mut self) {
        self.watched.lock().unwrap().clear();
    }

    fn watch(&mut self, directory: &Path) -> Result<(), WatchError> {
        if self.refused.lock().unwrap().contains(directory) {
            return Err(WatchError::Other("permission denied".into()));
        }
        self.watched.lock().unwrap().insert(directory.to_owned());
        Ok(())
    }

    fn unwatch(&mut self, directory: &Path) {
        self.watched.lock().unwrap().remove(directory);
    }
}

type Reports = Arc<Mutex<Vec<Report>>>;

fn fake_worker(backend: Fake) -> (Worker<Fake>, Reports) {
    let reports: Reports = Arc::default();
    let sink = Arc::clone(&reports);
    let worker =
        Worker::new(backend, move |report| sink.lock().unwrap().push(report));
    (worker, reports)
}

fn changes(reports: &Reports) -> Vec<bool> {
    reports
        .lock()
        .unwrap()
        .drain(..)
        .filter_map(|report| match report {
            Report::Changed { differs, .. } => Some(differs),
            _ => None,
        })
        .collect()
}

/// Runs the scheduled reconciliations, including catch-ups, until idle.
fn settle(worker: &mut Worker<Fake>, now: &mut Instant) {
    while let Some(wait) = worker.timeout(*now) {
        *now += wait;
        worker.tick(*now);
    }
}

#[test]
fn watching_off_delivers_nothing_and_turning_it_on_catches_up() {
    let scratch = Scratch::new();
    let config = scratch.path("config.toml");
    write(&config, "[font]\nsize = 14.0\n");
    let (mut worker, reports) = fake_worker(Fake::default());
    let mut now = Instant::now();
    worker.configure(settings(&config, true), now);
    settle(&mut worker, &mut now);
    assert_eq!(changes(&reports), Vec::<bool>::new());
    // A mutation arrives while a reconciliation is queued, then watching
    // turns off before the quiet period ends.
    write(&config, "[font]\nsize = 15.0\n");
    worker.event(&modified(&config), now);
    assert!(worker.timeout(now).is_some());
    worker.configure(
        Settings {
            enabled: false,
            ..settings(&config, false)
        },
        now,
    );
    assert_eq!(worker.timeout(now), None, "disabling cancels queued work");
    // Further mutations while off schedule nothing.
    write(&config, "[font]\nsize = 16.0\n");
    worker.event(&modified(&config), now);
    now += QUIET_PERIOD * 10;
    worker.tick(now);
    assert_eq!(worker.timeout(now), None);
    assert_eq!(changes(&reports), Vec::<bool>::new());
    assert!(reports.lock().unwrap().iter().all(|report| matches!(
        report,
        Report::Status {
            watching: false,
            ..
        }
    )));
    // Turning it on reconciles once against the last attempted load.
    let mut stale = settings(&config, true);
    stale.inputs.fingerprint = crate::config::Fingerprint::default();
    worker.configure(stale, now);
    assert_eq!(changes(&reports), vec![true]);
}

#[test]
fn an_idle_watcher_arms_no_timer_and_reads_nothing() {
    let scratch = Scratch::new();
    let config = scratch.path("config.toml");
    write(&config, "[font]\nsize = 14.0\n");
    let (mut worker, reports) = fake_worker(Fake::default());
    let mut now = Instant::now();
    worker.configure(settings(&config, true), now);
    settle(&mut worker, &mut now);
    let reconciliations = worker.reconciliations;
    reports.lock().unwrap().clear();
    assert_eq!(worker.timeout(now), None);
    for _ in 0..3 {
        now += Duration::from_hours(1);
        worker.tick(now);
    }
    assert_eq!(worker.reconciliations, reconciliations);
    assert_eq!(*reports.lock().unwrap(), Vec::new());
}

#[test]
fn a_backend_that_cannot_start_warns_once() {
    let scratch = Scratch::new();
    let config = scratch.path("config.toml");
    let (mut worker, reports) = fake_worker(Fake {
        fail_start: true,
        ..Fake::default()
    });
    let now = Instant::now();
    worker.configure(settings(&config, true), now);
    worker.configure(settings(&config, true), now);
    let reports = reports.lock().unwrap();
    assert_eq!(
        reports
            .iter()
            .filter(|report| matches!(report, Report::Unavailable(_)))
            .count(),
        1
    );
    assert!(matches!(
        reports.last(),
        Some(Report::Status {
            watching: false,
            ..
        })
    ));
}

#[test]
fn a_removed_directory_is_watched_again_once_it_returns() {
    let scratch = Scratch::new();
    let directory = scratch.path("huterm");
    let config = scratch.path("huterm/config.toml");
    write(&config, VALID_A);
    let backend = Fake::default();
    let (mut worker, reports) = fake_worker(backend.clone());
    let mut now = Instant::now();
    worker.configure(settings(&config, true), now);
    settle(&mut worker, &mut now);
    assert_eq!(backend.watched(), vec![directory.clone()]);
    // The directory goes, taking its registration, and a new one takes
    // its place before the quiet period ends.
    fs::remove_dir_all(&directory).unwrap();
    backend.invalidate(&directory);
    worker.event(
        &event(EventKind::Remove(RemoveKind::Folder), &directory),
        now,
    );
    write(&config, "[font]\nsize = 18.0\n");
    settle(&mut worker, &mut now);
    assert_eq!(backend.watched(), vec![directory.clone()]);
    // Nothing reloaded, so the catch-up reports the change again.
    let changes = changes(&reports);
    assert!(!changes.is_empty() && changes.iter().all(|differs| *differs));
}

#[test]
fn a_removal_event_registers_the_directory_again_without_an_identity_change() {
    let scratch = Scratch::new();
    let directory = scratch.path("huterm");
    let config = scratch.path("huterm/config.toml");
    write(&config, VALID_A);
    let backend = Fake::default();
    let (mut worker, _) = fake_worker(backend.clone());
    let mut now = Instant::now();
    worker.configure(settings(&config, true), now);
    settle(&mut worker, &mut now);
    // A directory removed and recreated can reuse its inode, so only the
    // removal event shows that inotify dropped the registration.
    worker.event(
        &event(EventKind::Remove(RemoveKind::Folder), &directory),
        now,
    );
    backend.invalidate(&directory);
    settle(&mut worker, &mut now);
    assert_eq!(backend.watched(), vec![directory]);
}

#[cfg(unix)]
#[test]
fn a_directory_replaced_under_its_watch_is_registered_again() {
    let scratch = Scratch::new();
    let directory = scratch.path("huterm");
    let config = scratch.path("huterm/config.toml");
    write(&config, VALID_A);
    let backend = Fake::default();
    let (mut worker, _) = fake_worker(backend.clone());
    let mut now = Instant::now();
    worker.configure(settings(&config, true), now);
    settle(&mut worker, &mut now);
    // inotify keeps following a renamed directory, so no event names the
    // replacement; only its identity shows the change.
    fs::rename(&directory, scratch.path("huterm.old")).unwrap();
    write(&config, VALID_A);
    backend.invalidate(&directory);
    worker.event(&modified(&config), now);
    settle(&mut worker, &mut now);
    assert_eq!(backend.watched(), vec![directory]);
}

/// How many `Unavailable` warnings `reports` holds.
fn warnings(reports: &Reports) -> usize {
    reports
        .lock()
        .unwrap()
        .iter()
        .filter(|report| matches!(report, Report::Unavailable(_)))
        .count()
}

/// The last reported `watching` state.
fn watching(reports: &Reports) -> Option<bool> {
    reports
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|report| match report {
            Report::Status { watching, .. } => Some(*watching),
            _ => None,
        })
}

#[test]
fn a_failed_descendant_registration_keeps_the_ancestor_and_warns_once() {
    let scratch = Scratch::new();
    let directory = scratch.path("huterm");
    let config = scratch.path("huterm/config.toml");
    let backend = Fake::default();
    let (mut worker, reports) = fake_worker(backend.clone());
    let mut now = Instant::now();
    worker.configure(settings(&config, true), now);
    settle(&mut worker, &mut now);
    assert_eq!(backend.watched(), vec![scratch.0.clone()]);
    assert_eq!(watching(&reports), Some(true));
    backend.refused.lock().unwrap().insert(directory.clone());
    fs::create_dir(&directory).unwrap();
    worker.event(
        &event(EventKind::Create(CreateKind::Folder), &directory),
        now,
    );
    settle(&mut worker, &mut now);
    assert_eq!(backend.watched(), vec![scratch.0.clone()]);
    assert_eq!(worker.probe().installed, vec![scratch.0.clone()]);
    // The ancestor misses edits inside the directory it could not watch.
    assert_eq!(warnings(&reports), 1, "{:?}", reports.lock().unwrap());
    assert_eq!(watching(&reports), Some(false));
    // Reconciling again while the failure lasts does not warn again.
    worker.event(&modified(&directory), now);
    settle(&mut worker, &mut now);
    assert_eq!(warnings(&reports), 1, "{:?}", reports.lock().unwrap());
    assert_eq!(watching(&reports), Some(false));
}

#[test]
fn no_registration_at_all_reports_watching_off() {
    let scratch = Scratch::new();
    let config = scratch.path("config.toml");
    let backend = Fake::default();
    backend.refused.lock().unwrap().insert(scratch.0.clone());
    let (mut worker, reports) = fake_worker(backend);
    worker.configure(settings(&config, true), Instant::now());
    let reports = reports.lock().unwrap();
    assert!(
        reports
            .iter()
            .any(|report| matches!(report, Report::Unavailable(_))),
        "{reports:?}"
    );
    assert!(
        matches!(
            reports.last(),
            Some(Report::Status {
                watching: false,
                ..
            })
        ),
        "{reports:?}"
    );
}

/// The desktop's side of reloading, driven step by step: a started reload
/// reads the file at once and is held until `finish`.
struct Desk {
    config: PathBuf,
    worker: Worker<Fake>,
    reports: Reports,
    reloads: Reloads,
    held: Option<crate::config::Reloaded>,
    /// Whether each finished reload loaded.
    outcomes: Vec<bool>,
    now: Instant,
}

impl Desk {
    fn new(config: &Path) -> Self {
        let (mut worker, reports) = fake_worker(Fake::default());
        let mut now = Instant::now();
        worker.configure(settings(config, true), now);
        settle(&mut worker, &mut now);
        reports.lock().unwrap().clear();
        Self {
            config: config.to_owned(),
            worker,
            reports,
            reloads: Reloads::default(),
            held: None,
            outcomes: Vec::new(),
            now,
        }
    }

    /// Delivers an event for the configuration and lets the quiet period
    /// pass, then hands the reports to the latch.
    fn saved(&mut self, contents: &str) {
        write(&self.config, contents);
        self.worker.event(&modified(&self.config), self.now);
        settle(&mut self.worker, &mut self.now);
        self.deliver();
    }

    fn deliver(&mut self) {
        for differs in changes(&self.reports) {
            if self.reloads.changed(differs) {
                self.start();
            }
        }
    }

    fn start(&mut self) {
        assert!(self.held.is_none(), "one reload at a time");
        self.held = Some(crate::config::reload(&self.config));
    }

    /// Completes the held reload as `finish_reload` does.
    fn finish(&mut self) {
        let reloaded = self.held.take().expect("a reload is running");
        self.outcomes.push(reloaded.config.is_ok());
        self.worker.configure(
            Settings {
                enabled: true,
                config: self.config.clone(),
                inputs: reloaded.inputs,
            },
            self.now,
        );
        settle(&mut self.worker, &mut self.now);
        if self.reloads.finished().is_some() {
            self.start();
        }
        self.deliver();
    }

    fn finish_all(&mut self) {
        for _ in 0..8 {
            if self.held.is_none() {
                return;
            }
            self.finish();
        }
        panic!("reloads did not settle: {:?}", self.outcomes);
    }
}

const VALID_A: &str = "[font]\nsize = 14.0\n";
const INVALID_B: &str = "[font\nsize = 14.0\n";

#[test]
fn restoring_the_last_good_file_after_a_broken_save_reloads() {
    let scratch = Scratch::new();
    let config = scratch.path("config.toml");
    write(&config, VALID_A);
    let mut desk = Desk::new(&config);
    desk.saved(INVALID_B);
    desk.finish_all();
    assert_eq!(desk.outcomes, vec![false]);
    // The exact earlier content differs from the last attempt, B.
    desk.saved(VALID_A);
    desk.finish_all();
    assert_eq!(desk.outcomes, vec![false, true]);
    assert!(!desk.reloads.reloading());
}

#[test]
fn a_restore_during_a_held_broken_reload_runs_one_more_reload() {
    let scratch = Scratch::new();
    let config = scratch.path("config.toml");
    write(&config, VALID_A);
    let mut desk = Desk::new(&config);
    desk.saved(INVALID_B);
    assert!(desk.held.is_some(), "B is loading");
    // A matches the last attempt, but the event lands during B's reload.
    desk.saved(VALID_A);
    desk.finish_all();
    assert_eq!(desk.outcomes.first(), Some(&false));
    assert_eq!(desk.outcomes.last(), Some(&true), "{:?}", desk.outcomes);
    assert!(desk.outcomes.len() <= 3, "{:?}", desk.outcomes);
    assert!(!desk.reloads.reloading());
}

#[test]
fn a_change_during_a_reload_is_loaded_by_the_follow_up() {
    let scratch = Scratch::new();
    let config = scratch.path("config.toml");
    write(&config, VALID_A);
    let mut desk = Desk::new(&config);
    desk.saved("[font]\nsize = 15.0\n");
    desk.saved("[font]\nsize = 16.0\n");
    desk.finish();
    let follow_up = desk.held.as_ref().expect("a follow-up reload runs");
    assert_eq!(
        follow_up
            .config
            .as_ref()
            .map(|config| config.font.size)
            .ok(),
        Some(16.0)
    );
    desk.finish_all();
    assert!(desk.outcomes.iter().all(|loaded| *loaded));
    assert!(!desk.reloads.reloading());
}

/// A running worker over the platform watcher.
struct Native {
    handle: Handle,
    reports: mpsc::Receiver<Report>,
    /// What the worker was last configured with.
    settings: Mutex<Settings>,
}

/// Generous: hosted CI runners can deliver native events late.
const NATIVE_DEADLINE: Duration = Duration::from_secs(20);

type WatchHook = Box<dyn FnMut(&Path) + Send>;
type ReportHook = Box<dyn FnMut(&Report) + Send>;

/// Test seams around the platform watcher, all run on the worker thread.
#[derive(Default)]
struct NativeHooks {
    /// Runs before each registration.
    before_watch: Option<WatchHook>,
    /// Runs as the worker reports, before the test sees the report.
    on_report: Option<ReportHook>,
    /// While set, native events are lost, as they are while `FSEvents`
    /// restarts its stream.
    muted: Arc<AtomicBool>,
}

impl Native {
    fn start(settings: Settings) -> Self {
        Self::start_with(settings, NativeHooks::default())
    }

    /// Starts the worker with `hooks` around the platform watcher, and
    /// waits for its first registration to settle.
    fn start_with(settings: Settings, hooks: NativeHooks) -> Self {
        let NativeHooks {
            before_watch,
            on_report,
            muted,
        } = hooks;
        let (sender, reports) = mpsc::channel();
        let mut on_report = on_report.unwrap_or_else(|| Box::new(|_| {}));
        let handle = spawn_with(
            move |events| Hooked {
                inner: NotifyBackend {
                    events: muffled(events, muted),
                    watcher: None,
                },
                before_watch: before_watch.unwrap_or_else(|| Box::new(|_| {})),
            },
            move |report| {
                on_report(&report);
                let _ = sender.send(report);
            },
        );
        handle.configure(settings.clone());
        let native = Self {
            handle,
            reports,
            settings: Mutex::new(settings),
        };
        native.wait("registration", |report| {
            matches!(report, Report::Status { watching: true, .. })
        });
        native.settle();
        native
    }

    /// Configures the worker as a reload would, then waits for that and
    /// any earlier work to finish and discards what it reported, so the
    /// next step only sees its own reports.
    fn configure(&self, settings: Settings) {
        *self.settings.lock().unwrap() = settings.clone();
        self.handle.configure(settings);
        self.settle();
    }

    fn settle(&self) {
        self.wait_probe("earlier work to settle", |probe| !probe.timer_armed);
        while self.reports.try_recv().is_ok() {}
    }

    /// Waits for a report matching `predicate`, reporting what arrived on
    /// timeout.
    fn wait(&self, label: &str, predicate: impl Fn(&Report) -> bool) {
        let deadline = Instant::now() + NATIVE_DEADLINE;
        let mut seen = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.reports.recv_timeout(remaining) {
                Ok(report) if predicate(&report) => return,
                Ok(report) => seen.push(report),
                Err(
                    mpsc::RecvTimeoutError::Timeout
                    | mpsc::RecvTimeoutError::Disconnected,
                ) => panic!(
                    "timed out waiting for {label}; saw {seen:?}; worker {:?}",
                    self.handle.probe()
                ),
            }
        }
    }

    /// Waits for a change report carrying the inputs' content as it is when
    /// the report arrives, so a report about an earlier step's write does
    /// not count.
    fn wait_changed(&self, label: &str) {
        let current = || {
            let settings = self.settings.lock().unwrap();
            crate::config::fingerprint(
                &settings.config,
                &settings.inputs.themes,
            )
        };
        self.wait(label, |report| match report {
            Report::Changed {
                differs: true,
                fingerprint,
            } => current() == Some(*fingerprint),
            _ => false,
        });
    }

    /// Polls the worker until `predicate` holds for its probe.
    fn wait_probe(
        &self,
        label: &str,
        predicate: impl Fn(&Probe) -> bool,
    ) -> Probe {
        let deadline = Instant::now() + NATIVE_DEADLINE;
        loop {
            let probe = self.handle.probe().expect("the worker answers");
            if predicate(&probe) {
                return probe;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {label}; last {probe:?}"
            );
            // Drain reports so the channel does not grow while polling.
            while self.reports.try_recv().is_ok() {}
            thread::yield_now();
        }
    }
}

/// Forwards native events to the worker unless `muted` is set.
fn muffled(
    worker: mpsc::Sender<Request>,
    muted: Arc<AtomicBool>,
) -> mpsc::Sender<Request> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for request in receiver {
            if !muted.load(Ordering::SeqCst) && worker.send(request).is_err() {
                return;
            }
        }
    });
    sender
}

struct Hooked {
    inner: NotifyBackend,
    before_watch: WatchHook,
}

impl Backend for Hooked {
    fn start(&mut self) -> Result<(), String> {
        self.inner.start()
    }

    fn stop(&mut self) {
        self.inner.stop();
    }

    fn watch(&mut self, directory: &Path) -> Result<(), WatchError> {
        (self.before_watch)(directory);
        self.inner.watch(directory)
    }

    fn unwatch(&mut self, directory: &Path) {
        self.inner.unwatch(directory);
    }
}

#[test]
fn native_events_reload_external_symlink_target_and_theme_writes() {
    let scratch = Scratch::new();
    let dotfiles = scratch.path("dotfiles/huterm.toml");
    write(&dotfiles, "[theme]\nname = \"child\"\n");
    write(
        &scratch.path("huterm/themes/child.toml"),
        "[theme]\nextends = \"parent\"\nbackground = \"#101010\"\n",
    );
    write(
        &scratch.path("huterm/themes/parent.toml"),
        "[theme]\nforeground = \"#eeeeee\"\n",
    );
    let config = scratch.path("huterm/config.toml");
    std::os::unix::fs::symlink(&dotfiles, &config).unwrap();
    let native = Native::start(settings(&config, true));
    let reload = |native: &Native| native.configure(settings(&config, true));
    // An edit to the symlink's target.
    write(
        &dotfiles,
        "[theme]\nname = \"child\"\nbackground = \"#000000\"\n",
    );
    native.wait_changed("the symlink target write");
    reload(&native);
    // The active theme, then the theme it extends.
    write(
        &scratch.path("huterm/themes/child.toml"),
        "[theme]\nextends = \"parent\"\nbackground = \"#202020\"\n",
    );
    native.wait_changed("the active theme write");
    reload(&native);
    write(
        &scratch.path("huterm/themes/parent.toml"),
        "[theme]\nforeground = \"#dddddd\"\n",
    );
    native.wait_changed("the parent theme write");
    reload(&native);
    // An editor that replaces the link's target file by renaming.
    let staged = scratch.path("dotfiles/.huterm.toml.tmp");
    write(
        &staged,
        "[theme]\nname = \"child\"\nbackground = \"#111111\"\n",
    );
    fs::rename(&staged, &dotfiles).unwrap();
    native.wait_changed("the renamed save");
}

#[test]
fn native_first_run_sees_the_directory_and_file_created_later() {
    let scratch = Scratch::new();
    let config = scratch.path("config/huterm/config.toml");
    let native = Native::start(settings(&config, true));
    let probe = native.handle.probe().unwrap();
    assert_eq!(
        probe.installed,
        vec![scratch.0.clone()],
        "the nearest ancestor is watched"
    );
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    write(&config, "[font]\nsize = 12.0\n");
    native.wait_changed("the new configuration");
    native.wait_probe("the watch to move down", |probe| {
        probe.installed == vec![config.parent().unwrap().to_owned()]
            && !probe.timer_armed
    });
}

#[test]
fn native_registration_catches_a_new_dependency_changed_while_registering() {
    let scratch = Scratch::new();
    let config = scratch.path("huterm/config.toml");
    // The theme's directory does not exist yet, so its parent stands in.
    write(&config, "[theme]\nname = \"mine\"\n");
    let themes = scratch.path("huterm/themes");
    let theme = scratch.path("huterm/themes/mine.toml");
    let muted = Arc::new(AtomicBool::new(false));
    let registered = Arc::new(AtomicBool::new(false));
    let hook_themes = themes.clone();
    let hook_registered = Arc::clone(&registered);
    let watch_registered = Arc::clone(&registered);
    let hook_muted = Arc::clone(&muted);
    let hook_theme = theme.clone();
    let hooks = NativeHooks {
        before_watch: Some(Box::new(move |directory| {
            if directory == hook_themes {
                watch_registered.store(true, Ordering::SeqCst);
            }
        })),
        on_report: Some(Box::new(move |report| {
            // The reconciliation that registered the themes directory has
            // already fingerprinted the inputs. The theme appears now, and
            // its events are lost, as they are while `FSEvents` restarts
            // its stream, so only the catch-up reconciliation can see it.
            if matches!(report, Report::Status { .. })
                && hook_registered.swap(false, Ordering::SeqCst)
            {
                hook_muted.store(true, Ordering::SeqCst);
                write(&hook_theme, "[theme]\nbackground = \"#222222\"\n");
            }
        })),
        muted: Arc::clone(&muted),
    };
    let native = Native::start_with(settings(&config, true), hooks);
    assert_eq!(
        native.handle.probe().unwrap().installed,
        vec![scratch.path("huterm")]
    );
    fs::create_dir(&themes).unwrap();
    native.wait_changed("the theme written after registration");
    muted.store(false, Ordering::SeqCst);
    native.wait_probe("the themes directory watch", |probe| {
        probe.installed.contains(&themes) && !probe.timer_armed
    });
    // The new watch reports later edits.
    native.configure(settings(&config, true));
    write(&theme, "[theme]\nbackground = \"#333333\"\n");
    native.wait_changed("a later theme edit");
}

#[test]
fn native_retarget_to_identical_content_still_moves_the_watch() {
    let scratch = Scratch::new();
    let first = scratch.path("a/real.toml");
    let second = scratch.path("b/real.toml");
    write(&first, VALID_A);
    write(&second, VALID_A);
    let config = scratch.path("config.toml");
    std::os::unix::fs::symlink(&first, &config).unwrap();
    let native = Native::start(settings(&config, true));
    let staged = scratch.path("config.toml.new");
    std::os::unix::fs::symlink(&second, &staged).unwrap();
    fs::rename(&staged, &config).unwrap();
    let probe = native.wait_probe("the watch to follow the link", |probe| {
        probe.installed.contains(&scratch.path("b")) && !probe.timer_armed
    });
    assert!(!probe.installed.contains(&scratch.path("a")), "{probe:?}");
    write(&second, "[font]\nsize = 18.0\n");
    native.wait_changed("the new target's edit");
}

#[test]
fn native_descendant_removed_before_registration_recovers_and_goes_idle() {
    let scratch = Scratch::new();
    let directory = scratch.path("huterm");
    let config = scratch.path("huterm/config.toml");
    let removed = Arc::new(AtomicUsize::new(0));
    let hook_removed = Arc::clone(&removed);
    let hook_directory = directory.clone();
    let hooks = NativeHooks {
        before_watch: Some(Box::new(move |watched| {
            // The first time the new directory is about to be watched, it
            // vanishes.
            if watched == hook_directory
                && hook_removed.fetch_add(1, Ordering::SeqCst) == 0
            {
                fs::remove_dir_all(&hook_directory).unwrap();
            }
        })),
        ..NativeHooks::default()
    };
    let native = Native::start_with(settings(&config, true), hooks);
    fs::create_dir(&directory).unwrap();
    native.wait_probe("the vanished registration", |probe| {
        removed.load(Ordering::SeqCst) == 1 && !probe.timer_armed
    });
    let probe = native.handle.probe().unwrap();
    assert_eq!(
        probe.installed,
        vec![scratch.0.clone()],
        "the ancestor stays watched"
    );
    write(&config, "[font]\nsize = 12.0\n");
    native.wait_changed("recovery");
    let idle = native.wait_probe("idle", |probe| {
        probe.installed == vec![directory.clone()] && !probe.timer_armed
    });
    // No events, no timer: nothing more runs.
    let again = native.handle.probe().unwrap();
    assert_eq!(again.reconciliations, idle.reconciliations);
    assert!(!again.timer_armed);
}

/// Re-points the symlink at `link` to `target` as `ln -sf` does: removes
/// the link, then creates a new one.
fn repoint(link: &Path, target: &Path) {
    fs::remove_file(link).unwrap();
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn native_retargeting_the_config_link_moves_the_watch_and_reloads() {
    let scratch = Scratch::new();
    let original = scratch.path("dotfiles/config.toml");
    let identical = scratch.path("spare/config.toml");
    let alternate = scratch.path("dotfiles/alternate.toml");
    write(&original, VALID_A);
    write(&identical, VALID_A);
    write(&alternate, "[font]\nsize = 18.0\n");
    let link = scratch.path("linked/config.toml");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&original, &link).unwrap();
    // The configured path runs through a symlinked directory, as macOS's
    // `/tmp` and `/var` do, so native events name the link by another path.
    let alias = scratch.path("alias");
    std::os::unix::fs::symlink(&scratch.0, &alias).unwrap();
    let config = alias.join("linked/config.toml");
    let native = Native::start(settings(&config, true));
    // Identical content moves the watch without a reload.
    repoint(&link, &identical);
    let probe = native.wait_probe("the watch to follow the link", |probe| {
        probe.installed.contains(&scratch.path("spare")) && !probe.timer_armed
    });
    assert!(
        !probe.installed.contains(&scratch.path("dotfiles")),
        "{probe:?}"
    );
    native.configure(settings(&config, true));
    // Different content reloads.
    repoint(&link, &alternate);
    native.wait_changed("the link retargeted to other content");
}

#[test]
fn native_a_replaced_directory_is_watched_again() {
    let scratch = Scratch::new();
    let directory = scratch.path("huterm");
    let config = scratch.path("huterm/config.toml");
    write(&config, VALID_A);
    let native = Native::start(settings(&config, true));
    let before = native.handle.probe().unwrap().reconciliations;
    // Replace the watched directory with a new one, as a dotfiles tool
    // that swaps a whole directory into place does.
    let staged = scratch.path("huterm.new");
    write(&staged.join("config.toml"), VALID_A);
    fs::rename(&directory, scratch.path("huterm.old")).unwrap();
    fs::rename(&staged, &directory).unwrap();
    // Only the replacement's own events may reconcile; configuring again
    // would register the directory whether or not they did.
    native.wait_probe("the replacement to reconcile", |probe| {
        probe.reconciliations > before
            && probe.installed == vec![directory.clone()]
            && !probe.timer_armed
    });
    write(&config, "[font]\nsize = 18.0\n");
    native.wait_changed("an edit inside the new directory");
}
