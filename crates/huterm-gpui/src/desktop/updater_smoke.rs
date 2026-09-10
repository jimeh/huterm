//! Packaged Sparkle startup and application-command smoke.

use std::io::Write as _;

use gpui::App;
use huterm_protocol::ids;

use super::{Desktop, run_with_startup};

const UNPACKAGED_DIAGNOSTIC: &str =
    "self-updates are available only in the packaged Huterm application";

pub(crate) fn run() -> anyhow::Result<()> {
    run_with_startup(|cx| {
        if let Err(error) = check(cx) {
            eprintln!("NATIVE_UPDATER_SMOKE failed: {error:#}");
            std::process::exit(1);
        }
        cx.quit();
    })
}

fn check(cx: &mut App) -> anyhow::Result<()> {
    let unpackaged =
        std::env::var_os("HUTERM_UPDATER_SMOKE_UNPACKAGED").is_some();
    if unpackaged {
        let error = cx
            .global::<Desktop>()
            .updater
            .startup_error()
            .unwrap_or("updater unexpectedly started");
        anyhow::ensure!(
            error == UNPACKAGED_DIAGNOSTIC,
            "unexpected unpackaged diagnostic: {error}"
        );
        marker("unpackaged-diagnostic");
        return Ok(());
    }

    let updater = &cx.global::<Desktop>().updater;
    anyhow::ensure!(
        updater.startup_error().is_none(),
        "controller startup failed: {}",
        updater.startup_error().unwrap_or_default()
    );
    marker("controller-started");

    let expected_framework = std::env::var("HUTERM_UPDATER_SMOKE_FRAMEWORK")?;
    let loaded_framework = updater
        .framework_bundle_path()
        .map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        loaded_framework == expected_framework,
        "expected framework {expected_framework}, loaded {loaded_framework}"
    );
    marker("packaged-framework");

    anyhow::ensure!(
        updater
            .can_check_for_updates()
            .map_err(anyhow::Error::msg)?,
        "Sparkle reported canCheckForUpdates=false"
    );
    marker("can-check");

    let checks_before = updater.manual_check_count();
    crate::commands::invoke(ids::CHECK_FOR_UPDATES).dispatch(cx);
    crate::commands::invoke(ids::CHECK_FOR_UPDATES).dispatch(cx);
    anyhow::ensure!(
        cx.global::<Desktop>().updater.manual_check_count()
            == checks_before + 2,
        "repeated application commands did not reach the updater"
    );
    marker("application-command");
    Ok(())
}

fn marker(value: &str) {
    println!("NATIVE_UPDATER_SMOKE {value}");
    std::io::stdout()
        .flush()
        .expect("flush updater smoke marker");
}
