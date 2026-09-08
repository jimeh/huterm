//! File-driven native input probe. The desktop and PTY are production instances.

#[path = "input_smoke/native.rs"]
mod native;
pub(super) fn post_event(command: &str) -> anyhow::Result<()> {
    native::post(command)
}

pub(crate) fn run() -> anyhow::Result<()> {
    if let Some(mode) = std::env::args().nth(1) {
        return native::clipboard_file(
            &mode,
            &std::env::args().nth(2).ok_or_else(|| {
                anyhow::anyhow!("clipboard helper requires a file")
            })?,
        );
    }
    native::validate_layout()?;
    let directory =
        std::path::PathBuf::from(std::env::var("HUTERM_INPUT_SMOKE")?);
    super::run_with_startup(move |cx| {
        cx.spawn(async move |cx| {
            let mut sequence = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(10))
                    .await;
                let state = cx.update(read_state).expect("read smoke state");
                std::fs::write(directory.join("state"), state)
                    .expect("write smoke state");
                let file = directory.join(format!("event-{sequence}"));
                if let Ok(command) = std::fs::read_to_string(&file) {
                    let result = if command == "shutdown" {
                        cx.update(|cx| cx.quit())
                    } else if let Some(text) =
                        command.strip_prefix("clipboard\t")
                    {
                        cx.update(|cx| {
                            cx.write_to_clipboard(
                                gpui::ClipboardItem::new_string(text.into()),
                            );
                        })
                    } else {
                        native::post(&command)
                    };
                    if let Err(error) = result {
                        eprintln!(
                            "NATIVE_INPUT_SMOKE event {sequence}: {error:#}"
                        );
                        cx.update(|cx| cx.quit()).expect("quit failed probe");
                        return;
                    }
                    std::fs::write(
                        directory.join(format!("posted-{sequence}")),
                        "posted",
                    )
                    .expect("write native event acknowledgement");
                    sequence += 1;
                }
            }
        })
        .detach();
    })
}

fn read_state(cx: &mut gpui::App) -> String {
    let desktop = cx.global::<super::Desktop>();
    let policy = format!("{:?}", desktop.config.terminal.macos_option_as_alt);
    let reloading = desktop.reloading;
    let bindings = desktop.config.keybindings.len();
    let view = desktop.windows.first().and_then(gpui::WeakEntity::upgrade);
    let (tabs, selection, exited, ready, active) =
        view.map_or((0, false, false, false, 0), |view| {
            let view = view.read(cx);
            let terminal = view.active_view();
            let terminal = terminal.as_ref().map(|tab| tab.read(cx));
            let selected = terminal.is_some_and(|tab| {
                tab.selection
                    .and_then(super::super::Selection::range)
                    .is_some()
            });
            let exited = terminal.is_some_and(|tab| tab.exited);
            let ready = terminal.is_some_and(|tab| {
                tab.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot
                        .cells()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                        .contains("READY")
                })
            });
            let active = view
                .tabs
                .iter()
                .position(|tab| Some(tab.id) == view.active)
                .unwrap_or(0);
            (view.tabs.len(), selected, exited, ready, active)
        });
    let pending = cx.windows().first().copied().is_some_and(|window| {
        window
            .update(cx, |_, window, _| window.has_pending_keystrokes())
            .unwrap_or(false)
    });
    format!(
        "policy={policy} reloading={reloading} bindings={bindings} tabs={tabs} selection={selection} pending={pending} exited={exited} ready={ready} active={active}"
    )
}
