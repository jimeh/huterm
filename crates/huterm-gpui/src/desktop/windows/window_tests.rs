//! Tier 2 view tests: the production `Desktop` and `WorkspaceView` roots in
//! GPUI test windows, attached to sessions that the test builds through
//! Mux, as another client would. The harness section of
//! `docs/plans/terminal-viewers.md` describes what this tier covers.

use std::time::{Duration, Instant};

use gpui::{Keystroke, TestAppContext, VisualTestContext};
use huterm_protocol::{
    CellSize, GridSize, TerminalCommand, TerminalPresentation,
};

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
