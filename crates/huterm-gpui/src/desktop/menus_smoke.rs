//! Real menu assertions use the same installation function as startup/reload.

use std::io::Write as _;

use super::*;
use crate::config::KeybindingEntry;
use crate::native_quit::{MenuShortcut, menu_shortcut};

const COMMAND_MODIFIER: usize = 1 << 20;

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
            binding("cmd-j", "open_command_palette"),
        ],
        ..Config::default()
    };
    let loaded = config::LoadedConfig {
        config: config.clone(),
        path: "menu-smoke.toml".into(),
        error: None,
        fatal: false,
    };
    let (installed, error) = windows::install_startup_keymap(cx, &loaded);
    anyhow::ensure!(error.is_none(), "startup keymap: {error:?}");
    unbound_menu_item("Check for Updates...")?;
    marker("startup-update-command");
    shortcut("Reload Configuration", "r")?;
    marker("startup-user-shortcut");
    shortcut("New Tab", "t")?;
    marker("untouched-default-shortcut");
    shortcut("Toggle Fullscreen", "\r")?;
    shortcut("Next Tab", "\t")?;
    shortcut("Open Command Palette", "j")?;
    anyhow::ensure!(
        installed
            .shortcuts(ids::OPEN_COMMAND_PALETTE, &[], None)
            .iter()
            .any(|binding| binding.key == "cmd-j"),
        "startup effective palette binding missing"
    );
    marker("startup-palette-shortcut");
    marker("startup-special-shortcuts");

    config.keybindings[1].key = "cmd-y".into();
    config.keybindings[2].key = "cmd-f".into();
    config.keybindings[3].key = "cmd-enter".into();
    config.keybindings[5].key = "cmd-k".into();
    let compiled = keymap::compile(Platform::MacOs, &config.keybindings)?;
    // Reload invokes this same function after configuration validation.
    let installed = bind_keymap(cx, compiled);
    unbound_menu_item("Check for Updates...")?;
    marker("reloaded-update-command");
    shortcut("Reload Configuration", "y")?;
    marker("reloaded-user-shortcut");
    shortcut("Toggle Fullscreen", "f")?;
    shortcut("Next Tab", "\r")?;
    shortcut("Open Command Palette", "k")?;
    anyhow::ensure!(
        installed
            .shortcuts(ids::OPEN_COMMAND_PALETTE, &[], None)
            .iter()
            .any(|binding| binding.key == "cmd-k"),
        "reloaded effective palette binding missing"
    );
    marker("reloaded-palette-shortcut");
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
        modifiers: COMMAND_MODIFIER,
    };
    anyhow::ensure!(
        actual == expected,
        "{title}: expected {expected:?}, got {actual:?}"
    );
    Ok(())
}

fn unbound_menu_item(title: &str) -> anyhow::Result<()> {
    let actual = menu_shortcut(title)?;
    let expected = MenuShortcut {
        key: String::new(),
        // AppKit retains GPUI's command mask even with no key equivalent.
        // The empty key makes the menu item unbound.
        modifiers: COMMAND_MODIFIER,
    };
    anyhow::ensure!(
        actual == expected,
        "{title}: expected {expected:?}, got {actual:?}"
    );
    Ok(())
}

fn marker(value: &str) {
    println!("NATIVE_MENUS_SMOKE {value}");
    std::io::stdout().flush().expect("flush menu smoke marker");
}
