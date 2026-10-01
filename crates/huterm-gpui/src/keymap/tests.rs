use super::*;
use crate::commands::invoke;
use gpui::{KeyContext, Keymap};

fn entry(key: &str, command: &str) -> KeybindingEntry {
    KeybindingEntry {
        key: key.into(),
        command: command.into(),
        args: None,
        when: None,
        description: None,
    }
}

fn with_args(
    mut entry: KeybindingEntry,
    args: &[(&str, toml::Value)],
) -> KeybindingEntry {
    entry.args = Some(
        args.iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect(),
    );
    entry
}

fn keystroke(source: &str) -> Keystroke {
    Keystroke::parse(source).unwrap()
}

fn command_of(binding: &KeyBinding) -> CommandId {
    bound_command(binding).expect("binding carries a catalog action")
}

#[test]
fn defaults_compile_on_both_platforms_with_descriptions() {
    for platform in [Platform::MacOs, Platform::Linux] {
        let compiled = compile(platform, &[]).unwrap();
        let defaults = defaults(platform);
        assert_eq!(compiled.effective.len(), defaults.len());
        assert!(compiled.conflicts.is_empty());
        for (entry, effective) in defaults.iter().zip(&compiled.effective) {
            assert!(
                entry
                    .description
                    .as_deref()
                    .is_some_and(|d| !d.trim().is_empty()),
                "{}: default without description",
                entry.key
            );
            assert_eq!(effective.origin, Origin::Default);
            assert_eq!(effective.key, entry.key);
        }
        assert!(compiled.reserved.is_reserved(&keystroke("shift-pageup")));
        assert!(!compiled.reserved.is_reserved(&keystroke("ctrl-c")));
        let palette = compiled
            .effective
            .iter()
            .find(|binding| binding.command == ids::OPEN_COMMAND_PALETTE)
            .expect("platform default opens the command palette");
        assert_eq!(
            palette.key,
            match platform {
                Platform::MacOs => "cmd-shift-p",
                Platform::Linux => "ctrl-shift-p",
            }
        );
        let palette_key = palette.key.clone();
        let keymap = Keymap::new(compiled.bindings);
        let contexts = [KeyContext::parse("Workspace").unwrap()];
        let (bindings, pending) =
            keymap.bindings_for_input(&[keystroke(&palette_key)], &contexts);
        assert!(!pending);
        assert_eq!(
            bindings.first().map(command_of),
            Some(ids::OPEN_COMMAND_PALETTE)
        );
    }
    let macos = compile(Platform::MacOs, &[]).unwrap().reserved;
    for chord in [
        "cmd-c",
        "cmd-v",
        "cmd-h",
        "cmd-alt-h",
        "cmd-enter",
        "cmd-<",
        "cmd-9",
        "cmd-shift-w",
        "ctrl-tab",
    ] {
        assert!(macos.is_reserved(&keystroke(chord)), "{chord}");
    }
    for chord in ["cmd-alt-c", "ctrl-shift-c", "alt-9"] {
        assert!(!macos.is_reserved(&keystroke(chord)), "{chord}");
    }
    let linux = compile(Platform::Linux, &[]).unwrap().reserved;
    for chord in [
        "ctrl-shift-c",
        "ctrl-shift-v",
        "ctrl-<",
        "alt-9",
        "ctrl-shift-q",
        "ctrl-shift-tab",
    ] {
        assert!(linux.is_reserved(&keystroke(chord)), "{chord}");
    }
    for chord in ["ctrl-shift-alt-c", "cmd-c", "cmd-9"] {
        assert!(!linux.is_reserved(&keystroke(chord)), "{chord}");
    }
    let mut with_function = keystroke("cmd-c");
    with_function.modifiers.function = true;
    assert!(!macos.is_reserved(&with_function));
}

#[test]
fn descriptions_resolve_from_title_and_arguments() {
    let user = [with_args(
        entry("cmd-3", "select_tab"),
        &[("index", toml::Value::Integer(3))],
    )];
    let compiled = compile(Platform::MacOs, &user).unwrap();
    let effective = compiled.effective.last().unwrap();
    assert_eq!(effective.description, "Select Tab (index: 3)");
    assert_eq!(effective.origin, Origin::User);
    assert_eq!(
        compile(Platform::MacOs, &[entry("cmd-t", "new_tab")])
            .unwrap()
            .effective
            .last()
            .unwrap()
            .description,
        "New Tab"
    );
}

#[test]
fn override_replaces_default_and_unbind_removes_all_variants() {
    let user = [
        entry("cmd-t", "new_window"),
        entry("cmd-c", "unbind"),
        KeybindingEntry {
            when: Some("Terminal".into()),
            ..entry("cmd-v", "copy")
        },
        entry("cmd-v", "unbind"),
    ];
    let compiled = compile(Platform::MacOs, &user).unwrap();
    assert!(compiled.conflicts.is_empty());
    let for_key = |key: &str| {
        compiled
            .effective
            .iter()
            .filter(|binding| binding.key == key)
            .collect::<Vec<_>>()
    };
    let new_tab_keys = for_key("cmd-t");
    assert_eq!(new_tab_keys.len(), 1);
    assert_eq!(new_tab_keys[0].command, ids::NEW_WINDOW);
    assert_eq!(new_tab_keys[0].origin, Origin::User);
    assert!(for_key("cmd-c").is_empty());
    assert!(for_key("cmd-v").is_empty());
    assert!(!compiled.reserved.is_reserved(&keystroke("cmd-c")));
    assert!(!compiled.reserved.is_reserved(&keystroke("cmd-v")));
    assert!(compiled.reserved.is_reserved(&keystroke("cmd-t")));
    let bound = compiled
        .bindings
        .iter()
        .filter(|binding| {
            binding.match_keystrokes(&[keystroke("cmd-t")]) == Some(false)
        })
        .map(command_of)
        .collect::<Vec<_>>();
    assert_eq!(bound, vec![ids::NEW_WINDOW]);
}

#[test]
fn duplicate_user_entries_report_a_conflict_and_the_last_wins() {
    let user = [entry("alt-x", "new_tab"), entry("alt-x", "new_window")];
    let compiled = compile(Platform::Linux, &user).unwrap();
    assert_eq!(
        compiled.conflicts,
        vec![
            "keybinding 2 (\"alt-x\"): replaces keybinding 1 with the same key and when".to_owned()
        ]
    );
    let bound = compiled
        .effective
        .iter()
        .filter(|binding| binding.key == "alt-x")
        .map(|binding| binding.command)
        .collect::<Vec<_>>();
    assert_eq!(bound, vec![ids::NEW_WINDOW]);
}

#[test]
fn unconditional_user_bindings_lead_menus_and_conditional_ones_win_ties() {
    let user = [
        KeybindingEntry {
            when: Some("Terminal".into()),
            ..entry("cmd-w", "toggle_fullscreen")
        },
        entry("cmd-r", "reload_config"),
    ];
    let compiled = compile(Platform::MacOs, &user).unwrap();
    let keymap = Keymap::new(compiled.bindings);
    // Menus take the first binding listed for the action.
    let reload = invoke(ids::RELOAD_CONFIG).into_boxed();
    let shown =
        keymap
            .bindings_for_action(reload.as_ref())
            .next()
            .map(|binding| {
                binding
                    .keystrokes()
                    .iter()
                    .map(|keystroke| keystroke.inner().clone())
                    .collect::<Vec<_>>()
            });
    assert_eq!(shown, Some(vec![keystroke("cmd-r")]));
    // Equal-depth ties go to the later binding, so a conditional user
    // binding must follow the default for the same key.
    let stack = [
        KeyContext::parse("Workspace").unwrap(),
        KeyContext::parse("Terminal").unwrap(),
    ];
    let (bindings, _) =
        keymap.bindings_for_input(&[keystroke("cmd-w")], &stack);
    assert_eq!(
        bindings.first().map(command_of),
        Some(ids::TOGGLE_FULLSCREEN)
    );
}

#[test]
fn a_trailing_plus_is_the_plus_key() {
    let compiled =
        compile(Platform::MacOs, &[entry("cmd-+", "new_tab")]).unwrap();
    assert!(compiled.reserved.is_reserved(&keystroke("cmd-+")));
}

#[test]
fn invalid_entries_fail_with_index_and_key() {
    let cases: Vec<(KeybindingEntry, &str)> = vec![
        (entry("cmd-x-y", "new_tab"), "invalid key"),
        (entry("cmd+<", "unbind"), "join modifiers with `-`, not `+`"),
        (
            KeybindingEntry {
                when: Some("Terminal &&".into()),
                ..entry("cmd-t", "new_tab")
            },
            "invalid when",
        ),
        (entry("cmd-t", "teleport"), "unknown command `teleport`"),
        (
            with_args(
                entry("cmd-t", "rename_tab"),
                &[
                    ("name", toml::Value::String("x".into())),
                    ("tab", toml::Value::Integer(1)),
                ],
            ),
            "argument `tab` is a runtime identity",
        ),
        (
            with_args(
                entry("cmd-3", "select_tab"),
                &[("index", toml::Value::Integer(12))],
            ),
            "expected 1 to 9",
        ),
        (
            with_args(
                entry("cmd-3", "select_tab"),
                &[("index", toml::Value::String("3".into()))],
            ),
            "must be",
        ),
        (
            with_args(
                entry("cmd-t", "rename_tab"),
                &[("name", toml::Value::Float(1.5))],
            ),
            "argument `name` must be a string, integer, or boolean, not \
             float",
        ),
        (
            with_args(
                entry("cmd-t", "unbind"),
                &[("index", toml::Value::Integer(1))],
            ),
            "unbind takes no args",
        ),
        (
            KeybindingEntry {
                when: Some("Terminal".into()),
                ..entry("cmd-t", "unbind")
            },
            "takes no `when`",
        ),
    ];
    for (user, expected) in cases {
        let key = user.key.clone();
        let entries = [entry("cmd-t", "new_tab"), user];
        let error = compile(Platform::MacOs, &entries)
            .err()
            .unwrap_or_else(|| panic!("{key}: expected {expected}"))
            .to_string();
        assert!(
            error.starts_with(&format!("keybinding 2 ({key:?}): ")),
            "{error}"
        );
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn reserved_keys_follow_bound_and_unbound_chords() {
    let user = [
        entry("ctrl-k ctrl-t", "new_tab"),
        entry("alt-r", "reload_config"),
        entry("shift-pageup", "unbind"),
    ];
    let compiled = compile(Platform::Linux, &user).unwrap();
    for chord in ["ctrl-k", "alt-r"] {
        assert!(compiled.reserved.is_reserved(&keystroke(chord)), "{chord}");
    }
    assert!(!compiled.reserved.is_reserved(&keystroke("ctrl-t")));
    assert!(!compiled.reserved.is_reserved(&keystroke("shift-pageup")));
    let mut typed = keystroke("alt-r");
    typed.key_char = Some("®".into());
    assert!(compiled.reserved.is_reserved(&typed));
}

#[test]
fn later_chord_strokes_are_unbound_without_their_prefix() {
    for chord in ["alt-k alt-r", "alt-k alt-r alt-j"] {
        let compiled =
            compile(Platform::MacOs, &[entry(chord, "reload_config")]).unwrap();
        let matcher = Keymap::new(compiled.bindings);
        let context = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        let strokes =
            chord.split_whitespace().map(keystroke).collect::<Vec<_>>();
        let (bindings, pending) =
            matcher.bindings_for_input(&strokes[..1], &context);
        assert!(bindings.is_empty());
        assert!(pending);
        assert!(compiled.reserved.is_reserved(&strokes[0]));
        for stroke in &strokes[1..] {
            let (bindings, pending) = matcher
                .bindings_for_input(std::slice::from_ref(stroke), &context);
            assert!(bindings.is_empty());
            assert!(!pending);
            assert!(
                !compiled.reserved.is_reserved(stroke),
                "standalone {} from {chord}",
                stroke.key
            );
        }
        let (bindings, pending) =
            matcher.bindings_for_input(&strokes, &context);
        assert!(!pending);
        assert_eq!(
            bindings.iter().map(command_of).collect::<Vec<_>>(),
            vec![ids::RELOAD_CONFIG]
        );
    }
}

#[test]
fn conditional_bindings_reserve_only_multi_key_starting_strokes() {
    let user = [
        KeybindingEntry {
            when: Some("selection".into()),
            ..entry("ctrl-c", "copy")
        },
        KeybindingEntry {
            when: Some("Terminal".into()),
            ..entry("ctrl-k ctrl-x", "new_tab")
        },
    ];
    let compiled = compile(Platform::Linux, &user).unwrap();
    // Ctrl-C must still interrupt the shell when nothing is selected.
    assert!(!compiled.reserved.is_reserved(&keystroke("ctrl-c")));
    assert!(compiled.reserved.is_reserved(&keystroke("ctrl-k")));
    assert!(!compiled.reserved.is_reserved(&keystroke("ctrl-x")));
    // The bindings themselves still exist for GPUI to match.
    let bound = compiled
        .bindings
        .iter()
        .filter_map(bound_command)
        .collect::<Vec<_>>();
    assert!(bound.contains(&ids::COPY) && bound.contains(&ids::NEW_TAB));
}

#[test]
fn compiled_bindings_match_through_the_real_gpui_keymap() {
    for (platform, reload) in
        [(Platform::MacOs, "cmd-<"), (Platform::Linux, "ctrl-<")]
    {
        let user = [
            entry("alt-r", "new_window"),
            KeybindingEntry {
                when: Some("Terminal && !confirming".into()),
                ..entry("alt-p", "paste")
            },
        ];
        let compiled = compile(platform, &user).unwrap();
        let keymap = Keymap::new(compiled.bindings);
        let terminal = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        let confirming = [
            KeyContext::parse("Workspace confirming").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        let matched = |event: Keystroke, contexts: &[KeyContext]| {
            let (bindings, pending) =
                keymap.bindings_for_input(&[event], contexts);
            assert!(!pending);
            bindings.iter().map(command_of).collect::<Vec<_>>()
        };
        // Both native backends fold Shift+comma into '<' without Shift.
        assert_eq!(
            matched(keystroke(reload), &terminal),
            vec![ids::RELOAD_CONFIG]
        );
        let mut option_r = keystroke("alt-r");
        option_r.key_char = Some("®".into());
        assert_eq!(matched(option_r, &terminal), vec![ids::NEW_WINDOW]);
        assert_eq!(matched(keystroke("alt-p"), &terminal), vec![ids::PASTE]);
        assert!(matched(keystroke("alt-p"), &confirming).is_empty());
        let select_last = keystroke(if platform == Platform::MacOs {
            "cmd-9"
        } else {
            "alt-9"
        });
        assert_eq!(matched(select_last, &terminal), vec![ids::SELECT_TAB]);
    }
}

#[test]
fn window_context_predicates_need_the_descendant_form_to_beat_defaults() {
    // GPUI ranks a predicate-free binding at the full stack depth and an
    // identifier predicate at the depth of the context that contains it,
    // so `fullscreen` alone ranks below the default and only the
    // descendant form `fullscreen > Terminal` wins.
    let terminal = [
        KeyContext::parse("Workspace").unwrap(),
        KeyContext::parse("Terminal").unwrap(),
    ];
    let fullscreen = [
        KeyContext::parse("Workspace fullscreen").unwrap(),
        KeyContext::parse("Terminal").unwrap(),
    ];
    let first = |when: &str, contexts: &[KeyContext]| {
        let user = [KeybindingEntry {
            when: Some(when.into()),
            ..entry("cmd-w", "toggle_fullscreen")
        }];
        let compiled = compile(Platform::MacOs, &user).unwrap();
        let keymap = Keymap::new(compiled.bindings);
        let (bindings, _) =
            keymap.bindings_for_input(&[keystroke("cmd-w")], contexts);
        bindings.first().map(command_of)
    };
    assert_eq!(first("fullscreen", &fullscreen), Some(ids::CLOSE_TAB));
    assert_eq!(
        first("fullscreen > Terminal", &fullscreen),
        Some(ids::TOGGLE_FULLSCREEN)
    );
    assert_eq!(
        first("fullscreen > Terminal", &terminal),
        Some(ids::CLOSE_TAB)
    );
}

#[test]
fn installed_shortcuts_use_captured_context_and_exact_arguments() {
    let user = [
        KeybindingEntry {
            when: Some("Terminal && !confirming".into()),
            ..with_args(
                entry("alt-r", "rename_tab"),
                &[("name", toml::Value::String("work".into()))],
            )
        },
        with_args(
            entry("alt-4", "select_tab"),
            &[("index", toml::Value::Integer(4))],
        ),
        entry("alt-q", "show_quake"),
        with_args(
            entry("alt-l", "show_quake"),
            &[("profile", toml::Value::String("logs".into()))],
        ),
    ];
    let (_, installed) =
        compile(Platform::Linux, &user).unwrap().install_parts();
    let terminal = [
        KeyContext::parse("Workspace").unwrap(),
        KeyContext::parse("Terminal").unwrap(),
    ];
    let confirming = [
        KeyContext::parse("Workspace confirming").unwrap(),
        KeyContext::parse("Terminal").unwrap(),
    ];
    assert_eq!(
        installed.shortcuts(ids::RENAME_TAB, &terminal, None).len(),
        1
    );
    assert!(
        installed
            .shortcuts(ids::RENAME_TAB, &confirming, None)
            .is_empty()
    );
    assert!(
        installed
            .shortcuts(
                ids::SELECT_TAB,
                &terminal,
                Some(&[CommandArgument::new(
                    "index",
                    CommandValue::Integer(4),
                )]),
            )
            .iter()
            .any(|binding| binding.key == "alt-4")
    );
    assert!(
        installed
            .shortcuts(
                ids::SELECT_TAB,
                &terminal,
                Some(&[CommandArgument::new(
                    "index",
                    CommandValue::Integer(5),
                )]),
            )
            .iter()
            .all(|binding| binding.key != "alt-4")
    );
    let quake = installed.shortcuts(
        ids::SHOW_QUAKE,
        &terminal,
        Some(&[CommandArgument::new(
            "profile",
            CommandValue::Text("logs".into()),
        )]),
    );
    assert_eq!(
        quake.first().map(|binding| binding.key.as_str()),
        Some("alt-l")
    );
    assert!(quake.iter().any(|binding| binding.key == "alt-q"));
}

#[test]
fn palette_defaults_dispatch_through_invoke_palette() {
    let compiled = compile(Platform::MacOs, &[]).unwrap();
    let keymap = Keymap::new(compiled.bindings);
    let palette = [
        KeyContext::parse("Workspace palette").unwrap(),
        KeyContext::parse("Palette").unwrap(),
    ];
    let matched = |key: &str, contexts: &[KeyContext]| {
        let (bindings, pending) =
            keymap.bindings_for_input(&[keystroke(key)], contexts);
        assert!(!pending);
        bindings.first().map(command_of)
    };
    assert_eq!(matched("cmd-c", &palette), Some(ids::TEXT_COPY));
    assert_eq!(matched("down", &palette), Some(ids::PALETTE_SELECT_NEXT));
    assert_eq!(matched("enter", &palette), Some(ids::PALETTE_CONFIRM));

    let terminal = [
        KeyContext::parse("Workspace").unwrap(),
        KeyContext::parse("Terminal").unwrap(),
    ];
    assert_eq!(matched("cmd-c", &terminal), Some(ids::COPY));
    assert_eq!(matched("down", &terminal), None);
}

#[test]
fn dialog_defaults_match_only_while_confirming() {
    for platform in [Platform::MacOs, Platform::Linux] {
        let compiled = compile(platform, &[]).unwrap();
        for key in ["enter", "escape", "tab", "shift-tab", "left", "right"] {
            assert!(
                !compiled.reserved.is_reserved(&keystroke(key)),
                "{platform:?}: {key} reserved from terminal input"
            );
        }
        let keymap = Keymap::new(compiled.bindings);
        let confirming = [KeyContext::parse("Workspace confirming").unwrap()];
        let terminal = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        let palette = [
            KeyContext::parse("Workspace palette").unwrap(),
            KeyContext::parse("Palette").unwrap(),
        ];
        let matched = |key: &str, contexts: &[KeyContext]| {
            let (bindings, pending) =
                keymap.bindings_for_input(&[keystroke(key)], contexts);
            assert!(!pending, "{platform:?}: {key} left a pending chord");
            bindings.iter().map(command_of).collect::<Vec<_>>()
        };
        for (key, command) in [
            ("enter", ids::DIALOG_CONFIRM),
            ("escape", ids::DIALOG_CANCEL),
            ("tab", ids::DIALOG_FOCUS_NEXT),
            ("right", ids::DIALOG_FOCUS_NEXT),
            ("shift-tab", ids::DIALOG_FOCUS_PREVIOUS),
            ("left", ids::DIALOG_FOCUS_PREVIOUS),
        ] {
            assert_eq!(
                matched(key, &confirming),
                vec![command],
                "{platform:?}: {key} while confirming"
            );
            assert!(
                matched(key, &terminal).is_empty(),
                "{platform:?}: {key} reached a terminal binding"
            );
            assert!(
                !matched(key, &palette).contains(&command),
                "{platform:?}: {key} dispatched a dialog command in the palette"
            );
        }
        assert_eq!(matched("enter", &palette), vec![ids::PALETTE_CONFIRM]);
        assert_eq!(matched("escape", &palette), vec![ids::PALETTE_BACK]);
    }
}

#[test]
fn menu_defaults_match_only_while_a_menu_has_focus() {
    for platform in [Platform::MacOs, Platform::Linux] {
        let compiled = compile(platform, &[]).unwrap();
        for key in [
            "down", "up", "home", "end", "right", "left", "enter", "space",
            "escape", "tab",
        ] {
            assert!(
                !compiled.reserved.is_reserved(&keystroke(key)),
                "{platform:?}: {key} reserved from terminal input"
            );
        }
        let keymap = Keymap::new(compiled.bindings);
        let menu = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("menu").unwrap(),
        ];
        let terminal = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        let confirming = [KeyContext::parse("Workspace confirming").unwrap()];
        let notices = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("notices").unwrap(),
        ];
        let palette = [
            KeyContext::parse("Workspace palette").unwrap(),
            KeyContext::parse("Palette").unwrap(),
        ];
        let matched = |key: &str, contexts: &[KeyContext]| {
            let (bindings, pending) =
                keymap.bindings_for_input(&[keystroke(key)], contexts);
            assert!(!pending, "{platform:?}: {key} left a pending chord");
            bindings.iter().map(command_of).collect::<Vec<_>>()
        };
        for (key, command) in [
            ("down", ids::MENU_SELECT_NEXT),
            ("up", ids::MENU_SELECT_PREVIOUS),
            ("home", ids::MENU_SELECT_FIRST),
            ("end", ids::MENU_SELECT_LAST),
            ("right", ids::MENU_SELECT_RIGHT),
            ("left", ids::MENU_SELECT_LEFT),
            ("enter", ids::MENU_CONFIRM),
            ("space", ids::MENU_CONFIRM),
            ("escape", ids::MENU_CLOSE),
            ("tab", ids::MENU_CLOSE),
        ] {
            assert_eq!(
                matched(key, &menu),
                vec![command],
                "{platform:?}: {key} with a focused menu"
            );
            for (name, contexts) in [
                ("terminal", &terminal[..]),
                ("dialog", &confirming[..]),
                ("notices", &notices[..]),
                ("palette", &palette[..]),
            ] {
                assert!(
                    !matched(key, contexts).contains(&command),
                    "{platform:?}: {key} dispatched a menu command in the {name}"
                );
            }
        }
        assert_eq!(matched("enter", &notices), vec![ids::NOTICE_RUN_ACTION]);
        assert_eq!(matched("escape", &palette), vec![ids::PALETTE_BACK]);
    }
}

#[test]
fn linux_binds_ctrl_comma_to_open_settings_and_reserves_it() {
    let compiled = compile(Platform::Linux, &[]).unwrap();
    assert!(compiled.reserved.is_reserved(&keystroke("ctrl-,")));
    assert!(!compiled.reserved.is_reserved(&keystroke(",")));
    let (bindings, installed) = compiled.install_parts();
    let keymap = Keymap::new(bindings);
    let terminal = [
        KeyContext::parse("Workspace").unwrap(),
        KeyContext::parse("Terminal").unwrap(),
    ];
    let (bindings, pending) =
        keymap.bindings_for_input(&[keystroke("ctrl-,")], &terminal);
    assert!(!pending);
    assert_eq!(
        bindings.iter().map(command_of).collect::<Vec<_>>(),
        vec![ids::OPEN_SETTINGS]
    );
    let keys: Vec<&str> = installed
        .shortcuts(ids::OPEN_SETTINGS, &terminal, None)
        .iter()
        .map(|binding| binding.key.as_str())
        .collect();
    assert_eq!(keys, vec!["ctrl-,"]);
    let mac = compile(Platform::MacOs, &[]).unwrap();
    assert!(!mac.reserved.is_reserved(&keystroke("ctrl-,")));
    assert!(mac.reserved.is_reserved(&keystroke("cmd-,")));
}

#[test]
fn notice_defaults_match_only_while_a_toast_has_focus() {
    for platform in [Platform::MacOs, Platform::Linux] {
        let compiled = compile(platform, &[]).unwrap();
        for key in ["down", "up", "enter", "escape", "delete"] {
            assert!(
                !compiled.reserved.is_reserved(&keystroke(key)),
                "{platform:?}: {key} reserved from terminal input"
            );
        }
        let keymap = Keymap::new(compiled.bindings);
        let notices = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("notices").unwrap(),
        ];
        let terminal = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        let confirming = [KeyContext::parse("Workspace confirming").unwrap()];
        let palette = [
            KeyContext::parse("Workspace palette").unwrap(),
            KeyContext::parse("Palette").unwrap(),
        ];
        let matched = |key: &str, contexts: &[KeyContext]| {
            let (bindings, pending) =
                keymap.bindings_for_input(&[keystroke(key)], contexts);
            assert!(!pending, "{platform:?}: {key} left a pending chord");
            bindings.iter().map(command_of).collect::<Vec<_>>()
        };
        for (key, command) in [
            ("down", ids::NOTICE_NEXT),
            ("up", ids::NOTICE_PREVIOUS),
            ("enter", ids::NOTICE_RUN_ACTION),
            ("escape", ids::NOTICE_DISMISS),
            ("delete", ids::NOTICE_DISMISS),
        ] {
            assert_eq!(
                matched(key, &notices),
                vec![command],
                "{platform:?}: {key} with a focused toast"
            );
            for (name, contexts) in [
                ("terminal", &terminal[..]),
                ("dialog", &confirming[..]),
                ("palette", &palette[..]),
            ] {
                assert!(
                    !matched(key, contexts).contains(&command),
                    "{platform:?}: {key} dispatched a notice command in the {name}"
                );
            }
        }
        assert_eq!(matched("enter", &confirming), vec![ids::DIALOG_CONFIRM]);
        assert_eq!(matched("down", &palette), vec![ids::PALETTE_SELECT_NEXT]);
    }
}

#[test]
fn user_palette_override_wins_at_equal_depth() {
    let compiled =
        compile(Platform::MacOs, &[entry("up", "text_line_start")]).unwrap();
    let keymap = Keymap::new(compiled.bindings);
    let contexts = [
        KeyContext::parse("Workspace palette").unwrap(),
        KeyContext::parse("Palette").unwrap(),
    ];
    let (bindings, pending) =
        keymap.bindings_for_input(&[keystroke("up")], &contexts);
    assert!(!pending);
    assert_eq!(bindings.first().map(command_of), Some(ids::TEXT_LINE_START));
}

#[test]
fn palette_bindings_never_reserve_terminal_strokes() {
    let compiled =
        compile(Platform::MacOs, &[entry("ctrl-c", "text_copy")]).unwrap();
    assert!(!compiled.reserved.is_reserved(&keystroke("ctrl-c")));
    let binding = compiled
        .effective
        .iter()
        .find(|binding| {
            binding.origin == Origin::User && binding.key == "ctrl-c"
        })
        .expect("user binding is effective");
    assert_eq!(binding.context, Some("Palette"));
    assert_eq!(binding.when, None);
}

#[test]
fn context_and_when_combine() {
    let user = [KeybindingEntry {
        when: Some("!confirming".into()),
        ..entry("ctrl-n", "palette_select_next")
    }];
    let compiled = compile(Platform::MacOs, &user).unwrap();
    let binding = compiled
        .effective
        .iter()
        .find(|binding| {
            binding.origin == Origin::User && binding.key == "ctrl-n"
        })
        .expect("user binding is effective");
    let palette = [
        KeyContext::parse("Workspace palette").unwrap(),
        KeyContext::parse("Palette").unwrap(),
    ];
    let confirming = [
        KeyContext::parse("Workspace palette confirming").unwrap(),
        KeyContext::parse("Palette").unwrap(),
    ];
    assert!(binding.matches_context(&palette));
    assert!(!binding.matches_context(&confirming));
}

#[test]
fn partial_bindings_compile_but_bad_values_fail() {
    assert!(
        compile(Platform::MacOs, &[entry("cmd-shift-o", "select_tab")],)
            .is_ok()
    );

    let range = compile(
        Platform::MacOs,
        &[with_args(
            entry("cmd-shift-o", "select_tab"),
            &[("index", toml::Value::Integer(12))],
        )],
    )
    .err()
    .expect("out-of-range index fails")
    .to_string();
    assert!(range.contains("expected 1 to 9"), "{range}");

    let identity = compile(
        Platform::MacOs,
        &[with_args(
            entry("cmd-shift-o", "select_tab"),
            &[("tab", toml::Value::Integer(1))],
        )],
    )
    .err()
    .expect("configured runtime identity fails")
    .to_string();
    assert!(identity.contains("runtime identity"), "{identity}");
}
