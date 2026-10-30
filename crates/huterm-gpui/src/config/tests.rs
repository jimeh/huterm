use std::sync::atomic::{AtomicUsize, Ordering};

use huterm_config::{TabCloseButton, TabNotch, TabStyle, TabWidth};

use super::*;

static TEST_DIRECTORY_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

fn rgb(value: u32) -> huterm_protocol::Rgb {
    let [_, red, green, blue] = value.to_be_bytes();
    huterm_protocol::Rgb { red, green, blue }
}

#[test]
fn schema_fixtures_match_application_parsing_and_keymap_compilation() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../../../../schemas/fixtures.json"))
            .unwrap();
    let fixtures = fixtures.as_array().unwrap();
    assert_eq!(fixtures.len(), 227);
    for fixture in fixtures {
        let source = fixture["toml"].as_str().unwrap();
        let expected = fixture["valid"].as_bool().unwrap();
        let name = fixture["name"].as_str().unwrap();
        let result = if fixture["theme"].as_bool() == Some(true) {
            let directory = test_directory();
            fs::create_dir_all(directory.join("themes")).unwrap();
            fs::write(directory.join("themes/fixture.toml"), source).unwrap();
            let result = parse_at(
                "[theme]\nname = 'fixture'",
                &directory.join("config.toml"),
            );
            fs::remove_dir_all(directory).unwrap();
            result
        } else {
            parse(source)
        };
        let accepted = result.is_ok_and(|config| {
            crate::keymap::compile(
                crate::keymap::Platform::current(),
                &config.keybindings,
            )
            .is_ok_and(|local| {
                crate::quake::hotkeys::compile(
                    &config.global_keybindings,
                    &config.quake,
                    &local,
                )
                .is_ok()
            })
        });
        assert_eq!(accepted, expected, "{name}: {source}");
    }
}

fn parse(source: &str) -> Result<Config, ConfigError> {
    // Keep bundled-theme tests independent of the crate's themes directory.
    let directory = test_directory();
    fs::create_dir(&directory).expect("empty config directory");
    let result = parse_at(source, &directory.join("config.toml"));
    fs::remove_dir(directory).expect("remove empty config directory");
    result
}

#[test]
fn scrollback_rows_and_bytes_reach_the_terminal_limit() {
    let defaults = parse("").unwrap().terminal.scrollback;
    assert_eq!((defaults.rows(), defaults.bytes()), (10_000, None));
    let both = parse(
        "[terminal]\nscrollback_rows = 100000\nscrollback_bytes = '64MiB'",
    )
    .unwrap()
    .terminal
    .scrollback;
    assert_eq!((both.rows(), both.bytes()), (100_000, Some(64 << 20)));
}

#[test]
fn scrollback_compression_defaults_on_and_can_be_turned_off() {
    assert!(parse("").unwrap().terminal.scrollback_compression);
    assert!(
        !parse("[terminal]\nscrollback_compression = false")
            .unwrap()
            .terminal
            .scrollback_compression
    );
    assert!(parse("[terminal]\nscrollback_compression = 1").is_err());
}

#[test]
fn pane_zoom_defaults_to_the_view_and_can_be_shared() {
    use huterm_config::{PaneZoomScope, PanesConfig};
    assert_eq!(parse(DEFAULT_CONFIG).unwrap().panes, PanesConfig::default());
    assert_eq!(PanesConfig::default().zoom, PaneZoomScope::View);
    let shared =
        parse(&DEFAULT_CONFIG.replace("zoom = \"view\"", "zoom = \"shared\""))
            .unwrap();
    assert_eq!(shared.panes.zoom, PaneZoomScope::Shared);
    assert!(
        parse(&DEFAULT_CONFIG.replace("zoom = \"view\"", "zoom = \"tab\""))
            .is_err()
    );
}

#[test]
fn pane_title_bars_default_to_visible_at_the_top() {
    use huterm_config::{PaneTitleBar, PaneTitleBarPosition};
    let config = parse(DEFAULT_CONFIG).unwrap();
    assert_eq!(config.panes.title_bar, PaneTitleBar::Visible);
    assert_eq!(config.panes.title_bar_position, PaneTitleBarPosition::Top);
    let changed = parse(
        &DEFAULT_CONFIG
            .replace("title_bar = \"visible\"", "title_bar = \"hidden\"")
            .replace(
                "title_bar_position = \"top\"",
                "title_bar_position = \"bottom\"",
            ),
    )
    .unwrap();
    assert_eq!(changed.panes.title_bar, PaneTitleBar::Hidden);
    assert_eq!(
        changed.panes.title_bar_position,
        PaneTitleBarPosition::Bottom
    );
    for (known, unknown) in [
        ("title_bar = \"visible\"", "title_bar = \"hover\""),
        (
            "title_bar_position = \"top\"",
            "title_bar_position = \"left\"",
        ),
    ] {
        assert!(parse(&DEFAULT_CONFIG.replace(known, unknown)).is_err());
    }
}

#[test]
fn tab_settings_default_and_override_every_field() {
    let config = parse(DEFAULT_CONFIG).unwrap();
    assert_eq!(config.tabs, TabsConfig::default());
    let config = parse(
        &DEFAULT_CONFIG
            .replace("# position = \"titlebar\"", "position = \"bottom\"")
            .replace("always_show = false", "always_show = true")
            .replace(
                "auto_hide_in_fullscreen = false",
                "auto_hide_in_fullscreen = true",
            )
            .replace("style = \"pill\"", "style = \"strip\"")
            .replace("pill_accent = false", "pill_accent = true")
            .replace("close_button = \"active\"", "close_button = \"always\"")
            .replace("notch = \"left\"", "notch = \"right\"")
            .replace("width = \"fit\"", "width = \"fill\"")
            .replace("min_width = 96.0", "min_width = 120.0")
            .replace("max_width = 240.0", "max_width = 360.0")
            .replace("label = \"smart\"", "label = \"title\"")
            .replace("directory = \"name\"", "directory = \"path\""),
    )
    .unwrap();
    assert_eq!(
        config.tabs,
        TabsConfig {
            position: TabPosition::Bottom,
            always_show: true,
            auto_hide_in_fullscreen: true,
            style: TabStyle::Strip,
            pill_accent: true,
            close_button: TabCloseButton::Always,
            notch: TabNotch::Right,
            width: TabWidth::Fill,
            min_width: 120.0,
            max_width: 360.0,
            label: huterm_config::TabLabel::Title,
            directory: huterm_config::TabDirectory::Path,
        }
    );
}

#[test]
fn legacy_window_tab_settings_name_their_replacements() {
    for (legacy, error) in [
        (
            "tab_position = 'top'",
            "window.tab_position moved to tabs.position",
        ),
        (
            "always_show_tab_bar = true",
            "window.always_show_tab_bar moved to tabs.always_show",
        ),
        (
            "auto_hide_tab_bar_in_fullscreen = true",
            "window.auto_hide_tab_bar_in_fullscreen moved to tabs.auto_hide_in_fullscreen",
        ),
    ] {
        assert_eq!(
            parse(&format!("[window]\n{legacy}"))
                .unwrap_err()
                .to_string(),
            error
        );
    }
}

#[test]
fn update_settings_are_optional_and_preserve_explicit_values() {
    let defaults = parse("").unwrap().updates;
    assert_eq!(defaults.automatic_checks, None);
    assert_eq!(defaults.check_interval_hours, None);

    let configured =
        parse("[updates]\nautomatic_checks = false\ncheck_interval_hours = 6")
            .unwrap()
            .updates;
    assert_eq!(configured.automatic_checks, Some(false));
    assert_eq!(configured.check_interval_hours, Some(6));
}

#[test]
fn update_interval_rejects_values_below_sparkles_minimum() {
    let error = parse("[updates]\ncheck_interval_hours = 0")
        .unwrap_err()
        .to_string();
    assert_eq!(error, "updates.check_interval_hours must be at least 1");
}

#[test]
fn generated_default_keeps_update_overrides_commented_out() {
    let document = default_document();
    assert!(document.contains("# automatic_checks = true"));
    assert!(document.contains("# check_interval_hours = 24"));
    assert_eq!(parse(&document).unwrap().updates, UpdateConfig::default());
}

#[test]
fn fullscreen_mode_defaults_spelling_reload_and_startup_fallback() {
    assert_eq!(
        parse("").unwrap().window.macos_fullscreen_mode,
        MacosFullscreenMode::NonNative
    );
    for (value, expected) in [
        ("native", MacosFullscreenMode::Native),
        ("non_native", MacosFullscreenMode::NonNative),
    ] {
        assert_eq!(
            parse(&format!("[window]\nmacos_fullscreen_mode = '{value}'"))
                .unwrap()
                .window
                .macos_fullscreen_mode,
            expected
        );
    }
    for value in ["nonnative", "NON_NATIVE", "simple", ""] {
        assert!(
            parse(&format!("[window]\nmacos_fullscreen_mode = '{value}'"))
                .is_err()
        );
    }
    let directory = test_directory();
    fs::create_dir(&directory).unwrap();
    let path = directory.join("config.toml");
    fs::write(&path, "[window]\nmacos_fullscreen_mode = 'non_native'").unwrap();
    let working = reload(&path).config.unwrap();
    fs::write(&path, "[terminal]\nengine = 'ghostty'\n[window]\nmacos_fullscreen_mode = 'simple'").unwrap();
    assert!(reload(&path).config.is_err());
    assert_eq!(
        working.window.macos_fullscreen_mode,
        MacosFullscreenMode::NonNative
    );
    let fallback = load_path(path);
    assert!(!fallback.fatal);
    assert!(fallback.error.is_some());
    assert_eq!(fallback.config.warning, None);
    assert_eq!(
        fallback.config.window.macos_fullscreen_mode,
        MacosFullscreenMode::NonNative
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn option_policy_defaults_and_validation_are_explicit() {
    assert_eq!(
        parse("").unwrap().terminal.macos_option_as_alt,
        MacosOptionAsAlt::Off
    );
    for (value, expected) in [
        ("off", MacosOptionAsAlt::Off),
        ("both", MacosOptionAsAlt::Both),
    ] {
        assert_eq!(
            parse(&format!("[terminal]\nmacos_option_as_alt = '{value}'"))
                .unwrap()
                .terminal
                .macos_option_as_alt,
            expected
        );
    }
    for value in ["left", "right", "yes", "BOTH"] {
        let error =
            parse(&format!("[terminal]\nmacos_option_as_alt = '{value}'"))
                .unwrap_err()
                .to_string();
        assert!(error.contains("off") && error.contains("both"), "{error}");
    }
}

#[test]
fn option_policy_reload_rejects_invalid_values_without_fallback() {
    let directory = test_directory();
    fs::create_dir(&directory).unwrap();
    let path = directory.join("config.toml");
    fs::write(&path, "[terminal]\nmacos_option_as_alt = 'both'").unwrap();
    let working = reload(&path).config.unwrap();
    assert_eq!(working.terminal.macos_option_as_alt, MacosOptionAsAlt::Both);
    fs::write(&path, "[terminal]\nmacos_option_as_alt = 'left'").unwrap();
    assert!(reload(&path).config.is_err());
    assert_eq!(working.terminal.macos_option_as_alt, MacosOptionAsAlt::Both);
    fs::write(&path, "[terminal]\nmacos_option_as_alt = 'off'").unwrap();
    assert_eq!(
        reload(&path).config.unwrap().terminal.macos_option_as_alt,
        MacosOptionAsAlt::Off
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn clipboard_write_policy_reload_rejects_invalid_values_without_fallback() {
    let directory = test_directory();
    fs::create_dir(&directory).unwrap();
    let path = directory.join("config.toml");
    fs::write(&path, "[terminal]\nclipboard_write = 'deny'").unwrap();
    let working = reload(&path).config.unwrap();
    assert_eq!(working.terminal.clipboard_write, ClipboardWritePolicy::Deny);
    fs::write(&path, "[terminal]\nclipboard_write = 'prompt'").unwrap();
    assert!(reload(&path).config.is_err());
    assert_eq!(working.terminal.clipboard_write, ClipboardWritePolicy::Deny);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn terminal_close_on_exit_defaults_true_and_requires_a_boolean() {
    assert!(Config::default().terminal.close_on_exit);
    assert!(parse("").unwrap().terminal.close_on_exit);
    assert!(parse(DEFAULT_CONFIG).unwrap().terminal.close_on_exit);
    assert!(
        !parse("[terminal]\nclose_on_exit = false")
            .unwrap()
            .terminal
            .close_on_exit
    );
    for value in ["0", "\"false\"", "[]"] {
        assert!(
            parse(&format!("[terminal]\nclose_on_exit = {value}")).is_err()
        );
    }
    assert!(parse("[terminal]\nclose_on_exiit = false").is_err());
    for name in ["alacritty", "ghostty"] {
        let config = parse(&format!(
            "[terminal]\nengine = '{name}'\nclose_on_exit = false"
        ))
        .unwrap();
        assert!(!config.terminal.close_on_exit);
    }
}

#[test]
fn terminal_identity_defaults_to_auto_and_accepts_only_supported_values() {
    use huterm_config::TerminalIdentity;

    assert_eq!(Config::default().terminal.term, TerminalIdentity::Auto);
    for (value, expected) in [
        ("auto", TerminalIdentity::Auto),
        ("xterm-huterm", TerminalIdentity::XtermHuterm),
        ("xterm-256color", TerminalIdentity::Xterm256Color),
    ] {
        assert_eq!(
            parse(&format!("[terminal]\nterm = '{value}'"))
                .unwrap()
                .terminal
                .term,
            expected
        );
    }
    for value in ["huterm", "xterm", "AUTO"] {
        assert!(parse(&format!("[terminal]\nterm = '{value}'")).is_err());
    }
}

#[test]
fn legacy_engine_values_select_ghostty_and_only_alacritty_warns() {
    assert_eq!(parse("").unwrap().warning, None);
    assert_eq!(
        parse("[terminal]\nengine = 'ghostty'").unwrap().warning,
        None
    );
    assert_eq!(
        parse("[terminal]\nengine = 'alacritty'").unwrap().warning,
        Some(LEGACY_ALACRITTY_WARNING.to_owned())
    );
    for source in ["[terminal]\nengine = 'unknown'", "[terminal]\nengine = 12"]
    {
        assert!(parse(source).is_err(), "{source}");
    }
}

#[test]
fn config_fallback_validates_legacy_engine_and_preserves_clipboard_policy() {
    let directory = test_directory();
    fs::create_dir(&directory).unwrap();
    let file = directory.join("config.toml");
    for source in [
        "[terminal]\nengine = 'unknown'",
        "[terminal]\nengine = 12",
        "[terminal]\nclipboard_write = 'prompt'",
        "[terminal]\nclipboard_write = 1",
        "[terminal]\nengine = 'unknown'\n[font]\nsize =",
        "[terminal]\nclipboard_write = 'prompt'\n[font]\nsize = 'bad'",
        "[terminal]\nclipboard_write = 1\n[font]\nsize = 'bad'",
        "[terminal]\nengine = 'ghostty'\n[font]\nsize =",
        "[font]\nsize =",
    ] {
        fs::write(&file, source).unwrap();
        let loaded = load_path(file.clone());
        assert!(loaded.fatal, "{source}");
        assert!(loaded.error.is_some());
    }
    for (source, warning, expected_clipboard) in [
        (
            "[terminal]\nengine = 'alacritty'\n[font]\nsize = 'bad'",
            Some(LEGACY_ALACRITTY_WARNING),
            ClipboardWritePolicy::Allow,
        ),
        (
            "[terminal]\nengine = 'alacritty'\nclipboard_write = 'deny'\n[font]\nsize = 'bad'",
            Some(LEGACY_ALACRITTY_WARNING),
            ClipboardWritePolicy::Deny,
        ),
        (
            "[terminal]\nengine = 'ghostty'\n[font]\nsize = 'bad'",
            None,
            ClipboardWritePolicy::Allow,
        ),
        (
            "[terminal]\nengine = 'ghostty'\nclipboard_write = 'deny'\n[font]\nsize = 'bad'",
            None,
            ClipboardWritePolicy::Deny,
        ),
        (
            "[terminal]\nclipboard_write = 'deny'\n[font]\nsize = 'bad'",
            None,
            ClipboardWritePolicy::Deny,
        ),
        ("[font]\nsize = 'bad'", None, ClipboardWritePolicy::Allow),
    ] {
        fs::write(&file, source).unwrap();
        let loaded = load_path(file.clone());
        assert!(!loaded.fatal, "{source}");
        assert!(loaded.error.is_some());
        assert_eq!(loaded.warning.as_deref(), warning);
        assert_eq!(loaded.config.warning.as_deref(), warning);
        assert_eq!(loaded.config.terminal.clipboard_write, expected_clipboard);
        assert_eq!(loaded.config.font, Config::default().font);
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn tab_placement_accepts_four_edges_and_rejects_other_values() {
    for (name, expected) in [
        ("top", TabPosition::Top),
        ("bottom", TabPosition::Bottom),
        ("left", TabPosition::Left),
        ("right", TabPosition::Right),
        ("titlebar", TabPosition::Titlebar),
    ] {
        let config = parse(&DEFAULT_CONFIG.replace(
            "# position = \"titlebar\"",
            &format!("position = \"{name}\""),
        ))
        .unwrap();
        assert_eq!(config.tabs.position, expected);
    }
    assert!(
        parse(
            &DEFAULT_CONFIG
                .replace("# position = \"titlebar\"", "position = \"middle\"")
        )
        .is_err()
    );
}

#[test]
fn named_themes_layer_individual_colors_and_legacy_ansi() {
    let theme = parse(
        r##"
[theme]
name = "mine"
ansi_red = "#010203"
[themes.mine]
extends = "catppuccin-mocha"
background = "#040506"
"##,
    )
    .unwrap()
    .theme;
    assert_eq!(theme.background, rgb(0x0004_0506));
    assert_eq!(theme.ansi[1], rgb(0x0001_0203));
    assert_eq!(theme.ansi[2], rgb(0x00a6_e3a1));
    assert_eq!(theme.selection_foreground, Some(rgb(0x001e_1e2e)));
    let legacy = format!(
        "[theme]\nansi = [{}]\nansi_red = '#abcdef'",
        ["'#123456'"; 16].join(",")
    );
    let theme = parse(&legacy).unwrap().theme;
    assert_eq!(theme.ansi[0], rgb(0x0012_3456));
    assert_eq!(theme.ansi[1], rgb(0x00ab_cdef));
    assert!(parse("[theme]\nansi = ['#123456']").is_err());
}

#[test]
fn every_builtin_resolves_and_default_stays_unchanged() {
    assert_eq!(parse("").unwrap(), Config::default());
    assert_eq!(
        parse("[theme]\nname = 'huterm-dark'").unwrap().theme,
        Theme::huterm_dark()
    );
    // No `[theme]`, or an empty one, is huterm-dark with its hand-picked
    // chrome, so a fresh install matches the named theme.
    assert_eq!(parse("[theme]").unwrap().theme, Theme::huterm_dark());
    // Setting a key, even to its default value, is a real definition
    // that derives its chrome rather than taking the hand-picked set.
    let same = parse("[theme]\nbackground = '#1d1f21'").unwrap().theme;
    assert_eq!(same.background, Theme::default().background);
    assert_eq!(same.tab_bar_background, None);
    // An empty named definition follows the same rule.
    assert_eq!(
        parse("[theme]\nname = 'bare'\n[themes.bare]")
            .unwrap()
            .theme,
        Theme::huterm_dark()
    );
    // An unnamed theme derives its chrome from its own colors rather
    // than inheriting huterm-dark's hand-picked dark set.
    let light =
        parse("[theme]\nbackground = '#ffffff'\nforeground = '#000000'")
            .unwrap()
            .theme;
    assert_eq!(light.tab_bar_background, None);
    assert_ne!(
        light.ui().tab_bar_background,
        Theme::huterm_dark().ui().tab_bar_background
    );
    for name in [
        "tokyo-night",
        "tokyo-night-storm",
        "tokyo-night-moon",
        "tokyo-night-day",
        "catppuccin-latte",
        "catppuccin-frappe",
        "catppuccin-macchiato",
        "catppuccin-mocha",
        "one-dark-pro",
        "tomorrow-night",
        "dracula",
        "nord",
        "tango-with-monokai",
    ] {
        let config = parse(&format!("[theme]\nname = '{name}'")).unwrap();
        assert_ne!(config.theme.background, config.theme.foreground, "{name}");
    }
    assert_eq!(
        parse("[theme]\nname = 'tango-with-monokai'")
            .unwrap()
            .theme
            .ansi[1],
        rgb(0x00cf_5041)
    );
}

#[test]
fn one_dark_pro_uses_its_terminal_palette_and_opaque_selection() {
    let theme = parse("[theme]\nname = 'one-dark-pro'").unwrap().theme;
    assert_eq!(theme.background, rgb(0x0028_2c34));
    assert_eq!(theme.foreground, rgb(0x00ab_b2bf));
    assert_eq!(theme.cursor, rgb(0x0052_8bff));
    assert_eq!(theme.selection, rgb(0x0041_454e));
    assert_eq!(theme.ansi[1], rgb(0x00e0_5561));
    assert_eq!(theme.ansi[9], rgb(0x00ff_616e));
    assert_eq!(theme.selection_foreground, None);
}

#[test]
fn tomorrow_night_matches_upstream_iterm_without_changing_huterm_dark() {
    let theme = parse("[theme]\nname = 'tomorrow-night'").unwrap().theme;
    assert_eq!(theme.background, rgb(0x001d_1f21));
    assert_eq!(theme.foreground, rgb(0x00c5_c8c6));
    assert_eq!(theme.cursor, theme.foreground);
    assert_eq!(theme.selection, rgb(0x0037_3b41));
    assert_eq!(theme.selection_foreground, Some(theme.foreground));
    let ansi = [
        0x0000_0000,
        0x00cc_6666,
        0x00b5_bd68,
        0x00f0_c674,
        0x0081_a2be,
        0x00b2_94bb,
        0x008a_beb7,
        0x00ff_ffff,
    ]
    .map(rgb);
    assert_eq!(theme.ansi[..8], ansi);
    assert_eq!(theme.ansi[8..], ansi);
    let default = parse("[theme]\nname = 'huterm-dark'").unwrap().theme;
    assert_eq!(default, Theme::huterm_dark());
    assert_ne!(theme, default);
}

#[test]
fn themes_reject_cycles_unknown_keys_colors_and_path_names() {
    for source in [
        "[theme]\nname = '../escape'",
        "[theme]\nname = 'absent'",
        "[theme]\nansi_pink = '#123456'",
        "[theme]\nansi_red = '#12345z'",
        "[theme]\nextends = 'nord'",
        "[theme]\nname = 'a'\n[themes.a]\nextends = 'b'\n[themes.b]\nextends = 'a'",
        "[themes.unused]\nansi_red = 'invalid'",
        "[themes.unused]\nname = 'nord'",
    ] {
        assert!(parse(source).is_err(), "{source}");
    }
}

#[test]
fn theme_lookup_prefers_inline_then_adjacent_file_then_builtin() {
    let directory = test_directory();
    fs::create_dir_all(directory.join("themes")).unwrap();
    let path = directory.join("config.toml");
    fs::write(
        directory.join("themes/nord.toml"),
        "[theme]\nbackground = '#010203'",
    )
    .unwrap();
    let source = "[theme]\nname = 'nord'";
    assert_eq!(
        parse_at(source, &path).unwrap().theme.background,
        rgb(0x0001_0203)
    );
    let inline = format!("{source}\n[themes.nord]\nforeground = '#040506'");
    let theme = parse_at(&inline, &path).unwrap().theme;
    assert_eq!(theme.foreground, rgb(0x0004_0506));
    assert_eq!(theme.background, Theme::default().background);
    fs::write(
        directory.join("themes/nord.toml"),
        "[theme]\nansi_red = 'bad'",
    )
    .unwrap();
    assert!(parse_at(source, &path).is_err());
    fs::remove_file(directory.join("themes/nord.toml")).unwrap();
    assert_eq!(
        parse_at(source, &path).unwrap().theme.background,
        rgb(0x002e_3440)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn reload_rereads_theme_files_and_never_falls_back_on_errors() {
    let directory = test_directory();
    fs::create_dir_all(directory.join("themes")).unwrap();
    let path = directory.join("config.toml");
    let theme_path = directory.join("themes/custom.toml");
    fs::write(&path, "[theme]\nname = 'custom'").unwrap();
    fs::write(&theme_path, "[theme]\nextends = 'nord'").unwrap();
    let first = reload(&path).config.unwrap();
    fs::write(&theme_path, "[theme]\nextends = 'dracula'").unwrap();
    assert_ne!(reload(&path).config.unwrap().theme, first.theme);
    fs::write(&theme_path, "[theme]\nforeground = 'bad'").unwrap();
    assert!(reload(&path).config.is_err());
    fs::remove_file(&path).unwrap();
    assert!(reload(&path).config.is_err());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn default_document_should_parse() {
    let parsed = parse(&default_document()).expect("default should parse");
    assert_eq!(parsed.font.family, Config::default().font.family);
    assert_eq!(parsed.window, WindowConfig::default());
    assert_eq!(parsed.tabs, TabsConfig::default());
}

#[test]
fn window_settings_are_optional_and_individually_defaulted() {
    let legacy = DEFAULT_CONFIG.split("[window]").next().unwrap().to_owned()
        + "[theme]"
        + DEFAULT_CONFIG.split("[theme]").nth(1).unwrap();
    assert_eq!(parse(&legacy).unwrap().window, WindowConfig::default());
    let balanced = parse(&(legacy + "\n[window]\npadding_balance = true\n"))
        .unwrap()
        .window;
    assert_eq!(
        balanced,
        WindowConfig {
            padding_balance: true,
            ..WindowConfig::default()
        }
    );
}

#[test]
fn window_padding_rejects_invalid_values_and_accepts_zero() {
    for key in ["padding_x", "padding_y"] {
        for value in ["-1.0", "257.0", "nan", "inf"] {
            let source = DEFAULT_CONFIG
                .replace(&format!("{key} = 4.0"), &format!("{key} = {value}"));
            assert!(parse(&source).is_err(), "accepted {key} = {value}");
        }
    }
    let config = parse(&DEFAULT_CONFIG.replace("= 4.0", "= 0.0")).unwrap();
    assert_eq!(
        config.window,
        WindowConfig {
            padding_x: 0.0,
            padding_y: 0.0,
            padding_balance: false,
            macos_fullscreen_mode: MacosFullscreenMode::NonNative,
            menu_button: true,
            shortcut_hints: false,
            macos_native_menus: true,
        }
    );
}

#[test]
fn keybinding_entries_are_structurally_validated() {
    let source = format!(
        "{DEFAULT_CONFIG}\n[[keybinding]]\nkey = \"cmd-1\"\ncommand = \
         \"select_tab\"\nargs = {{ index = 1 }}\nwhen = \"Terminal\"\n\
         description = \"First tab\"\n[[keybinding]]\nkey = \"cmd-w\"\n\
         command = \"unbind\"\n"
    );
    let config = parse(&source).unwrap();
    assert_eq!(config.keybindings.len(), 2);
    let first = &config.keybindings[0];
    assert_eq!(first.key, "cmd-1");
    assert_eq!(first.command, "select_tab");
    assert_eq!(
        first.args.as_ref().and_then(|args| args.get("index")),
        Some(&toml::Value::Integer(1))
    );
    assert_eq!(first.when.as_deref(), Some("Terminal"));
    assert_eq!(first.description.as_deref(), Some("First tab"));
    assert_eq!(config.keybindings[1].args, None);
    assert_eq!(parse(DEFAULT_CONFIG).unwrap().keybindings, []);

    let entry = |body: &str| {
        format!("{DEFAULT_CONFIG}\n[[keybinding]]\nkey = \"cmd-1\"\n{body}\n")
    };
    let error = parse(&entry("command = \"select_tab\"\nargs = [1]"))
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("keybinding 1 (\"cmd-1\"): "), "{error}");
    assert!(error.contains("named arguments index"), "{error}");
    let error = parse(&entry("command = \"select_tab\"\nargs = 1"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("not integer"), "{error}");
    let error = parse(&entry("command = \"select_tab\"\ndescription = \"  \""))
        .unwrap_err()
        .to_string();
    assert!(error.contains("description must not be blank"), "{error}");
    assert!(parse(&entry("command = \"select_tab\"\nkeys = \"x\"")).is_err());
}

#[test]
fn parser_should_reject_unknown_and_invalid_values() {
    assert!(
        parse(&DEFAULT_CONFIG.replace("size = 14.0", "size = 5.0")).is_err()
    );
    assert!(
        parse(&DEFAULT_CONFIG.replace("[font]", "[font]\nextra = true"))
            .is_err()
    );
    assert!(parse("[theme]\nforeground = 'red'").is_err());
}

#[test]
fn config_path_prefers_explicit_override_then_xdg() {
    let override_path = config_path_from(|name| match name {
        "HUTERM_CONFIG_FILE" => Some("/tmp/huterm-test.toml".into()),
        "XDG_CONFIG_HOME" => Some("/tmp/ignored".into()),
        "HOME" => Some("/tmp/home".into()),
        _ => None,
    });
    let xdg_path = config_path_from(|name| match name {
        "XDG_CONFIG_HOME" => Some("/tmp/xdg".into()),
        "HOME" => Some("/tmp/home".into()),
        _ => None,
    });

    assert_eq!(override_path, PathBuf::from("/tmp/huterm-test.toml"));
    assert_eq!(xdg_path, PathBuf::from("/tmp/xdg/huterm/config.toml"));
}

#[test]
fn loading_missing_and_malformed_files_falls_back_with_context() {
    let directory = test_directory();
    let path = directory.join("config.toml");
    let missing = load_path(path.clone());
    assert_eq!(missing.config, Config::default());
    assert!(missing.error.is_none());

    fs::create_dir_all(&directory).expect("test directory should exist");
    fs::write(&path, "not valid toml = [")
        .expect("malformed config should be written");
    let malformed = load_path(path.clone());
    assert_eq!(malformed.config, Config::default());
    assert!(malformed.error.is_some_and(|error| {
        error.contains(path.to_string_lossy().as_ref())
    }));
    fs::remove_dir_all(directory).expect("test directory should clean up");
}

#[test]
fn default_creation_never_overwrites_an_existing_file() {
    let directory = test_directory();
    let path = directory.join("nested/config.toml");
    create_default(&path).expect("default config should be created");
    assert_eq!(
        fs::read_to_string(&path).expect("default config should be readable"),
        default_document()
    );

    fs::write(&path, "user-owned = true\n")
        .expect("test config should be replaceable");
    create_default(&path).expect("existing config should remain usable");
    assert_eq!(
        fs::read_to_string(&path).expect("existing config should be readable"),
        "user-owned = true\n"
    );
    fs::remove_dir_all(directory).expect("test directory should clean up");
}

#[test]
fn indexed_palette_should_cover_cube_and_grayscale() {
    let theme = Theme::default();
    assert_eq!(theme.indexed(16), rgb(0x0000_0000));
    assert_eq!(theme.indexed(231), rgb(0x00ff_ffff));
    assert_eq!(theme.indexed(232), rgb(0x0008_0808));
    assert_eq!(theme.indexed(255), rgb(0x00ee_eeee));
}

#[test]
fn watch_config_defaults_on_parses_and_is_read_leniently_on_a_nonfatal_error() {
    assert!(Config::default().general.watch_config);
    assert!(parse("").unwrap().general.watch_config);
    assert!(parse(DEFAULT_CONFIG).unwrap().general.watch_config);
    assert!(DEFAULT_CONFIG.contains("[general]\n# Reload this file automatically when it changes on disk.\nwatch_config = true\n"));
    assert!(
        !parse("[general]\nwatch_config = false")
            .unwrap()
            .general
            .watch_config
    );
    assert!(parse("[general]\nwatch_config = 'no'").is_err());
    assert!(parse("[general]\nwatch = true").is_err());
    let directory = test_directory();
    fs::create_dir(&directory).unwrap();
    let path = directory.join("config.toml");
    // A value error elsewhere falls back to defaults but keeps the choice,
    // so a fix saved later still reloads, or still does not.
    for (general, expected) in [
        ("[general]\nwatch_config = false\n", false),
        ("[general]\nwatch_config = true\n", true),
        ("", true),
        ("[general]\nwatch_config = 'no'\n", true),
    ] {
        fs::write(&path, format!("{general}[font]\nsize = 1000.0\n")).unwrap();
        let loaded = load_path(path.clone());
        assert!(!loaded.fatal && loaded.error.is_some(), "{general}");
        assert_eq!(loaded.config.general.watch_config, expected, "{general}");
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn input_fingerprints_follow_the_config_and_every_theme_file_it_consumed() {
    let directory = test_directory();
    let themes = directory.join("themes");
    fs::create_dir_all(&themes).unwrap();
    let path = directory.join("config.toml");
    fs::write(&path, "[theme]\nname = 'child'\n").unwrap();
    fs::write(
        themes.join("child.toml"),
        "[theme]\nextends = 'parent'\nbackground = '#101010'\n",
    )
    .unwrap();
    fs::write(themes.join("parent.toml"), "[theme]\nextends = 'dracula'\n")
        .unwrap();
    let loaded = load_path(path.clone());
    assert!(loaded.error.is_none(), "{:?}", loaded.error);
    // Inheritance probes the parent and, before the bundled theme, a file
    // that does not exist.
    assert_eq!(
        loaded.inputs.themes,
        ["child", "parent", "dracula"]
            .map(|name| themes.join(format!("{name}.toml")))
    );
    assert_eq!(reload(&path).inputs, loaded.inputs);
    let current = || fingerprint(&path, &loaded.inputs.themes).unwrap();
    assert_eq!(current(), loaded.inputs.fingerprint);
    for (file, edit) in [
        (path.clone(), "[theme]\nname = 'child'\n# edited\n"),
        (
            themes.join("child.toml"),
            "[theme]\nextends = 'parent'\nbackground = '#202020'\n",
        ),
        (
            themes.join("parent.toml"),
            "[theme]\nextends = 'dracula' # x\n",
        ),
        (
            themes.join("dracula.toml"),
            "[theme]\nbackground = '#000000'\n",
        ),
    ] {
        let before = fs::read(&file).ok();
        fs::write(&file, edit).unwrap();
        assert_ne!(current(), loaded.inputs.fingerprint, "{}", file.display());
        match before {
            Some(bytes) => fs::write(&file, bytes).unwrap(),
            None => fs::remove_file(&file).unwrap(),
        }
        assert_eq!(current(), loaded.inputs.fingerprint, "{}", file.display());
    }
    // A missing configuration is an editor's save in progress.
    fs::remove_file(&path).unwrap();
    assert_eq!(fingerprint(&path, &loaded.inputs.themes), None);
    let missing = reload(&path);
    assert!(missing.missing && missing.config.is_err());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn the_validator_guards_patch_service_writes_without_side_effects() {
    use huterm_config::document::{ApplyError, ConfigFile, Patch};
    let directory = test_directory();
    fs::create_dir(&directory).unwrap();
    let path = directory.join("config.toml");
    fs::write(&path, DEFAULT_CONFIG).unwrap();
    let validate = validator(path.clone());
    validate(DEFAULT_CONFIG).expect("the template is valid");
    for invalid in [
        "[[keybinding]]\nkey = 'cmd-h'\ncommand = 'unbind'\ndisabled = true",
        "[[keybinding]]\nkey = 'cmd-h'\ncommand = 'hyde'",
        "[theme]\nname = 'missing-theme'",
        "[tabs]\nmin_width = 300\nmax_width = 200",
    ] {
        assert!(validate(invalid).is_err(), "{invalid}");
    }
    let file = ConfigFile::new(&path);
    let base = file.load().unwrap();
    let error = file
        .apply(
            &base,
            &[Patch::Set {
                path: vec!["theme".into(), "name".into()],
                value: "missing-theme".into(),
            }],
            &validate,
        )
        .unwrap_err();
    assert!(matches!(error, ApplyError::Rejected(_)), "{error}");
    assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);
    file.apply(
        &base,
        &[Patch::Set {
            path: vec!["font".into(), "size".into()],
            value: toml::Value::Float(16.0),
        }],
        &validate,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        DEFAULT_CONFIG.replace("size = 14.0", "size = 16.0")
    );
    fs::remove_dir_all(directory).unwrap();
}

fn test_directory() -> PathBuf {
    let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "huterm-config-test-{}-{sequence}",
        std::process::id()
    ))
}
