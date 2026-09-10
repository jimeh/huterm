//! Real menu assertions use the same installation function as startup/reload.

use std::io::Write as _;

use super::*;
use crate::config::KeybindingEntry;
use crate::native_quit::{MenuShortcut, menu_shortcut};

pub(crate) fn run() {
    Application::new().run(|cx| {
        if let Err(error) = check(cx) {
            eprintln!("NATIVE_MENUS_SMOKE failed: {error:#}");
            std::process::exit(1);
        }
        cx.quit();
    });
}

fn check(cx: &mut App) -> anyhow::Result<()> {
    bind_keymap(cx, keymap::compile(Platform::MacOs, &[])?);
    shortcut("Toggle Fullscreen", "\r")?;
    marker("default-fullscreen-shortcut");
    let mut config = Config {
        keybindings: vec![
            KeybindingEntry {
                key: "cmd-w".into(),
                // The menu must keep New Tab's default ahead of this conditional
                // binding, while dispatch gives the conditional binding priority.
                command: "new_tab".into(),
                when: Some("!confirming".into()),
                args: None,
                description: None,
            },
            KeybindingEntry {
                key: "cmd-r".into(),
                command: "reload_config".into(),
                when: None,
                args: None,
                description: None,
            },
            binding("cmd-enter", "toggle_fullscreen"),
            binding("cmd-tab", "next_tab"),
            binding("cmd-e", "unbind"),
        ],
        ..Config::default()
    };
    let loaded = config::LoadedConfig {
        config: config.clone(),
        path: "menu-smoke.toml".into(),
        error: None,
        fatal: false,
    };
    let (_, error) = windows::install_startup_keymap(cx, &loaded);
    anyhow::ensure!(error.is_none(), "startup keymap: {error:?}");
    unbound_menu_item("Check for Updates...")?;
    marker("startup-update-command");
    shortcut("Reload Configuration", "r")?;
    marker("startup-user-shortcut");
    shortcut("New Tab", "t")?;
    marker("untouched-default-shortcut");
    shortcut("Toggle Fullscreen", "\r")?;
    shortcut("Next Tab", "\t")?;
    marker("startup-special-shortcuts");

    config.keybindings[1].key = "cmd-y".into();
    config.keybindings[2].key = "cmd-f".into();
    config.keybindings[3].key = "cmd-enter".into();
    let compiled = keymap::compile(Platform::MacOs, &config.keybindings)?;
    // Reload invokes this same function after configuration validation.
    bind_keymap(cx, compiled);
    unbound_menu_item("Check for Updates...")?;
    marker("reloaded-update-command");
    shortcut("Reload Configuration", "y")?;
    marker("reloaded-user-shortcut");
    shortcut("Toggle Fullscreen", "f")?;
    shortcut("Next Tab", "\r")?;
    marker("reloaded-special-shortcuts");
    Ok(())
}

fn binding(key: &str, command: &str) -> KeybindingEntry {
    KeybindingEntry {
        key: key.into(),
        command: command.into(),
        when: None,
        args: None,
        description: None,
    }
}

fn shortcut(title: &str, key: &str) -> anyhow::Result<()> {
    let actual = menu_shortcut(title)?;
    let expected = MenuShortcut {
        key: key.into(),
        modifiers: 1 << 20,
    };
    anyhow::ensure!(
        actual == expected,
        "{title}: expected {expected:?}, got {actual:?}"
    );
    Ok(())
}

fn unbound_menu_item(title: &str) -> anyhow::Result<()> {
    let actual = menu_shortcut(title)?;
    anyhow::ensure!(
        actual.key.is_empty() && actual.modifiers == 0,
        "{title}: expected no shortcut, got {actual:?}"
    );
    Ok(())
}

fn marker(value: &str) {
    println!("NATIVE_MENUS_SMOKE {value}");
    std::io::stdout().flush().expect("flush menu smoke marker");
}
