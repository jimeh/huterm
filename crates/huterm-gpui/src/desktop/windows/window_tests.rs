//! Tier 2 view tests: the production `Desktop` and `WorkspaceView` roots in
//! GPUI test windows, attached to sessions that the test builds through
//! Mux, as another client would. The harness section of
//! `docs/plans/terminal-viewers.md` describes what this tier covers.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use gpui::{Keystroke, TestAppContext, VisualTestContext};
use huterm_protocol::{
    CellSize, GridSize, Rgb, TerminalCommand, TerminalInput, TerminalLifecycle,
    TerminalPresentation, TerminalSnapshot,
};

use super::notices::NoticeContent;
use super::worker_hooks::Stage;
use super::*;

/// How long a wait may take before the test fails with the last state it
/// observed. Below GPUI's 15-second limit for a parked test thread.
const WAIT_DEADLINE: Duration = Duration::from_secs(10);

/// How often a wait checks its condition when no task woke it sooner.
const WAIT_POLL: Duration = Duration::from_millis(20);

/// A child that prints `READY`, then echoes each line it reads as
/// `got:<line>`.
const ECHO: &str =
    "printf READY; while read line; do printf 'got:%s ' \"$line\"; done";

fn fixture(script: &str) -> TerminalCommand {
    TerminalCommand {
        program: "/bin/sh".into(),
        arguments: vec!["-c".into(), script.into()],
        working_directory: std::env::current_dir().unwrap(),
        environment: Vec::new(),
        grid_size: GridSize::clamped(INITIAL_COLUMNS, INITIAL_ROWS),
        cell_size: CellSize {
            width: 16,
            height: 32,
        },
        presentation: TerminalPresentation::default(),
    }
}

/// The production desktop over a real Mux, with one session and workspace
/// that windows attach to.
struct DesktopHarness {
    cx: TestAppContext,
    runtime: Arc<DesktopRuntime>,
    session: SessionId,
    workspace: WorkspaceId,
    /// The attachment [`Self::canonical`] observes through, made on first
    /// use.
    observer: Option<AttachmentId>,
}

impl DesktopHarness {
    fn new(cx: &mut TestAppContext) -> Self {
        Self::with_config(cx, |_| {})
    }

    fn with_config(
        cx: &mut TestAppContext,
        configure: impl FnOnce(&mut Config),
    ) -> Self {
        // Terminal runtimes wake views from their own threads.
        cx.executor().allow_parking();
        let runtime = Arc::new(DesktopRuntime::default());
        let (state, subscription) = runtime.subscribe_hierarchy();
        let mut config = Config::default();
        // A test window has no native title bar to merge tabs into.
        config.tabs.position = TabPosition::Top;
        configure(&mut config);
        let loaded = config::LoadedConfig {
            config,
            path: PathBuf::from("huterm-test.toml"),
            error: None,
            warning: None,
            fatal: false,
        };
        let app_runtime = Arc::clone(&runtime);
        cx.update(|cx| {
            install_desktop(
                cx,
                app_runtime,
                Projection::new(state, subscription),
                loaded,
            );
        });
        let (session, workspace) = {
            let mut mux = runtime.lock();
            let session = mux.create_session(None).unwrap();
            (session, mux.create_workspace(session, None).unwrap())
        };
        Self {
            cx: cx.clone(),
            runtime,
            session,
            workspace,
            observer: None,
        }
    }

    /// Opens a tab running `script` in the shared workspace, as another
    /// client would.
    fn open_tab(&self, script: &str) -> TabId {
        self.runtime
            .lock()
            .open_tab(self.workspace, &fixture(script))
            .unwrap()
            .tab
            .id
    }

    /// Sends `line` to `tab`'s terminal through a viewer of another
    /// client, which reaches the child even while a window refuses typed
    /// input. The returned viewer keeps the client's attachment in use.
    fn send_line(&mut self, tab: TabId, line: &str) -> TerminalViewer {
        self.wait_for("the projection to hold the tab", |cx| {
            projected(cx, tab)
        });
        let terminal = self.cx.update(|cx| {
            cx.global::<Desktop>()
                .hierarchy
                .state()
                .tab(tab)
                .unwrap()
                .layout
                .first_pane()
                .terminal_id
        });
        let mut mux = self.runtime.lock();
        let attachment = mux.attach_session(self.session).unwrap();
        let viewer = mux
            .subscribe_terminal(
                attachment,
                terminal,
                ViewerOptions::new(ViewerCapabilities::ALL),
            )
            .unwrap();
        viewer
            .send_input(TerminalInput::Text(format!("{line}\r")))
            .unwrap();
        viewer
    }

    /// Revokes `window`'s viewers as the smoke fixture's `revoke-window`
    /// does, leaving the hierarchy unchanged.
    fn revoke(&mut self, window: AnyWindowHandle) {
        let attachment = self
            .cx
            .update(|cx| {
                cx.global::<Desktop>()
                    .windows
                    .record(window.window_id())
                    .and_then(|record| record.attachment)
            })
            .unwrap();
        super::palette_smoke::revoke_viewers(&self.runtime, attachment)
            .unwrap();
    }

    /// Polls the canonical snapshot of `tab` until `done` holds, running
    /// tasks between polls. Fails after [`WAIT_DEADLINE`].
    fn wait_canonical(
        &mut self,
        tab: TabId,
        what: &str,
        mut done: impl FnMut(&TerminalSnapshot) -> bool,
    ) {
        let deadline = Instant::now() + WAIT_DEADLINE;
        loop {
            let snapshot = self.canonical(tab);
            if done(&snapshot) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; last canonical grid: {:?}",
                snapshot.size
            );
            let executor = self.cx.executor();
            self.cx
                .foreground_executor()
                .clone()
                .block_test(executor.timer(WAIT_POLL));
        }
    }

    /// Holds or releases `window`'s replacements. Reconcile replaces
    /// nothing in a busy window, so a test can let the runtime process a
    /// revocation before the replacement registers. In one batch the two
    /// would coalesce: the focus out and in cancel, and control never
    /// passes to another viewer. Releasing reconciles as the window's own
    /// completion does.
    fn hold_replacements(&mut self, window: AnyWindowHandle, hold: bool) {
        self.update_view(window, |view, window, cx| {
            view.busy = hold;
            if !hold {
                view.reconcile_own(window, cx);
            }
        });
    }

    /// The canonical snapshot of `tab`'s terminal, read through a fresh
    /// observing viewer that neither sizes nor focuses it.
    fn canonical(&mut self, tab: TabId) -> Arc<TerminalSnapshot> {
        let terminal = self.cx.update(|cx| {
            cx.global::<Desktop>()
                .hierarchy
                .state()
                .tab(tab)
                .unwrap()
                .layout
                .first_pane()
                .terminal_id
        });
        let viewer = {
            let mut mux = self.runtime.lock();
            let attachment = *self.observer.get_or_insert_with(|| {
                mux.attach_session(self.session).unwrap()
            });
            mux.subscribe_terminal(
                attachment,
                terminal,
                ViewerOptions::new(ViewerCapabilities::NONE),
            )
            .unwrap()
        };
        let request = viewer.request_snapshot().unwrap();
        let foreground = self.cx.foreground_executor().clone();
        foreground
            .block_test(async { request.recv().await })
            .unwrap()
            .snapshot
    }

    /// Opens a window without a shell and attaches it to the session, as
    /// the smoke fixture's `attach-window` does.
    fn attach_window(&mut self) -> AnyWindowHandle {
        let attachment =
            self.runtime.lock().attach_session(self.session).unwrap();
        let (session, workspace) = (self.session, self.workspace);
        self.cx.update(|cx| {
            let existing = cx.windows();
            open_window_inner(cx, false);
            let handle = cx
                .windows()
                .into_iter()
                .find(|handle| !existing.contains(handle))
                .expect("the attached window opened");
            handle
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>().unwrap().update(
                        cx,
                        |view, cx| {
                            view.adopt_attachment(
                                attachment, session, workspace, cx,
                            );
                        },
                    );
                })
                .unwrap();
            handle
        })
    }

    /// The window's view, for reading what it shows.
    fn workspace_view(
        &mut self,
        window: AnyWindowHandle,
    ) -> Entity<WorkspaceView> {
        self.cx
            .update(|cx| {
                window.update(cx, |root, _, _| {
                    root.downcast::<WorkspaceView>().unwrap()
                })
            })
            .unwrap()
    }

    /// Runs `update` on `window`'s view with its window.
    fn update_view<R>(
        &mut self,
        window: AnyWindowHandle,
        update: impl FnOnce(
            &mut WorkspaceView,
            &mut Window,
            &mut Context<'_, WorkspaceView>,
        ) -> R,
    ) -> R {
        self.cx
            .update(|cx| {
                window.update(cx, |root, window, cx| {
                    root.downcast::<WorkspaceView>()
                        .unwrap()
                        .update(cx, |view, cx| update(view, window, cx))
                })
            })
            .unwrap()
    }

    /// Removes `window` as the platform does, without its close pipeline.
    fn remove_window(&mut self, window: AnyWindowHandle) {
        self.cx
            .update(|cx| {
                window.update(cx, |_, window, _| window.remove_window())
            })
            .unwrap();
        self.wait_for("the window to close", |cx| {
            !cx.windows().contains(&window)
        });
    }

    /// Runs `window`'s close pipeline as a refresh does, then reports
    /// whether it is busy: a window that took a close starts its
    /// assessment, and so turns busy, in the same update.
    fn resume_close(&mut self, window: AnyWindowHandle) -> bool {
        self.update_view(window, |view, window, cx| {
            view.resume_close(window, cx);
            view.busy
        })
    }

    /// Selects `tab` in `window`, activates the window, and types a line
    /// into the tab, which makes an [`EXITS`] child exit.
    fn exit_tab(&mut self, window: AnyWindowHandle, tab: TabId) {
        self.update_view(window, |view, window, cx| {
            view.select(tab, window, cx);
        });
        self.activate(window, tab);
        self.wait_text(window, tab, "READY");
        self.type_line(window, "x");
    }

    /// Activates `window` and waits until its view of `tab` has keyboard
    /// focus in the active window.
    fn activate(&mut self, window: AnyWindowHandle, tab: TabId) {
        self.cx
            .update(|cx| {
                window.update(cx, |_, window, _| window.activate_window())
            })
            .unwrap();
        self.wait_for("the terminal to be focused", |cx| {
            focused(cx, window, tab)
        });
    }

    /// Makes no window active, as switching to another application does.
    fn deactivate(&mut self, window: AnyWindowHandle) {
        VisualTestContext::from_window(window, &self.cx).deactivate_window();
    }

    /// Waits until `window`'s view of `tab` shows `text`.
    fn wait_text(&mut self, window: AnyWindowHandle, tab: TabId, text: &str) {
        self.wait_for(&format!("{text:?} in window {window:?}"), |cx| {
            screen(cx, window, tab).is_some_and(|screen| screen.contains(text))
        });
    }

    /// Types `text` into `window`'s focused terminal, key by key, then
    /// Enter.
    fn type_line(&mut self, window: AnyWindowHandle, text: &str) {
        for key in text.chars().map(String::from).chain(["enter".into()]) {
            self.cx
                .dispatch_keystroke(window, Keystroke::parse(&key).unwrap());
        }
    }

    /// Runs every window's tasks, delivering frames, until `done` holds.
    /// Fails with the last observed state after [`WAIT_DEADLINE`].
    fn wait_for(&mut self, what: &str, done: impl FnMut(&mut App) -> bool) {
        self.wait(what, true, done);
    }

    /// Runs every window's tasks without delivering a frame until `done`
    /// holds, for progress that must not depend on display frames.
    fn wait_without_frames(
        &mut self,
        what: &str,
        done: impl FnMut(&mut App) -> bool,
    ) {
        self.wait(what, false, done);
    }

    fn wait(
        &mut self,
        what: &str,
        frames: bool,
        mut done: impl FnMut(&mut App) -> bool,
    ) {
        let executor = self.cx.executor();
        let foreground = self.cx.foreground_executor().clone();
        let deadline = Instant::now() + WAIT_DEADLINE;
        let cx = &mut self.cx;
        foreground.block_test(async {
            loop {
                let (satisfied, state) = cx.update(|cx| {
                    for handle in cx.windows().into_iter().filter(|_| frames) {
                        // GPUI's test mode draws dirty windows but never
                        // delivers frames, and only a frame renews a view's
                        // snapshot allowance.
                        let _ = handle.update(cx, |_, window, cx| {
                            window.simulate_next_frame(cx);
                        });
                    }
                    (done(cx), describe(cx))
                });
                if satisfied {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {what}; last state: {state}"
                );
                executor.timer(WAIT_POLL).await;
            }
        });
    }
}

/// The view of `tab` in `window`, if the window shows one.
fn terminal(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
) -> Option<Entity<TerminalView>> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx)
                .tabs
                .iter()
                .find(|view| view.id == tab)
                .map(|tab| tab.view.clone())
        })
        .ok()
        .flatten()
}

/// Whether `window` is active and its view of `tab` has keyboard focus.
fn focused(cx: &mut App, window: AnyWindowHandle, tab: TabId) -> bool {
    terminal(cx, window, tab).is_some_and(|view| {
        window
            .update(cx, |_, window, cx| {
                window.is_window_active()
                    && view.read(cx).focus.is_focused(window)
            })
            .unwrap_or(false)
    })
}

/// The bytes a hex-echo child reports having received after `marker`.
fn received(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    marker: &str,
) -> Option<String> {
    let text = screen(cx, window, tab)?;
    let (_, echoed) = text.split_once(marker)?;
    Some(echoed.chars().filter(char::is_ascii_hexdigit).collect())
}

/// The text `window`'s view of `tab` shows, if it has a snapshot.
fn screen(cx: &mut App, window: AnyWindowHandle, tab: TabId) -> Option<String> {
    let snapshot = terminal(cx, window, tab)?.read(cx).snapshot.clone()?;
    Some(snapshot.cells().map(|cell| cell.text.as_str()).collect())
}

/// Whether the projection still holds `tab`, so it was not closed.
fn projected(cx: &App, tab: TabId) -> bool {
    cx.global::<Desktop>()
        .hierarchy
        .state()
        .tab_workspace(tab)
        .is_some()
}

/// Whether `window`'s view of `tab` has seen its terminal exit.
fn exited(cx: &mut App, window: AnyWindowHandle, tab: TabId) -> bool {
    terminal(cx, window, tab).is_some_and(|view| view.read(cx).exited)
}

/// Whether `window`'s close pipeline is running an operation.
fn busy(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>()
                .is_ok_and(|view| view.read(cx).busy)
        })
        .unwrap_or(false)
}

/// Whether `window` shows a close confirmation.
fn confirming(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>()
                .is_ok_and(|view| view.read(cx).close.confirmation.is_some())
        })
        .unwrap_or(false)
}

/// The notices `window` shows, as smoke state lists them.
fn notices(cx: &mut App, window: AnyWindowHandle) -> Vec<String> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            Some(
                view.read(cx)
                    .notices
                    .contents()
                    .map(NoticeContent::smoke_line)
                    .collect(),
            )
        })
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn describe(cx: &App) -> String {
    let desktop = cx.global::<Desktop>();
    desktop
        .windows
        .records()
        .map(|record| {
            let tabs: Vec<String> = record
                .tabs
                .iter()
                .map(|entry| format!("{}={:?}", entry.id.get(), entry.title))
                .collect();
            format!(
                "window {:?}: active={:?} tabs=[{}]",
                record.id,
                record.active.map(TabId::get),
                tabs.join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[gpui::test]
fn a_window_attached_to_a_session_installs_and_shows_its_tabs(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let first = harness.attach_window();
    let second = harness.attach_window();
    harness.wait_for("both windows to show the child", |cx| {
        [first, second].into_iter().all(|window| {
            screen(cx, window, tab).is_some_and(|text| text.contains("READY"))
        })
    });
    harness.activate(first, tab);
    harness.type_line(first, "marker");
    harness.wait_for("the marker to reach the second window", |cx| {
        screen(cx, second, tab).is_some_and(|text| text.contains("got:marker"))
    });

    // A tab another client opens later installs in both windows, behind
    // the tab each shows.
    let later = harness.open_tab(ECHO);
    let record = |cx: &App, window: AnyWindowHandle| {
        let record = cx.global::<Desktop>().windows.record(window.window_id());
        record.map(|record| {
            let tabs: Vec<TabId> =
                record.tabs.iter().map(|entry| entry.id).collect();
            (tabs, record.active)
        })
    };
    harness.wait_for("both windows to install the later tab", |cx| {
        [first, second].into_iter().all(|window| {
            terminal(cx, window, later).is_some()
                && record(cx, window) == Some((vec![tab, later], Some(tab)))
        })
    });
}

/// A child that prints `READY`, then answers its first line by ringing once
/// and setting its title to `one`, its second by ringing twice and setting
/// it to `two`, and its third by setting it to `three` without ringing.
/// Each answer prints `DONE<n>` last. A view that shows an answer's title
/// has therefore applied the poll that carried its bells, which come first,
/// and bells ring only in answer to a line typed into the active window,
/// where the view counts them.
const STATUS: &str = "printf READY; n=0; while read line; do n=$((n+1)); \
     case $n in \
     1) printf '\\007\\033]0;one\\007DONE1';; \
     2) printf '\\007\\007\\033]0;two\\007DONE2';; \
     *) printf '\\033]0;three\\007DONE3';; esac; done";

#[gpui::test]
fn the_refresh_applies_each_status_change_once(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(STATUS);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    let state = |cx: &mut App| {
        let view = terminal(cx, window, tab).unwrap();
        let view = view.read(cx);
        (view.title.clone(), view.bell_flash_count)
    };
    for (line, title, bells) in [("w", "one", 1), ("x", "two", 3)] {
        harness.type_line(window, line);
        harness.wait_for(&format!("the title {title:?}"), |cx| {
            state(cx).0 == title
        });
        assert_eq!(harness.cx.update(state), (title.to_owned(), bells));
    }
    // A later status change without bells applies none again.
    harness.type_line(window, "y");
    harness.wait_for("the title \"three\"", |cx| state(cx).0 == "three");
    assert_eq!(harness.cx.update(state), ("three".to_owned(), 3));
}

#[gpui::test]
fn a_view_installed_after_an_exit_shows_it_without_announcing_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.terminal.close_on_exit = false;
    });
    let tab = harness.open_tab("printf READY; read line; exit 3");
    let early = harness.attach_window();
    harness.activate(early, tab);
    harness.wait_text(early, tab, "READY");
    harness.type_line(early, "x");
    let exited = |cx: &mut App, window: AnyWindowHandle| {
        terminal(cx, window, tab).is_some_and(|view| view.read(cx).exited)
    };
    harness.wait_for("the exit", |cx| exited(cx, early));
    let late = harness.attach_window();
    harness.wait_for("the late view to show the exit", |cx| {
        exited(cx, late)
            && cx
                .global::<Desktop>()
                .windows
                .record(late.window_id())
                .is_some_and(|record| {
                    record
                        .tabs
                        .iter()
                        .any(|entry| entry.title.ends_with("· exited"))
                })
    });
    let notices = |harness: &mut DesktopHarness, window| {
        let view = harness.workspace_view(window);
        harness
            .cx
            .update(|cx| view.read(cx).notices.contents().count())
    };
    assert_eq!(
        notices(&mut harness, early),
        1,
        "the window that saw the exit announces it"
    );
    assert_eq!(
        notices(&mut harness, late),
        0,
        "a view installed after the exit announces nothing"
    );
}

#[gpui::test]
fn an_exited_tab_reflows_through_its_view(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.terminal.close_on_exit = false;
    });
    let tab = harness.open_tab("printf READY; read line; exit 0");
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    harness.type_line(window, "x");
    let grids = |cx: &mut App| {
        terminal(cx, window, tab).and_then(|view| {
            let view = view.read(cx);
            view.exited
                .then(|| (view.requested_grid, view.displayed_grid()))
        })
    };
    harness.wait_for("the exit to show at the requested grid", |cx| {
        grids(cx)
            .is_some_and(|(requested, displayed)| Some(requested) == displayed)
    });
    let (before, _) = harness.cx.update(grids).unwrap();
    harness
        .cx
        .simulate_window_resize(window, gpui::size(px(500.0), px(400.0)));
    harness.wait_for("the exited tab to show its new grid", |cx| {
        grids(cx).is_some_and(|(requested, displayed)| {
            requested != before && Some(requested) == displayed
        })
    });
}

/// A child that prints `READY`, enables focus reports once it reads a byte,
/// prints `GO`, and then prints every byte it receives as two hex digits.
const FOCUS_ECHO: &str = "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null >/dev/null; \
     printf '\\033[?1004hGO'; \
     while byte=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\n'); \
     [ -n \"$byte\" ]; do printf '%s' \"$byte\"; done";

#[gpui::test]
fn focus_a_key_and_blur_reach_the_child_in_order(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(FOCUS_ECHO);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    harness
        .cx
        .dispatch_keystroke(window, Keystroke::parse("g").unwrap());
    harness.wait_text(window, tab, "GO");
    harness.deactivate(window);
    harness.wait_for("the blur", |cx| {
        received(cx, window, tab, "GO").is_some_and(|bytes| bytes.len() >= 6)
    });
    harness.activate(window, tab);
    harness
        .cx
        .dispatch_keystroke(window, Keystroke::parse("a").unwrap());
    harness.deactivate(window);
    let expected = "1b5b4f1b5b49611b5b4f";
    harness.wait_for("focus in, the key, and focus out", |cx| {
        received(cx, window, tab, "GO")
            .is_some_and(|bytes| bytes.len() >= expected.len())
    });
    let bytes = harness.cx.update(|cx| received(cx, window, tab, "GO"));
    assert_eq!(bytes.as_deref(), Some(expected));
}

/// A child that prints `READY` and exits once it reads a line.
const EXITS: &str = "printf READY; read line; exit 0";

#[gpui::test]
fn an_exit_closes_its_tab_under_close_on_exit(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let exits = harness.open_tab(EXITS);
    let stays = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.exit_tab(window, exits);
    harness.wait_for("the exited tab to close", |cx| {
        !projected(cx, exits) && terminal(cx, window, exits).is_none()
    });
    harness.cx.update(|cx| {
        assert!(terminal(cx, window, stays).is_some(), "the sibling stays");
        assert_eq!(notices(cx, window), Vec::<String>::new());
    });
}

#[gpui::test]
fn the_last_tabs_exit_closes_its_window(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let exits = harness.open_tab(EXITS);
    let window = harness.attach_window();
    harness.exit_tab(window, exits);
    harness.wait_for("the emptied window to close", |cx| {
        !cx.windows().contains(&window)
    });
    harness.cx.update(|cx| assert!(!projected(cx, exits)));
}

#[gpui::test]
fn an_exit_while_the_window_is_busy_closes_once_the_busy_close_ends(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let exits = harness.open_tab(EXITS);
    let closed = harness.open_tab(ECHO);
    let stays = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_for("the window to install every tab", |cx| {
        [exits, closed, stays]
            .into_iter()
            .all(|tab| terminal(cx, window, tab).is_some())
    });
    let hooks = Arc::clone(&harness.runtime);
    let hooks = &hooks.worker_hooks;
    hooks.hold(Stage::Assess);
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Tab(closed), window, cx);
    });
    harness.wait_for("the user close to be assessed", |_| {
        hooks.held(Stage::Assess) == 1
    });
    let _client = harness.send_line(exits, "x");
    harness
        .wait_for("the window to see the exit", |cx| exited(cx, window, exits));
    // The refresh that saw the exit ran the pipeline in the same update; a
    // close it started would have taken the exit-close.
    let owed = harness.cx.update(|cx| {
        cx.global::<Desktop>()
            .exit_closes
            .owed()
            .collect::<Vec<_>>()
    });
    assert_eq!(owed, [exits], "busy defers the exit-close");
    hooks.release(Stage::Assess);
    harness.wait_for("both closes to finish", |cx| {
        !projected(cx, exits)
            && !projected(cx, closed)
            && terminal(cx, window, stays).is_some()
    });
    assert_eq!(hooks.entered(Stage::Assess), 2);
    assert_eq!(
        harness.cx.update(|cx| notices(cx, window)),
        Vec::<String>::new()
    );
}

/// A child that keeps a running job, which a close of its tab must confirm,
/// and prints `READY` once the job exists.
const RUNS_JOB: &str = "sleep 300 & printf READY; wait";

#[gpui::test]
fn cancelling_a_window_close_that_absorbed_an_exit_close_still_closes_the_tab(
    cx: &mut TestAppContext,
) {
    // The window is the session's only attachment, so closing it closes the
    // session, which assesses the running job and asks. The exit therefore
    // comes from typing in the window, not from another client.
    let mut harness = DesktopHarness::new(cx);
    let exits = harness.open_tab(EXITS);
    let runs = harness.open_tab(RUNS_JOB);
    let window = harness.attach_window();
    harness.wait_for("the window to install both tabs", |cx| {
        [exits, runs]
            .into_iter()
            .all(|tab| terminal(cx, window, tab).is_some())
    });
    harness.update_view(window, |view, window, cx| {
        view.select(runs, window, cx);
    });
    harness.activate(window, runs);
    harness.wait_text(window, runs, "READY");
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Assess);
    harness.exit_tab(window, exits);
    harness.wait_for("the window to take the exit-close", |cx| {
        exited(cx, window, exits) && hooks.held(Stage::Assess) == 1
    });
    // Queued behind the exit-close, the window close absorbs it.
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Window, window, cx);
    });
    hooks.release(Stage::Assess);
    harness.wait_for("the window close to ask", |cx| confirming(cx, window));
    assert_eq!(hooks.entered(Stage::Commit), 0);
    harness.update_view(window, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });
    harness.wait_for("the exited tab to close", |cx| {
        !projected(cx, exits) && terminal(cx, window, exits).is_none()
    });
    // The exit-close, the window close that absorbed it, then the
    // exit-close again.
    assert_eq!(hooks.entered(Stage::Assess), 3);
    assert_eq!(hooks.entered(Stage::Commit), 1, "one close was committed");
    harness.cx.update(|cx| {
        assert!(cx.windows().contains(&window), "the window stays open");
        assert!(terminal(cx, window, runs).is_some(), "the job's tab stays");
        assert!(!confirming(cx, window));
        assert_eq!(notices(cx, window), Vec::<String>::new());
    });
}

/// Two windows showing an [`EXITS`] tab and an [`ECHO`] sibling, with
/// every view installed.
fn two_windows(
    harness: &mut DesktopHarness,
) -> ([AnyWindowHandle; 2], TabId, TabId) {
    let exits = harness.open_tab(EXITS);
    let stays = harness.open_tab(ECHO);
    let windows = [harness.attach_window(), harness.attach_window()];
    harness.wait_for("both windows to install both tabs", |cx| {
        windows.into_iter().all(|window| {
            [exits, stays]
                .into_iter()
                .all(|tab| terminal(cx, window, tab).is_some())
        })
    });
    (windows, exits, stays)
}

/// Exits `tab` from another client while `stage` is held, and waits until
/// both windows saw the exit and a close reached `stage`. Returns the
/// window holding the close, then the other; callers check that the other
/// takes nothing.
fn exit_held_at(
    harness: &mut DesktopHarness,
    windows: [AnyWindowHandle; 2],
    tab: TabId,
    stage: Stage,
) -> (AnyWindowHandle, AnyWindowHandle) {
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(stage);
    let _client = harness.send_line(tab, "x");
    harness.wait_for(
        "both windows to see the exit and one close to wait",
        |cx| {
            windows.into_iter().all(|window| exited(cx, window, tab))
                && runtime.worker_hooks.held(stage) > 0
        },
    );
    let [first, second] = windows;
    if harness.cx.update(|cx| busy(cx, first)) {
        (first, second)
    } else {
        (second, first)
    }
}

/// Waits until `tab` closed and no remaining window shows it, then checks
/// that one assessment and one commit ran and that no window prompted.
fn assert_closed_once(
    harness: &mut DesktopHarness,
    windows: &[AnyWindowHandle],
    tab: TabId,
) {
    harness.wait_for("the exited tab to close everywhere", |cx| {
        !projected(cx, tab)
            && windows
                .iter()
                .all(|window| terminal(cx, *window, tab).is_none())
    });
    let hooks = &harness.runtime.worker_hooks;
    assert_eq!(hooks.entered(Stage::Assess), 1, "one close was assessed");
    assert_eq!(hooks.entered(Stage::Commit), 1, "one close was committed");
    harness.cx.update(|cx| {
        for window in windows {
            assert!(!confirming(cx, *window), "no window prompted");
        }
    });
}

#[gpui::test]
fn an_exit_shown_in_two_windows_closes_once_without_a_prompt(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (windows, exits, stays) = two_windows(&mut harness);
    let (claimer, other) =
        exit_held_at(&mut harness, windows, exits, Stage::Assess);
    assert!(
        !harness.resume_close(other),
        "the other window takes nothing"
    );
    harness.runtime.worker_hooks.release(Stage::Assess);
    assert_closed_once(&mut harness, &windows, exits);
    harness.cx.update(|cx| {
        for window in [claimer, other] {
            assert!(terminal(cx, window, stays).is_some());
            assert_eq!(notices(cx, window), Vec::<String>::new());
        }
    });
}

#[gpui::test]
fn a_view_installed_after_the_exit_takes_a_close_its_busy_window_cannot(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let exits = harness.open_tab(EXITS);
    let closed = harness.open_tab(ECHO);
    let stays = harness.open_tab(ECHO);
    let busy_window = harness.attach_window();
    harness.wait_for("the window to install every tab", |cx| {
        [exits, closed, stays]
            .into_iter()
            .all(|tab| terminal(cx, busy_window, tab).is_some())
    });
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Assess);
    harness.update_view(busy_window, |view, window, cx| {
        view.request_close(CloseTarget::Tab(closed), window, cx);
    });
    let _client = harness.send_line(exits, "x");
    harness.wait_for("the busy window to see the exit", |cx| {
        hooks.held(Stage::Assess) == 1 && exited(cx, busy_window, exits)
    });
    let late = harness.attach_window();
    harness.wait_for("the late view to take the close", |cx| {
        exited(cx, late, exits) && hooks.held(Stage::Assess) == 2
    });
    // Both assessments return together, so whichever commit lands second
    // can be refused as stale and assessed again; only the outcome counts.
    hooks.release(Stage::Assess);
    harness.wait_for("both closes to finish", |cx| {
        !projected(cx, exits) && !projected(cx, closed)
    });
    harness.cx.update(|cx| {
        for window in [busy_window, late] {
            assert!(terminal(cx, window, stays).is_some());
            assert_eq!(notices(cx, window), Vec::<String>::new());
        }
    });
}

#[gpui::test]
fn closing_the_claiming_window_during_its_assessment_hands_the_close_on(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let exits = harness.open_tab(EXITS);
    let stays = harness.open_tab(ECHO);
    let claimer = harness.attach_window();
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Assess);
    let _client = harness.send_line(exits, "x");
    harness.wait_for("the window to take the close", |cx| {
        exited(cx, claimer, exits) && hooks.held(Stage::Assess) == 1
    });
    let late = harness.attach_window();
    harness.wait_for("the late view to show the exit", |cx| {
        exited(cx, late, exits) && terminal(cx, late, stays).is_some()
    });
    assert!(!harness.resume_close(late), "the close is taken");
    harness.remove_window(claimer);
    harness.wait_for("the late window to take the close", |_| {
        hooks.held(Stage::Assess) == 2
    });
    hooks.release(Stage::Assess);
    harness.wait_for("the late window to close the tab", |cx| {
        !projected(cx, exits) && terminal(cx, late, exits).is_none()
    });
    assert_eq!(hooks.entered(Stage::Commit), 1);
    harness.cx.update(|cx| {
        assert!(terminal(cx, late, stays).is_some());
        assert_eq!(notices(cx, late), Vec::<String>::new());
    });
}

#[gpui::test]
fn dropping_the_claiming_view_while_its_close_is_held_starts_no_second_one(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (windows, exits, _) = two_windows(&mut harness);
    let (claimer, other) =
        exit_held_at(&mut harness, windows, exits, Stage::Assess);
    let drop_view = |harness: &mut DesktopHarness| {
        harness.update_view(claimer, |view, _, cx| {
            view.drop_tab_views(&[exits], cx);
        });
    };
    drop_view(&mut harness);
    assert!(!harness.resume_close(other));
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Commit);
    hooks.release(Stage::Assess);
    harness.wait_for("the claiming window to commit", |_| {
        hooks.held(Stage::Commit) == 1
    });
    drop_view(&mut harness);
    assert!(!harness.resume_close(other));
    hooks.release(Stage::Commit);
    assert_closed_once(&mut harness, &windows, exits);
    harness.cx.update(|cx| {
        assert_eq!(notices(cx, other), Vec::<String>::new());
    });
}

#[gpui::test]
fn closing_the_claiming_window_while_it_commits_closes_once_without_a_notice(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (windows, exits, stays) = two_windows(&mut harness);
    let (claimer, other) =
        exit_held_at(&mut harness, windows, exits, Stage::Commit);
    harness.remove_window(claimer);
    assert!(
        !harness.resume_close(other),
        "the commit still owns the close"
    );
    harness.runtime.worker_hooks.release(Stage::Commit);
    assert_closed_once(&mut harness, &[other], exits);
    harness.cx.update(|cx| {
        assert!(terminal(cx, other, stays).is_some());
        assert_eq!(notices(cx, other), Vec::<String>::new());
    });
}

#[gpui::test]
fn a_failed_assessment_settles_the_exit_close_with_one_notice(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (windows, exits, _) = two_windows(&mut harness);
    let (claimer, other) =
        exit_held_at(&mut harness, windows, exits, Stage::Assess);
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.fail(Stage::Assess);
    hooks.release(Stage::Assess);
    harness.wait_for("the failure notice", |cx| {
        !busy(cx, claimer) && !notices(cx, claimer).is_empty()
    });
    for window in [claimer, other] {
        assert!(!harness.resume_close(window), "nothing retries the close");
    }
    harness.cx.update(|cx| {
        assert!(projected(cx, exits));
        let shown = notices(cx, claimer);
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert!(shown[0].contains("Cannot assess close"), "{shown:?}");
        assert_eq!(notices(cx, other), Vec::<String>::new());
    });
    assert_eq!(hooks.entered(Stage::Assess), 1);
}

#[gpui::test]
fn a_failed_commit_settles_the_exit_close_with_one_notice(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (windows, exits, _) = two_windows(&mut harness);
    let (claimer, other) =
        exit_held_at(&mut harness, windows, exits, Stage::Commit);
    harness.runtime.worker_hooks.fail(Stage::Commit);
    harness.runtime.worker_hooks.release(Stage::Commit);
    assert_closed_once(&mut harness, &windows, exits);
    harness
        .wait_for("the failure notice", |cx| !notices(cx, claimer).is_empty());
    harness.cx.update(|cx| {
        let shown = notices(cx, claimer);
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert!(shown[0].contains("Close failed"), "{shown:?}");
        assert_eq!(notices(cx, other), Vec::<String>::new());
    });
}

#[gpui::test]
fn a_tab_that_exited_without_close_on_exit_stays_open_after_enabling_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.terminal.close_on_exit = false;
    });
    let (windows, exits, _) = two_windows(&mut harness);
    let _client = harness.send_line(exits, "x");
    harness.wait_for("both windows to see the exit", |cx| {
        windows.into_iter().all(|window| exited(cx, window, exits))
    });
    for window in windows {
        harness.update_view(window, |view, _, _| {
            view.config.terminal.close_on_exit = true;
        });
        assert!(!harness.resume_close(window), "the exit stays retained");
    }
    assert!(harness.cx.update(|cx| projected(cx, exits)));
    assert_eq!(harness.runtime.worker_hooks.entered(Stage::Assess), 0);
}

/// The view of `tab` in `window` once it holds its `epoch`th viewer and is
/// not revoked.
fn replaced(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    epoch: u64,
) -> Option<Entity<TerminalView>> {
    terminal(cx, window, tab).filter(|view| {
        let view = view.read(cx);
        view.viewer_epoch == epoch && !view.revoked
    })
}

fn selected(cx: &mut App, window: AnyWindowHandle, tab: TabId) -> bool {
    terminal(cx, window, tab)
        .is_some_and(|view| view.read(cx).selection.is_some())
}

#[gpui::test]
fn a_revoked_view_is_replaced_in_place_and_keeps_focus_and_control(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(FOCUS_ECHO);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    harness
        .cx
        .dispatch_keystroke(window, Keystroke::parse("g").unwrap());
    harness.wait_text(window, tab, "GO");
    let original = harness.cx.update(|cx| terminal(cx, window, tab)).unwrap();
    let before = harness
        .cx
        .update(|cx| received(cx, window, tab, "GO"))
        .unwrap();
    let digest =
        |cx: &mut App| cx.global::<Desktop>().hierarchy.state().digest();
    let structure = harness.cx.update(digest);
    harness.hold_replacements(window, true);
    harness.revoke(window);
    harness.wait_canonical(
        tab,
        "the revoked viewer's focus to end",
        |snapshot| {
            let text: String =
                snapshot.cells().map(|cell| cell.text.as_str()).collect();
            text.trim_end().ends_with("1b5b4f")
        },
    );
    harness.hold_replacements(window, false);
    harness.wait_for("the focused replacement to report focus", |cx| {
        replaced(cx, window, tab, 1).is_some()
            && received(cx, window, tab, "GO").is_some_and(|bytes| {
                bytes.len() > before.len() && bytes.ends_with("1b5b49")
            })
    });
    let after = harness
        .cx
        .update(|cx| received(cx, window, tab, "GO"))
        .unwrap();
    assert_eq!(
        &after[before.len()..],
        "1b5b4f1b5b49",
        "the revoked viewer's focus ends, then the replacement's begins"
    );
    harness.cx.update(|cx| {
        let view = terminal(cx, window, tab).unwrap();
        assert_eq!(view.entity_id(), original.entity_id(), "replaced in place");
        assert!(focused(cx, window, tab), "the view keeps focus");
        let record = cx.global::<Desktop>().windows.record(window.window_id());
        assert_eq!(record.and_then(|record| record.active), Some(tab));
        assert_eq!(notices(cx, window), Vec::<String>::new());
        assert_eq!(
            digest(cx),
            structure,
            "revoking left the hierarchy as it was"
        );
    });
    // The replacement controls the terminal's size again.
    let requested = |cx: &mut App| {
        terminal(cx, window, tab).map(|view| view.read(cx).requested_grid)
    };
    let small = harness.cx.update(requested).unwrap();
    harness
        .cx
        .simulate_window_resize(window, gpui::size(px(500.0), px(400.0)));
    harness.wait_for("the canonical grid to follow the replacement", |cx| {
        requested(cx).is_some_and(|grid| grid != small)
    });
    let grid = harness.cx.update(requested).unwrap();
    harness.wait_for("the shown grid to follow", |cx| {
        terminal(cx, window, tab)
            .is_some_and(|view| view.read(cx).displayed_grid() == Some(grid))
    });
    assert_eq!(harness.canonical(tab).size, grid);
}

#[gpui::test]
fn a_replacement_that_does_not_resize_keeps_the_selection(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    let completed = |cx: &mut App| {
        terminal(cx, window, tab).map_or(0, |view| {
            view.read(cx).scroll.diagnostics().requests_completed
        })
    };
    let started = |cx: &mut App| {
        terminal(cx, window, tab).map_or(0, |view| {
            view.read(cx).scroll.diagnostics().requests_started
        })
    };
    harness
        .wait_for("no snapshot in flight", |cx| started(cx) == completed(cx));
    let view = harness.cx.update(|cx| terminal(cx, window, tab)).unwrap();
    harness
        .cx
        .update(|cx| view.update(cx, TerminalView::select_all));
    let before = harness.cx.update(completed);
    harness.revoke(window);
    harness.wait_for("the replacement's first snapshot", |cx| {
        replaced(cx, window, tab, 1).is_some() && completed(cx) > before
    });
    assert!(harness.cx.update(|cx| selected(cx, window, tab)));
}

/// A child that prints `READY`, then answers `out` with `OUT`, `title` by
/// setting its title to `two`, `clip` by writing `clip` to the clipboard
/// through OSC 52, and `exit` by exiting.
const REPLIES: &str = "printf READY; while read line; do case \"$line\" in \
     out) printf OUT;; \
     title) printf '\\033]0;two\\007';; \
     clip) printf '\\033]52;c;Y2xpcA==\\007';; \
     exit) exit 0;; \
     esac; done";

#[gpui::test]
fn a_replaced_view_receives_output_status_exit_and_clipboard_without_frames(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        // Snapshots then wait for nothing but the activity drain.
        config.terminal.refresh = huterm_config::RefreshMode::Unlimited;
        config.terminal.close_on_exit = false;
    });
    let tab = harness.open_tab(REPLIES);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    harness.revoke(window);
    harness.wait_for("the replacement", |cx| {
        replaced(cx, window, tab, 1).is_some()
    });
    let view = |cx: &mut App| terminal(cx, window, tab).unwrap();
    // The replacement's own first snapshot must not carry the output.
    harness.wait_without_frames("the replacement's first snapshot", |cx| {
        !view(cx).read(cx).scroll.in_flight()
    });
    let _out = harness.send_line(tab, "out");
    harness.wait_without_frames("the output", |cx| {
        screen(cx, window, tab).is_some_and(|text| text.contains("OUT"))
    });
    let _title = harness.send_line(tab, "title");
    harness.wait_without_frames("the title", |cx| {
        view(cx).read(cx).title == "two"
    });
    let _clip = harness.send_line(tab, "clip");
    harness.wait_without_frames("the clipboard write", |cx| {
        view(cx).read(cx).clipboard_writes == 1
    });
    let _exit = harness.send_line(tab, "exit");
    harness.wait_without_frames("the exit", |cx| exited(cx, window, tab));
}

#[gpui::test]
fn a_view_whose_replacement_fails_is_removed(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    let attachment = harness
        .cx
        .update(|cx| {
            cx.global::<Desktop>()
                .windows
                .record(window.window_id())
                .and_then(|record| record.attachment)
        })
        .unwrap();
    // Retargeting the attachment emits no hierarchy event, so the window
    // still shows the session's workspace while its attachment cannot
    // subscribe there.
    {
        let mut mux = harness.runtime.lock();
        let elsewhere = mux.create_session(None).unwrap();
        mux.retarget_attachment(attachment, elsewhere).unwrap();
    }
    harness.wait_for("the view to be removed", |cx| {
        terminal(cx, window, tab).is_none()
    });
    harness.cx.update(|cx| {
        assert!(projected(cx, tab), "the terminal stays");
        assert_eq!(notices(cx, window), Vec::<String>::new());
    });
}

#[gpui::test]
fn replacing_the_controlling_viewer_beside_another_size_resizes_twice(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let other = harness.attach_window();
    let window = harness.attach_window();
    harness
        .cx
        .simulate_window_resize(other, gpui::size(px(400.0), px(300.0)));
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    let requested = |cx: &mut App| {
        terminal(cx, window, tab).map(|view| view.read(cx).requested_grid)
    };
    let grid = harness.cx.update(requested).unwrap();
    harness.wait_for("the active window to control the size", |cx| {
        terminal(cx, window, tab)
            .is_some_and(|view| view.read(cx).displayed_grid() == Some(grid))
            && terminal(cx, other, tab)
                .is_some_and(|view| view.read(cx).requested_grid != grid)
    });
    let original = harness.canonical(tab);
    let selecting = harness.cx.update(|cx| terminal(cx, window, tab)).unwrap();
    harness
        .cx
        .update(|cx| selecting.update(cx, TerminalView::select_all));
    assert!(harness.cx.update(|cx| selected(cx, window, tab)));
    let other_grid = harness
        .cx
        .update(|cx| {
            terminal(cx, other, tab).map(|view| view.read(cx).requested_grid)
        })
        .unwrap();
    harness.hold_replacements(window, true);
    harness.revoke(window);
    harness.wait_canonical(
        tab,
        "control to pass to the other window",
        |snapshot| snapshot.size == other_grid,
    );
    harness.hold_replacements(window, false);
    harness.wait_for("the replacement to take the size back", |cx| {
        replaced(cx, window, tab, 1).is_some()
            && terminal(cx, window, tab).is_some_and(|view| {
                let view = view.read(cx);
                view.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.size == grid
                        && snapshot.geometry_revision
                            > original.geometry_revision
                })
            })
    });
    let settled = harness.canonical(tab);
    assert_eq!(settled.size, grid);
    assert_eq!(
        settled.geometry_revision - original.geometry_revision,
        2,
        "control moved to the other size and back"
    );
    assert!(
        !harness.cx.update(|cx| selected(cx, window, tab)),
        "each resize advanced the content generation"
    );
}

/// The bytes a [`FOCUS_ECHO`] child reports in the canonical snapshot after
/// its `GO`.
fn canonical_bytes(snapshot: &TerminalSnapshot) -> String {
    let text: String =
        snapshot.cells().map(|cell| cell.text.as_str()).collect();
    text.split_once("GO")
        .map(|(_, echoed)| {
            echoed.chars().filter(char::is_ascii_hexdigit).collect()
        })
        .unwrap_or_default()
}

#[gpui::test]
fn a_replacement_started_before_its_window_blurs_takes_neither_focus_nor_control(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(FOCUS_ECHO);
    let other = harness.attach_window();
    let window = harness.attach_window();
    harness
        .cx
        .simulate_window_resize(other, gpui::size(px(400.0), px(300.0)));
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    harness
        .cx
        .dispatch_keystroke(window, Keystroke::parse("g").unwrap());
    harness.wait_text(window, tab, "GO");
    let grid = |cx: &mut App, window| {
        terminal(cx, window, tab).map(|view| view.read(cx).requested_grid)
    };
    harness.wait_for("the windows to request different grids", |cx| {
        grid(cx, other).is_some() && grid(cx, other) != grid(cx, window)
    });
    let other_grid = harness.cx.update(|cx| grid(cx, other)).unwrap();
    let before = canonical_bytes(&harness.canonical(tab));
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Subscribe);
    harness.revoke(window);
    harness.wait_for("the replacement to start", |_| {
        hooks.held(Stage::Subscribe) == 1
    });
    // While the replacement is in flight, its window blurs and the other
    // window takes focus and the size.
    harness.activate(other, tab);
    let expected = format!("{before}1b5b4f1b5b49");
    harness.wait_canonical(
        tab,
        "the other window to take focus and the size",
        |snapshot| {
            snapshot.size == other_grid && canonical_bytes(snapshot) == expected
        },
    );
    hooks.release(Stage::Subscribe);
    harness.wait_for("the replacement", |cx| {
        replaced(cx, window, tab, 1).is_some()
    });
    // With no window active the program sees focus end, so the
    // replacement did not report the focus it started with.
    harness.deactivate(other);
    let expected = format!("{expected}1b5b4f");
    harness.wait_canonical(tab, "focus to end", |snapshot| {
        canonical_bytes(snapshot) == expected
    });
    assert_eq!(
        harness.canonical(tab).size,
        other_grid,
        "the active window kept the size"
    );
}

#[gpui::test]
fn a_replacement_started_before_a_resize_takes_the_current_size(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    let grid = |cx: &mut App| {
        terminal(cx, window, tab).map(|view| view.read(cx).requested_grid)
    };
    let small = harness.cx.update(grid).unwrap();
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Subscribe);
    harness.revoke(window);
    harness.wait_for("the replacement to start", |_| {
        hooks.held(Stage::Subscribe) == 1
    });
    harness
        .cx
        .simulate_window_resize(window, gpui::size(px(500.0), px(400.0)));
    harness.wait_for("the view to request the new grid", |cx| {
        grid(cx).is_some_and(|grid| grid != small)
    });
    let large = harness.cx.update(grid).unwrap();
    hooks.release(Stage::Subscribe);
    harness.wait_for("the replacement", |cx| {
        replaced(cx, window, tab, 1).is_some()
    });
    harness.wait_canonical(
        tab,
        "the canonical grid to follow the new size",
        |snapshot| snapshot.size == large,
    );
}

/// A raw-mode child that prints `READY`, asks for the default foreground
/// color once it reads a byte, prints `ASKED`, and then reports every byte
/// it receives in hex, the color reply included.
const ASKS_FOREGROUND: &str = "stty raw -echo; printf READY; \
     dd bs=1 count=1 2>/dev/null >/dev/null; printf '\\033]10;?\\007ASKED'; \
     while byte=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\n'); \
     [ -n \"$byte\" ]; do printf '%s' \"$byte\"; done";

#[gpui::test]
fn a_replacement_started_before_a_theme_change_takes_the_current_colors(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ASKS_FOREGROUND);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Subscribe);
    harness.revoke(window);
    harness.wait_for("the replacement to start", |_| {
        hooks.held(Stage::Subscribe) == 1
    });
    // As a reload does: the revoked viewer refuses the new colors.
    let theme = Theme {
        foreground: Rgb {
            red: 0x12,
            green: 0x34,
            blue: 0x56,
        },
        ..Theme::default()
    };
    harness.cx.update(|cx| {
        terminal(cx, window, tab).unwrap().update(cx, |view, _| {
            view.publish_presentation(&theme);
            view.theme = theme.clone();
        });
    });
    hooks.release(Stage::Subscribe);
    harness.wait_for("the replacement", |cx| {
        replaced(cx, window, tab, 1).is_some()
    });
    harness.type_line(window, "q");
    harness.wait_for("the child to receive the new foreground", |cx| {
        received(cx, window, tab, "ASKED")
            .is_some_and(|hex| hex.contains(&hex_of("rgb:1212/3434/5656")))
    });
}

/// `text` as the lowercase hex a hex-echo child prints for it.
fn hex_of(text: &str) -> String {
    text.bytes().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// Exits the [`EXITS`] tab while its window's replacement viewer is held
/// between its start and its subscription, so no view sees the exit
/// happen, then releases the replacement. Returns the window and the tab.
fn exit_while_replacing(
    harness: &mut DesktopHarness,
) -> (AnyWindowHandle, TabId) {
    let exits = harness.open_tab(EXITS);
    let stays = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_for("the window to install both tabs", |cx| {
        terminal(cx, window, stays).is_some()
    });
    harness.wait_text(window, exits, "READY");
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Subscribe);
    harness.revoke(window);
    harness.wait_for("the replacement to start", |cx| {
        hooks.held(Stage::Subscribe) > 0
            && terminal(cx, window, exits)
                .is_some_and(|view| view.read(cx).revoked)
    });
    // A revoked viewer's poll still reports an exit, so the tab's old
    // activity drain must have made its last refresh and ended before the
    // shell exits. It ends once the runtime finalizes the revoked viewer
    // and closes its wake; the swap later replaces the placeholder.
    let drain = harness.update_view(window, |view, _, _| {
        let tab = view.tabs.iter_mut().find(|tab| tab.id == exits).unwrap();
        std::mem::replace(&mut tab.activity_task, Task::ready(()))
    });
    harness.cx.foreground_executor().clone().block_test(drain);
    let client = harness.send_line(exits, "x");
    let mut seen = false;
    harness.wait_for("the exit", |_| {
        let update = client.poll();
        seen |= update.exited.is_some()
            || update.status.is_some_and(|status| {
                matches!(status.lifecycle, TerminalLifecycle::Exited(_))
            });
        seen
    });
    assert!(
        !harness.cx.update(|cx| exited(cx, window, exits)),
        "the revoked view did not see the exit"
    );
    hooks.release(Stage::Subscribe);
    (window, exits)
}

#[gpui::test]
fn an_exit_while_a_live_view_is_replaced_still_closes_its_tab(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, exits) = exit_while_replacing(&mut harness);
    harness.wait_for("the exited tab to close", |cx| {
        !projected(cx, exits) && terminal(cx, window, exits).is_none()
    });
}

#[gpui::test]
fn an_exit_while_a_live_view_is_replaced_is_announced_when_kept(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.terminal.close_on_exit = false;
    });
    let (window, exits) = exit_while_replacing(&mut harness);
    harness.wait_for("the exit notice", |cx| {
        exited(cx, window, exits)
            && notices(cx, window)
                .iter()
                .any(|notice| notice.contains("Process exited"))
    });
}

impl DesktopHarness {
    /// Closes the harness session through Mux, as another client would.
    fn close_session(&self) {
        self.runtime.lock().close_session(self.session).unwrap();
    }

    /// Closes `tab` through Mux, as another client would.
    fn close_tab(&self, tab: TabId) {
        self.runtime.lock().close_tab(self.workspace, tab).unwrap();
    }

    /// Opens a window attached to the session and waits until it shows
    /// every tab in `tabs`.
    fn attach_showing(&mut self, tabs: &[TabId]) -> AnyWindowHandle {
        let window = self.attach_window();
        self.wait_for("the window to install its tabs", |cx| {
            tabs.iter().all(|tab| terminal(cx, window, *tab).is_some())
        });
        window
    }

    /// Selects `tab` in `window`, activates it, and waits until `tab`
    /// answers a typed line, so its child is alive.
    fn answers(&mut self, window: AnyWindowHandle, tab: TabId, line: &str) {
        self.update_view(window, |view, window, cx| {
            view.select(tab, window, cx);
        });
        self.activate(window, tab);
        self.type_line(window, line);
        self.wait_text(window, tab, &format!("got:{line}"));
    }

    fn wait_closed(&mut self, window: AnyWindowHandle) {
        self.wait_for("the window to close", |cx| {
            !cx.windows().contains(&window)
        });
    }

    /// Close failures reported in any window, including removed ones.
    fn close_failures(&mut self) -> usize {
        self.cx.update(|cx| cx.global::<Desktop>().close_failures)
    }

    /// Requests Quit in `window`, which queues it behind a busy close.
    fn queue_quit(&mut self, window: AnyWindowHandle) {
        self.update_view(window, |view, window, cx| {
            assert!(view.busy, "the Quit would not queue");
            view.request_close(CloseTarget::Application, window, cx);
        });
    }

    /// Creates a session no window shows, which keeps the application
    /// running once its windows are gone, and returns it.
    fn unviewed_session(&self) -> SessionId {
        self.runtime.lock().create_session(None).unwrap()
    }

    /// Waits until Quit captured the restore set and began teardown, and
    /// checks that the capture holds `session`.
    fn wait_quit(&mut self, session: SessionId) {
        let runtime = Arc::clone(&self.runtime);
        self.wait_for("the application to quit", |_| {
            runtime.terminating.load(Ordering::Acquire)
        });
        let restore = runtime.restore.lock().unwrap();
        let captured = restore.as_ref().expect("Quit captured nothing");
        assert!(
            captured
                .hierarchy
                .sessions
                .iter()
                .any(|kept| kept.id == session),
            "the capture lacks the unviewed session"
        );
    }
}

/// Whether the session still exists in Mux.
fn session_exists(harness: &DesktopHarness) -> bool {
    harness.runtime.lock().session(harness.session).is_some()
}

#[gpui::test]
fn closing_one_of_two_windows_detaches_it_without_a_prompt(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let runs = harness.open_tab(RUNS_JOB);
    let stays = harness.open_tab(ECHO);
    let closed = harness.attach_showing(&[runs, stays]);
    let kept = harness.attach_showing(&[runs, stays]);
    harness.wait_text(closed, runs, "READY");
    harness.update_view(closed, |view, window, cx| {
        view.request_close(CloseTarget::Window, window, cx);
    });
    harness.wait_closed(closed);
    harness.cx.update(|cx| {
        assert!(!confirming(cx, kept));
        assert!(terminal(cx, kept, runs).is_some());
    });
    harness.answers(kept, stays, "kept");
    assert_eq!(harness.runtime.worker_hooks.entered(Stage::Assess), 1);
}

/// Opens two windows over one tab running `script`, holds both window
/// closes at assessment and commit, and releases them together. Returns
/// the windows.
fn close_both_windows_at_once(
    harness: &mut DesktopHarness,
    script: &str,
) -> [AnyWindowHandle; 2] {
    let tab = harness.open_tab(script);
    let windows = [
        harness.attach_showing(&[tab]),
        harness.attach_showing(&[tab]),
    ];
    harness.wait_text(windows[0], tab, "READY");
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Assess);
    hooks.hold(Stage::Commit);
    for window in windows {
        harness.update_view(window, |view, window, cx| {
            view.request_close(CloseTarget::Window, window, cx);
        });
    }
    harness.wait_for("both closes to be assessed", |_| {
        hooks.held(Stage::Assess) == 2
    });
    hooks.release(Stage::Assess);
    harness
        .wait_for("both commits to wait", |_| hooks.held(Stage::Commit) == 2);
    hooks.release(Stage::Commit);
    windows
}

#[gpui::test]
fn concurrent_closes_of_two_windows_terminate_their_session_once(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let windows = close_both_windows_at_once(&mut harness, ECHO);
    for window in windows {
        harness.wait_closed(window);
    }
    let hooks = &harness.runtime.worker_hooks;
    // The second commit was refused as stale and assessed again.
    assert_eq!(hooks.entered(Stage::Assess), 3);
    assert_eq!(hooks.entered(Stage::Commit), 3);
    assert!(!session_exists(&harness));
    assert_eq!(harness.runtime.lock().terminal_count(), 0);
}

#[gpui::test]
fn the_surviving_concurrent_window_close_prompts_after_its_retry(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let windows = close_both_windows_at_once(&mut harness, RUNS_JOB);
    harness.wait_for("one window to close and the other to prompt", |cx| {
        let open: Vec<_> = windows
            .into_iter()
            .filter(|window| cx.windows().contains(window))
            .collect();
        open.len() == 1 && confirming(cx, open[0])
    });
    let survivor = harness.cx.update(|cx| {
        windows
            .into_iter()
            .find(|window| cx.windows().contains(window))
            .unwrap()
    });
    let hooks = Arc::clone(&harness.runtime);
    let hooks = &hooks.worker_hooks;
    assert_eq!(hooks.entered(Stage::Assess), 3);
    assert_eq!(hooks.entered(Stage::Commit), 2);
    assert!(
        session_exists(&harness),
        "nothing terminated before consent"
    );
    assert_eq!(
        harness.cx.update(|cx| notices(cx, survivor)),
        Vec::<String>::new()
    );
    harness.update_view(survivor, |view, window, cx| {
        view.run_dialog_command(ids::DIALOG_CONFIRM, window, cx)
            .unwrap();
    });
    harness.wait_closed(survivor);
    assert!(!session_exists(&harness));
}

#[gpui::test]
fn closing_the_shared_workspaces_last_tab_closes_both_windows(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let closer = harness.attach_showing(&[tab]);
    let other = harness.attach_showing(&[tab]);
    harness.update_view(closer, |view, window, cx| {
        view.request_close(CloseTarget::Tab(tab), window, cx);
    });
    harness.wait_for("both windows to close", |cx| {
        !cx.windows().contains(&closer) && !cx.windows().contains(&other)
    });
    assert!(!session_exists(&harness));
    assert_eq!(harness.close_failures(), 0);
}

#[gpui::test]
fn a_session_another_client_closes_closes_its_windows(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let windows = [
        harness.attach_showing(&[tab]),
        harness.attach_showing(&[tab]),
    ];
    harness.close_session();
    harness.wait_for("both windows to close", |cx| {
        windows.iter().all(|window| !cx.windows().contains(window))
    });
    // An unknown attachment removes a window silently: no commit ran.
    assert_eq!(harness.runtime.worker_hooks.entered(Stage::Commit), 0);
    assert_eq!(harness.close_failures(), 0);
}

#[gpui::test]
fn a_tab_opened_during_an_emptied_close_assessment_keeps_the_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Assess);
    harness.close_tab(first);
    harness.wait_for("the emptied close to be assessed", |cx| {
        terminal(cx, window, first).is_none() && hooks.held(Stage::Assess) == 1
    });
    let arriving = harness.open_tab(ECHO);
    hooks.release(Stage::Assess);
    harness.wait_text(window, arriving, "READY");
    harness.answers(window, arriving, "arrived");
    assert_eq!(hooks.entered(Stage::Commit), 0);
    harness.cx.update(|cx| {
        assert!(cx.windows().contains(&window));
        assert_eq!(notices(cx, window), Vec::<String>::new());
    });
}

/// Holds viewer subscriptions, opens a tab that `window` starts
/// installing, then closes `closed`, the window's only view, from the
/// window. Waits until the emptied close that follows was refused, and
/// returns the installing tab.
fn close_last_tab_while_installing(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    closed: TabId,
) -> TabId {
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Subscribe);
    let installing = harness.open_tab(ECHO);
    harness.wait_for("the window to start installing", |_| {
        hooks.held(Stage::Subscribe) == 1
    });
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Tab(closed), window, cx);
    });
    // The tab close, then the emptied close.
    harness.wait_for("the follow-up close to finish", |cx| {
        !projected(cx, closed)
            && hooks.entered(Stage::Assess) == 2
            && !busy(cx, window)
    });
    harness.cx.update(|cx| {
        assert!(cx.windows().contains(&window), "the window closed");
        assert!(projected(cx, installing), "the installing tab closed");
    });
    installing
}

#[gpui::test]
fn closing_the_last_tab_while_another_installs_keeps_the_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let closed = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[closed]);
    let installing =
        close_last_tab_while_installing(&mut harness, window, closed);
    harness.runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_text(window, installing, "READY");
    harness.answers(window, installing, "installed");
    assert_eq!(harness.runtime.worker_hooks.entered(Stage::Commit), 1);
}

#[gpui::test]
fn a_refused_emptied_close_runs_again_once_the_installing_tab_goes(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let closed = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[closed]);
    let installing =
        close_last_tab_while_installing(&mut harness, window, closed);
    harness.close_tab(installing);
    harness.wait_closed(window);
    harness.runtime.worker_hooks.release(Stage::Subscribe);
    assert!(!session_exists(&harness));
}

#[gpui::test]
fn moving_the_last_tab_to_a_sibling_workspace_detaches_the_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    {
        let mut mux = harness.runtime.lock();
        let sibling = mux.create_workspace(harness.session, None).unwrap();
        mux.move_tab(harness.workspace, tab, sibling, None).unwrap();
    }
    harness.wait_closed(window);
    {
        let mux = harness.runtime.lock();
        assert!(mux.session(harness.session).is_some());
        assert_eq!(mux.capture_hierarchy().attachments, []);
    }
    let _client = harness.send_line(tab, "moved");
    harness.wait_canonical(tab, "the moved child to answer", |snapshot| {
        let text: String =
            snapshot.cells().map(|cell| cell.text.as_str()).collect();
        text.contains("got:moved")
    });
}

#[gpui::test]
fn a_tab_confirmation_whose_tab_left_the_window_is_cancelled(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let runs = harness.open_tab(RUNS_JOB);
    let stays = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[runs, stays]);
    harness.wait_text(window, runs, "READY");
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Tab(runs), window, cx);
    });
    harness.wait_for("the tab confirmation", |cx| confirming(cx, window));
    harness.close_tab(runs);
    harness.wait_for("the confirmation to be cancelled", |cx| {
        terminal(cx, window, runs).is_none() && !confirming(cx, window)
    });
    assert_eq!(harness.runtime.worker_hooks.entered(Stage::Commit), 0);
    harness.answers(window, stays, "stays");
}

/// Whether the application is quitting.
fn quitting(cx: &App) -> bool {
    cx.global::<Desktop>().quitting
}

#[gpui::test]
fn a_quit_confirmation_outlives_its_windows_session_until_cancelled(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(RUNS_JOB);
    let window = harness.attach_showing(&[tab]);
    harness.wait_text(window, tab, "READY");
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Application, window, cx);
    });
    harness.wait_for("the Quit confirmation", |cx| confirming(cx, window));
    harness.close_session();
    harness.wait_for("the window to drop the closed tab", |cx| {
        terminal(cx, window, tab).is_none()
    });
    harness.cx.update(|cx| {
        assert!(confirming(cx, window), "the Quit confirmation went away");
        assert!(quitting(cx));
    });
    harness.update_view(window, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });
    assert!(!harness.cx.update(|cx| quitting(cx)));
    harness.wait_closed(window);
}

#[gpui::test]
fn a_confirmed_quit_committing_when_its_windows_session_closes_still_quits(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(RUNS_JOB);
    let window = harness.attach_showing(&[tab]);
    harness.wait_text(window, tab, "READY");
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Commit);
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Application, window, cx);
    });
    harness.wait_for("the Quit confirmation", |cx| confirming(cx, window));
    harness.update_view(window, |view, window, cx| {
        view.run_dialog_command(ids::DIALOG_CONFIRM, window, cx)
            .unwrap();
    });
    harness.wait_for("the Quit to commit", |_| hooks.held(Stage::Commit) == 1);
    harness.close_session();
    hooks.release(Stage::Commit);
    harness.wait_for("the application to quit", |_| {
        runtime.terminating.load(Ordering::Acquire)
    });
    // The stale commit was assessed again before it committed.
    assert_eq!(hooks.entered(Stage::Assess), 2);
    assert_eq!(hooks.entered(Stage::Commit), 2);
    assert!(runtime.restore.lock().unwrap().is_some());
}

#[gpui::test]
fn a_failed_emptied_close_assessment_raises_one_notice_and_is_not_retried(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Assess);
    harness.close_tab(tab);
    harness.wait_for("the emptied close to be assessed", |cx| {
        terminal(cx, window, tab).is_none() && hooks.held(Stage::Assess) == 1
    });
    hooks.fail(Stage::Assess);
    hooks.release(Stage::Assess);
    harness.wait_for("the failure notice", |cx| {
        !busy(cx, window) && !notices(cx, window).is_empty()
    });
    assert!(!harness.resume_close(window), "nothing retries the close");
    harness.cx.update(|cx| {
        assert!(cx.windows().contains(&window));
        let shown = notices(cx, window);
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert!(shown[0].contains("Cannot assess close"), "{shown:?}");
    });
    assert_eq!(hooks.entered(Stage::Assess), 1);
}

/// Clicks `window` at `position` without drawing first, so the click is
/// dispatched against the last drawn frame.
fn click_drawn_frame(
    window: &mut Window,
    cx: &mut App,
    position: gpui::Point<Pixels>,
) {
    let modifiers = gpui::Modifiers::none();
    window.dispatch_event(
        gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
            position,
            modifiers,
            button: gpui::MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    );
    window.dispatch_event(
        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
            position,
            modifiers,
            button: gpui::MouseButton::Left,
            click_count: 1,
        }),
        cx,
    );
}

#[gpui::test]
fn a_confirm_click_drawn_before_cancel_neither_assesses_nor_commits(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let runs = harness.open_tab(RUNS_JOB);
    let stays = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[runs, stays]);
    harness.wait_text(window, runs, "READY");
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    let ask = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, window, cx| {
            view.request_close(CloseTarget::Tab(runs), window, cx);
        });
        harness.wait_for("the drawn confirmation", |cx| {
            window
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>()
                        .unwrap()
                        .read(cx)
                        .drawn_dialog_groups
                        .is_some()
                })
                .unwrap()
        });
        VisualTestContext::from_window(window, &harness.cx)
            .debug_bounds("confirm-close")
            .expect("the Confirm button was drawn")
            .center()
    };
    let clicks = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, _, _| view.close.confirm_clicks)
    };
    let confirm = ask(&mut harness);
    // Cancel, then click in the same update, before a frame drops the
    // dialog: the press lands on the frame that drew it.
    harness
        .cx
        .update(|cx| {
            window.update(cx, |root, window, cx| {
                root.downcast::<WorkspaceView>().unwrap().update(
                    cx,
                    |view, cx| {
                        view.cancel_confirmation(window, cx);
                    },
                );
                click_drawn_frame(window, cx, confirm);
            })
        })
        .unwrap();
    assert_eq!(clicks(&mut harness), 1, "the stale click reached Confirm");
    harness.cx.run_until_parked();
    assert_eq!(hooks.entered(Stage::Assess), 1);
    assert_eq!(hooks.entered(Stage::Commit), 0);
    assert!(!harness.cx.update(|cx| confirming(cx, window)));

    let confirm = ask(&mut harness);
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_click(confirm, gpui::Modifiers::none());
    assert_eq!(clicks(&mut harness), 2);
    harness.wait_for("the confirmed tab to close", |cx| {
        !projected(cx, runs) && terminal(cx, window, runs).is_none()
    });
    assert_eq!(hooks.entered(Stage::Commit), 1);
    harness.cx.update(|cx| {
        assert!(terminal(cx, window, stays).is_some());
    });
}

/// Holds viewer subscriptions and opens a tab that `window` starts
/// installing, then closes `shown`, the window's only view, through Mux.
/// Waits until the runtime refused the emptied close that follows, while
/// its result is held before it reaches the window, and returns the
/// installing tab. Viewer subscriptions stay held.
fn refuse_an_emptied_close_and_hold_it(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    shown: TabId,
) -> TabId {
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Subscribe);
    let installing = harness.open_tab(ECHO);
    harness.wait_for("the window to start installing", |_| {
        hooks.held(Stage::Subscribe) == 1
    });
    hooks.hold(Stage::Assessed);
    harness.close_tab(shown);
    harness.wait_for("the refused emptied close to be held", |cx| {
        terminal(cx, window, shown).is_none()
            && hooks.held(Stage::Assessed) == 1
    });
    installing
}

#[gpui::test]
fn a_tab_removed_after_core_refused_an_emptied_close_closes_the_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let shown = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[shown]);
    let installing =
        refuse_an_emptied_close_and_hold_it(&mut harness, window, shown);
    harness.close_tab(installing);
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.release(Stage::Assessed);
    // The install stays held, so only the refusal's handling can close the
    // window.
    harness.wait_closed(window);
    hooks.release(Stage::Subscribe);
    assert!(!session_exists(&harness));
    assert_eq!(hooks.entered(Stage::Assess), 2);
    assert_eq!(harness.close_failures(), 0);
}

#[gpui::test]
fn a_session_closed_after_core_refused_an_emptied_close_removes_the_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let shown = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[shown]);
    refuse_an_emptied_close_and_hold_it(&mut harness, window, shown);
    harness.close_session();
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.release(Stage::Assessed);
    harness.wait_closed(window);
    hooks.release(Stage::Subscribe);
    // Removed without asking the runtime again.
    assert_eq!(hooks.entered(Stage::Assess), 1);
    assert_eq!(hooks.entered(Stage::Commit), 0);
    assert_eq!(harness.close_failures(), 0);
}

/// Closes `tab`, `window`'s only view, through Mux while `stage` is held,
/// and waits until the emptied close that follows waits at `stage`.
fn hold_the_emptied_close_at(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    tab: TabId,
    stage: Stage,
) {
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(stage);
    harness.close_tab(tab);
    harness.wait_for("the emptied close to wait", |cx| {
        terminal(cx, window, tab).is_none() && hooks.held(stage) == 1
    });
}

#[gpui::test]
fn a_session_closed_while_an_emptied_close_commits_removes_the_window_silently(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    hold_the_emptied_close_at(&mut harness, window, tab, Stage::Commit);
    harness.close_session();
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.release(Stage::Commit);
    harness.wait_closed(window);
    // The stale commit assessed again and found the attachment gone.
    assert_eq!(hooks.entered(Stage::Commit), 1);
    assert_eq!(hooks.entered(Stage::Assess), 2);
    assert_eq!(harness.close_failures(), 0);
}

#[gpui::test]
fn a_quit_queued_behind_an_emptied_close_of_a_closed_session_still_quits(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let unviewed = harness.unviewed_session();
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    hold_the_emptied_close_at(&mut harness, window, tab, Stage::Assess);
    harness.queue_quit(window);
    harness.close_session();
    harness.runtime.worker_hooks.release(Stage::Assess);
    harness.wait_closed(window);
    harness.wait_quit(unviewed);
    assert_eq!(harness.close_failures(), 0);
}

#[gpui::test]
fn a_quit_queued_behind_a_committing_emptied_close_still_quits(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let unviewed = harness.unviewed_session();
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    hold_the_emptied_close_at(&mut harness, window, tab, Stage::Commit);
    harness.queue_quit(window);
    harness.runtime.worker_hooks.release(Stage::Commit);
    harness.wait_closed(window);
    harness.wait_quit(unviewed);
    assert!(!session_exists(&harness));
}

#[gpui::test]
fn a_quit_queued_behind_a_window_close_still_quits(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let unviewed = harness.unviewed_session();
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Commit);
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Window, window, cx);
    });
    harness.wait_for("the window close to commit", |_| {
        hooks.held(Stage::Commit) == 1
    });
    harness.queue_quit(window);
    hooks.release(Stage::Commit);
    harness.wait_closed(window);
    harness.wait_quit(unviewed);
    assert!(!session_exists(&harness));
}

#[gpui::test]
fn a_window_close_of_a_closed_session_removes_the_window_without_a_notice(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Assess);
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Window, window, cx);
    });
    harness.wait_for("the window close to be assessed", |_| {
        hooks.held(Stage::Assess) == 1
    });
    harness.close_session();
    hooks.release(Stage::Assess);
    harness.wait_closed(window);
    assert_eq!(hooks.entered(Stage::Commit), 0);
    assert_eq!(harness.close_failures(), 0);
}

#[gpui::test]
fn cancelling_a_tab_confirmation_whose_tab_left_keeps_a_queued_tab_close(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let runs = harness.open_tab(RUNS_JOB);
    let queued = harness.open_tab(ECHO);
    let stays = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[runs, queued, stays]);
    harness.wait_text(window, runs, "READY");
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Assess);
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Tab(runs), window, cx);
    });
    harness.wait_for("the first tab close to be assessed", |_| {
        hooks.held(Stage::Assess) == 1
    });
    // A second tab close queues behind the first and outlives its prompt.
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Tab(queued), window, cx);
    });
    hooks.release(Stage::Assess);
    harness
        .wait_for("the first tab's confirmation", |cx| confirming(cx, window));
    harness.close_tab(runs);
    harness.wait_for("the queued tab close to run", |cx| {
        !projected(cx, queued) && terminal(cx, window, queued).is_none()
    });
    harness.cx.update(|cx| {
        assert!(!confirming(cx, window));
        assert!(terminal(cx, window, stays).is_some());
    });
    harness.answers(window, stays, "stays");
}

#[gpui::test]
fn a_failed_emptied_close_assessment_keeps_a_queued_quit(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let unviewed = harness.unviewed_session();
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    hold_the_emptied_close_at(&mut harness, window, tab, Stage::Assess);
    harness.queue_quit(window);
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.fail(Stage::Assess);
    hooks.release(Stage::Assess);
    harness.wait_quit(unviewed);
    assert_eq!(harness.close_failures(), 1);
}

/// Whether every view in `window` holds the geometry its chrome gives it.
fn views_hold_window_geometry(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, window, cx| {
            let view = root.downcast::<WorkspaceView>().unwrap().read(cx);
            let geometry = view.terminal_geometry(window);
            geometry.bounds.is_some()
                && view
                    .tabs
                    .iter()
                    .all(|tab| tab.view.read(cx).geometry == geometry)
        })
        .unwrap_or(false)
}

#[gpui::test]
fn a_sidebar_drag_reaches_hidden_views_which_resize_when_shown(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.position = TabPosition::Left;
    });
    let first = harness.open_tab(ECHO);
    let hidden = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_for("both views to install", |cx| {
        terminal(cx, window, first).is_some()
            && terminal(cx, window, hidden).is_some()
    });
    harness.activate(window, first);
    harness.wait_for("every view to take the window's geometry", |cx| {
        views_hold_window_geometry(cx, window)
    });
    let requested = |cx: &mut App, tab: TabId| {
        let view = terminal(cx, window, tab).unwrap();
        view.read(cx).requested_grid
    };
    let before = harness.cx.update(|cx| requested(cx, hidden));

    // Narrow the sidebar from its default 180 points to 140.
    harness.update_view(window, |view, window, cx| {
        view.resize_sidebar(gpui::point(px(140.0), px(10.0)), window, cx);
    });
    harness.wait_for("every view to take the narrower sidebar", |cx| {
        views_hold_window_geometry(cx, window)
    });
    let shown = harness.cx.update(|cx| requested(cx, first));
    assert!(shown.columns > before.columns, "{shown:?} vs {before:?}");
    // A hidden view keeps its grid until it is shown, as before.
    assert_eq!(harness.cx.update(|cx| requested(cx, hidden)), before);

    harness.update_view(window, |view, window, cx| {
        view.select_tab(hidden, true, window, cx);
    });
    harness.wait_for("the shown view to request the wider grid", |cx| {
        requested(cx, hidden) == shown
    });
}
