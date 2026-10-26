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
        scrollback: huterm_protocol::ScrollbackLimit::DEFAULT,
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
                .map(|tab| tab.focused_view().clone())
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
    let records = desktop
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
        .join("; ");
    let views = cx
        .windows()
        .into_iter()
        .filter_map(|handle| handle.downcast::<WorkspaceView>()?.read(cx).ok())
        .flat_map(|view| &view.tabs)
        .map(|tab| describe_view(tab.id, tab.focused_view().read(cx)))
        .collect::<Vec<_>>()
        .join("; ");
    format!("{records}; views: {views}")
}

/// A view's viewer, polled status, and the end of its screen: output a
/// fixture prints after a host effect shows whether the terminal parsed it.
fn describe_view(tab: TabId, view: &TerminalView) -> String {
    let text = view.snapshot.as_ref().map_or_else(String::new, |snapshot| {
        snapshot.cells().map(|cell| cell.text.as_str()).collect()
    });
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let tail = text
        .char_indices()
        .rev()
        .nth(59)
        .map_or(text.as_str(), |(index, _)| &text[index..]);
    format!(
        "tab {}: epoch={} revoked={} exited={} title={:?} clipboard_writes={} text_tail={tail:?}",
        tab.get(),
        view.viewer_epoch,
        view.revoked,
        view.exited,
        view.title,
        view.clipboard_writes,
    )
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
        let desktop = cx.global::<Desktop>();
        desktop
            .exit_closes
            .owed()
            .filter_map(|pane| desktop.hierarchy.state().pane_tab(pane))
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
/// through OSC 52 and then printing `CLIPPED`, and `exit` by exiting.
const REPLIES: &str = "printf READY; while read line; do case \"$line\" in \
     out) printf OUT;; \
     title) printf '\\033]0;two\\007';; \
     clip) printf '\\033]52;c;Y2xpcA==\\007CLIPPED';; \
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

/// A child that prints 3,000 numbered rows, then `READY`, and waits.
const PRINTS_HISTORY: &str = "i=0; while [ $i -lt 3000 ]; do \
     printf 'row %s\n' $i; i=$((i+1)); done; printf READY; read line";

fn scrollback(rows: u32) -> huterm_protocol::ScrollbackLimit {
    huterm_protocol::ScrollbackLimit::new(rows).unwrap()
}

#[gpui::test]
fn an_installed_view_applies_the_configured_limit_to_an_older_terminal(
    cx: &mut TestAppContext,
) {
    // The terminal starts under the default limit, as one opened before a
    // reload or by another client would.
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.terminal.scrollback = scrollback(1000);
    });
    let tab = harness.open_tab(PRINTS_HISTORY);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    harness.wait_canonical(tab, "the configured limit", |snapshot| {
        snapshot.history_size <= 1000
    });
}

#[gpui::test]
fn a_reload_shrinks_the_history_of_a_hidden_tab(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let shown = harness.open_tab(ECHO);
    let hidden = harness.open_tab(PRINTS_HISTORY);
    let window = harness.attach_window();
    harness.activate(window, shown);
    harness.wait_for("the window to install both tabs", |cx| {
        terminal(cx, window, hidden).is_some()
    });
    harness.wait_canonical(hidden, "the history to print", |snapshot| {
        snapshot.history_size > 2000
    });
    // As a reload does for every tab in the window, shown or not.
    let reloaded = config::TerminalConfig {
        scrollback: scrollback(1000),
        ..config::TerminalConfig::default()
    };
    harness.cx.update(|cx| {
        terminal(cx, window, hidden)
            .unwrap()
            .update(cx, |view, cx| {
                assert!(!view.visible);
                view.reload_terminal_config(reloaded, cx);
            });
    });
    harness.wait_canonical(hidden, "the reloaded limit", |snapshot| {
        snapshot.history_size <= 1000
    });
}

#[gpui::test]
fn a_replacement_started_before_a_scrollback_change_takes_the_current_limit(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(PRINTS_HISTORY);
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
    // As a reload does: the revoked viewer refuses the new limit.
    harness.cx.update(|cx| {
        terminal(cx, window, tab).unwrap().update(cx, |view, _| {
            view.scrollback = scrollback(1000);
            view.publish_scrollback();
        });
    });
    hooks.release(Stage::Subscribe);
    harness.wait_for("the replacement", |cx| {
        replaced(cx, window, tab, 1).is_some()
    });
    harness.wait_canonical(tab, "the current limit", |snapshot| {
        snapshot.history_size <= 1000
    });
}

#[gpui::test]
fn a_limit_refused_before_a_replacement_starts_reaches_the_replacement(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(PRINTS_HISTORY);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    harness.hold_replacements(window, true);
    harness.revoke(window);
    // A reload reaches the revoked viewer before its replacement starts,
    // so the replacement starts with the new limit already in the view.
    harness.cx.update(|cx| {
        terminal(cx, window, tab).unwrap().update(cx, |view, _| {
            view.scrollback = scrollback(1000);
            view.publish_scrollback();
        });
    });
    harness.hold_replacements(window, false);
    harness.wait_for("the replacement", |cx| {
        replaced(cx, window, tab, 1).is_some()
    });
    harness.wait_canonical(tab, "the refused limit", |snapshot| {
        snapshot.history_size <= 1000
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
        std::mem::replace(&mut tab.panes[0].activity_task, Task::ready(()))
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
                && view.tabs.iter().all(|tab| {
                    tab.views().all(|view| view.read(cx).geometry == geometry)
                })
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

/// The view of `pane` in `window`'s view of `tab`.
fn pane_view(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    pane: PaneId,
) -> Option<Entity<TerminalView>> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx)
                .tabs
                .iter()
                .find(|view| view.id == tab)
                .and_then(|view| view.pane(pane))
                .map(|pane| pane.view.clone())
        })
        .ok()
        .flatten()
}

/// The text `window`'s view of `pane` shows, if it has a snapshot.
fn pane_screen(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    pane: PaneId,
) -> Option<String> {
    let snapshot = pane_view(cx, window, tab, pane)?
        .read(cx)
        .snapshot
        .clone()?;
    Some(snapshot.cells().map(|cell| cell.text.as_str()).collect())
}

/// `window`'s pane views of `tab`, and the one it focuses.
fn panes_shown(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
) -> Option<(Vec<PaneId>, PaneId)> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let tab = view.read(cx).tabs.iter().find(|view| view.id == tab)?;
            Some((
                tab.panes.iter().map(|pane| pane.pane).collect(),
                tab.focused().pane,
            ))
        })
        .ok()
        .flatten()
}

impl DesktopHarness {
    /// The first pane of `tab`, from the runtime.
    fn first_pane(&self, tab: TabId) -> PaneId {
        self.runtime.lock().tab(tab).unwrap().first_pane().id
    }

    /// Splits `pane` of `tab` through Mux, as another client would.
    fn split(
        &self,
        tab: TabId,
        pane: PaneId,
        direction: SplitDirection,
        script: &str,
    ) -> PaneId {
        self.runtime
            .lock()
            .split_pane(self.workspace, tab, pane, direction, &fixture(script))
            .unwrap()
            .pane
            .id
    }

    /// Runs `command` in `window` as a binding would.
    fn invoke(
        &mut self,
        window: AnyWindowHandle,
        command: huterm_protocol::CommandId,
    ) -> Result<CommandOutcome, CommandError> {
        self.update_view(window, |view, window, cx| {
            view.invoke_interactive(
                &CommandInvocation::new(command, vec![]),
                window,
                cx,
            )
        })
    }
}

#[gpui::test]
fn a_pane_another_client_splits_installs_beside_the_focused_one(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    let first = harness.first_pane(tab);
    let whole = harness.cx.update(|cx| {
        pane_view(cx, window, tab, first)
            .unwrap()
            .read(cx)
            .requested_grid
    });
    let second = harness.split(tab, first, SplitDirection::Right, ECHO);
    harness.wait_for("the second pane to install and show", |cx| {
        pane_screen(cx, window, tab, second)
            .is_some_and(|text| text.contains("READY"))
    });
    // Each pane takes its share of the tab, and both views report it.
    harness.wait_for("both panes to request narrower grids", |cx| {
        [first, second].into_iter().all(|pane| {
            pane_view(cx, window, tab, pane).is_some_and(|view| {
                let grid = view.read(cx).requested_grid;
                grid.columns < whole.columns / 2 + 1 && grid.rows < whole.rows
            })
        })
    });
    // Another client's split leaves this window's focus where it was.
    assert_eq!(
        harness.cx.update(|cx| panes_shown(cx, window, tab)),
        Some((vec![first, second], first))
    );
    harness.type_line(window, "one");
    harness.wait_for("the first pane to read the line", |cx| {
        pane_screen(cx, window, tab, first)
            .is_some_and(|text| text.contains("got:one"))
    });
    assert!(
        harness.cx.update(|cx| {
            !pane_screen(cx, window, tab, second)
                .unwrap_or_default()
                .contains("got:one")
        }),
        "the unfocused pane read the line"
    );
}

#[gpui::test]
fn pane_focus_commands_route_typing_and_wrap_or_stop_at_edges(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    let first = harness.first_pane(tab);
    let second = harness.split(tab, first, SplitDirection::Right, ECHO);
    harness.wait_for("the second pane to show", |cx| {
        pane_screen(cx, window, tab, second)
            .is_some_and(|text| text.contains("READY"))
    });
    harness.invoke(window, ids::FOCUS_NEXT_PANE).unwrap();
    harness.wait_for("the second pane to take keyboard focus", |cx| {
        pane_view(cx, window, tab, second).is_some_and(|view| {
            window
                .update(cx, |_, window, cx| {
                    view.read(cx).focus.is_focused(window)
                })
                .unwrap_or(false)
        })
    });
    harness.type_line(window, "two");
    harness.wait_for("the second pane to read the line", |cx| {
        pane_screen(cx, window, tab, second)
            .is_some_and(|text| text.contains("got:two"))
    });
    // Terminal commands follow focus too.
    let routed = harness.update_view(window, |view, _, cx| {
        view.active_view(cx).map(|view| view.entity_id())
    });
    let second_view = harness
        .cx
        .update(|cx| pane_view(cx, window, tab, second).map(|v| v.entity_id()));
    assert_eq!(routed, second_view);
    // Next wraps; a direction stops at the layout's edge.
    harness.invoke(window, ids::FOCUS_NEXT_PANE).unwrap();
    assert_eq!(
        harness
            .cx
            .update(|cx| panes_shown(cx, window, tab))
            .map(|(_, f)| f),
        Some(first)
    );
    harness.invoke(window, ids::FOCUS_PANE_LEFT).unwrap();
    assert_eq!(
        harness
            .cx
            .update(|cx| panes_shown(cx, window, tab))
            .map(|(_, f)| f),
        Some(first)
    );
    harness.invoke(window, ids::FOCUS_PANE_RIGHT).unwrap();
    assert_eq!(
        harness
            .cx
            .update(|cx| panes_shown(cx, window, tab))
            .map(|(_, f)| f),
        Some(second)
    );
}

#[gpui::test]
fn close_pane_closes_the_focused_pane_and_the_last_pane_closes_its_tab(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let other = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    let first = harness.first_pane(tab);
    let second = harness.split(tab, first, SplitDirection::Down, ECHO);
    harness.wait_for("the second pane to show", |cx| {
        pane_screen(cx, window, tab, second)
            .is_some_and(|text| text.contains("READY"))
    });
    harness.invoke(window, ids::FOCUS_NEXT_PANE).unwrap();
    harness.invoke(window, ids::CLOSE_PANE).unwrap();
    harness.wait_for("the focused pane to close", |cx| {
        panes_shown(cx, window, tab) == Some((vec![first], first))
            && cx
                .global::<Desktop>()
                .hierarchy
                .state()
                .tab(tab)
                .is_some_and(|info| info.layout.pane_count() == 1)
    });
    // Focus falls back to the remaining pane, which still answers.
    harness.wait_for("the remaining pane to take keyboard focus", |cx| {
        focused(cx, window, tab)
    });
    harness.type_line(window, "after");
    harness.wait_for("the remaining pane to read the line", |cx| {
        pane_screen(cx, window, tab, first)
            .is_some_and(|text| text.contains("got:after"))
    });
    // Closing the only pane closes the tab.
    harness.invoke(window, ids::CLOSE_PANE).unwrap();
    harness.wait_for("the tab to close", |cx| !projected(cx, tab));
    assert!(harness.cx.update(|cx| projected(cx, other)));
}

#[gpui::test]
fn an_exited_pane_closes_alone_under_close_on_exit(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    let first = harness.first_pane(tab);
    let exits = harness.split(tab, first, SplitDirection::Right, EXITS);
    harness.wait_for("the exiting pane to show", |cx| {
        pane_screen(cx, window, tab, exits)
            .is_some_and(|text| text.contains("READY"))
    });
    harness.invoke(window, ids::FOCUS_NEXT_PANE).unwrap();
    harness.type_line(window, "x");
    harness.wait_for("only the exited pane to close", |cx| {
        panes_shown(cx, window, tab) == Some((vec![first], first))
    });
    assert!(harness.cx.update(|cx| projected(cx, tab)));
    harness.type_line(window, "still");
    harness.wait_for("the sibling to answer", |cx| {
        pane_screen(cx, window, tab, first)
            .is_some_and(|text| text.contains("got:still"))
    });
}

#[gpui::test]
fn two_windows_focus_the_panes_of_one_tab_independently(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (left, right) = (harness.attach_window(), harness.attach_window());
    let first = harness.first_pane(tab);
    let second = harness.split(tab, first, SplitDirection::Right, ECHO);
    harness.wait_for("both windows to show both panes", |cx| {
        [left, right].into_iter().all(|window| {
            panes_shown(cx, window, tab)
                .is_some_and(|(panes, _)| panes.len() == 2)
        })
    });
    harness.invoke(left, ids::FOCUS_NEXT_PANE).unwrap();
    assert_eq!(
        harness
            .cx
            .update(|cx| panes_shown(cx, left, tab))
            .map(|(_, f)| f),
        Some(second)
    );
    assert_eq!(
        harness
            .cx
            .update(|cx| panes_shown(cx, right, tab))
            .map(|(_, f)| f),
        Some(first)
    );
    // Closing the pane one window focuses moves that window's focus.
    harness
        .runtime
        .lock()
        .close_pane(harness.workspace, tab, second)
        .unwrap();
    harness.wait_for("both windows to drop the pane", |cx| {
        [left, right].into_iter().all(|window| {
            panes_shown(cx, window, tab) == Some((vec![first], first))
        })
    });
}

#[gpui::test]
fn a_split_that_would_leave_a_pane_under_the_minimum_is_refused(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    let first = harness.first_pane(tab);
    let fits = |harness: &mut DesktopHarness, direction| {
        harness.update_view(window, |view, window, cx| {
            view.split_grid(tab, first, direction, window, cx)
        })
    };
    assert!(fits(&mut harness, SplitDirection::Down).is_ok());
    // Shrink the window until the lone pane is six rows tall: a split
    // across rows leaves each half under two rows once it takes a title
    // bar, while a split across columns keeps every row.
    let (chrome, cell) = harness.update_view(window, |view, window, _| {
        let bounds = view.terminal_geometry(window).bounds.unwrap();
        (
            window.viewport_size().height - bounds.size.height,
            view.metrics.at_scale(window.scale_factor()).cell_height,
        )
    });
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(gpui::size(px(800.0), chrome + cell * 6.5));
    harness.wait_for("the lone pane to request six rows", |cx| {
        pane_view(cx, window, tab, first)
            .is_some_and(|view| view.read(cx).requested_grid.rows == 6)
    });
    let refused = fits(&mut harness, SplitDirection::Down);
    assert!(
        matches!(
            &refused,
            Err(CommandError::Unavailable(reason)) if reason.contains("too small")
        ),
        "{refused:?}"
    );
    let beside = fits(&mut harness, SplitDirection::Right).unwrap();
    assert!(beside.rows >= 2, "{beside:?}");
}

#[gpui::test]
fn a_pane_title_bars_close_button_closes_only_its_pane(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    let first = harness.first_pane(tab);
    let second = harness.split(tab, first, SplitDirection::Right, ECHO);
    harness.wait_for("the second pane to show", |cx| {
        pane_screen(cx, window, tab, second)
            .is_some_and(|text| text.contains("READY"))
    });
    let close = harness.update_view(window, |view, window, cx| {
        let bounds = view.terminal_geometry(window).bounds.unwrap();
        let rects = view.tab_panes(tab, bounds, None, window, cx).unwrap();
        let title_bar = rects.pane(second).unwrap().title_bar.unwrap();
        WorkspaceView::pane_close_center(title_bar)
    });
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_click(close, gpui::Modifiers::none());
    harness.wait_for("the clicked pane to close", |cx| {
        panes_shown(cx, window, tab) == Some((vec![first], first))
    });
}

#[gpui::test]
fn closing_the_focused_pane_elsewhere_refocuses_the_survivor(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    let first = harness.first_pane(tab);
    let second = harness.split(tab, first, SplitDirection::Right, ECHO);
    harness.wait_for("the second pane to show", |cx| {
        pane_screen(cx, window, tab, second)
            .is_some_and(|text| text.contains("READY"))
    });
    harness.invoke(window, ids::FOCUS_NEXT_PANE).unwrap();
    harness.wait_for("the second pane to take keyboard focus", |cx| {
        focused(cx, window, tab)
            && panes_shown(cx, window, tab).is_some_and(|(_, f)| f == second)
    });
    harness
        .runtime
        .lock()
        .close_pane(harness.workspace, tab, second)
        .unwrap();
    harness.wait_for("the survivor to take keyboard focus", |cx| {
        panes_shown(cx, window, tab) == Some((vec![first], first))
            && focused(cx, window, tab)
    });
    harness.type_line(window, "survivor");
    harness.wait_for("the survivor to read the line", |cx| {
        pane_screen(cx, window, tab, first)
            .is_some_and(|text| text.contains("got:survivor"))
    });
}

#[gpui::test]
fn a_pane_split_while_its_tab_installs_a_pane_still_installs(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    let first = harness.first_pane(tab);
    let hooks = &harness.runtime.worker_hooks;
    hooks.hold(Stage::Subscribe);
    let second = harness.split(tab, first, SplitDirection::Right, ECHO);
    let runtime = Arc::clone(&harness.runtime);
    harness.wait_for("the second pane's install to wait", |_| {
        runtime.worker_hooks.held(Stage::Subscribe) == 1
    });
    // The tab's install is in flight, so reconcile skips the third pane.
    let third = harness.split(tab, second, SplitDirection::Down, ECHO);
    harness.wait_for("the projection to hold the third pane", |cx| {
        cx.global::<Desktop>()
            .hierarchy
            .state()
            .tab(tab)
            .is_some_and(|info| info.layout.pane(third).is_some())
    });
    harness.runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_for("both new panes to install", |cx| {
        panes_shown(cx, window, tab).is_some_and(|(panes, _)| {
            panes.contains(&second) && panes.contains(&third)
        })
    });
}

#[gpui::test]
fn a_pane_confirmation_whose_pane_closes_elsewhere_is_dismissed(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    let first = harness.first_pane(tab);
    let busy_pane = harness.split(tab, first, SplitDirection::Right, RUNS_JOB);
    harness.wait_for("the busy pane to show", |cx| {
        pane_screen(cx, window, tab, busy_pane)
            .is_some_and(|text| text.contains("READY"))
    });
    harness.invoke(window, ids::FOCUS_NEXT_PANE).unwrap();
    harness.invoke(window, ids::CLOSE_PANE).unwrap();
    harness
        .wait_for("the pane close to ask first", |cx| confirming(cx, window));
    harness
        .runtime
        .lock()
        .close_pane(harness.workspace, tab, busy_pane)
        .unwrap();
    harness.wait_for("the stale confirmation to go", |cx| {
        !confirming(cx, window)
            && panes_shown(cx, window, tab) == Some((vec![first], first))
    });
    harness.cx.update(|cx| {
        assert_eq!(notices(cx, window), Vec::<String>::new());
    });
}

#[gpui::test]
fn focus_falls_back_to_the_first_pane_in_layout_order(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    let middle = harness.first_pane(tab);
    // Installed after the middle pane, but laid out right and then left.
    let right = harness.split(tab, middle, SplitDirection::Right, ECHO);
    harness.wait_for("the right pane to install", |cx| {
        panes_shown(cx, window, tab).is_some_and(|(panes, _)| panes.len() == 2)
    });
    let left = harness.split(tab, middle, SplitDirection::Left, ECHO);
    harness.wait_for("the left pane to install", |cx| {
        panes_shown(cx, window, tab)
            == Some((vec![left, middle, right], middle))
    });
    harness
        .runtime
        .lock()
        .close_pane(harness.workspace, tab, middle)
        .unwrap();
    harness.wait_for("focus to fall back to the leftmost pane", |cx| {
        panes_shown(cx, window, tab) == Some((vec![left, right], left))
    });
    // A pane split in before the fallback in layout order leaves it focused.
    let far_left = harness.split(tab, left, SplitDirection::Left, ECHO);
    harness.wait_for("the far left pane to install", |cx| {
        panes_shown(cx, window, tab)
            == Some((vec![far_left, left, right], left))
    });
}

/// Whether `window`'s view of `pane` is visible, and the grid it requests.
fn pane_shown(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    pane: PaneId,
) -> Option<(bool, GridSize)> {
    let view = pane_view(cx, window, tab, pane)?;
    let view = view.read(cx);
    Some((view.visible, view.requested_grid))
}

/// The weights of the projected layout's root split of `tab`.
fn root_weights(cx: &App, tab: TabId) -> Option<Vec<u32>> {
    match cx
        .global::<Desktop>()
        .hierarchy
        .state()
        .tab(tab)?
        .layout
        .root()
    {
        huterm_protocol::PaneNode::Split(split) => {
            Some(split.children.iter().map(|child| child.weight).collect())
        }
        huterm_protocol::PaneNode::Pane(_) => None,
    }
}

impl DesktopHarness {
    /// A window showing `tab` split into two ECHO panes side by side, and
    /// the two panes, once both show their shells.
    fn split_window(
        &mut self,
        tab: TabId,
    ) -> (AnyWindowHandle, PaneId, PaneId) {
        let window = self.attach_window();
        self.wait_text(window, tab, "READY");
        self.activate(window, tab);
        let first = self.first_pane(tab);
        let second = self.split(tab, first, SplitDirection::Right, ECHO);
        self.wait_for("the second pane to show", |cx| {
            pane_screen(cx, window, tab, second)
                .is_some_and(|text| text.contains("READY"))
        });
        (window, first, second)
    }

    /// The centre of `window`'s first drawn divider grab zone.
    fn divider_center(
        &mut self,
        window: AnyWindowHandle,
    ) -> gpui::Point<Pixels> {
        self.update_view(window, |view, _, _| {
            view.drawn_dividers
                .borrow()
                .first()
                .map(|divider| divider.grab.center())
        })
        .expect("a drawn divider")
    }
}

#[gpui::test]
fn zooming_a_pane_hides_its_siblings_in_this_window_only(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    let other = harness.attach_window();
    harness.wait_for("the other window to show both panes", |cx| {
        panes_shown(cx, other, tab).is_some_and(|(panes, _)| panes.len() == 2)
    });
    let split_grid = harness
        .cx
        .update(|cx| pane_shown(cx, window, tab, second))
        .unwrap()
        .1;
    harness.activate(window, tab);
    harness.invoke(window, ids::FOCUS_NEXT_PANE).unwrap();
    harness.invoke(window, ids::TOGGLE_PANE_ZOOM).unwrap();
    harness.wait_for("the zoomed pane to take the whole tab", |cx| {
        pane_shown(cx, window, tab, second).is_some_and(|(visible, grid)| {
            visible && grid.columns > split_grid.columns
        }) && pane_shown(cx, window, tab, first)
            .is_some_and(|(visible, _)| !visible)
    });
    // The other window keeps both panes; the hidden one keeps its grid.
    harness.cx.update(|cx| {
        assert!(pane_shown(cx, other, tab, first).unwrap().0);
        assert!(pane_shown(cx, other, tab, second).unwrap().0);
        assert!(
            pane_shown(cx, window, tab, first).unwrap().1.columns
                <= split_grid.columns + 1
        );
    });
    // Focus navigation unzooms, as tmux does.
    harness.invoke(window, ids::FOCUS_PREVIOUS_PANE).unwrap();
    harness.wait_for("navigation to unzoom", |cx| {
        pane_shown(cx, window, tab, first).is_some_and(|(visible, _)| visible)
            && pane_shown(cx, window, tab, second)
                .is_some_and(|(visible, grid)| visible && grid == split_grid)
            && panes_shown(cx, window, tab)
                .is_some_and(|(_, focus)| focus == first)
    });
}

#[gpui::test]
fn a_shared_zoom_set_in_one_window_moves_the_others_focus(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.panes.zoom = huterm_config::PaneZoomScope::Shared;
    });
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    let other = harness.attach_window();
    harness.wait_for("the other window to show both panes", |cx| {
        panes_shown(cx, other, tab) == Some((vec![first, second], first))
    });
    harness.activate(window, tab);
    harness.invoke(window, ids::FOCUS_NEXT_PANE).unwrap();
    harness.invoke(window, ids::TOGGLE_PANE_ZOOM).unwrap();
    harness.wait_for("both windows to show the shared zoom", |cx| {
        [window, other].into_iter().all(|shown| {
            pane_shown(cx, shown, tab, first)
                .is_some_and(|(visible, _)| !visible)
                && pane_shown(cx, shown, tab, second)
                    .is_some_and(|(visible, _)| visible)
        }) && panes_shown(cx, other, tab)
            .is_some_and(|(_, focus)| focus == second)
    });
    // Closing the zoomed pane elsewhere clears the zoom.
    harness
        .runtime
        .lock()
        .close_pane(harness.workspace, tab, second)
        .unwrap();
    harness.wait_for("the survivor to show unzoomed", |cx| {
        [window, other].into_iter().all(|shown| {
            panes_shown(cx, shown, tab) == Some((vec![first], first))
                && pane_shown(cx, shown, tab, first)
                    .is_some_and(|(visible, _)| visible)
        })
    });
}

#[gpui::test]
fn a_title_bars_zoom_button_toggles_its_pane(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    let button = |harness: &mut DesktopHarness, pane| {
        harness.update_view(window, |view, window, cx| {
            let bounds = view.terminal_geometry(window).bounds.unwrap();
            let zoom = view
                .active_tab_view(cx)
                .and_then(|tab| view.shown_zoom(tab, cx));
            let rects = view.tab_panes(tab, bounds, zoom, window, cx).unwrap();
            WorkspaceView::pane_zoom_center(
                rects.pane(pane).unwrap().title_bar.unwrap(),
            )
        })
    };
    let zoom = button(&mut harness, first);
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_click(zoom, gpui::Modifiers::none());
    harness.wait_for("the first pane to zoom and take focus", |cx| {
        pane_shown(cx, window, tab, second).is_some_and(|(visible, _)| !visible)
            && panes_shown(cx, window, tab)
                .is_some_and(|(_, focus)| focus == first)
    });
    let unzoom = button(&mut harness, first);
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_click(unzoom, gpui::Modifiers::none());
    harness.wait_for("the second pane to show again", |cx| {
        pane_shown(cx, window, tab, second).is_some_and(|(visible, _)| visible)
    });
}

#[gpui::test]
fn a_divider_drag_previews_reflows_and_commits_once(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    let before = harness
        .cx
        .update(|cx| pane_shown(cx, window, tab, first))
        .unwrap()
        .1;
    let start = harness.divider_center(window);
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_down(
        start,
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    visual.simulate_mouse_move(
        start + gpui::point(px(-120.0), px(0.0)),
        Some(MouseButton::Left),
        gpui::Modifiers::none(),
    );
    // The preview resizes both panes while the runtime keeps its weights.
    harness.wait_for("the preview to narrow the first pane", |cx| {
        pane_shown(cx, window, tab, first)
            .is_some_and(|(_, grid)| grid.columns < before.columns)
            && root_weights(cx, tab) == Some(vec![500_000, 500_000])
    });
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_up(
        start + gpui::point(px(-120.0), px(0.0)),
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    harness.wait_for("the release to commit the weights", |cx| {
        root_weights(cx, tab).is_some_and(|weights| weights[0] < 500_000)
    });
    let shown = harness
        .cx
        .update(|cx| pane_shown(cx, window, tab, first))
        .unwrap()
        .1;
    assert!(shown.columns < before.columns, "the divider snapped back");
    // No terminal saw the press: neither pane selected anything.
    harness.cx.update(|cx| {
        for pane in [first, second] {
            let view = pane_view(cx, window, tab, pane).unwrap();
            assert!(view.read(cx).selection.is_none());
        }
    });
}

#[gpui::test]
fn escape_or_losing_the_split_cancels_a_divider_drag(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    let start = harness.divider_center(window);
    let drag = |harness: &mut DesktopHarness| {
        let mut visual = VisualTestContext::from_window(window, &harness.cx);
        visual.simulate_mouse_down(
            start,
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        visual.simulate_mouse_move(
            start + gpui::point(px(80.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::none(),
        );
    };
    let dragging = |cx: &mut App| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .is_ok_and(|view| view.read(cx).divider_drag.is_some())
            })
            .unwrap_or(false)
    };
    drag(&mut harness);
    harness.wait_for("the drag to start", dragging);
    harness
        .cx
        .dispatch_keystroke(window, Keystroke::parse("escape").unwrap());
    harness.wait_for("Escape to cancel the drag", |cx| !dragging(cx));
    drag(&mut harness);
    harness.wait_for("the drag to start again", dragging);
    harness
        .runtime
        .lock()
        .close_pane(harness.workspace, tab, second)
        .unwrap();
    harness.wait_for("the closed split to cancel the drag", |cx| {
        !dragging(cx)
            && panes_shown(cx, window, tab) == Some((vec![first], first))
    });
    assert_eq!(harness.cx.update(|cx| root_weights(cx, tab)), None);
}

#[gpui::test]
fn resize_commands_move_the_pane_edge_and_stop_at_the_minimum(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    let grids = |cx: &mut App| {
        (
            pane_shown(cx, window, tab, first).unwrap().1,
            pane_shown(cx, window, tab, second).unwrap().1,
        )
    };
    let (left, right) = harness.cx.update(grids);
    harness.invoke(window, ids::RESIZE_PANE_RIGHT).unwrap();
    harness.wait_for("the first pane to grow by five columns", |cx| {
        let (now_left, now_right) = grids(cx);
        now_left.columns == left.columns + 5
            && now_right.columns + 5 == right.columns
    });
    // The first pane has no left neighbour, so moving its left edge does
    // nothing; a large step right stops with the second at the minimum.
    harness.invoke(window, ids::RESIZE_PANE_LEFT).unwrap();
    harness
        .update_view(window, |view, window, cx| {
            view.invoke_interactive(
                &CommandInvocation::new(
                    ids::RESIZE_PANE_RIGHT,
                    vec![huterm_protocol::CommandArgument::new(
                        "amount",
                        huterm_protocol::CommandValue::Integer(100),
                    )],
                ),
                window,
                cx,
            )
        })
        .unwrap();
    harness.wait_for("the second pane to stop at four columns", |cx| {
        grids(cx).1.columns == MIN_PANE_COLUMNS
    });
}

#[gpui::test]
fn a_selection_drag_stops_at_its_panes_edge(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    let (from, to, columns) =
        harness.update_view(window, |view, window, cx| {
            let bounds = view.terminal_geometry(window).bounds.unwrap();
            let rects = view.tab_panes(tab, bounds, None, window, cx).unwrap();
            let left = rects.pane(first).unwrap().terminal;
            let right = rects.pane(second).unwrap().terminal;
            let columns = view
                .active_tab_view(cx)
                .and_then(|tab| tab.pane(first))
                .map(|pane| pane.view.read(cx).requested_grid.columns)
                .unwrap();
            (
                left.origin + gpui::point(px(20.0), px(20.0)),
                right.center(),
                columns,
            )
        });
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_down(
        from,
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    visual.simulate_mouse_move(
        to,
        Some(MouseButton::Left),
        gpui::Modifiers::none(),
    );
    visual.simulate_mouse_up(to, MouseButton::Left, gpui::Modifiers::none());
    harness.wait_for("the first pane to select up to its own edge", |cx| {
        pane_view(cx, window, tab, first).is_some_and(|view| {
            view.read(cx)
                .selection
                .is_some_and(|selection| selection.head.column + 1 == columns)
        })
    });
    harness.cx.update(|cx| {
        let other = pane_view(cx, window, tab, second).unwrap();
        assert!(
            other.read(cx).selection.is_none(),
            "the drag selected in the other pane"
        );
    });
}

#[gpui::test]
fn queued_resize_commands_land_in_order(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, _) = harness.split_window(tab);
    let columns =
        |cx: &mut App| pane_shown(cx, window, tab, first).unwrap().1.columns;
    let before = harness.cx.update(columns);
    {
        // Both commits wait on the Mux, so only their order decides the
        // weights they leave.
        let runtime = Arc::clone(&harness.runtime);
        let _held = runtime.lock();
        harness.invoke(window, ids::RESIZE_PANE_RIGHT).unwrap();
        harness.invoke(window, ids::RESIZE_PANE_RIGHT).unwrap();
    }
    harness.wait_for("both steps to land", |cx| {
        columns(cx) == before + 10
            && window
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>().is_ok_and(|view| {
                        view.read(cx).resize_commits.is_empty()
                    })
                })
                .unwrap_or(false)
    });
    assert_eq!(harness.cx.update(columns), before + 10);
}

#[gpui::test]
fn a_drag_carries_its_boundary_over_another_windows_resize(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    let third = harness.split(tab, second, SplitDirection::Right, ECHO);
    harness.wait_for("the third pane to show", |cx| {
        panes_shown(cx, window, tab).is_some_and(|(panes, _)| panes.len() == 3)
    });
    let start = harness.divider_center(window);
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_down(
        start,
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    visual.simulate_mouse_move(
        start + gpui::point(px(-40.0), px(0.0)),
        Some(MouseButton::Left),
        gpui::Modifiers::none(),
    );
    // Another client moves the boundary between the second and third panes.
    let (split, children, mut weights) = harness.cx.update(|cx| {
        match &cx
            .global::<Desktop>()
            .hierarchy
            .state()
            .tab(tab)
            .unwrap()
            .layout
            .root()
        {
            huterm_protocol::PaneNode::Split(split) => (
                split.id,
                split
                    .children
                    .iter()
                    .map(|child| child.node.id())
                    .collect::<Vec<_>>(),
                split
                    .children
                    .iter()
                    .map(|child| child.weight)
                    .collect::<Vec<_>>(),
            ),
            huterm_protocol::PaneNode::Pane(_) => panic!("not split"),
        }
    });
    let moved = weights[2] / 2;
    weights[1] += moved;
    weights[2] -= moved;
    let elsewhere = weights.clone();
    harness
        .runtime
        .lock()
        .resize_split(harness.workspace, tab, split, &children, &elsewhere)
        .unwrap();
    harness.wait_for("the projection to hold the other resize", |cx| {
        root_weights(cx, tab).as_deref() == Some(&elsewhere[..])
    });
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_move(
        start + gpui::point(px(-60.0), px(0.0)),
        Some(MouseButton::Left),
        gpui::Modifiers::none(),
    );
    visual.simulate_mouse_up(
        start + gpui::point(px(-60.0), px(0.0)),
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    harness.wait_for("the drag to commit over the other resize", |cx| {
        root_weights(cx, tab)
            .is_some_and(|now| now[0] < elsewhere[0] && now[2] == elsewhere[2])
    });
    harness.cx.update(|cx| {
        assert!(pane_shown(cx, window, tab, third).is_some());
        assert!(pane_shown(cx, window, tab, first).is_some());
    });
}

#[gpui::test]
fn a_shared_zoom_from_elsewhere_cancels_a_divider_drag(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.panes.zoom = huterm_config::PaneZoomScope::Shared;
    });
    let tab = harness.open_tab(ECHO);
    let (window, _, second) = harness.split_window(tab);
    let start = harness.divider_center(window);
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_down(
        start,
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    let dragging = |cx: &mut App| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .is_ok_and(|view| view.read(cx).divider_drag.is_some())
            })
            .unwrap_or(false)
    };
    harness.wait_for("the drag to start", dragging);
    harness
        .runtime
        .lock()
        .set_pane_zoom(harness.workspace, tab, Some(second))
        .unwrap();
    harness.wait_for("the zoom to cancel the drag", |cx| {
        !dragging(cx)
            && pane_shown(cx, window, tab, second)
                .is_some_and(|(visible, _)| visible)
    });
}

#[gpui::test]
fn a_rejected_resize_drops_its_preview_for_the_new_layout(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    let start = harness.divider_center(window);
    let end = start + gpui::point(px(-80.0), px(0.0));
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_down(
        start,
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    visual.simulate_mouse_move(
        end,
        Some(MouseButton::Left),
        gpui::Modifiers::none(),
    );
    let runtime = Arc::clone(&harness.runtime);
    {
        // The release queues the commit without running it, and another
        // client then splits the first pane across the same axis, changing
        // the split's children before the commit reaches the Mux. Tests run
        // workers on this thread, so nothing may run while the lock is held.
        let mut mux = runtime.lock();
        harness.cx.update(|cx| {
            window
                .update(cx, |_, window, cx| {
                    window.dispatch_event(
                        gpui::PlatformInput::MouseUp(MouseUpEvent {
                            button: MouseButton::Left,
                            position: end,
                            modifiers: gpui::Modifiers::none(),
                            click_count: 1,
                        }),
                        cx,
                    );
                })
                .unwrap();
        });
        mux.split_pane(
            harness.workspace,
            tab,
            first,
            SplitDirection::Right,
            &fixture(ECHO),
        )
        .unwrap();
    }
    harness.wait_for("the rejected commit to show the new layout", |cx| {
        let settled = window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    let view = view.read(cx);
                    view.resize_commits.is_empty()
                        && view.divider_drag.is_none()
                })
            })
            .unwrap_or(false);
        settled
            && panes_shown(cx, window, tab)
                .is_some_and(|(panes, _)| panes.len() == 3)
            && root_weights(cx, tab).is_some_and(|weights| weights.len() == 3)
    });
    // The other client's split of the undragged halves stands.
    assert_eq!(
        harness.cx.update(|cx| root_weights(cx, tab)),
        Some(vec![250_000, 250_000, 500_000])
    );
    harness
        .cx
        .update(|cx| assert!(pane_shown(cx, window, tab, second).is_some()));
}

#[gpui::test]
fn a_carried_drag_never_shrinks_a_neighbour_under_the_minimum(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, _, second) = harness.split_window(tab);
    let third = harness.split(tab, second, SplitDirection::Right, ECHO);
    harness.wait_for("the third pane to show", |cx| {
        panes_shown(cx, window, tab).is_some_and(|(panes, _)| panes.len() == 3)
    });
    let (split, children) = harness.cx.update(|cx| {
        match cx
            .global::<Desktop>()
            .hierarchy
            .state()
            .tab(tab)
            .unwrap()
            .layout
            .root()
        {
            huterm_protocol::PaneNode::Split(split) => (
                split.id,
                split
                    .children
                    .iter()
                    .map(|child| child.node.id())
                    .collect::<Vec<_>>(),
            ),
            huterm_protocol::PaneNode::Pane(_) => panic!("not split"),
        }
    });
    let resize = |harness: &mut DesktopHarness, weights: &[u32]| {
        harness
            .runtime
            .lock()
            .resize_split(harness.workspace, tab, split, &children, weights)
            .unwrap();
        let weights = weights.to_vec();
        harness.wait_for("the projection to hold the resize", |cx| {
            root_weights(cx, tab) == Some(weights.clone())
        });
    };
    resize(&mut harness, &[400_000, 300_000, 300_000]);
    // Drag the first boundary as far right as the middle pane allows.
    let start = harness.divider_center(window);
    let end = start + gpui::point(px(400.0), px(0.0));
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_down(
        start,
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    visual.simulate_mouse_move(
        end,
        Some(MouseButton::Left),
        gpui::Modifiers::none(),
    );
    // Another client moves the middle pane's right edge past the dragged
    // boundary, which carried over would leave the middle pane empty.
    resize(&mut harness, &[400_000, 100_000, 500_000]);
    let middle =
        |cx: &mut App| pane_shown(cx, window, tab, second).unwrap().1.columns;
    harness
        .wait_for("the middle pane to keep the minimum while dragged", |cx| {
            middle(cx) >= MIN_PANE_COLUMNS
        });
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_up(end, MouseButton::Left, gpui::Modifiers::none());
    harness.wait_for("the release to commit", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    let view = view.read(cx);
                    view.divider_drag.is_none()
                        && view.resize_commits.is_empty()
                })
            })
            .unwrap_or(false)
    });
    harness.cx.update(|cx| {
        assert!(
            middle(cx) >= MIN_PANE_COLUMNS,
            "{:?}",
            root_weights(cx, tab)
        );
        assert!(pane_shown(cx, window, tab, third).is_some());
    });
}
