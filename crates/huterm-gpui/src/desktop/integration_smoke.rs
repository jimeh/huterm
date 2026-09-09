//! Native delivery probe using production windows, snapshots, gestures and PTYs.
use anyhow::Context as _;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

#[cfg(target_os = "macos")]
#[path = "integration_smoke/native.rs"]
mod native;

pub(crate) fn run() -> anyhow::Result<()> {
    let directory =
        std::path::PathBuf::from(std::env::var("HUTERM_INTEGRATION_SMOKE")?);
    super::run_with_startup(move |cx| {
        cx.spawn(async move |cx| {
            let mut sequence = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(10))
                    .await;
                let state = cx.update(read_state).expect("integration state");
                publish(&directory, "state", &state);
                if let Ok(command) = std::fs::read_to_string(
                    directory.join(format!("command-{sequence}")),
                ) {
                    #[cfg(target_os = "macos")]
                    let native_result = if let Some(command) =
                        command.strip_prefix("native\t")
                    {
                        Some(super::input_smoke::post_event(command))
                    } else if command == "native_close" {
                        Some(native::close_window())
                    } else {
                        command.strip_prefix("drop\t").map(native::drop_event)
                    };
                    let invoke = || {
                        cx.update(|cx| {
                            if command == "shutdown" {
                                cx.quit();
                                return Ok(());
                            }
                            if command == "activate_window" {
                                let handle = cx
                                    .windows()
                                    .first()
                                    .copied()
                                    .context("no window")?;
                                handle.update(cx, |_, window, _| {
                                    window.activate_window();
                                })?;
                                return Ok(());
                            }
                            let spec = huterm_protocol::lookup(command.trim())
                                .context("unknown command")?;
                            let window = cx.windows().first().copied();
                            let outcome = super::Desktop::invoke(
                                cx,
                                &huterm_protocol::CommandInvocation {
                                    id: spec.id,
                                    args: Vec::new(),
                                },
                                window,
                            );
                            anyhow::ensure!(
                                outcome.is_ok(),
                                "command failed: {outcome:?}"
                            );
                            Ok(())
                        })
                        .and_then(std::convert::identity)
                    };
                    #[cfg(target_os = "macos")]
                    let result = native_result.unwrap_or_else(invoke);
                    #[cfg(not(target_os = "macos"))]
                    let result = invoke();
                    publish(
                        &directory,
                        &format!("result-{sequence}"),
                        &result.map_or_else(
                            |error| format!("error: {error:#}"),
                            |()| "ok".into(),
                        ),
                    );
                    sequence += 1;
                }
            }
        })
        .detach();
    })
}

fn publish(directory: &Path, name: &str, value: &str) {
    let temporary = directory.join(format!("{name}.tmp"));
    std::fs::write(&temporary, value).expect("write integration state");
    std::fs::rename(temporary, directory.join(name))
        .expect("publish integration state");
}

fn record_open(destination: &str, _: &mut gpui::App) {
    let directory = std::env::var("HUTERM_INTEGRATION_SMOKE")
        .expect("integration directory");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(Path::new(&directory).join("opened"))
        .expect("opened record");
    writeln!(file, "{destination}").expect("record opened URL");
}

fn read_state(cx: &mut gpui::App) -> String {
    let mut state = String::new();
    writeln!(
        state,
        "windows={}\ndrag_active={}",
        cx.windows().len(),
        cx.has_active_drag()
    )
    .unwrap();
    if let Some(handle) = cx.windows().first().copied() {
        let _ = handle.update(cx, |root, window, cx| {
            let Ok(workspace) = root.downcast::<super::WorkspaceView>() else { return; };
            let confirming = workspace.read(cx).close.confirmation.is_some();
            let tabs = workspace.read(cx).tabs.len();
            let Some(terminal) = workspace.read(cx).active_view() else { return; };
            terminal.update(cx, |view, _| {
                view.open_link = record_open;
                let bounds = view.content_bounds(window);
                let layout = view.terminal_layout(window);
                let origin = bounds.origin + layout.bounds.origin;
                let queue = view.scroll.diagnostics();
                writeln!(state,"x={}\ny={}\ncell_width={}\ncell_height={}\ncolumns={}\nrows={}\nexited={}\nconfirming={}\ntabs={}\nhover={}\nowned={}\nrequests={}\ncompletions={}\nlookup_us={}\nlatency_us={}\nconcurrent={}\npending={}\nexternal_drag={}\nselection={}\nstatus={}", f32::from(origin.x),f32::from(origin.y),f32::from(view.metrics.cell_width),f32::from(view.metrics.cell_height),view.last_grid_size.columns,view.last_grid_size.rows,view.exited,confirming,tabs,view.links.hover().map_or("",|link| link.destination.as_str()),view.links.owns_press(),view.link_requests,view.link_completions,view.link_max_lookup.as_micros(),view.link_max_latency.as_micros(),queue.maximum_concurrent,queue.maximum_queued,view.external_drag,view.selecting,view.status.as_deref().unwrap_or("")).unwrap();
                if let Some(snapshot) = &view.snapshot {
                    writeln!(state,"generation={}\nmouse={:?}\ntext={}",snapshot.generation,snapshot.modes.mouse_tracking,snapshot.cells().map(|cell| cell.text.as_str()).collect::<String>()).unwrap();
                }
            });
        });
    }
    state
}
