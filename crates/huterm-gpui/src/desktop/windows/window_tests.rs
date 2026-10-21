//! Tier 2 view tests: the production `Desktop` and `WorkspaceView` roots in
//! GPUI test windows, attached to sessions that the test builds through
//! Mux, as another client would. The harness section of
//! `docs/plans/terminal-viewers.md` describes what this tier covers.

use std::time::{Duration, Instant};

use gpui::{Keystroke, TestAppContext, VisualTestContext};
use huterm_protocol::{
    CellSize, GridSize, TerminalCommand, TerminalInput, TerminalPresentation,
};

use super::close_hooks::Stage;
use super::notices::NoticeContent;
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
    fn wait_for(&mut self, what: &str, mut done: impl FnMut(&mut App) -> bool) {
        let executor = self.cx.executor();
        let foreground = self.cx.foreground_executor().clone();
        let deadline = Instant::now() + WAIT_DEADLINE;
        let cx = &mut self.cx;
        foreground.block_test(async {
            loop {
                let (satisfied, state) = cx.update(|cx| {
                    for handle in cx.windows() {
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

/// A child that sets its title to `one`, rings once, and prints `READY`,
/// then answers each line by setting its title to `two`, ringing twice, and
/// printing `DONE<n>` for the `n`th line.
const STATUS: &str = "printf '\\033]0;one\\007\\007READY'; n=0; \
     while read line; do n=$((n+1)); \
     printf '\\033]0;two\\007\\007\\007DONE%s' $n; done";

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
    harness.wait_for("the first title", |cx| state(cx).0 == "one");
    assert_eq!(harness.cx.update(state), ("one".to_owned(), 1));
    harness.type_line(window, "x");
    harness.wait_text(window, tab, "DONE1");
    harness.wait_for("the second title", |cx| state(cx).0 == "two");
    // Polling again after the change, as every later wake does, applies
    // nothing more.
    harness.type_line(window, "y");
    harness.wait_text(window, tab, "DONE2");
    assert_eq!(harness.cx.update(state), ("two".to_owned(), 5));
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
    let hooks = &hooks.close_hooks;
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
    runtime.close_hooks.hold(stage);
    let _client = harness.send_line(tab, "x");
    harness.wait_for(
        "both windows to see the exit and one close to wait",
        |cx| {
            windows.into_iter().all(|window| exited(cx, window, tab))
                && runtime.close_hooks.held(stage) > 0
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
    let hooks = &harness.runtime.close_hooks;
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
    harness.runtime.close_hooks.release(Stage::Assess);
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
    let hooks = &runtime.close_hooks;
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
    let hooks = &runtime.close_hooks;
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
    let hooks = &runtime.close_hooks;
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
    harness.runtime.close_hooks.release(Stage::Commit);
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
    let hooks = &runtime.close_hooks;
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
    harness.runtime.close_hooks.fail(Stage::Commit);
    harness.runtime.close_hooks.release(Stage::Commit);
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
    assert_eq!(harness.runtime.close_hooks.entered(Stage::Assess), 0);
}
