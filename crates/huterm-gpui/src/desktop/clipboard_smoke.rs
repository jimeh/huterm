//! File-driven application visibility commands for the clipboard smoke.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, ensure};
use gpui::App;
use huterm_protocol::{CommandInvocation, CommandOutcome, ids};

use super::Desktop;

pub(crate) fn run() -> anyhow::Result<()> {
    let directory = PathBuf::from(std::env::var("HUTERM_CLIPBOARD_SMOKE")?);
    super::run_with_startup(move |cx| {
        cx.spawn(async move |cx| {
            let mut sequence = 0;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(10))
                    .await;
                let command_path =
                    directory.join(format!("command-{sequence}"));
                let Ok(command) = std::fs::read_to_string(command_path) else {
                    continue;
                };
                let result = cx
                    .update(|cx| execute(cx, command.trim()))
                    .and_then(std::convert::identity);
                publish(
                    &directory,
                    &format!("result-{sequence}"),
                    &result.unwrap_or_else(|error| format!("error: {error:#}")),
                );
                sequence += 1;
            }
        })
        .detach();
    })
}

fn execute(cx: &mut App, command: &str) -> anyhow::Result<String> {
    match command {
        "hide" => {
            let outcome = Desktop::invoke(
                cx,
                &CommandInvocation::new(ids::HIDE, Vec::new()),
                None,
            )?;
            ensure!(
                outcome == CommandOutcome::Completed,
                "hide returned {outcome:?}"
            );
            Ok("hide completed".to_owned())
        }
        "show" => {
            let handle =
                *cx.windows().first().context("clipboard smoke window")?;
            handle.update(cx, |_, window, cx| {
                window.activate_window();
                cx.activate(true);
            })?;
            Ok("show requested".to_owned())
        }
        _ => anyhow::bail!("unknown clipboard smoke command {command:?}"),
    }
}

fn publish(directory: &Path, name: &str, text: &str) {
    let temporary = directory.join(format!("{name}.tmp"));
    std::fs::write(&temporary, text).expect("write clipboard smoke result");
    std::fs::rename(temporary, directory.join(name))
        .expect("publish clipboard smoke result");
}
