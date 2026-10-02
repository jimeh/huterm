use super::model::remove_tab;
use super::*;

#[test]
fn a_cancelled_spawn_publishes_nothing_and_cleans_up_only_before_teardown() {
    assert_eq!(
        spawn_disposition(Resolution::Ready, false),
        SpawnDisposition::Publish
    );
    assert_eq!(
        spawn_disposition(Resolution::Cancelled, false),
        SpawnDisposition::Abandon {
            cleanup: true,
            notify: true
        }
    );
    // Teardown owns every terminal and the application is leaving.
    assert_eq!(
        spawn_disposition(Resolution::Cancelled, true),
        SpawnDisposition::Abandon {
            cleanup: false,
            notify: false
        }
    );
}

#[test]
fn quit_from_a_sibling_waits_for_a_spawn_still_waiting_on_the_projection() {
    let mut pending_spawns = 0;
    let mut quit_pending = false;
    assert!(quit_now(pending_spawns, &mut quit_pending));
    assert!(!quit_pending);
    // A spawn is pending from its start through its sequence wait and
    // publication.
    pending_spawns += 1;
    assert!(!quit_now(pending_spawns, &mut quit_pending));
    assert!(quit_pending);
    assert!(!can_open_window(true, false, quit_pending));
    assert!(settle_spawn(&mut pending_spawns, &mut quit_pending));
    assert_eq!((pending_spawns, quit_pending), (0, false));

    // With two spawns, Quit resumes only after the last settles.
    pending_spawns = 2;
    assert!(!quit_now(pending_spawns, &mut quit_pending));
    assert!(!settle_spawn(&mut pending_spawns, &mut quit_pending));
    assert!(quit_pending);
    assert!(settle_spawn(&mut pending_spawns, &mut quit_pending));
    assert!(!settle_spawn(&mut pending_spawns, &mut quit_pending));
}

#[test]
fn a_stale_cancel_after_confirm_cannot_reach_the_commit() {
    let runtime = DesktopRuntime::default();
    let session = runtime.lock().create_session(None).unwrap();
    let mut close = CloseState::default();
    let target = close.begin_check(CloseTarget::Window);
    close.assessment =
        Some(runtime.assess(CloseRequest::Session(session)).unwrap());
    assert_eq!(
        close.checked(true),
        Some(CloseDecision::Confirm(target.clone()))
    );
    assert!(close.can_cancel_confirmation());
    let (_, confirmed) = close.begin_commit(&target).unwrap();
    assert!(confirmed);
    // A Cancel or Escape drawn before Confirm now finds nothing to cancel.
    assert!(!close.can_cancel_confirmation());
    assert_eq!(close.current, Some(target));
}

#[test]
fn close_commits_retry_only_when_the_runtime_refused_them() {
    assert!(close_commit_retries(&Err(MuxError::StaleClose)));
    assert!(close_commit_retries(&Err(MuxError::ConfirmationRequired)));
    // An accepted close detaches its attachment even when terminal
    // teardown then fails.
    assert!(!close_commit_retries(&Ok(())));
    assert!(!close_commit_retries(&Err(MuxError::Runtime(
        huterm_core::RuntimeError::ShutdownTimedOut
    ))));
}

#[test]
fn the_window_title_names_the_active_tab_before_huterm() {
    assert_eq!(window_title(Some("cargo build")), "cargo build — Huterm");
    assert_eq!(window_title(Some("zsh · exited")), "zsh · exited — Huterm");
    assert_eq!(window_title(None), "Huterm");
    // Unchanged text must not reach `set_window_title` again; the sync
    // compares against the last title it set.
    let mut last = String::new();
    let mut sets = 0;
    for active in [Some("zsh"), Some("zsh"), Some("vim"), None, None] {
        let title = window_title(active);
        if title != last {
            sets += 1;
            last = title;
        }
    }
    assert_eq!(sets, 3);
}

#[test]
fn copy_tab_directory_uses_the_reported_path_local_or_remote() {
    use huterm_protocol::{TerminalDirectory, TerminalMetadata};
    let local = TerminalMetadata::new(
        Some(TerminalDirectory::new(None, "/home/jim/src".into(), true)),
        None,
    );
    assert_eq!(tab_directory_path(&local).as_deref(), Some("/home/jim/src"));
    let remote = TerminalMetadata::new(
        Some(TerminalDirectory::new(
            Some("build-host".into()),
            "/srv/build".into(),
            false,
        )),
        Some("ssh".into()),
    );
    assert_eq!(tab_directory_path(&remote).as_deref(), Some("/srv/build"));
    assert_eq!(tab_directory_path(&TerminalMetadata::default()), None);
    let blank = TerminalMetadata::new(
        Some(TerminalDirectory::new(None, String::new(), true)),
        None,
    );
    assert_eq!(tab_directory_path(&blank), None, "a blank path is unknown");
}

#[cfg(not(all(target_os = "macos", feature = "macos-updater")))]
#[test]
fn check_for_updates_reports_updater_build_requirement() {
    assert_eq!(
        unsupported_update_error(),
        CommandError::Unavailable(
            "self-updates are available only in updater-enabled macOS release builds"
                .to_owned()
        )
    );
}

#[test]
fn interactive_invocation_without_required_arguments_opens_the_palette() {
    let select_tab = CommandInvocation::new(ids::SELECT_TAB, Vec::new());
    assert!(interactive_spec(&select_tab).unwrap().1);

    let toggle_quake = CommandInvocation::new(ids::TOGGLE_QUAKE, Vec::new());
    assert!(!interactive_spec(&toggle_quake).unwrap().1);
}

#[test]
fn programmatic_invocation_without_required_arguments_fails() {
    let invocation = CommandInvocation::new(ids::SELECT_TAB, Vec::new());
    assert_eq!(
        validate(&invocation),
        Err(CommandError::MissingArgument {
            command: ids::SELECT_TAB,
            name: "tab",
        })
    );
}

fn messages(stack: &NoticeStack) -> Vec<String> {
    stack
        .contents()
        .map(|content| content.message.clone())
        .collect()
}

#[test]
fn startup_diagnostics_become_separate_notices_that_reloads_replace() {
    let path = Path::new("/home/me/.config/huterm/huterm.toml");
    let diagnostics = config_diagnostics(
        path,
        Some("config error"),
        Some("keymap error"),
        &["binding conflict".to_owned()],
        Some(config::LEGACY_ALACRITTY_WARNING),
    );
    let sources: Vec<_> = diagnostics
        .iter()
        .map(|content| (content.severity, content.source.clone()))
        .collect();
    assert_eq!(
        sources,
        [
            (Severity::Error, NoticeSource::Config),
            (Severity::Error, NoticeSource::Keymap),
            (Severity::Warning, NoticeSource::Keymap),
            (Severity::Warning, NoticeSource::Config),
        ]
    );
    assert!(
        diagnostics
            .iter()
            .all(|content| content.lifetime == Lifetime::Persistent
                && content.location.as_deref() == Some(path.to_str().unwrap()))
    );
    let actions = |index: usize| {
        diagnostics[index]
            .actions
            .iter()
            .map(|action| (action.label.as_str(), action.command.id))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        actions(0),
        [
            ("Open Settings", ids::OPEN_SETTINGS),
            ("Reload", ids::RELOAD_CONFIG)
        ]
    );
    assert_eq!(actions(3), [("Open Settings", ids::OPEN_SETTINGS)]);

    let now = Instant::now();
    let mut stack = NoticeStack::default();
    stack.push(
        NoticeContent::command_failure("Close", "Close failed: busy"),
        now,
    );
    assert!(stack.replace_diagnostics(&diagnostics, now));
    assert_eq!(
        messages(&stack),
        [
            "config error",
            "keymap error",
            "binding conflict",
            config::LEGACY_ALACRITTY_WARNING,
            "Close failed: busy",
        ]
    );
    // Dismissing a diagnostic hides it until the next reload.
    let newest = stack.newest().unwrap();
    assert!(stack.dismiss(newest));
    assert_eq!(messages(&stack)[0], "keymap error");
    // A reload that still fails raises every diagnostic again.
    assert!(stack.replace_diagnostics(&diagnostics, now));
    assert_eq!(messages(&stack)[0], "config error");
    assert_eq!(stack.contents().len(), 5);
    // A fixed file clears them but leaves the command notice alone.
    let fixed = config_diagnostics(path, None, None, &[], None);
    assert!(fixed.is_empty());
    assert!(stack.replace_diagnostics(&fixed, now));
    assert_eq!(messages(&stack), ["Close failed: busy"]);
}

#[test]
fn a_failed_reload_keeps_the_active_configs_own_diagnostics() {
    let path = Path::new("/tmp/huterm.toml");
    let active = config_diagnostics(
        path,
        None,
        None,
        &["binding conflict".to_owned()],
        Some(config::LEGACY_ALACRITTY_WARNING),
    );
    let failed = failed_reload_diagnostics("bad toml", &active);
    assert_eq!(
        failed
            .iter()
            .map(|content| content.message.as_str())
            .collect::<Vec<_>>(),
        [
            "Config reload failed: bad toml",
            "binding conflict",
            config::LEGACY_ALACRITTY_WARNING
        ]
    );
    assert_eq!(failed[0].source, NoticeSource::Config);
    assert_eq!(failed[0].severity, Severity::Error);
    assert_eq!(failed[0].actions.len(), 2);

    let now = Instant::now();
    let mut stack = NoticeStack::default();
    stack.replace_diagnostics(&active, now);
    stack.replace_diagnostics(&failed, now);
    assert_eq!(
        messages(&stack),
        [
            "Config reload failed: bad toml",
            "binding conflict",
            config::LEGACY_ALACRITTY_WARNING,
        ],
        "the keymap conflict from the active config stays"
    );
}

#[test]
fn terminal_failures_are_routed_with_their_tab() {
    let tab = TabId::new(7);
    let notice = terminal_notice(
        tab,
        "~/project",
        TerminalFailure {
            severity: Severity::Warning,
            title: "Input rejected",
            message: "Input buffer full".to_owned(),
        },
    );
    assert_eq!(
        notice.source,
        NoticeSource::Terminal {
            tab,
            title: "~/project".to_owned()
        }
    );
    assert_eq!(notice.severity, Severity::Warning);
    assert_eq!(notice.title, "Input rejected");
    assert_eq!(notice.location.as_deref(), Some("~/project"));
    assert_eq!(notice.lifetime, Lifetime::Expiring);
    assert_eq!(
        notice.smoke_line(),
        "warning|terminal:~/project|Input buffer full"
    );
    let now = Instant::now();
    let mut stack = NoticeStack::default();
    stack.push(notice.clone(), now);
    stack.push(
        terminal_notice(
            TabId::new(8),
            "other",
            TerminalFailure {
                severity: Severity::Error,
                title: "Terminal error",
                message: "runtime stopped".to_owned(),
            },
        ),
        now,
    );
    assert!(stack.replace_source(&notice.source, Vec::new(), now));
    assert_eq!(messages(&stack), ["runtime stopped"], "keyed by tab");
}

#[test]
fn copy_is_unavailable_without_selection() {
    assert_eq!(
        copy_availability(None),
        Err(CommandError::Unavailable("no selection".to_owned()))
    );
    assert_eq!(
        copy_availability(Some(Selection {
            generation: 1,
            anchor: BufferPoint {
                rows_from_live_bottom: 0,
                column: 0,
            },
            head: BufferPoint {
                rows_from_live_bottom: 0,
                column: 1,
            },
        })),
        Ok(())
    );
}

#[test]
fn hidden_and_every_overlay_frame_preserve_full_terminal_bounds() {
    for position in [
        TabPosition::Top,
        TabPosition::Bottom,
        TabPosition::Left,
        TabPosition::Right,
    ] {
        for viewport in [size(px(800.0), px(600.0)), size(px(3.0), px(2.0))] {
            let safe = gpui::Edges {
                top: px(40.0),
                left: px(5.0),
                right: px(7.0),
                bottom: px(3.0),
            };
            let hidden = ChromeLayout::with_safe_area(
                viewport,
                px(0.0),
                position,
                px(180.0),
                safe,
            )
            .present(Presentation::Hidden, position, 0.0);
            let expected = if viewport.width == px(800.0) {
                Bounds::new(
                    point(px(5.0), px(40.0)),
                    size(px(788.0), px(557.0)),
                )
            } else {
                Bounds::new(point(px(3.0), px(2.0)), size(px(0.0), px(0.0)))
            };
            assert_eq!(hidden.terminal, expected);
            for progress in [0.0, 0.1, 0.5, 0.9, 1.0] {
                let overlay = ChromeLayout::with_safe_area(
                    viewport,
                    px(0.0),
                    position,
                    px(180.0),
                    safe,
                )
                .present(
                    Presentation::Overlay,
                    position,
                    progress,
                );
                assert_eq!(
                    overlay.terminal, expected,
                    "{position:?} at {progress}"
                );
            }
        }
    }
}

#[test]
fn exit_queue_retains_inactive_siblings_while_busy_and_cancel_consumes_only_current()
 {
    let first = TabId::new(1);
    let second = TabId::new(2);
    let mut queue = ExitQueue::default();
    let mut close = CloseState::default();
    close.begin_check(CloseTarget::Tab(TabId::new(3)));
    queue.observe(first, false, true, true);
    queue.observe(second, false, true, true);
    // Busy structural work leaves both requests queued, independent of active tab.
    assert_eq!(close.next_request(&mut queue, true, |_| true), None);
    close.cancel();
    let target = close.next_request(&mut queue, false, |_| true).unwrap();
    assert_eq!(target, CloseTarget::Tab(first));
    close.begin_check(target.clone());
    assert_eq!(
        close.checked(true),
        Some(CloseDecision::Confirm(target.clone()))
    );
    assert_eq!(close.next_request(&mut queue, false, |_| true), None);
    assert_eq!(close.cancel(), Some(target));
    queue.observe(first, true, true, true);
    queue.observe(second, true, true, true);
    assert_eq!(queue.take_next(|_| true), Some(second));
    assert_eq!(queue.take_next(|_| true), None);
}

#[test]
fn exit_queue_reload_affects_future_transitions_and_discards_removed_tabs() {
    let first = TabId::new(1);
    let second = TabId::new(2);
    let third = TabId::new(3);
    let mut queue = ExitQueue::default();
    queue.observe(first, false, true, false);
    queue.observe(first, true, true, true);
    queue.observe(second, false, true, true);
    queue.observe(third, false, true, true);
    assert_eq!(queue.take_next(|id| id != second), Some(third));
    assert_eq!(queue.take_next(|_| true), None);
}

#[test]
fn reorder_projection_and_preview_stay_in_bar_for_all_four_placements() {
    for position in [
        TabPosition::Top,
        TabPosition::Bottom,
        TabPosition::Left,
        TabPosition::Right,
    ] {
        let layout =
            ChromeLayout::new(size(px(600.0), px(240.0)), px(32.0), position);
        let strip = TabStrip::new(
            layout.tabs,
            position.vertical(),
            TabExtents::Uniform(8),
            px(0.0),
            false,
        );
        let extent = strip.tab_extent(0);
        let pointer = if strip.vertical {
            strip.bounds.origin + point(px(20.0), extent * 1.1)
        } else {
            strip.bounds.origin + point(extent * 1.1, px(10.0))
        };
        assert_eq!(strip.slot(pointer), 1, "{position:?}");
        for excursion in [-10_000.0, 10_000.0] {
            let perpendicular = pointer
                + if strip.vertical {
                    point(px(excursion), px(0.0))
                } else {
                    point(px(0.0), px(excursion))
                };
            assert_eq!(strip.slot(perpendicular), 1, "{position:?}");
            let beyond = pointer
                + if strip.vertical {
                    point(px(0.0), px(excursion))
                } else {
                    point(px(excursion), px(0.0))
                };
            let slot = strip.slot(beyond);
            assert_eq!(
                slot,
                if excursion < 0.0 {
                    0
                } else {
                    strip.slot(
                        strip.bounds.origin
                            + point(
                                strip.bounds.size.width,
                                strip.bounds.size.height,
                            ),
                    )
                }
            );
            for bounds in [
                strip.preview(beyond, 0),
                strip.preview(perpendicular, 0),
                strip.marker(slot),
            ] {
                assert!(
                    bounds.origin.x >= strip.bounds.origin.x
                        && bounds.origin.y >= strip.bounds.origin.y
                );
                assert!(
                    bounds.origin.x + bounds.size.width
                        <= strip.bounds.origin.x + strip.bounds.size.width
                );
                assert!(
                    bounds.origin.y + bounds.size.height
                        <= strip.bounds.origin.y + strip.bounds.size.height
                );
            }
        }
    }
}

#[test]
fn safe_area_keeps_tabs_and_terminal_below_notch_for_every_placement() {
    for position in [
        TabPosition::Top,
        TabPosition::Bottom,
        TabPosition::Left,
        TabPosition::Right,
    ] {
        let layout = ChromeLayout::with_safe_area(
            size(px(800.0), px(600.0)),
            px(0.0),
            position,
            SIDEBAR_WIDTH,
            gpui::Edges {
                top: px(48.5),
                ..Default::default()
            },
        );
        assert!(
            layout.terminal.origin.y >= px(48.5),
            "{position:?}: {:?}",
            layout.terminal
        );
        if position.vertical() {
            // The column spans the safe area; its rows do not.
            assert_eq!(layout.tabs.origin.y, px(0.0));
            let strip = layout.strip_bounds(huterm_config::TabsConfig {
                position,
                ..Default::default()
            });
            assert_eq!(strip.origin.y, px(48.5));
            assert_eq!(strip.bottom(), layout.tabs.bottom());
        } else {
            assert!(layout.tabs.origin.y >= px(48.5));
        }
        for bounds in [layout.tabs, layout.terminal] {
            assert!(bounds.bottom() <= px(600.0));
        }
        let column_extra = if position.vertical() {
            f32::from(layout.tabs.size.width) * 48.5
        } else {
            0.0
        };
        let area = f32::from(layout.tabs.size.width)
            * f32::from(layout.tabs.size.height)
            + f32::from(layout.terminal.size.width)
                * f32::from(layout.terminal.size.height);
        assert!((area - column_extra - 800.0 * 551.5).abs() < 0.01);
    }
}

#[test]
fn safe_area_bounds_include_side_insets_and_clamp_small_windows() {
    let insets = gpui::Edges {
        top: px(48.5),
        right: px(6.0),
        bottom: px(8.0),
        left: px(4.0),
    };
    for position in [
        TabPosition::Top,
        TabPosition::Bottom,
        TabPosition::Left,
        TabPosition::Right,
    ] {
        for viewport in [size(px(800.0), px(600.0)), size(px(3.0), px(2.0))] {
            let layout = ChromeLayout::with_safe_area(
                viewport,
                px(0.0),
                position,
                SIDEBAR_WIDTH,
                insets,
            );
            for bounds in [layout.tabs, layout.terminal] {
                assert!(bounds.origin.x >= px(4.0).min(viewport.width));
                let floor = if position.vertical() && bounds == layout.tabs {
                    px(0.0)
                } else {
                    px(48.5).min(viewport.height)
                };
                assert!(bounds.origin.y >= floor);
                assert!(
                    bounds.size.width >= px(0.0)
                        && bounds.size.height >= px(0.0)
                );
                assert!(
                    bounds.right()
                        <= (viewport.width - px(6.0)).max(bounds.origin.x)
                );
                assert!(
                    bounds.bottom()
                        <= (viewport.height - px(8.0)).max(bounds.origin.y)
                );
            }
        }
    }
}

#[test]
fn sidebar_resize_handle_never_overlaps_terminal_input() {
    for position in [TabPosition::Left, TabPosition::Right] {
        for (width, preferred) in
            [(1000.0, 180.0), (300.0, 400.0), (3.0, 140.0)]
        {
            let layout = ChromeLayout::with_sidebar(
                size(px(width), px(600.0)),
                px(32.0),
                position,
                px(preferred),
            );
            let handle = layout.sidebar_resize_handle(position);
            let handle_end = handle.origin.x + handle.size.width;
            let terminal_end =
                layout.terminal.origin.x + layout.terminal.size.width;
            assert!(
                handle_end <= layout.terminal.origin.x
                    || handle.origin.x >= terminal_end,
                "resize handle overlaps terminal: {position:?}, width {width}",
            );
            assert!(handle.origin.x >= layout.tabs.origin.x);
            assert!(
                handle_end <= layout.tabs.origin.x + layout.tabs.size.width
            );
            assert_eq!(handle.origin.y, layout.tabs.origin.y);
            assert_eq!(handle.size.height, layout.tabs.size.height);
            assert_eq!(
                handle.size.width,
                px(SIDEBAR_HANDLE_WIDTH).min(layout.tabs.size.width)
            );
        }
    }
}

#[test]
fn sidebar_width_clamps_to_window_without_losing_preference() {
    for position in [TabPosition::Left, TabPosition::Right] {
        let preferred = px(350.0);
        let wide = ChromeLayout::with_sidebar(
            size(px(1000.0), px(600.0)),
            px(32.0),
            position,
            preferred,
        );
        let narrow = ChromeLayout::with_sidebar(
            size(px(300.0), px(600.0)),
            px(32.0),
            position,
            preferred,
        );
        let restored = ChromeLayout::with_sidebar(
            size(px(1000.0), px(600.0)),
            px(32.0),
            position,
            preferred,
        );
        assert_eq!(wide.tabs.size.width, px(350.0));
        assert_eq!(narrow.tabs.size.width, px(150.0));
        assert_eq!(restored.tabs.size.width, px(350.0));
        let minimum = ChromeLayout::with_sidebar(
            size(px(1000.0), px(600.0)),
            px(32.0),
            position,
            px(20.0),
        );
        let maximum = ChromeLayout::with_sidebar(
            size(px(1000.0), px(600.0)),
            px(32.0),
            position,
            px(900.0),
        );
        assert_eq!(minimum.tabs.size.width, px(140.0));
        assert_eq!(maximum.tabs.size.width, px(400.0));
        assert_eq!(wide.terminal.size.width + wide.tabs.size.width, px(1000.0));
    }
}

#[test]
fn application_mouse_coordinates_follow_all_tab_placements() {
    use crate::config::TabPosition;
    let cell = size(px(8.0), px(16.0));
    for inset in [px(0.0), px(48.5)] {
        for placement in [
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Right,
        ] {
            let chrome = ChromeLayout::with_safe_area(
                size(px(800.0), px(600.0)),
                px(32.0),
                placement,
                SIDEBAR_WIDTH,
                gpui::Edges {
                    top: inset,
                    ..Default::default()
                },
            );
            let layout = TerminalLayout::new(
                chrome.terminal.size,
                cell,
                WindowConfig::default(),
            );
            let start = chrome.terminal.origin + layout.bounds.origin;
            assert_eq!(
                application_mouse_geometry(
                    start,
                    chrome.terminal.origin,
                    layout,
                    cell
                ),
                (true, MousePosition::default()),
                "{placement:?}"
            );
            assert_eq!(
                application_mouse_geometry(
                    start + point(px(8.0), px(16.0)),
                    chrome.terminal.origin,
                    layout,
                    cell
                ),
                (true, MousePosition { column: 1, row: 1 }),
                "{placement:?}"
            );
            let tab_center = chrome.tabs.origin
                + point(
                    chrome.tabs.size.width / 2.0,
                    chrome.tabs.size.height / 2.0,
                );
            assert!(
                !application_mouse_geometry(
                    tab_center,
                    chrome.terminal.origin,
                    layout,
                    cell
                )
                .0,
                "{placement:?}"
            );
        }
    }
}

fn lifecycle_command() -> TerminalCommand {
    TerminalCommand {
        program: "/bin/sh".into(),
        arguments: vec!["-c".into(), "printf READY; read value".into()],
        working_directory: std::env::current_dir().unwrap(),
        environment: Vec::new(),
        grid_size: GridSize::clamped(40, 8),
        cell_size: CellSize {
            width: 8,
            height: 16,
        },
        presentation: huterm_protocol::TerminalPresentation::default(),
    }
}

#[test]
fn attachment_allocation_failure_rolls_back_published_pty() {
    let runtime = DesktopRuntime::default();
    runtime.mux.lock().unwrap().reserve_through(u64::MAX - 5);
    assert!(matches!(
        runtime
            .open_tab(None, None, &lifecycle_command(), true)
            .result,
        Err(MuxError::IdExhausted)
    ));
    let mux = runtime.mux.lock().unwrap();
    assert!(mux.sessions().is_empty());
    assert_eq!(mux.terminal_count(), 0);
}

#[test]
fn open_tab_returns_registered_host_effect_recipient() {
    let runtime = DesktopRuntime::default();
    let mut command = lifecycle_command();
    command.arguments = vec![
        "-c".into(),
        "read value; printf '\\033]52;c;ZGVza3RvcABjbGlwYm9hcmQ=\\007'; read value"
            .into(),
    ];
    let (session, _, _, _, viewer) =
        runtime.open_tab(None, None, &command, true).result.unwrap();
    let recipient = viewer.host_effects().unwrap();

    viewer
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let pending = loop {
        if let Some(pending) = recipient.try_next() {
            break pending;
        }
        assert!(Instant::now() < deadline, "clipboard effect not delivered");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(recipient.is_current(&pending));
    let HostEffect::ClipboardWrite(write) = pending.effect() else {
        panic!("unexpected host effect");
    };
    assert_eq!(write.text(), "desktop\0clipboard");

    drop(pending);
    runtime.mux.lock().unwrap().close_session(session).unwrap();
}

#[test]
fn orphaned_publication_preserves_another_attachment_and_transferred_tab() {
    let runtime = DesktopRuntime::default();
    let (session, workspace, opened, attachment, _authority) = runtime
        .open_tab(None, None, &lifecycle_command(), true)
        .result
        .unwrap();
    let second = runtime.mux.lock().unwrap().attach_session(session).unwrap();
    runtime.cleanup_spawn(session, workspace, opened.tab.id, attachment);
    assert_eq!(
        runtime
            .mux
            .lock()
            .unwrap()
            .attachment_session(second)
            .unwrap(),
        session
    );
    assert!(runtime.mux.lock().unwrap().tab(opened.tab.id).is_some());
    let (source, source_workspace, transferred, initial, _authority) = runtime
        .open_tab(None, None, &lifecycle_command(), true)
        .result
        .unwrap();
    runtime
        .mux
        .lock()
        .unwrap()
        .move_tab(source_workspace, transferred.tab.id, workspace, None)
        .unwrap();
    runtime.cleanup_spawn(
        source,
        source_workspace,
        transferred.tab.id,
        initial,
    );
    let mux = runtime.mux.lock().unwrap();
    assert!(mux.session(source).is_none());
    assert_eq!(mux.select_tab(transferred.tab.id).unwrap().session, session);
    drop(mux);
    runtime.terminate().unwrap();
}

#[test]
fn orphaned_publication_never_terminates_a_retargeted_destination() {
    let runtime = DesktopRuntime::default();
    let (source, workspace, opened, attachment, _authority) = runtime
        .open_tab(None, None, &lifecycle_command(), true)
        .result
        .unwrap();
    let destination = runtime
        .mux
        .lock()
        .unwrap()
        .create_session(Some("survivor"))
        .unwrap();
    runtime
        .mux
        .lock()
        .unwrap()
        .retarget_attachment(attachment.unwrap(), destination)
        .unwrap();
    runtime.cleanup_spawn(source, workspace, opened.tab.id, attachment);
    let mux = runtime.mux.lock().unwrap();
    assert!(mux.session(source).is_none());
    assert!(mux.session(destination).is_some());
    assert!(mux.capture_hierarchy().attachments.is_empty());
}

#[test]
fn quit_captures_zero_view_hierarchy_and_window_navigation_once() {
    let runtime = DesktopRuntime::default();
    let mut mux = runtime.mux.lock().unwrap();
    let visible = mux.create_session(Some("visible")).unwrap();
    let workspace = mux.create_workspace(visible, None).unwrap();
    let attachment = mux.attach_session(visible).unwrap();
    let unviewed = mux.create_session(Some("unviewed")).unwrap();
    mux.create_workspace(unviewed, Some("retained")).unwrap();
    drop(mux);
    let bounds = WindowBounds::Windowed(Bounds {
        origin: point(px(5.0), px(10.0)),
        size: size(px(800.0), px(600.0)),
    });
    let windows = vec![WindowRestore {
        attachment,
        workspace: Some(workspace),
        active: None,
        bounds,
        tab_position: TabPosition::Left,
        sidebar_width: px(220.0),
        quake_profile: None,
    }];
    let assessment = runtime.assess(CloseRequest::Application).unwrap();
    runtime
        .commit(&assessment, false, Some(windows))
        .result
        .unwrap();
    runtime.terminate().unwrap();
    let restore = runtime.restore.lock().unwrap();
    let restore = restore.as_ref().unwrap();
    assert_eq!(
        restore
            .hierarchy
            .sessions
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>(),
        vec![visible, unviewed]
    );
    assert_eq!(restore.hierarchy.workspaces.len(), 2);
    assert_eq!(restore.windows.len(), 1);
    assert_eq!(restore.windows[0].workspace, Some(workspace));
    assert_eq!(restore.windows[0].sidebar_width, px(220.0));
    assert!(runtime.mux.lock().unwrap().sessions().is_empty());
}

#[test]
fn rejected_quit_never_sets_termination_gate_or_capture() {
    let runtime = DesktopRuntime::default();
    runtime.mux.lock().unwrap().create_session(None).unwrap();
    let assessment = runtime.assess(CloseRequest::Application).unwrap();
    runtime
        .mux
        .lock()
        .unwrap()
        .create_session(Some("new survivor"))
        .unwrap();
    assert!(matches!(
        runtime.commit(&assessment, false, Some(Vec::new())).result,
        Err(MuxError::StaleClose)
    ));
    assert!(!runtime.terminating.load(Ordering::Acquire));
    assert!(runtime.restore.lock().unwrap().is_none());
    assert_eq!(runtime.mux.lock().unwrap().sessions().len(), 2);
    let assessment = runtime.assess(CloseRequest::Application).unwrap();
    runtime
        .commit(&assessment, false, Some(Vec::new()))
        .result
        .unwrap();
    assert!(runtime.terminating.load(Ordering::Acquire));
    assert_eq!(
        runtime
            .restore
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .hierarchy
            .sessions
            .len(),
        2
    );
    assert!(runtime.mux.lock().unwrap().sessions().is_empty());
}

#[test]
fn cancellation_invalidates_assessment_generation_and_application_intent() {
    let mut state = CloseState::default();
    state.begin_check(CloseTarget::Application);
    let generation = state.generation;
    assert_eq!(state.cancel(), Some(CloseTarget::Application));
    assert_ne!(state.generation, generation);
    assert_eq!(state.checked(true), None);
}

#[test]
fn private_session_spawn_failure_rolls_back_and_cleanup_keeps_siblings() {
    let runtime = DesktopRuntime::default();
    let mut command = TerminalCommand {
        program: "/huterm-nonexistent-shell".into(),
        arguments: vec!["-c".into(), "printf READY; read value".into()],
        working_directory: std::env::current_dir().unwrap(),
        environment: Vec::new(),
        grid_size: GridSize::clamped(40, 8),
        cell_size: CellSize {
            width: 8,
            height: 16,
        },
        presentation: huterm_protocol::TerminalPresentation::default(),
    };
    assert!(runtime.open_tab(None, None, &command, true).result.is_err());
    assert!(runtime.mux.lock().unwrap().sessions().is_empty());
    assert_eq!(runtime.mux.lock().unwrap().terminal_count(), 0);
    command.program = "/bin/sh".into();
    let (session, workspace, first, attachment, first_viewer) =
        runtime.open_tab(None, None, &command, true).result.unwrap();
    let (sibling, _, _, _, second_viewer) =
        runtime.open_tab(None, None, &command, true).result.unwrap();
    assert_ne!(session, sibling);
    command.program = "/huterm-nonexistent-shell".into();
    assert!(runtime.open_tab(None, None, &command, true).result.is_err());
    assert!(
        runtime
            .open_tab(Some(workspace), attachment, &command, true)
            .result
            .is_err()
    );
    assert_eq!(runtime.mux.lock().unwrap().sessions().len(), 2);
    assert_eq!(
        runtime
            .mux
            .lock()
            .unwrap()
            .workspace(workspace)
            .unwrap()
            .tabs,
        [first.tab]
    );
    runtime.mux.lock().unwrap().close_session(session).unwrap();
    assert!(runtime.mux.lock().unwrap().workspace(workspace).is_none());
    assert_eq!(runtime.mux.lock().unwrap().sessions().len(), 1);
    // Closing the session revoked its viewer and stopped its terminal.
    assert!(matches!(
        first_viewer.read_snapshot(),
        Err(RuntimeError::Revoked)
    ));
    assert!(second_viewer.read_snapshot().is_ok());
    runtime.mux.lock().unwrap().close_session(sibling).unwrap();
    assert!(runtime.mux.lock().unwrap().sessions().is_empty());
    assert_eq!(runtime.mux.lock().unwrap().terminal_count(), 0);
    runtime.mux.lock().unwrap().reserve_through(u64::MAX - 1);
    assert!(matches!(
        runtime.open_tab(None, None, &command, true).result,
        Err(MuxError::IdExhausted)
    ));
    assert!(runtime.mux.lock().unwrap().sessions().is_empty());
}

#[test]
fn native_termination_drains_existing_terminals_and_rejects_queued_spawns() {
    let runtime = Arc::new(DesktopRuntime::default());
    let command = TerminalCommand {
        program: "/bin/sh".into(),
        arguments: vec!["-c".into(), "printf READY; read value".into()],
        working_directory: std::env::current_dir().unwrap(),
        environment: Vec::new(),
        grid_size: GridSize::clamped(40, 8),
        cell_size: CellSize {
            width: 8,
            height: 16,
        },
        presentation: huterm_protocol::TerminalPresentation::default(),
    };
    let (_, _, _, _, viewer) =
        runtime.open_tab(None, None, &command, true).result.unwrap();
    // Hold the structural lock as an already-running spawn would, then
    // queue another spawn and invoke the exact native-hook cleanup method.
    let guard = runtime.mux.lock().unwrap();
    let spawn_runtime = Arc::clone(&runtime);
    let spawn = std::thread::spawn(move || {
        spawn_runtime.open_tab(None, None, &command, true).result
    });
    let quit_runtime = Arc::clone(&runtime);
    let (finished, completion) = std::sync::mpsc::channel();
    let quit = std::thread::spawn(move || {
        quit_runtime.terminate().unwrap();
        finished.send(()).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    while !runtime.terminating.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline, "quit did not start");
        std::thread::yield_now();
    }
    assert!(
        completion.try_recv().is_err(),
        "quit must await the structural owner"
    );
    drop(guard);
    assert!(matches!(
        spawn.join().unwrap(),
        Err(MuxError::Runtime(RuntimeError::Stopped))
    ));
    quit.join().unwrap();
    assert_eq!(runtime.mux.lock().unwrap().terminal_count(), 0);
    assert!(matches!(viewer.read_snapshot(), Err(RuntimeError::Stopped)));
    runtime.terminate().unwrap();
}

#[test]
fn queued_quit_after_final_window_close_admits_a_confirmation_host() {
    let runtime = DesktopRuntime::default();
    let mut mux = runtime.mux.lock().unwrap();
    let visible = mux.create_session(None).unwrap();
    let attachment = mux.attach_session(visible).unwrap();
    let unviewed = mux.create_session(Some("keep alive")).unwrap();
    drop(mux);
    let assessment = runtime.assess(CloseRequest::Window(attachment)).unwrap();
    let mut close = CloseState::default();
    close.begin_check(CloseTarget::Window);
    close.current = Some(CloseTarget::Window);
    close.queue(CloseTarget::Application);
    runtime.commit(&assessment, false, None).result.unwrap();
    assert_eq!(
        close.take_pending(|_| false),
        Some(CloseTarget::Application)
    );
    assert_eq!(
        runtime
            .mux
            .lock()
            .unwrap()
            .sessions()
            .iter()
            .map(|session| session.id)
            .collect::<Vec<_>>(),
        vec![unviewed]
    );
    // Production retains quitting=true while removing the final window.
    assert!(
        can_open_window(false, true, false),
        "ongoing Quit lost its confirmation host"
    );
    assert!(
        !can_open_window(true, true, false),
        "Quit admitted a new shell"
    );
}

#[test]
fn widening_a_tab_check_rechecks_all_targets_before_confirmation() {
    let mut close = CloseState::default();
    close.begin_check(CloseTarget::Tab(TabId::new(1)));
    close.queue(CloseTarget::Application);
    assert_eq!(
        close.checked(false),
        Some(CloseDecision::Check(CloseTarget::Application))
    );
    close.begin_check(CloseTarget::Application);
    assert_eq!(
        close.checked(true),
        Some(CloseDecision::Confirm(CloseTarget::Application))
    );
    assert_eq!(close.cancel(), Some(CloseTarget::Application));
}

#[test]
fn close_check_completion_preserves_application_and_window_scope() {
    for (current, later) in [
        (CloseTarget::Application, CloseTarget::Window),
        (CloseTarget::Application, CloseTarget::Tab(TabId::new(1))),
        (CloseTarget::Window, CloseTarget::Tab(TabId::new(1))),
    ] {
        let mut close = CloseState::default();
        close.begin_check(current.clone());
        close.queue(later);
        assert_eq!(
            close.checked(true),
            Some(CloseDecision::Confirm(current.clone()))
        );
        assert_eq!(
            close.cancel(),
            Some(current),
            "cancel must retain application scope so it clears quitting"
        );
        assert!(close.pending.is_none());
    }
}

#[test]
fn tab_removal_precedes_queued_window_confirmation_and_cancel() {
    let (first, second) = (TabId::new(1), TabId::new(2));
    let mut tabs = vec![first, second];
    let mut active = Some(first);
    let mut close = CloseState::default();
    close.begin_check(CloseTarget::Tab(first));
    assert_eq!(
        close.checked(false),
        Some(CloseDecision::Close(CloseTarget::Tab(first)))
    );
    close.queue(CloseTarget::Window);
    remove_tab(&mut tabs, &mut active, first, |id| *id);
    let next = close.take_pending(|id| tabs.contains(&id)).unwrap();
    close.begin_check(next);
    assert_eq!(
        close.checked(true),
        Some(CloseDecision::Confirm(CloseTarget::Window))
    );
    close.cancel();
    assert_eq!(active, Some(second));
    assert!(tabs.contains(&active.unwrap()));
}

#[test]
fn repeated_tab_close_is_discarded_after_removal() {
    let (first, second) = (TabId::new(1), TabId::new(2));
    let mut tabs = vec![first, second];
    let mut active = Some(first);
    let mut close = CloseState::default();
    close.queue(CloseTarget::Tab(first));
    remove_tab(&mut tabs, &mut active, first, |id| *id);
    assert_eq!(close.take_pending(|id| tabs.contains(&id)), None);
    assert_eq!(active, Some(second));
    remove_tab(&mut tabs, &mut active, first, |id| *id);
    assert_eq!(tabs, vec![second]);
    assert_eq!(active, Some(second));
}

#[test]
fn queued_close_preserves_the_widest_requested_scope() {
    let tab = CloseTarget::Tab(TabId::new(1));
    assert_eq!(merge_close(None, tab.clone()), tab);
    assert_eq!(
        merge_close(Some(tab.clone()), CloseTarget::Window),
        CloseTarget::Window
    );
    assert_eq!(
        merge_close(Some(CloseTarget::Window), tab.clone()),
        CloseTarget::Window
    );
    assert_eq!(
        merge_close(Some(CloseTarget::Application), CloseTarget::Window),
        CloseTarget::Application
    );
    assert_eq!(
        merge_close(Some(tab), CloseTarget::Application),
        CloseTarget::Application
    );
}

#[test]
fn tab_sets_union_with_tab_targets_and_lose_to_wider_scopes() {
    let (first, second, third) = (TabId::new(1), TabId::new(2), TabId::new(3));
    let set = CloseTarget::Tabs(vec![first, second]);
    // A single tab still replaces a single tab.
    assert_eq!(
        merge_close(Some(CloseTarget::Tab(first)), CloseTarget::Tab(second)),
        CloseTarget::Tab(second)
    );
    // Unions keep first-occurrence order; `request_close` restores
    // window order before assessing.
    assert_eq!(
        merge_close(Some(CloseTarget::Tab(third)), set.clone()),
        CloseTarget::Tabs(vec![third, first, second])
    );
    assert_eq!(
        merge_close(Some(set.clone()), CloseTarget::Tab(third)),
        CloseTarget::Tabs(vec![first, second, third])
    );
    assert_eq!(
        merge_close(Some(set.clone()), CloseTarget::Tab(second)),
        set
    );
    assert_eq!(
        merge_close(Some(set.clone()), CloseTarget::Tabs(vec![second, third])),
        CloseTarget::Tabs(vec![first, second, third])
    );
    assert_eq!(
        merge_close(Some(set.clone()), CloseTarget::Window),
        CloseTarget::Window
    );
    assert_eq!(
        merge_close(Some(CloseTarget::Application), set.clone()),
        CloseTarget::Application
    );
    // A queued set drops removed tabs and collapses to a single tab.
    let mut close = CloseState::default();
    close.queue(set);
    assert_eq!(
        close.take_pending(|id| id == second),
        Some(CloseTarget::Tab(second))
    );
    close.queue(CloseTarget::Tabs(vec![first, second]));
    assert_eq!(close.take_pending(|_| false), None);
}

#[test]
fn tab_closes_are_refused_while_a_confirmation_is_pending() {
    let mut close = CloseState::default();
    assert_eq!(close.check_tab_close_available(), Ok(()));
    close.begin_check(CloseTarget::Tab(TabId::new(1)));
    assert_eq!(
        close.checked(true),
        Some(CloseDecision::Confirm(CloseTarget::Tab(TabId::new(1))))
    );
    assert_eq!(close.dialog_focus, DialogFocus::Primary);
    assert_eq!(
        close.check_tab_close_available(),
        Err(CommandError::Unavailable(
            "close confirmation pending".to_owned()
        ))
    );
    close.cancel();
    assert_eq!(close.check_tab_close_available(), Ok(()));
}

#[test]
fn tab_closes_are_refused_while_the_about_panel_shows() {
    let close = CloseState::default();
    assert_eq!(tab_close_availability(&close, false), Ok(()));
    assert_eq!(
        tab_close_availability(&close, true),
        Err(CommandError::Unavailable(
            "About panel is showing".to_owned()
        ))
    );
}

#[test]
fn a_repeated_close_window_is_refused_while_its_confirmation_shows() {
    let pending = Err(CommandError::Unavailable(
        "close confirmation pending".to_owned(),
    ));
    let mut close = CloseState::default();
    assert_eq!(close.check_close_available(&CloseTarget::Window), Ok(()));
    close.begin_check(CloseTarget::Tab(TabId::new(1)));
    close.checked(true);
    assert_eq!(
        close.check_close_available(&CloseTarget::Window),
        Ok(()),
        "closing the window widens a tab confirmation"
    );
    close.cancel();
    close.begin_check(CloseTarget::Window);
    assert_eq!(
        close.checked(true),
        Some(CloseDecision::Confirm(CloseTarget::Window))
    );
    close.dialog_focus = DialogFocus::Cancel;
    assert_eq!(close.check_close_available(&CloseTarget::Window), pending);
    assert_eq!(
        close.check_close_available(&CloseTarget::Application),
        Ok(()),
        "Quit still widens a window confirmation"
    );
    assert_eq!(
        close.dialog_focus,
        DialogFocus::Cancel,
        "the refused repeat leaves the focused button alone"
    );
    close.cancel();
    close.begin_check(CloseTarget::Application);
    close.checked(true);
    assert_eq!(close.check_close_available(&CloseTarget::Window), pending);
}

#[test]
fn a_replaced_focused_toast_releases_focus_and_resumes_expiry() {
    let now = Instant::now();
    let tab = TabId::new(1);
    let failure = |message: &str| {
        terminal_notice(
            tab,
            "shell",
            TerminalFailure {
                severity: Severity::Error,
                title: "Terminal error",
                message: message.to_owned(),
            },
        )
    };
    let mut stack = NoticeStack::default();
    let first = stack.push(failure("first"), now);
    let mut focused = Some(first);
    let mut hovered = HashSet::from([first]);
    assert!(
        !reconcile_notice_state(&mut stack, &mut focused, &mut hovered, now),
        "a live focused toast keeps focus"
    );
    assert_eq!(focused, Some(first));
    assert_eq!(stack.next_deadline(), None, "focus pauses expiry");

    // The tab's next failure replaces its toast, including the focused one.
    let source = NoticeSource::Terminal {
        tab,
        title: "shell".to_owned(),
    };
    assert!(stack.replace_source(&source, vec![failure("second")], now));
    assert!(
        reconcile_notice_state(&mut stack, &mut focused, &mut hovered, now),
        "the focused toast went away, so focus returns to the terminal"
    );
    assert_eq!(focused, None);
    assert!(
        hovered.is_empty(),
        "the hover id of the replaced toast goes"
    );
    assert!(
        stack.next_deadline().is_some(),
        "nothing live is focused or hovered, so expiry resumes"
    );
}

#[test]
fn a_set_check_widens_to_a_tab_queued_during_the_assessment() {
    let (first, second, third) = (TabId::new(1), TabId::new(2), TabId::new(3));
    let mut close = CloseState::default();
    close.begin_check(CloseTarget::Tabs(vec![first, second]));
    close.queue(CloseTarget::Tab(third));
    assert_eq!(
        close.checked(true),
        Some(CloseDecision::Check(CloseTarget::Tabs(vec![
            first, second, third
        ])))
    );
    assert!(close.pending.is_none());
}

#[test]
fn tab_set_resolution_follows_window_order() {
    let (first, second, third) = (TabId::new(1), TabId::new(2), TabId::new(3));
    let order = [first, second, third];
    assert_eq!(other_tabs(&order, first), vec![second, third]);
    assert_eq!(other_tabs(&order, second), vec![first, third]);
    assert_eq!(other_tabs(&order, third), vec![first, second]);
    assert_eq!(other_tabs(&[first], first), Vec::<TabId>::new());
    assert_eq!(tabs_after(&order, first), vec![second, third]);
    assert_eq!(tabs_after(&order, second), vec![third]);
    assert_eq!(tabs_after(&order, third), Vec::<TabId>::new());
    assert_eq!(tabs_after(&order, TabId::new(9)), Vec::<TabId>::new());
    assert_eq!(tabs_target(Vec::new()), None);
    assert_eq!(tabs_target(vec![third]), Some(CloseTarget::Tab(third)));
    assert_eq!(
        tabs_target(vec![second, third]),
        Some(CloseTarget::Tabs(vec![second, third]))
    );
}

#[test]
fn close_dialog_input_maps_busy_terminals_to_tabs() {
    use huterm_core::{JobProcess, JobState};
    let (first, second, third) = (TabId::new(1), TabId::new(2), TabId::new(3));
    let titles = vec![
        TabTitle {
            tab: first,
            terminal: TerminalId::new(10),
            title: "build".to_owned(),
        },
        TabTitle {
            tab: second,
            terminal: TerminalId::new(20),
            title: "shell".to_owned(),
        },
        TabTitle {
            tab: third,
            terminal: TerminalId::new(30),
            title: "editor".to_owned(),
        },
    ];
    let cargo = JobProcess {
        pid: 41,
        group: 41,
        group_started: None,
        foreground: true,
        identity: "cargo".to_owned(),
        command: "cargo".to_owned(),
        command_line: Some("cargo build".to_owned()),
    };
    let jobs = [
        (TerminalId::new(10), JobState::Running(vec![cargo.clone()])),
        (TerminalId::new(20), JobState::Idle),
        (TerminalId::new(30), JobState::Unknown),
    ];
    let jobs = || jobs.iter().map(|(terminal, state)| (*terminal, state));
    let row = ProcessRow {
        command: "cargo".to_owned(),
        pid: 41,
        foreground: true,
        command_line: Some("cargo build".to_owned()),
    };

    let single = close_dialog_input(&CloseTarget::Tab(first), jobs(), &titles);
    assert_eq!(
        single.target,
        CloseDialogTarget::Tab {
            title: "build".to_owned()
        }
    );
    assert_eq!(
        single.groups,
        vec![
            ProcessGroup {
                tab_title: None,
                state: ProcessGroupState::Known(vec![row.clone()]),
            },
            ProcessGroup {
                tab_title: None,
                state: ProcessGroupState::Unknown,
            },
        ],
        "idle tabs are omitted and single-tab dialogs have no headings"
    );

    let several = close_dialog_input(
        &CloseTarget::Tabs(vec![first, second, third]),
        jobs(),
        &titles,
    );
    assert_eq!(several.target, CloseDialogTarget::Tabs { count: 3 });
    assert_eq!(
        several.groups,
        vec![
            ProcessGroup {
                tab_title: Some("build".to_owned()),
                state: ProcessGroupState::Known(vec![row.clone()]),
            },
            ProcessGroup {
                tab_title: Some("editor".to_owned()),
                state: ProcessGroupState::Unknown,
            },
        ]
    );

    let window = close_dialog_input(&CloseTarget::Window, jobs(), &titles);
    assert_eq!(window.target, CloseDialogTarget::Window);
    assert_eq!(window.groups, several.groups);

    let unviewed =
        [(TerminalId::new(99), JobState::Running(vec![cargo.clone()]))];
    let quit = close_dialog_input(
        &CloseTarget::Application,
        unviewed.iter().map(|(terminal, state)| (*terminal, state)),
        &titles,
    );
    assert_eq!(quit.target, CloseDialogTarget::Application);
    assert_eq!(
        quit.groups,
        vec![ProcessGroup {
            tab_title: Some("Detached terminal".to_owned()),
            state: ProcessGroupState::Known(vec![row]),
        }]
    );
}

#[test]
fn a_one_tab_window_dialog_has_no_headings() {
    use huterm_core::{JobProcess, JobState};
    let titles = [TabTitle {
        tab: TabId::new(1),
        terminal: TerminalId::new(10),
        title: "build".to_owned(),
    }];
    let cargo = JobProcess {
        pid: 41,
        group: 41,
        group_started: None,
        foreground: true,
        identity: "cargo".to_owned(),
        command: "cargo".to_owned(),
        command_line: None,
    };
    let running = JobState::Running(vec![cargo]);
    let jobs = [(TerminalId::new(10), &running)];
    let window = close_dialog_input(&CloseTarget::Window, jobs, &titles);
    assert_eq!(
        window
            .groups
            .iter()
            .map(|group| &group.tab_title)
            .collect::<Vec<_>>(),
        [&None],
        "the dialog covers one tab, so its rows need no heading"
    );
    let quit = close_dialog_input(&CloseTarget::Application, jobs, &titles);
    assert_eq!(
        quit.groups[0].tab_title.as_deref(),
        Some("build"),
        "Quit may cover other windows, so it keeps headings"
    );
}

#[test]
fn all_placements_share_nonoverlapping_terminal_and_tab_bounds() {
    for position in [
        TabPosition::Top,
        TabPosition::Bottom,
        TabPosition::Left,
        TabPosition::Right,
        TabPosition::Titlebar,
    ] {
        for titlebar in [px(0.0), px(32.0)] {
            let layout = ChromeLayout::new(
                size(px(800.0), px(600.0)),
                titlebar,
                position,
            );
            // The merged row lives in the titlebar, so it and the
            // terminal partition the whole window instead.
            let shared = if position == TabPosition::Titlebar {
                px(600.0)
            } else {
                px(600.0) - titlebar
            };
            assert_eq!(
                layout.terminal.size.width
                    * f32::from(layout.terminal.size.height)
                    + layout.tabs.size.width
                        * f32::from(layout.tabs.size.height),
                px(800.0) * f32::from(shared)
            );
            match position {
                TabPosition::Top | TabPosition::Titlebar => {
                    assert_eq!(layout.tabs.bottom(), layout.terminal.top());
                }
                TabPosition::Bottom => {
                    assert_eq!(layout.terminal.bottom(), layout.tabs.top());
                }
                TabPosition::Left => {
                    assert_eq!(layout.tabs.right(), layout.terminal.left());
                }
                TabPosition::Right => {
                    assert_eq!(layout.terminal.right(), layout.tabs.left());
                }
            }
            assert!(layout.terminal.top() >= titlebar);
        }
    }
}

#[test]
fn merged_traffic_lights_centre_any_button_size_at_the_native_inset() {
    let native = |x: f64, size: f64| Bounds {
        origin: point(x, 5.0),
        size: gpui::size(size, size),
    };
    // AppKit's 12-point buttons keep the placement GPUI 0.2.2 needed.
    assert_eq!(
        merged_traffic_lights(native(7.0, 12.0)),
        point(px(7.0), px(10.0))
    );
    // Larger 14-point buttons move right with AppKit and stay centred.
    assert_eq!(
        merged_traffic_lights(native(9.0, 14.0)),
        point(px(9.0), px(9.0))
    );
    // Buttons taller than the strip start at its top edge.
    assert_eq!(
        merged_traffic_lights(native(9.0, 40.0)),
        point(px(9.0), px(0.0))
    );
}

#[test]
fn the_title_bar_row_holds_tabs_after_the_traffic_lights() {
    let viewport = size(px(800.0), px(600.0));
    let strip = px(32.0);
    let top = ChromeLayout::new(viewport, strip, TabPosition::Top);
    let merged = ChromeLayout::new(viewport, strip, TabPosition::Titlebar);
    // The terminal gains the bar's height: it starts under the strip.
    assert_eq!(
        merged.terminal,
        Bounds::new(point(px(0.0), strip), size(px(800.0), px(568.0)))
    );
    assert_eq!(
        merged.terminal.size.height,
        top.terminal.size.height + top.tabs.size.height
    );
    // The row is the strip itself: full width at the strip's height.
    assert_eq!(
        merged.tabs,
        Bounds::new(point(px(0.0), px(0.0)), size(px(800.0), strip))
    );
    // Tabs start after the traffic lights; Pill adds its usual lead.
    let tabs = |style| TabsConfig {
        position: TabPosition::Titlebar,
        style,
        ..TabsConfig::default()
    };
    assert_eq!(
        merged.strip_bounds(tabs(TabStyle::Strip)),
        Bounds::new(
            point(TRAFFIC_LIGHT_INSET, px(0.0)),
            size(px(800.0) - TRAFFIC_LIGHT_INSET, strip)
        )
    );
    assert_eq!(
        merged.strip_bounds(tabs(TabStyle::Pill)).origin.x,
        TRAFFIC_LIGHT_INSET + PILL_INSET - PILL_MARGIN_LEFT
    );
    // The menu button keeps the plain strip's spot at the row's right
    // end, and the strip reserves that slot beside `+`.
    let bar_inset = (merged.tabs.size.height - CONTROL_SIZE) / 2.0;
    assert_eq!(
        point(
            merged.tabs.right() - CONTROL_SLOT + CONTROL_INSET,
            merged.tabs.origin.y + bar_inset
        ),
        point(px(800.0) - CONTROL_INSET - CONTROL_SIZE, CONTROL_INSET)
    );
    let strip_geometry = TabStrip::new(
        merged.strip_bounds(tabs(TabStyle::Strip)),
        false,
        TabExtents::Uniform(3),
        px(0.0),
        true,
    );
    assert_eq!(
        strip_geometry.available(),
        px(800.0) - TRAFFIC_LIGHT_INSET - CONTROL_SLOT * 2.0
    );
    // Hiding the bar frees no terminal space: the strip stays.
    let hidden = ChromeLayout::new(viewport, strip, TabPosition::Titlebar)
        .present(Presentation::Hidden, TabPosition::Titlebar, 0.0);
    assert_eq!(hidden.terminal, merged.terminal);
    // A tiny window clamps the row to the viewport.
    let tiny = ChromeLayout::new(
        size(px(20.0), px(10.0)),
        strip,
        TabPosition::Titlebar,
    );
    assert_eq!(tiny.tabs.size, size(px(20.0), px(10.0)));
    assert_eq!(tiny.terminal.size.height, px(0.0));
    assert_eq!(tiny.strip_bounds(tabs(TabStyle::Strip)).size.width, px(0.0));
}

fn framed(state: FrameState) -> WindowFrame {
    framed_with(state, ButtonLayout::standard())
}

fn framed_with(state: FrameState, buttons: ButtonLayout) -> WindowFrame {
    WindowFrame::resolve(
        resolve_tab_position(
            TabPosition::Titlebar,
            TabHost {
                platform: Platform::Linux,
                fullscreen: false,
                quake: false,
                client_decorations: state.client_decorations(),
            },
        ),
        state,
        buttons,
    )
}

#[test]
fn the_drawn_title_row_and_terminal_sit_inside_the_frame() {
    if Platform::current() != Platform::Linux {
        return;
    }
    let viewport = size(px(800.0), px(600.0));
    let state = FrameState {
        decorations: gpui::Decorations::Client {
            tiling: gpui::Tiling::default(),
        },
        maximized: false,
        fullscreen: false,
    };
    let frame = framed(state);
    assert!(frame.controls);
    assert!(frame.decorated());
    let inset = client_frame::CLIENT_INSET;
    assert_eq!(frame.inset, gpui::Edges::all(inset));
    let layout =
        ChromeLayout::with_frame(viewport, TabPosition::Titlebar, frame);
    // The content is the viewport less the border on every side.
    assert_eq!(
        layout.content,
        Bounds::new(
            point(inset, inset),
            size(px(800.0) - inset * 2.0, px(600.0) - inset * 2.0)
        )
    );
    // The row spans the content's top at the bar's height; the
    // terminal takes the rest, shrunk by the inset on every side.
    assert_eq!(
        layout.tabs,
        Bounds::new(
            point(inset, inset),
            size(px(800.0) - inset * 2.0, TAB_HEIGHT)
        )
    );
    assert_eq!(
        layout.terminal,
        Bounds::new(
            point(inset, inset + TAB_HEIGHT),
            size(
                px(800.0) - inset * 2.0,
                px(600.0) - inset * 2.0 - TAB_HEIGHT
            )
        )
    );
    assert_eq!(layout.tabs.bottom(), layout.terminal.top());
    // Tabs start after the small lead and stop before the window
    // controls; the menu button slot sits just before them.
    let tabs = TabsConfig {
        position: TabPosition::Titlebar,
        style: TabStyle::Strip,
        ..TabsConfig::default()
    };
    let strip = layout.strip_bounds(tabs);
    assert_eq!(strip.origin, point(inset + TITLE_ROW_LEAD, inset));
    assert_eq!(
        strip.right(),
        layout.tabs.right() - ButtonLayout::standard().trailing.width()
    );
    assert_eq!(
        layout.title_row_border(),
        Bounds::new(
            point(inset, inset + TAB_HEIGHT - px(1.0)),
            size(px(800.0) - inset * 2.0, px(1.0))
        )
    );
    // Hiding the bar frees nothing: the row is the title bar.
    let hidden =
        layout.present(Presentation::Hidden, TabPosition::Titlebar, 0.0);
    assert_eq!(hidden.terminal, layout.terminal);
    // The row is the title bar: it takes its height from the content
    // like AppKit's strip, not a reservation on top of it.
    assert_eq!(title_row_height(false, frame), TAB_HEIGHT);
    assert_eq!(
        ChromeLayout::bar_reservation(tabs, SIDEBAR_WIDTH),
        size(px(0.0), px(0.0))
    );
}

#[test]
fn the_title_row_follows_the_desktop_button_layout() {
    if Platform::current() != Platform::Linux {
        return;
    }
    let viewport = size(px(800.0), px(600.0));
    let state = FrameState {
        decorations: gpui::Decorations::Client {
            tiling: gpui::Tiling::default(),
        },
        maximized: false,
        fullscreen: false,
    };
    let inset = client_frame::CLIENT_INSET;
    let tabs = TabsConfig {
        position: TabPosition::Titlebar,
        style: TabStyle::Strip,
        ..TabsConfig::default()
    };
    // Buttons at the start push the tabs after them; with none at the
    // end the strip runs to the row's end, as beside macOS's lights.
    let left = ButtonLayout::parse("close,minimize,maximize:");
    let layout = ChromeLayout::with_frame(
        viewport,
        TabPosition::Titlebar,
        framed_with(state, left),
    );
    let strip = layout.strip_bounds(tabs);
    assert_eq!(strip.left(), inset + left.leading.width());
    assert_eq!(strip.right(), layout.tabs.right());
    // Split buttons reserve both ends.
    let split = ButtonLayout::parse("close:maximize");
    let layout = ChromeLayout::with_frame(
        viewport,
        TabPosition::Titlebar,
        framed_with(state, split),
    );
    let strip = layout.strip_bounds(tabs);
    assert_eq!(strip.left(), inset + split.leading.width());
    assert_eq!(strip.right(), layout.tabs.right() - split.trailing.width());
    // No buttons keep the small lead and reserve nothing at the end.
    let layout = ChromeLayout::with_frame(
        viewport,
        TabPosition::Titlebar,
        framed_with(state, ButtonLayout::parse("appmenu:")),
    );
    let strip = layout.strip_bounds(tabs);
    assert_eq!(strip.left(), inset + TITLE_ROW_LEAD);
    assert_eq!(strip.right(), layout.tabs.right());
    // A window without the drawn row carries no buttons at all.
    let fallback = framed_with(FrameState::default(), left);
    assert_eq!(fallback, WindowFrame::default());
}

#[test]
fn tiled_maximized_and_fullscreen_frames_keep_the_row_but_no_border() {
    if Platform::current() != Platform::Linux {
        return;
    }
    let viewport = size(px(800.0), px(600.0));
    let client = |tiling| gpui::Decorations::Client { tiling };
    for state in [
        FrameState {
            decorations: client(gpui::Tiling {
                left: true,
                ..gpui::Tiling::default()
            }),
            maximized: false,
            fullscreen: false,
        },
        FrameState {
            decorations: client(gpui::Tiling::default()),
            maximized: true,
            fullscreen: false,
        },
    ] {
        let frame = framed(state);
        assert!(frame.controls, "{state:?}");
        assert!(!frame.decorated(), "{state:?}");
        assert_eq!(frame.inset, gpui::Edges::default(), "{state:?}");
        let layout =
            ChromeLayout::with_frame(viewport, TabPosition::Titlebar, frame);
        assert_eq!(
            layout.content,
            Bounds::new(point(px(0.0), px(0.0)), viewport)
        );
        assert_eq!(
            layout.tabs,
            Bounds::new(point(px(0.0), px(0.0)), size(px(800.0), TAB_HEIGHT))
        );
        assert_eq!(layout.terminal.origin, point(px(0.0), TAB_HEIGHT));
        assert_eq!(layout.terminal.right(), px(800.0));
        assert_eq!(layout.terminal.bottom(), px(600.0));
    }
    // Fullscreen hides the row: the tabs become a top bar with no frame.
    let fullscreen = WindowFrame::resolve(
        resolve_tab_position(
            TabPosition::Titlebar,
            TabHost {
                platform: Platform::Linux,
                fullscreen: true,
                quake: false,
                client_decorations: true,
            },
        ),
        FrameState {
            decorations: client(gpui::Tiling::tiled()),
            maximized: false,
            fullscreen: true,
        },
        ButtonLayout::standard(),
    );
    assert_eq!(fullscreen, WindowFrame::default());
    assert_eq!(title_row_height(true, fullscreen), px(0.0));
    // Without a compositor the row is the window manager's.
    let fallback = framed(FrameState::default());
    assert_eq!(fallback, WindowFrame::default());
    assert_eq!(title_row_height(false, fallback), px(0.0));
    let layout = ChromeLayout::with_frame(viewport, TabPosition::Top, fallback);
    assert_eq!(layout.terminal.origin, point(px(0.0), TAB_HEIGHT));
}

#[test]
fn initial_windows_reserve_no_height_for_the_merged_row() {
    let tabs = |position| TabsConfig {
        position,
        always_show: true,
        ..TabsConfig::default()
    };
    assert_eq!(
        ChromeLayout::bar_reservation(
            tabs(TabPosition::Titlebar),
            SIDEBAR_WIDTH
        ),
        size(px(0.0), px(0.0))
    );
    assert_eq!(
        ChromeLayout::bar_reservation(tabs(TabPosition::Top), SIDEBAR_WIDTH),
        size(px(0.0), tab_bar_height(tabs(TabPosition::Top)))
    );
    assert_eq!(
        ChromeLayout::bar_reservation(tabs(TabPosition::Bottom), SIDEBAR_WIDTH),
        size(px(0.0), tab_bar_height(tabs(TabPosition::Bottom)))
    );
    for column in [TabPosition::Left, TabPosition::Right] {
        assert_eq!(
            ChromeLayout::bar_reservation(tabs(column), SIDEBAR_WIDTH),
            size(SIDEBAR_WIDTH, px(0.0))
        );
    }
    // Without a title bar the configured row becomes a top bar and
    // takes its height again.
    let host = TabHost {
        platform: Platform::Linux,
        fullscreen: false,
        quake: false,
        client_decorations: false,
    };
    let resolved = layout_tabs(tabs(TabPosition::Titlebar), host);
    assert_eq!(resolved.position, TabPosition::Top);
    assert_eq!(
        ChromeLayout::bar_reservation(resolved, SIDEBAR_WIDTH).height,
        tab_bar_height(resolved)
    );
}

#[test]
fn tiny_windows_keep_nonnegative_bounds_and_a_bounded_sidebar() {
    for position in [
        TabPosition::Top,
        TabPosition::Bottom,
        TabPosition::Left,
        TabPosition::Right,
    ] {
        let layout =
            ChromeLayout::new(size(px(3.0), px(2.0)), px(32.0), position);
        assert!(layout.terminal.size.width >= px(0.0));
        assert!(layout.terminal.size.height >= px(0.0));
        assert!(layout.terminal.bottom() <= px(2.0));
        assert!(layout.terminal.right() <= px(3.0));
        if position.vertical() {
            assert!(layout.tabs.size.width <= px(1.5));
        }
    }
}

#[test]
fn native_tab_shortcuts_match_actions_and_are_reserved_from_terminal_input() {
    for platform in [Platform::MacOs, Platform::Linux] {
        let macos = platform == Platform::MacOs;
        let compiled = keymap::compile(platform, &[]).unwrap();
        let prefix = if macos { "cmd" } else { "ctrl-shift" };
        for (chord, command) in [
            (format!("{prefix}-n"), ids::NEW_WINDOW),
            (format!("{prefix}-t"), ids::NEW_TAB),
            (format!("{prefix}-w"), ids::CLOSE_TAB),
            ("ctrl-tab".into(), ids::NEXT_TAB),
            ("ctrl-shift-tab".into(), ids::PREVIOUS_TAB),
            (
                format!("{}-9", if macos { "cmd" } else { "alt" }),
                ids::SELECT_TAB,
            ),
        ] {
            let key = Keystroke::parse(&chord).unwrap();
            let matched = compiled
                .bindings
                .iter()
                .filter(|binding| {
                    binding.match_keystrokes(std::slice::from_ref(&key))
                        == Some(false)
                })
                .map(keymap::bound_command)
                .collect::<Vec<_>>();
            assert_eq!(matched, vec![Some(command)], "{chord}");
            assert!(compiled.reserved.is_reserved(&key), "{chord}");
        }
    }
}

#[test]
fn hidden_tabs_coalesce_invalidations_without_requesting_snapshots() {
    use huterm_config::RefreshMode;

    let mut pacer = super::super::refresh::SnapshotPacer::default();
    let mut scroll = ScrollController::default();
    for _ in 0..100 {
        scroll.invalidate();
        assert!(
            pacer
                .begin(&mut scroll, false, RefreshMode::Display)
                .is_none()
        );
    }
    assert_eq!(scroll.diagnostics().requests_started, 0);
    assert!(
        pacer
            .begin(&mut scroll, true, RefreshMode::Display)
            .is_some()
    );
    assert_eq!(scroll.diagnostics().requests_started, 1);
}

#[test]
fn tab_labels_degrade_across_all_metadata_modes() {
    use huterm_config::{TabDirectory, TabLabel};
    use huterm_protocol::{TerminalDirectory, TerminalMetadata};

    let empty = TerminalMetadata::default();
    assert_eq!(
        resolve_tab_label(
            TabLabel::Title,
            TabDirectory::Name,
            "shell",
            &empty,
            &[]
        ),
        "shell"
    );
    assert_eq!(
        resolve_tab_label(
            TabLabel::Process,
            TabDirectory::Name,
            "shell",
            &empty,
            &[]
        ),
        "shell"
    );
    assert_eq!(
        resolve_tab_label(
            TabLabel::Directory,
            TabDirectory::Name,
            "shell",
            &empty,
            &[]
        ),
        "shell"
    );

    let process = TerminalMetadata::new(None, Some("vim".into()));
    assert_eq!(
        resolve_tab_label(
            TabLabel::Process,
            TabDirectory::Name,
            "shell",
            &process,
            &[]
        ),
        "vim"
    );
    assert_eq!(
        resolve_tab_label(
            TabLabel::ProcessAndDirectory,
            TabDirectory::Name,
            "shell",
            &process,
            &[]
        ),
        "vim"
    );

    let directory = TerminalMetadata::new(
        Some(TerminalDirectory::new(None, "/tmp/世界".into(), false)),
        None,
    );
    assert_eq!(
        resolve_tab_label(
            TabLabel::Directory,
            TabDirectory::Name,
            "shell",
            &directory,
            &[]
        ),
        "世界"
    );
    assert_eq!(
        resolve_tab_label(
            TabLabel::ProcessAndDirectory,
            TabDirectory::Name,
            "shell",
            &directory,
            &[]
        ),
        "世界"
    );

    let both = TerminalMetadata::new(
        Some(TerminalDirectory::new(
            Some("remote".into()),
            "/work/project".into(),
            false,
        )),
        Some("cargo".into()),
    );
    assert_eq!(
        resolve_tab_label(
            TabLabel::ProcessAndDirectory,
            TabDirectory::Name,
            "shell",
            &both,
            &[]
        ),
        "cargo · project"
    );
}

#[test]
fn smart_labels_follow_running_programs_and_idle_directories() {
    use huterm_config::{TabDirectory, TabLabel};
    use huterm_protocol::{TerminalDirectory, TerminalMetadata};

    let home = ["/Users/me".to_owned()];
    let project = Some(TerminalDirectory::new(
        None,
        "/Users/me/Projects/huterm".into(),
        true,
    ));
    let smart = |metadata: &TerminalMetadata, style| {
        resolve_tab_label(TabLabel::Smart, style, "shell", metadata, &home)
    };
    let idle = TerminalMetadata::new(project.clone(), None)
        .with_foreground_title(Some("me@host: ~/Projects/huterm".into()));
    assert_eq!(smart(&idle, TabDirectory::Name), "huterm");
    assert_eq!(smart(&idle, TabDirectory::Path), "~/Projects/huterm");
    let running = TerminalMetadata::new(project.clone(), Some("vim".into()));
    assert_eq!(smart(&running, TabDirectory::Name), "vim");
    let titled = running.with_foreground_title(Some("notes.txt - VIM".into()));
    assert_eq!(smart(&titled, TabDirectory::Name), "notes.txt - VIM");
    assert_eq!(
        smart(&TerminalMetadata::default(), TabDirectory::Name),
        "shell"
    );
}

#[test]
fn directory_styles_format_home_relative_and_remote_paths() {
    use huterm_config::TabDirectory::{Name, Path, Short};
    use huterm_protocol::TerminalDirectory;

    let home = ["/Users/me".to_owned()];
    for (path, local, name, full, short) in [
        ("/Users/me", true, "~", "~", "~"),
        ("/Users/me/", true, "~", "~", "~"),
        (
            "/Users/me/Projects",
            true,
            "Projects",
            "~/Projects",
            "~/Projects",
        ),
        (
            "/Users/me/Projects/huterm",
            true,
            "huterm",
            "~/Projects/huterm",
            "~/P/huterm",
        ),
        (
            "/Users/me/.t3/worktrees/huterm/t3code",
            true,
            "t3code",
            "~/.t3/worktrees/huterm/t3code",
            "~/.t/w/h/t3code",
        ),
        (
            "/Users/me/Ünïcode/app",
            true,
            "app",
            "~/Ünïcode/app",
            "~/Ü/app",
        ),
        (
            "/Users/meadow",
            true,
            "meadow",
            "/Users/meadow",
            "/U/meadow",
        ),
        (
            "/usr/local/share/man",
            true,
            "man",
            "/usr/local/share/man",
            "/u/l/s/man",
        ),
        ("/", true, "/", "/", "/"),
        (
            "/Users/me/src/app",
            false,
            "app",
            "/Users/me/src/app",
            "/U/m/s/app",
        ),
    ] {
        let directory = TerminalDirectory::new(None, path.into(), local);
        let label = |style| directory_label(&directory, style, &home).unwrap();
        assert_eq!(
            [label(Name), label(Path), label(Short)],
            [name, full, short],
            "{path} local={local}"
        );
    }
}

#[test]
fn tab_labels_show_the_local_home_directory_as_a_tilde() {
    use huterm_config::{TabDirectory, TabLabel};
    use huterm_protocol::{TerminalDirectory, TerminalMetadata};

    let home = ["/Users/me".to_owned()];
    let at_home = |path: &str, local: bool| {
        TerminalMetadata::new(
            Some(TerminalDirectory::new(None, path.into(), local)),
            Some("vim".into()),
        )
    };
    for path in ["/Users/me", "/Users/me/"] {
        assert_eq!(
            resolve_tab_label(
                TabLabel::Directory,
                TabDirectory::Name,
                "shell",
                &at_home(path, true),
                &home
            ),
            "~"
        );
    }
    assert_eq!(
        resolve_tab_label(
            TabLabel::ProcessAndDirectory,
            TabDirectory::Name,
            "shell",
            &at_home("/Users/me", true),
            &home
        ),
        "vim · ~"
    );
    assert_eq!(
        resolve_tab_label(
            TabLabel::Directory,
            TabDirectory::Name,
            "shell",
            &at_home("/Users/me/src", true),
            &home
        ),
        "src"
    );
    assert_eq!(
        resolve_tab_label(
            TabLabel::Directory,
            TabDirectory::Name,
            "shell",
            &at_home("/Users/me", false),
            &home
        ),
        "me",
        "a remote home is not this machine's"
    );
}

#[test]
fn new_tab_directory_inheritance_accepts_only_local_usable_directories() {
    use std::os::unix::fs::PermissionsExt as _;

    use huterm_config::NewTabDirectory;
    use huterm_protocol::{TerminalDirectory, TerminalMetadata};

    let root = std::env::temp_dir().join(format!(
        "huterm-directory-inheritance-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let local = TerminalMetadata::new(
        Some(TerminalDirectory::new(
            Some("localhost".into()),
            root.to_string_lossy().into_owned(),
            true,
        )),
        None,
    );
    let captured =
        inherited_directory(NewTabDirectory::Inherit, &local).unwrap();
    assert_eq!(captured, root);
    assert!(usable_launch_directory(&captured));
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o600))
        .unwrap();
    assert!(!usable_launch_directory(&captured));
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
        .unwrap();
    assert_eq!(inherited_directory(NewTabDirectory::Default, &local), None);

    let remote = TerminalMetadata::new(
        Some(TerminalDirectory::new(
            Some("remote".into()),
            root.to_string_lossy().into_owned(),
            false,
        )),
        None,
    );
    assert_eq!(inherited_directory(NewTabDirectory::Inherit, &remote), None);
    std::fs::remove_dir(&root).unwrap();
    assert!(!usable_launch_directory(&captured));
}
