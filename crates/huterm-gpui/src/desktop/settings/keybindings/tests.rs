use std::path::PathBuf;

use huterm_config::document::{ConfigFile, List};
use huterm_protocol::catalog;

use super::*;
use crate::desktop::key_hint::KeyHint;
use crate::keymap::{BindingState, Origin, Platform, inventory};

#[test]
fn every_command_has_one_group() {
    for spec in catalog() {
        let listed = GROUPS
            .iter()
            .filter(|(_, commands)| commands.contains(&spec.id))
            .count();
        let implied = usize::from(spec.context.is_some());
        assert_eq!(listed + implied, 1, "{} needs one group", spec.id.as_str());
    }
}

#[test]
fn the_contexts_match_the_documented_table() {
    let docs = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/keybindings.md"),
    )
    .unwrap();
    let table = docs
        .split("| Context | Present when |")
        .nth(1)
        .expect("the contexts table")
        .split("\n\n")
        .next()
        .unwrap();
    let documented: Vec<&str> = table
        .lines()
        .skip(2)
        .filter_map(|row| row.split('|').nth(1))
        .flat_map(|cell| cell.split(','))
        .map(|name| name.trim().trim_matches('`'))
        .collect();
    let listed: Vec<&str> =
        CONTEXTS.iter().map(|context| context.name).collect();
    assert_eq!(listed, documented);
}

#[test]
fn conditions_report_syntax_errors_and_unknown_contexts() {
    assert_eq!(check_condition(""), Ok(vec![]));
    assert_eq!(check_condition("Terminal && !selection"), Ok(vec![]));
    assert_eq!(
        check_condition("Workspace > Terminal && bogus || bogus"),
        Ok(vec!["bogus".to_owned()])
    );
    assert!(check_condition("Terminal &&").is_err());
    assert!(check_condition("Terminal )").is_err());
}

#[test]
fn completions_finish_the_context_name_before_the_caret() {
    let text = "Terminal && !sel";
    let (start, matches) = completions(text, text.len());
    assert_eq!(start, 13);
    assert_eq!(
        matches
            .iter()
            .map(|context| context.name)
            .collect::<Vec<_>>(),
        ["selection"]
    );
    // A complete name offers nothing more; an empty prefix offers all.
    assert_eq!(completions("Terminal", 8).1, vec![]);
    assert_eq!(completions("", 0).1.len(), CONTEXTS.len());
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, text: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "huterm-keybinding-edits-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("config.toml"), text).unwrap();
        Self(directory)
    }

    fn path(&self) -> PathBuf {
        self.0.join("config.toml")
    }

    /// Applies `edit` through the patch service and the real validator.
    fn apply(&self, edit: &Edit) -> String {
        let file = ConfigFile::new(self.path());
        let base = file.load().unwrap();
        let patches = edit
            .patches(
                &base.entries(List::Keybinding),
                &base.entries(List::GlobalKeybinding),
            )
            .unwrap();
        file.apply(&base, &patches, &crate::config::validator(self.path()))
            .unwrap();
        std::fs::read_to_string(self.path()).unwrap()
    }

    fn states(&self) -> Vec<(String, CommandId, Origin, BindingState)> {
        let text = std::fs::read_to_string(self.path()).unwrap();
        let config = crate::config::parse_at(&text, &self.path()).unwrap();
        inventory(Platform::current(), &config.keybindings)
            .unwrap()
            .bindings
            .into_iter()
            .map(|listed| {
                (
                    listed.binding.key,
                    listed.binding.command,
                    listed.binding.origin,
                    listed.state,
                )
            })
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A default on both platforms.
fn next_tab() -> Binding {
    Binding::new("ctrl-tab", ids::NEXT_TAB, None, &[])
}

#[test]
fn switching_a_default_off_and_on_writes_and_removes_one_entry() {
    let file = Scratch::new("switch", "# Mine.\n[font]\nsize = 13.0 # big\n");
    let text = file.apply(&Edit::SwitchOffDefault(next_tab()));
    assert_eq!(
        text,
        "# Mine.\n[font]\nsize = 13.0 # big\n\n[[keybinding]]\nkey = \"ctrl-tab\"\n\
         command = \"next_tab\"\ndisabled = true\n"
    );
    assert!(file.states().contains(&(
        "ctrl-tab".into(),
        ids::NEXT_TAB,
        Origin::Default,
        BindingState::Disabled(vec![0])
    )));
    let text = file.apply(&Edit::SwitchOnDefault(vec![0]));
    assert_eq!(text, "# Mine.\n[font]\nsize = 13.0 # big\n");
}

#[test]
fn changing_a_default_disables_it_and_adds_the_changed_binding() {
    let file = Scratch::new("change", "");
    let text = file.apply(&Edit::ChangeDefault {
        default: next_tab(),
        disabled: false,
        field: Field::When,
        value: Some(Value::String("Terminal".into())),
    });
    assert_eq!(
        text,
        "[[keybinding]]\nkey = \"ctrl-tab\"\ncommand = \"next_tab\"\ndisabled = true\n\n\
         [[keybinding]]\nkey = \"ctrl-tab\"\ncommand = \"next_tab\"\nwhen = \"Terminal\"\n"
    );
    let states = file.states();
    assert!(states.contains(&(
        "ctrl-tab".into(),
        ids::NEXT_TAB,
        Origin::User,
        BindingState::Active
    )));
}

#[test]
fn user_entry_fields_and_arguments_change_in_place() {
    let original = "[[keybinding]] # mine\nkey = \"ctrl-alt-r\"\n\
                    command = \"resize_pane_right\"\nargs = { amount = 5 }\n";
    let file = Scratch::new("user", original);
    let change = |field, value| Edit::ChangeUser {
        entry: 0,
        field,
        value,
    };
    let text = file.apply(&change(
        Field::Key,
        Some(Value::String("ctrl-alt-l".into())),
    ));
    assert!(
        text.starts_with("[[keybinding]] # mine\nkey = \"ctrl-alt-l\"\n"),
        "{text}"
    );
    let text = file.apply(&change(Field::Argument("amount".into()), None));
    assert!(!text.contains("args"), "{text}");
    let text = file.apply(&change(
        Field::Argument("amount".into()),
        Some(Value::Integer(3)),
    ));
    assert!(text.contains("amount = 3"), "{text}");
    let text = file.apply(&Edit::SwitchUser {
        entry: 0,
        on: false,
    });
    assert!(text.contains("disabled = true"), "{text}");
    let text = file.apply(&Edit::SwitchUser { entry: 0, on: true });
    assert!(!text.contains("disabled"), "{text}");
}

#[test]
fn adding_and_removing_entries_keeps_the_others() {
    let original = "[[keybinding]]\nkey = \"ctrl-alt-a\"\ncommand = \"new_tab\"\n\n\
                    # Keep this one.\n[[keybinding]]\nkey = \"ctrl-alt-b\"\n\
                    command = \"new_tab\"\n\n[[keybinding]]\nkey = \"ctrl-alt-c\"\n\
                    command = \"new_window\"\n";
    let file = Scratch::new("remove", original);
    let text = file.apply(&Edit::Add {
        key: "ctrl-alt-d".into(),
        command: ids::NEW_TAB,
    });
    assert!(text.ends_with(
        "[[keybinding]]\nkey = \"ctrl-alt-d\"\ncommand = \"new_tab\"\n"
    ));
    let text = file.apply(&Edit::Remove(vec![0, 3]));
    assert_eq!(
        text,
        "# Keep this one.\n[[keybinding]]\nkey = \"ctrl-alt-b\"\ncommand = \"new_tab\"\n\n\
         [[keybinding]]\nkey = \"ctrl-alt-c\"\ncommand = \"new_window\"\n"
    );
}

fn entry(key: &str, command: &str) -> crate::config::KeybindingEntry {
    crate::config::KeybindingEntry {
        key: key.into(),
        command: command.into(),
        args: None,
        when: None,
        description: None,
        disabled: None,
    }
}

/// The notes for `command`'s binding on `key`, from macOS defaults plus
/// `user`.
fn notes_for(
    user: &[crate::config::KeybindingEntry],
    command: CommandId,
    key: &str,
    origin: Origin,
) -> Vec<Note> {
    let inventory = inventory(Platform::MacOs, user).unwrap();
    let listed = inventory
        .bindings
        .iter()
        .find(|listed| {
            listed.binding.command == command
                && listed.binding.key == key
                && listed.binding.origin == origin
        })
        .unwrap();
    notes(&inventory, listed)
}

#[test]
fn taking_a_key_names_both_commands() {
    let user = [entry("ctrl-tab", "split_right")];
    assert_eq!(
        notes_for(&user, ids::NEXT_TAB, "ctrl-tab", Origin::Default),
        [Note::info(format!(
            "Replaced: your {} shortcut for Split Right took this key.",
            KeyHint::parse("ctrl-tab", Platform::current()).text()
        ))]
    );
    assert!(
        notes_for(&user, ids::SPLIT_RIGHT, "ctrl-tab", Origin::User).contains(
            &Note::warning(
                "Took this key from Next Tab, which no longer has it."
                    .to_owned()
            )
        )
    );
}

#[test]
fn a_shortcut_without_a_condition_warns_where_it_shares_a_key() {
    let user = [entry("cmd-f", "split_right")];
    let notes = notes_for(&user, ids::SPLIT_RIGHT, "cmd-f", Origin::User);
    assert!(
        notes.iter().any(|note| note.warning
            && note.text.starts_with(
                "Settings: Search also uses this key in Settings,"
            )),
        "{notes:?}"
    );
    // With a condition it only says so.
    let mut conditional = entry("cmd-f", "split_right");
    conditional.when = Some("Terminal".into());
    let notes =
        notes_for(&[conditional], ids::SPLIT_RIGHT, "cmd-f", Origin::User);
    assert!(notes.iter().all(|note| !note.warning), "{notes:?}");
}

#[test]
fn keys_programs_need_say_so_and_default_pairs_say_nothing() {
    let notes = notes_for(
        &[entry("tab", "new_tab")],
        ids::NEW_TAB,
        "tab",
        Origin::User,
    );
    assert!(
        notes
            .iter()
            .any(|note| note.warning
                && note.text.starts_with("Without a condition")),
        "{notes:?}"
    );
    let notes = notes_for(
        &[entry("ctrl-c", "copy")],
        ids::COPY,
        "ctrl-c",
        Origin::User,
    );
    assert!(
        notes
            .iter()
            .any(|note| note.text.starts_with("Programs in the terminal")),
        "{notes:?}"
    );
    // Copy and Text: Copy share cmd-c as shipped.
    assert_eq!(notes_for(&[], ids::COPY, "cmd-c", Origin::Default), []);
}

#[test]
fn global_shortcut_edits_change_only_their_entry() {
    let original = "# Mine.\n[quake.profiles.logs]\nheight = 0.3\n";
    let file = Scratch::new("globals", original);
    let added = file.apply(&Edit::AddGlobal {
        key: "ctrl-alt-shift-y".into(),
        command: ids::TOGGLE_QUAKE,
        profile: Some("logs".into()),
    });
    assert!(added.starts_with(original), "{added}");
    assert!(
        added.contains(
            "[[global_keybinding]]\nkey = \"ctrl-alt-shift-y\"\n\
             command = \"toggle_quake\"\nargs = { profile = \"logs\" }\n"
        ),
        "{added}"
    );
    let change = |field, value: Option<&str>| Edit::ChangeGlobal {
        entry: 0,
        field,
        value: value.map(|value| Value::String(value.to_owned())),
    };
    let text = file.apply(&change("command", Some("show_quake")));
    assert!(text.contains("command = \"show_quake\""), "{text}");
    let text = file.apply(&change("key", Some("ctrl-alt-shift-u")));
    assert!(text.contains("key = \"ctrl-alt-shift-u\""), "{text}");
    // The default profile leaves `args` out.
    let text = file.apply(&change("profile", None));
    assert!(!text.contains("args"), "{text}");
    let text = file.apply(&Edit::RemoveGlobal(0));
    assert!(!text.contains("global_keybinding"), "{text}");
    assert!(text.starts_with(original), "{text}");
}

#[test]
fn renaming_a_profile_rewrites_only_the_entries_that_run_it() {
    let original = "[quake.profiles.logs]\nheight = 0.3\n\n\
                    [quake.profiles.work]\n\n\
                    [[global_keybinding]]\nkey = \"ctrl-alt-shift-y\"\n\
                    command = \"hide_quake\"\nargs = { profile = \"logs\" }\n\n\
                    [[global_keybinding]]\nkey = \"ctrl-alt-shift-u\"\n\
                    command = \"toggle_quake\"\n\n\
                    [[keybinding]]\nkey = \"alt-l\"\ncommand = \"show_quake\"\n\
                    args = { profile = \"logs\" }\n\n\
                    [[keybinding]]\nkey = \"alt-k\"\ncommand = \"toggle_quake\"\n\
                    args = { profile = \"work\" }\n";
    let file = Scratch::new("rename-profile", original);
    let config = ConfigFile::new(file.path());
    let base = config.load().unwrap();
    let patches = rename_profile_references(
        &base.entries(List::Keybinding),
        &base.entries(List::GlobalKeybinding),
        "logs",
        "build",
    );
    assert_eq!(patches.len(), 2, "{patches:?}");
    // The table moves in the same save, or the validator refuses the
    // shortcut that names it.
    let mut all = vec![
        Patch::Set {
            path: vec!["quake".into(), "profiles".into(), "build".into()],
            value: Value::Table(Table::new()),
        },
        Patch::Remove {
            path: vec!["quake".into(), "profiles".into(), "logs".into()],
        },
    ];
    all.extend(patches);
    config
        .apply(&base, &all, &crate::config::validator(file.path()))
        .unwrap();
    let text = std::fs::read_to_string(file.path()).unwrap();
    assert_eq!(
        text,
        original
            .replace("[quake.profiles.logs]\nheight = 0.3\n\n", "")
            .replace(
                "[quake.profiles.work]\n",
                "[quake.profiles.work]\n\n[quake.profiles.build]\n"
            )
            .replace("\"logs\"", "\"build\"")
    );
}
