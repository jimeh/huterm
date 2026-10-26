use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    watched: Arc<Mutex<BTreeSet<PathBuf>>>,
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
            Report::Changed { differs } => Some(differs),
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
}

/// Generous: hosted CI runners can deliver native events late.
const NATIVE_DEADLINE: Duration = Duration::from_secs(20);

impl Native {
    fn start(settings: Settings) -> Self {
        Self::start_with(settings, |_| {})
    }

    /// Starts the worker with `before_watch` run before each registration.
    fn start_with(
        settings: Settings,
        before_watch: impl FnMut(&Path) + Send + 'static,
    ) -> Self {
        let (sender, reports) = mpsc::channel();
        let handle = spawn_with(
            move |events| Hooked {
                inner: NotifyBackend {
                    events,
                    watcher: None,
                },
                before_watch: Box::new(before_watch),
            },
            move |report| {
                let _ = sender.send(report);
            },
        );
        handle.configure(settings);
        let native = Self { handle, reports };
        native.wait("registration", |report| {
            matches!(report, Report::Status { watching: true, .. })
        });
        native
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

    fn wait_changed(&self, label: &str) {
        self.wait(label, |report| *report == Report::Changed { differs: true });
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

struct Hooked {
    inner: NotifyBackend,
    before_watch: Box<dyn FnMut(&Path) + Send>,
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
    let reload =
        |native: &Native| native.handle.configure(settings(&config, true));
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
    let hook_themes = themes.clone();
    let hook_theme = theme.clone();
    let native =
        Native::start_with(settings(&config, true), move |directory| {
            // The theme appears between resolving the new topology and the new
            // watch taking effect, so no event may report it.
            if directory == hook_themes {
                write(&hook_theme, "[theme]\nbackground = \"#222222\"\n");
            }
        });
    assert_eq!(
        native.handle.probe().unwrap().installed,
        vec![scratch.path("huterm")]
    );
    fs::create_dir(&themes).unwrap();
    native.wait_changed("the theme written during registration");
    native.wait_probe("the themes directory watch", |probe| {
        probe.installed.contains(&themes) && !probe.timer_armed
    });
    // The new watch reports later edits.
    native.handle.configure(settings(&config, true));
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
    let native = Native::start_with(settings(&config, true), move |watched| {
        // The first time the new directory is about to be watched, it
        // vanishes.
        if watched == hook_directory
            && hook_removed.fetch_add(1, Ordering::SeqCst) == 0
        {
            fs::remove_dir_all(&hook_directory).unwrap();
        }
    });
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
