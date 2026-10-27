use std::cell::{Cell, RefCell};
use std::sync::atomic::AtomicUsize;

use super::*;

fn set(path: &str, value: Value) -> Patch {
    Patch::Set {
        path: path.split('.').map(str::to_owned).collect(),
        value,
    }
}

fn remove(path: &str) -> Patch {
    Patch::Remove {
        path: path.split('.').map(str::to_owned).collect(),
    }
}

fn table(source: &str) -> Table {
    toml::from_str(source).expect("test table")
}

fn rendered(source: &str, patches: &[Patch]) -> String {
    render(source, patches).unwrap_or_else(|error| panic!("{error}"))
}

/// Locates entry `index` of the keybinding list in `source`.
fn locator(source: &str, index: usize) -> EntryLocator {
    snapshot_of(source)
        .entry(List::Keybinding, index)
        .expect("entry exists")
}

fn snapshot_of(source: &str) -> Snapshot {
    Snapshot {
        text: Some(source.to_owned()),
        revision: Revision {
            content: Some(hash(source.as_bytes())),
            target: PathBuf::new(),
            identity: None,
        },
    }
}

#[test]
fn replacing_a_value_keeps_its_key_spacing_and_trailing_comment() {
    let source = "# Font\n[font]\nfamily   =  \"Menlo\"   # the face\nsize=14.0 # points\n";
    assert_eq!(
        rendered(source, &[set("font.size", Value::Float(18.0))]),
        "# Font\n[font]\nfamily   =  \"Menlo\"   # the face\nsize=18.0 # points\n"
    );
}

#[test]
fn crlf_file_changes_only_the_one_value() {
    let source = "# Huterm\r\n[font]\r\n# The face\r\nfamily = \"Menlo\"\r\nsize = 14.0 # points\r\n\r\n[window]\r\npadding_x = 4.0\r\n";
    let output = rendered(source, &[set("font.size", Value::Float(13.5))]);
    assert_eq!(
        output,
        "# Huterm\r\n[font]\r\n# The face\r\nfamily = \"Menlo\"\r\nsize = 13.5 # points\r\n\r\n[window]\r\npadding_x = 4.0\r\n"
    );
}

#[test]
fn new_keys_follow_the_file_line_endings_and_final_newline() {
    // Every ending is CRLF, so new lines are too.
    assert_eq!(
        rendered(
            "[font]\r\nsize = 14.0\r\n",
            &[set("font.family", "Iosevka".into())]
        ),
        "[font]\r\nsize = 14.0\r\nfamily = \"Iosevka\"\r\n"
    );
    // Mixed endings fall back to LF.
    assert_eq!(
        rendered(
            "[font]\r\nsize = 14.0\n",
            &[set("font.family", "Iosevka".into())]
        ),
        "[font]\r\nsize = 14.0\nfamily = \"Iosevka\"\n"
    );
    // A file without a final newline keeps that.
    assert_eq!(
        rendered(
            "[font]\nsize = 14.0",
            &[set("font.family", "Iosevka".into())]
        ),
        "[font]\nsize = 14.0\nfamily = \"Iosevka\""
    );
    assert_eq!(
        rendered(
            "[font]\r\nsize = 14.0",
            &[set("tabs.style", "strip".into())]
        ),
        "[font]\r\nsize = 14.0\r\n\r\n[tabs]\r\nstyle = \"strip\""
    );
}

#[test]
fn adding_a_key_goes_after_the_section_last_statement() {
    let source = "[theme]\nname = \"huterm-dark\"\n# Override colors:\n# background = \"#1d1f21\"\n\n[font]\nsize = 14.0\n";
    assert_eq!(
        rendered(source, &[set("theme.background", "#000000".into())]),
        "[theme]\nname = \"huterm-dark\"\nbackground = \"#000000\"\n# Override colors:\n# background = \"#1d1f21\"\n\n[font]\nsize = 14.0\n"
    );
}

#[test]
fn a_new_table_goes_before_the_first_array_of_tables() {
    let source = "[font]\nsize = 14.0\n\n# Bindings\n[[keybinding]]\nkey = \"cmd-h\"\ncommand = \"hide\"\n";
    assert_eq!(
        rendered(
            source,
            &[set("general.watch_config", Value::Boolean(false))]
        ),
        "[font]\nsize = 14.0\n\n[general]\nwatch_config = false\n\n# Bindings\n[[keybinding]]\nkey = \"cmd-h\"\ncommand = \"hide\"\n"
    );
    assert_eq!(
        rendered(
            "[font]\nsize = 14.0\n",
            &[set("quake.profiles.logs.height", Value::Float(0.5))]
        ),
        "[font]\nsize = 14.0\n\n[quake.profiles.logs]\nheight = 0.5\n"
    );
    assert_eq!(
        rendered("", &[set("font.size", Value::Float(12.0))]),
        "[font]\nsize = 12.0\n"
    );
}

#[test]
fn removing_a_key_removes_its_statement_and_leading_comments() {
    let source =
        "[font]\n# The face\nfamily = \"Menlo\" # trailing\nsize = 14.0\n";
    assert_eq!(
        rendered(source, &[remove("font.family")]),
        "[font]\nsize = 14.0\n"
    );
    // Removing a missing key changes nothing.
    assert_eq!(rendered(source, &[remove("font.weight")]), source);
}

#[test]
fn removing_the_last_key_drops_an_uncommented_grouping_table() {
    let source = "[font]\nsize = 14.0\n\n[tabs]\nstyle = \"strip\"\n\n[window]\npadding_x = 2.0\n";
    assert_eq!(
        rendered(source, &[remove("tabs.style")]),
        "[font]\nsize = 14.0\n\n[window]\npadding_x = 2.0\n"
    );
    assert_eq!(
        rendered("[tabs]\nstyle = \"strip\"\n", &[remove("tabs.style")]),
        ""
    );
}

#[test]
fn a_commented_grouping_table_stays_when_its_last_key_goes() {
    for source in [
        "# Tab bar\n[tabs]\nstyle = \"strip\"\n",
        "[tabs] # bar\nstyle = \"strip\"\n",
        "[tabs]\n# style = \"pill\"\nstyle = \"strip\"\n\n# spare\n",
    ] {
        let output = rendered(source, &[remove("tabs.style")]);
        assert!(output.contains("[tabs]"), "{source:?} -> {output:?}");
    }
    assert_eq!(
        rendered(
            "# Tab bar\n[tabs]\nstyle = \"strip\"\n",
            &[remove("tabs.style")]
        ),
        "# Tab bar\n[tabs]\n"
    );
}

#[test]
fn resetting_a_named_profile_last_key_keeps_the_profile() {
    let source = "[quake.profiles.logs]\nheight = 0.5\n";
    assert_eq!(
        rendered(source, &[remove("quake.profiles.logs.height")]),
        "[quake.profiles.logs]\n"
    );
    // A dotted layout keeps the profile as an empty inline table.
    let source =
        "[quake]\nprofiles.logs.height = 0.5\nprofiles.main.width = 1.0\n";
    assert_eq!(
        rendered(source, &[remove("quake.profiles.logs.height")]),
        "[quake]\nprofiles.logs = {}\nprofiles.main.width = 1.0\n"
    );
}

#[test]
fn dotted_keys_gain_dotted_statements_and_lose_single_statements() {
    let source = "font.size = 13\nfont.family = \"Menlo\" # face\n\n[tabs]\nstyle = \"strip\"\n";
    assert_eq!(
        rendered(source, &[set("font.size", Value::Float(15.0))]),
        "font.size = 15.0\nfont.family = \"Menlo\" # face\n\n[tabs]\nstyle = \"strip\"\n"
    );
    assert_eq!(
        rendered(
            "font.size = 13\n\n[tabs]\nstyle = \"strip\"\n",
            &[set("font.family", "Iosevka".into())]
        ),
        "font.size = 13\nfont.family = \"Iosevka\"\n\n[tabs]\nstyle = \"strip\"\n"
    );
    assert_eq!(
        rendered(source, &[remove("font.size")]),
        "font.family = \"Menlo\" # face\n\n[tabs]\nstyle = \"strip\"\n"
    );
    // The last dotted key of a grouping table takes the table with it.
    assert_eq!(
        rendered(
            "[terminal]\nbell.visual = false\nlinks = true\n",
            &[remove("terminal.bell.visual")]
        ),
        "[terminal]\nlinks = true\n"
    );
    // Dotted keys inside a section.
    assert_eq!(
        rendered(
            "[terminal]\nbell.visual = false\n",
            &[set("terminal.links", Value::Boolean(false))]
        ),
        "[terminal]\nbell.visual = false\nlinks = false\n"
    );
}

#[test]
fn inline_table_fields_change_without_disturbing_siblings() {
    let source = "font = { family = \"Menlo\",  size = 18.0 } # mine\n";
    assert_eq!(
        rendered(source, &[set("font.size", Value::Float(12.0))]),
        "font = { family = \"Menlo\",  size = 12.0 } # mine\n"
    );
    assert_eq!(
        rendered(source, &[remove("font.size")]),
        "font = { family = \"Menlo\" } # mine\n"
    );
    assert_eq!(
        rendered(
            "font = { family = \"Menlo\" } # mine\n",
            &[set("font.size", Value::Float(12.0))]
        ),
        "font = { family = \"Menlo\", size = 12.0 } # mine\n"
    );
}

#[test]
fn a_commented_multiline_inline_table_is_unsupported() {
    let source = "font = {\n  family = \"Menlo\", # face\n  size = 18.0,\n}\n";
    let error =
        render(source, &[set("font.size", Value::Float(12.0))]).unwrap_err();
    assert!(matches!(error, ApplyError::UnsupportedLayout(_)), "{error}");
}

#[test]
fn multiline_values_are_replaced_whole() {
    let source = "[theme]\nansi = [\n  \"#000000\",\n  \"#111111\",\n] # palette\nname = \"x\"\n";
    let output = rendered(source, &[set("theme.name", "y".into())]);
    assert_eq!(output, source.replace("\"x\"", "\"y\""));
    let output = rendered("[keybinding.args]\n", &[]);
    assert_eq!(output, "[keybinding.args]\n");
    let source = "[terminal]\nterm = '''\nauto'''\nlinks = true\n";
    assert_eq!(
        rendered(source, &[set("terminal.term", "xterm-huterm".into())]),
        "[terminal]\nterm = \"xterm-huterm\"\nlinks = true\n"
    );
}

#[test]
fn multiline_statements_are_removed_whole_and_commented_ones_are_not_replaced()
{
    let source = "[theme]\nname = \"x\"\n# Palette\nansi = [\n  \"#000000\", # black\n  \"#111111\",\n] # end\ncursor = \"#ffffff\"\n";
    assert_eq!(
        rendered(source, &[remove("theme.ansi")]),
        "[theme]\nname = \"x\"\ncursor = \"#ffffff\"\n"
    );
    let error = render(source, &[set("theme.ansi", Value::Array(Vec::new()))])
        .unwrap_err();
    assert!(matches!(error, ApplyError::UnsupportedLayout(_)), "{error}");
}

#[test]
fn a_missing_subtable_becomes_its_own_section() {
    let source = "[terminal]\nlinks = true\n\n[[keybinding]]\nkey = \"a\"\ncommand = \"copy\"\n";
    assert_eq!(
        rendered(
            source,
            &[set("terminal.bell.visual", Value::Boolean(false))]
        ),
        "[terminal]\nlinks = true\n\n[terminal.bell]\nvisual = false\n\n[[keybinding]]\nkey = \"a\"\ncommand = \"copy\"\n"
    );
    // Inside an entry, new tables stay in the entry as inline values.
    let output = rendered(
        source,
        &[Patch::SetEntryField {
            locator: locator(source, 0),
            field: "args".into(),
            value: Value::Table(table("index = 2")),
        }],
    );
    assert_eq!(output, format!("{source}args = {{ index = 2 }}\n"));
}

#[test]
fn unknown_fields_are_kept() {
    let source =
        "[future]\n# Not ours\nflavour = \"new\"\n\n[font]\nsize = 14.0\n";
    assert_eq!(
        rendered(source, &[set("font.size", Value::Float(16.0))]),
        "[future]\n# Not ours\nflavour = \"new\"\n\n[font]\nsize = 16.0\n"
    );
}

const BINDINGS: &str = "\
[font]
size = 14.0

# First: copy
[[keybinding]]
key = \"alt-c\"
command = \"copy\"

# Second: select the first tab
[[keybinding]]
key = \"alt-1\"
command = \"select_tab\" # tab one
args = { index = 1 }

# Third: hide
[[keybinding]]
key = \"cmd-h\"
command = \"hide\"
disabled = true
";

#[test]
fn removing_a_middle_entry_keeps_the_other_entries_comments() {
    let output = rendered(
        BINDINGS,
        &[Patch::RemoveEntry {
            locator: locator(BINDINGS, 1),
        }],
    );
    assert_eq!(
        output,
        "\
[font]
size = 14.0

# First: copy
[[keybinding]]
key = \"alt-c\"
command = \"copy\"

# Third: hide
[[keybinding]]
key = \"cmd-h\"
command = \"hide\"
disabled = true
"
    );
    let output = rendered(
        BINDINGS,
        &[Patch::RemoveEntry {
            locator: locator(BINDINGS, 2),
        }],
    );
    assert_eq!(output, BINDINGS.split("\n# Third").next().unwrap());
    let output = rendered(
        BINDINGS,
        &[Patch::RemoveEntry {
            locator: locator(BINDINGS, 0),
        }],
    );
    assert!(
        output.starts_with("[font]\nsize = 14.0\n\n# Second: select"),
        "{output}"
    );
}

#[test]
fn removing_an_entry_before_a_commented_section_keeps_that_comment() {
    let source = "[[keybinding]]\nkey = \"a\"\ncommand = \"copy\"\n# The palette\n[palette]\nplacement = \"center\"\n";
    assert_eq!(
        rendered(
            source,
            &[Patch::RemoveEntry {
                locator: locator(source, 0)
            }]
        ),
        "# The palette\n[palette]\nplacement = \"center\"\n"
    );
}

#[test]
fn removing_an_entry_takes_its_args_subtable_past_an_unrelated_section() {
    let source = "\
[[keybinding]]
key = \"alt-1\"
command = \"select_tab\"

# Unrelated
[font]
size = 13.0 # small

# Its arguments
[keybinding.args]
index = 1

[[keybinding]]
key = \"alt-c\"
command = \"copy\"
";
    assert_eq!(
        rendered(
            source,
            &[Patch::RemoveEntry {
                locator: locator(source, 0)
            }]
        ),
        "\
# Unrelated
[font]
size = 13.0 # small

[[keybinding]]
key = \"alt-c\"
command = \"copy\"
"
    );
    // Setting a field of that entry edits the subtable in place.
    let output = rendered(
        source,
        &[Patch::SetEntryField {
            locator: locator(source, 0),
            field: "args".into(),
            value: Value::Table(table("index = 2")),
        }],
    );
    assert_eq!(output, source.replace("index = 1", "index = 2"));
}

#[test]
fn setting_one_entry_field_keeps_its_other_fields_and_comments() {
    let output = rendered(
        BINDINGS,
        &[Patch::SetEntryField {
            locator: locator(BINDINGS, 1),
            field: "key".into(),
            value: "alt-2".into(),
        }],
    );
    assert_eq!(output, BINDINGS.replace("\"alt-1\"", "\"alt-2\""));
    let output = rendered(
        BINDINGS,
        &[Patch::SetEntryField {
            locator: locator(BINDINGS, 1),
            field: "when".into(),
            value: "Terminal".into(),
        }],
    );
    assert_eq!(
        output,
        BINDINGS.replace(
            "args = { index = 1 }\n",
            "args = { index = 1 }\nwhen = \"Terminal\"\n"
        )
    );
    let output = rendered(
        BINDINGS,
        &[Patch::SetEntryField {
            locator: locator(BINDINGS, 1),
            field: "args".into(),
            value: Value::Table(table("index = 3")),
        }],
    );
    assert_eq!(output, BINDINGS.replace("index = 1", "index = 3"));
    let output = rendered(
        BINDINGS,
        &[Patch::ClearEntryField {
            locator: locator(BINDINGS, 2),
            field: "disabled".into(),
        }],
    );
    assert_eq!(output, BINDINGS.replace("disabled = true\n", ""));
    let output = rendered(
        BINDINGS,
        &[Patch::UpdateEntry {
            locator: locator(BINDINGS, 1),
            entry: table(
                "key = 'alt-1'\ncommand = 'select_tab'\nargs = { index = 2 }\ndisabled = true",
            ),
        }],
    );
    assert_eq!(
        output,
        BINDINGS.replace(
            "args = { index = 1 }\n",
            "args = { index = 2 }\ndisabled = true\n"
        )
    );
}

#[test]
fn appending_an_entry_goes_after_the_last_entry() {
    let entry = table("key = 'cmd-m'\ncommand = 'minimize'\ndisabled = true");
    let output = rendered(
        BINDINGS,
        &[Patch::AppendEntry {
            list: List::Keybinding,
            entry: entry.clone(),
        }],
    );
    assert_eq!(
        output,
        format!(
            "{BINDINGS}\n[[keybinding]]\nkey = \"cmd-m\"\ncommand = \"minimize\"\ndisabled = true\n"
        )
    );
    let output = rendered(
        "[font]\nsize = 14.0\n",
        &[Patch::AppendEntry {
            list: List::Keybinding,
            entry: table(
                "key = 'alt-1'\ncommand = 'select_tab'\nargs = { index = 1 }\nwhen = 'Terminal'",
            ),
        }],
    );
    assert_eq!(
        output,
        "[font]\nsize = 14.0\n\n[[keybinding]]\nkey = \"alt-1\"\ncommand = \"select_tab\"\nwhen = \"Terminal\"\nargs = { index = 1 }\n"
    );
}

#[test]
fn locators_resolve_by_content_or_conflict() {
    let source = "\
[[keybinding]]
key = \"alt-1\"
command = \"select_tab\"
args = { index = 1 }

[[keybinding]]
key = \"alt-1\"
command = \"select_tab\"
args = { index = 2 }
";
    let second = locator(source, 1);
    // Duplicate key and command with different args stay distinct.
    let output = rendered(
        source,
        &[Patch::RemoveEntry {
            locator: second.clone(),
        }],
    );
    assert!(
        output.contains("index = 1") && !output.contains("index = 2"),
        "{output}"
    );
    // An insertion before it moves the entry; content still finds it.
    let inserted = format!(
        "[[keybinding]]\nkey = \"alt-x\"\ncommand = \"copy\"\n\n{source}"
    );
    let output = rendered(
        &inserted,
        &[Patch::RemoveEntry {
            locator: second.clone(),
        }],
    );
    assert!(
        output.contains("index = 1") && !output.contains("index = 2"),
        "{output}"
    );
    // A deletion of the entry itself conflicts.
    let deleted =
        source.split("\n[[keybinding]]").next().unwrap().to_owned() + "\n";
    assert!(matches!(
        render(
            &deleted,
            &[Patch::RemoveEntry {
                locator: second.clone()
            }]
        ),
        Err(ApplyError::Conflict(_))
    ));
    // Reordering keeps resolving by content.
    let reordered = "[[keybinding]]\nkey = \"alt-1\"\ncommand = \"select_tab\"\nargs = { index = 2 }\n\n[[keybinding]]\nkey = \"alt-1\"\ncommand = \"select_tab\"\nargs = { index = 1 }\n";
    let output = rendered(
        reordered,
        &[Patch::RemoveEntry {
            locator: second.clone(),
        }],
    );
    assert!(
        output.contains("index = 1") && !output.contains("index = 2"),
        "{output}"
    );
    // Two exact copies elsewhere are ambiguous.
    let doubled = "[[keybinding]]\nkey = \"alt-x\"\ncommand = \"copy\"\n\n[[keybinding]]\nkey = \"alt-y\"\ncommand = \"copy\"\n\n[[keybinding]]\nkey = \"alt-1\"\ncommand = \"select_tab\"\nargs = { index = 2 }\n\n[[keybinding]]\nkey = \"alt-1\"\ncommand = \"select_tab\"\nargs = { index = 2 }\n";
    assert!(matches!(
        render(doubled, &[Patch::RemoveEntry { locator: second }]),
        Err(ApplyError::Conflict(_))
    ));
}

#[test]
fn field_edits_in_one_call_address_the_entry_located_at_the_start() {
    let second = locator(BINDINGS, 1);
    let output = rendered(
        BINDINGS,
        &[
            Patch::SetEntryField {
                locator: second.clone(),
                field: "key".into(),
                value: "alt-2".into(),
            },
            Patch::SetEntryField {
                locator: second,
                field: "args".into(),
                value: Value::Table(table("index = 2")),
            },
        ],
    );
    assert_eq!(
        output,
        BINDINGS
            .replace("\"alt-1\"", "\"alt-2\"")
            .replace("index = 1", "index = 2")
    );
}

const TWINS: &str = "\
[[keybinding]]
key = \"alt-x\"
command = \"copy\"

[[keybinding]]
key = \"alt-1\"
command = \"select_tab\"

[[keybinding]]
key = \"alt-1\"
command = \"select_tab\"
";

#[test]
fn edits_to_one_of_identical_entries_stay_on_that_entry() {
    let first = locator(TWINS, 1);
    let output = rendered(
        TWINS,
        &[
            Patch::SetEntryField {
                locator: first.clone(),
                field: "key".into(),
                value: "alt-2".into(),
            },
            Patch::SetEntryField {
                locator: first,
                field: "disabled".into(),
                value: Value::Boolean(true),
            },
        ],
    );
    assert_eq!(
        output,
        "\
[[keybinding]]
key = \"alt-x\"
command = \"copy\"

[[keybinding]]
key = \"alt-2\"
command = \"select_tab\"
disabled = true

[[keybinding]]
key = \"alt-1\"
command = \"select_tab\"
"
    );
}

#[test]
fn a_remove_and_an_edit_in_one_call_keep_their_entries() {
    // The removal shifts the edited entry; its identical twin stays put.
    let output = rendered(
        TWINS,
        &[
            Patch::RemoveEntry {
                locator: locator(TWINS, 0),
            },
            Patch::SetEntryField {
                locator: locator(TWINS, 1),
                field: "disabled".into(),
                value: Value::Boolean(true),
            },
        ],
    );
    assert_eq!(
        output,
        "\
[[keybinding]]
key = \"alt-1\"
command = \"select_tab\"
disabled = true

[[keybinding]]
key = \"alt-1\"
command = \"select_tab\"
"
    );
    // An entry edited earlier in the call can still be removed.
    let output = rendered(
        BINDINGS,
        &[
            Patch::SetEntryField {
                locator: locator(BINDINGS, 0),
                field: "key".into(),
                value: "alt-v".into(),
            },
            Patch::RemoveEntry {
                locator: locator(BINDINGS, 0),
            },
        ],
    );
    assert!(
        output.starts_with("[font]\nsize = 14.0\n\n# Second"),
        "{output}"
    );
    // An entry removed earlier in the call is gone.
    assert!(matches!(
        render(
            BINDINGS,
            &[
                Patch::RemoveEntry {
                    locator: locator(BINDINGS, 0),
                },
                Patch::ClearEntryField {
                    locator: locator(BINDINGS, 0),
                    field: "key".into(),
                },
            ]
        ),
        Err(ApplyError::Conflict(_))
    ));
}

#[test]
fn clearing_args_written_as_a_subtable_removes_its_section() {
    let source = "\
[[keybinding]]
key = \"alt-1\"
command = \"select_tab\"

# Its arguments
[keybinding.args]
index = 1

[[keybinding]]
key = \"alt-c\"
command = \"copy\"
";
    let cleared = "\
[[keybinding]]
key = \"alt-1\"
command = \"select_tab\"

[[keybinding]]
key = \"alt-c\"
command = \"copy\"
";
    assert_eq!(
        rendered(
            source,
            &[Patch::ClearEntryField {
                locator: locator(source, 0),
                field: "args".into(),
            }]
        ),
        cleared
    );
    // An update that drops `args` does the same.
    assert_eq!(
        rendered(
            source,
            &[Patch::UpdateEntry {
                locator: locator(source, 0),
                entry: table("key = 'alt-1'\ncommand = 'select_tab'"),
            }]
        ),
        cleared
    );
}

#[test]
fn clearing_subtable_args_keeps_an_unrelated_section_in_between() {
    let source = "\
[[keybinding]]
key = \"alt-1\"
command = \"select_tab\"

# Unrelated
[font]
size = 13.0 # small

# Its arguments
[keybinding.args]
index = 1

[[keybinding]]
key = \"alt-c\"
command = \"copy\"
";
    assert_eq!(
        rendered(
            source,
            &[Patch::ClearEntryField {
                locator: locator(source, 0),
                field: "args".into(),
            }]
        ),
        "\
[[keybinding]]
key = \"alt-1\"
command = \"select_tab\"

# Unrelated
[font]
size = 13.0 # small

[[keybinding]]
key = \"alt-c\"
command = \"copy\"
"
    );
}

#[test]
fn resetting_the_last_key_drops_an_implicit_grouping_parent() {
    assert_eq!(
        rendered(
            "[terminal.bell]\nvisual = false\n",
            &[remove("terminal.bell.visual")]
        ),
        ""
    );
    assert_eq!(
        rendered(
            "[font]\nsize = 14.0\n\n[terminal.bell]\nvisual = false\n",
            &[remove("terminal.bell.visual")]
        ),
        "[font]\nsize = 14.0\n"
    );
}

#[test]
fn resetting_a_dotted_profile_key_keeps_its_trailing_comment() {
    assert_eq!(
        rendered(
            "[quake]\nprofiles.logs.height = 10 # tall\n",
            &[remove("quake.profiles.logs.height")]
        ),
        "[quake]\nprofiles.logs = {} # tall\n"
    );
}

#[test]
fn removing_the_item_under_a_directive_keeps_the_directive() {
    let source = "#:schema ./huterm.json\n[[keybinding]]\nkey = \"cmd-h\"\ncommand = \"hide\"\n";
    assert_eq!(
        rendered(
            source,
            &[Patch::RemoveEntry {
                locator: locator(source, 0)
            }]
        ),
        "#:schema ./huterm.json\n"
    );
    assert_eq!(
        rendered(
            "#:schema ./huterm.json\nfoo = 1\nbar = 2\n",
            &[remove("foo")]
        ),
        "#:schema ./huterm.json\nbar = 2\n"
    );
}

#[test]
fn guards_refuse_splices_that_change_anything_else() {
    let source = "# keep\n[font]\nsize = 14.0\nfamily = \"Menlo\"\n";
    let layout = Layout::parse(source).unwrap();
    let op = Op::Set(keys(&["font".into(), "size".into()]), Value::Float(16.0));
    let edits = splice::splice(&layout, &op).unwrap();
    let good = edits.apply(source);
    splice::verify(&layout, &op, &edits, &good)
        .expect("the real splice passes");
    // A sibling that disappeared.
    let error =
        splice::verify(&layout, &op, &edits, "# keep\n[font]\nsize = 16.0\n")
            .unwrap_err();
    assert!(matches!(error, ApplyError::Guard(_)), "{error}");
    // A comment that disappeared.
    let error = splice::verify(
        &layout,
        &op,
        &edits,
        "[font]\nsize = 16.0\nfamily = \"Menlo\"\n",
    )
    .unwrap_err();
    assert!(
        matches!(error, ApplyError::Guard(ref message) if message.contains("# keep")),
        "{error}"
    );
    // A value changed too far.
    let error = splice::verify(
        &layout,
        &op,
        &edits,
        "# keep\n[font]\nsize = 17.0\nfamily = \"Menlo\"\n",
    )
    .unwrap_err();
    assert!(matches!(error, ApplyError::Guard(_)), "{error}");
}

/// A temporary directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "huterm-document-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the validator signature returns a result"
)]
fn accept(_: &str) -> Result<(), String> {
    Ok(())
}

/// Rejects files whose values fail `RawConfig` validation.
fn validate(text: &str) -> Result<(), String> {
    let raw: crate::RawConfig =
        toml::from_str(text).map_err(|error| error.to_string())?;
    raw.validate_values().map_err(|error| error.to_string())
}

#[test]
fn a_disjoint_external_edit_rebases_and_an_overlapping_one_conflicts() {
    let scratch = Scratch::new();
    let file = ConfigFile::new(scratch.path("config.toml"));
    fs::write(file.logical_path(), "[font]\nsize = 14.0\n").unwrap();
    let base = file.load().unwrap();
    fs::write(
        file.logical_path(),
        "[font]\nsize = 14.0\nfamily = \"Iosevka\"\n",
    )
    .unwrap();
    let written = file
        .apply(&base, &[set("font.size", Value::Float(16.0))], &accept)
        .unwrap();
    assert_eq!(
        fs::read_to_string(file.logical_path()).unwrap(),
        "[font]\nsize = 16.0\nfamily = \"Iosevka\"\n"
    );
    assert_eq!(
        written.text(),
        Some("[font]\nsize = 16.0\nfamily = \"Iosevka\"\n")
    );
    assert_eq!(written, file.load().unwrap());
    // An overlapping edit conflicts and writes nothing.
    fs::write(
        file.logical_path(),
        "[font]\nsize = 20.0\nfamily = \"Iosevka\"\n",
    )
    .unwrap();
    let error = file
        .apply(&written, &[set("font.size", Value::Float(12.0))], &accept)
        .unwrap_err();
    assert!(
        matches!(&error, ApplyError::Conflict(changes) if changes == &["font.size changed"]),
        "{error}"
    );
    assert_eq!(
        fs::read_to_string(file.logical_path()).unwrap(),
        "[font]\nsize = 20.0\nfamily = \"Iosevka\"\n"
    );
}

#[test]
fn an_invalid_current_file_writes_nothing() {
    let scratch = Scratch::new();
    let file = ConfigFile::new(scratch.path("config.toml"));
    for source in [
        "[font\nsize = 14.0\n",
        "[tabs]\nmin_width = 300\nmax_width = 200\n",
    ] {
        fs::write(file.logical_path(), source).unwrap();
        let base = file.load().unwrap();
        let error = file
            .apply(&base, &[set("font.size", Value::Float(16.0))], &validate)
            .unwrap_err();
        assert!(matches!(error, ApplyError::InvalidFile(_)), "{error}");
        assert_eq!(fs::read_to_string(file.logical_path()).unwrap(), source);
    }
}

#[test]
fn a_candidate_the_validator_rejects_writes_nothing() {
    let scratch = Scratch::new();
    let file = ConfigFile::new(scratch.path("config.toml"));
    let source = "[tabs]\nmax_width = 200\n";
    fs::write(file.logical_path(), source).unwrap();
    let base = file.load().unwrap();
    let error = file
        .apply(
            &base,
            &[set("tabs.min_width", Value::Float(300.0))],
            &validate,
        )
        .unwrap_err();
    assert!(
        matches!(&error, ApplyError::Rejected(message) if message.contains("min_width")),
        "{error}"
    );
    assert_eq!(fs::read_to_string(file.logical_path()).unwrap(), source);
    assert_eq!(
        fs::read_dir(&scratch.0).unwrap().count(),
        1,
        "no temporary file left"
    );
}

#[test]
fn a_missing_file_and_its_parents_are_created() {
    let scratch = Scratch::new();
    let file = ConfigFile::new(scratch.path("config/huterm/config.toml"));
    let base = file.load().unwrap();
    assert!(base.revision().is_absent());
    file.apply(
        &base,
        &[set("general.watch_config", Value::Boolean(false))],
        &validate,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(file.logical_path()).unwrap(),
        "[general]\nwatch_config = false\n"
    );
    // A base that saw the file refuses to recreate it once removed.
    let present = file.load().unwrap();
    fs::remove_file(file.logical_path()).unwrap();
    assert!(matches!(
        file.apply(
            &present,
            &[set("font.size", Value::Float(12.0))],
            &validate
        ),
        Err(ApplyError::Conflict(_))
    ));
    assert!(!file.logical_path().exists());
}

#[cfg(unix)]
#[test]
fn permissions_survive_and_symlinks_are_written_through() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    let scratch = Scratch::new();
    let target = scratch.path("dotfiles.toml");
    fs::write(&target, "[font]\nsize = 14.0\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let link = scratch.path("config.toml");
    symlink(&target, &link).unwrap();
    let file = ConfigFile::new(&link);
    let base = file.load().unwrap();
    file.apply(&base, &[set("font.size", Value::Float(12.0))], &accept)
        .unwrap();
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "[font]\nsize = 12.0\n"
    );
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // A dangling link is refused.
    fs::remove_file(&target).unwrap();
    assert!(matches!(file.load(), Err(ApplyError::DanglingSymlink(_))));
    assert!(matches!(
        file.apply(&base, &[set("font.size", Value::Float(13.0))], &accept),
        Err(ApplyError::DanglingSymlink(_))
    ));
    assert!(!target.exists());
}

/// Fails at one stage, or runs `at_recheck` at each recheck, and records
/// every stage it reaches.
struct TestHooks<'a> {
    fail: Option<Stage>,
    at_recheck: &'a dyn Fn(usize),
    rechecks: Cell<usize>,
    reached: RefCell<Vec<Stage>>,
}

impl Hooks for TestHooks<'_> {
    fn before(&self, stage: Stage) -> io::Result<()> {
        self.reached.borrow_mut().push(stage);
        if stage == Stage::Recheck {
            let count = self.rechecks.get();
            self.rechecks.set(count + 1);
            (self.at_recheck)(count);
        }
        if self.fail == Some(stage) {
            return Err(io::Error::other(format!("injected at {stage:?}")));
        }
        Ok(())
    }
}

fn hooks(fail: Option<Stage>, at_recheck: &dyn Fn(usize)) -> TestHooks<'_> {
    TestHooks {
        fail,
        at_recheck,
        rechecks: Cell::new(0),
        reached: RefCell::default(),
    }
}

#[test]
fn failures_before_the_rename_leave_the_original_and_no_temporary() {
    for stage in [
        Stage::TempWrite,
        Stage::TempSync,
        Stage::Recheck,
        Stage::Rename,
    ] {
        let scratch = Scratch::new();
        let file = ConfigFile::new(scratch.path("config.toml"));
        fs::write(file.logical_path(), "[font]\nsize = 14.0\n").unwrap();
        let base = file.load().unwrap();
        let error = file
            .apply_with(
                &base,
                &[set("font.size", Value::Float(12.0))],
                &accept,
                &hooks(Some(stage), &|_| {}),
            )
            .unwrap_err();
        assert!(matches!(error, ApplyError::Io(_)), "{stage:?}: {error}");
        assert_eq!(
            fs::read_to_string(file.logical_path()).unwrap(),
            "[font]\nsize = 14.0\n"
        );
        assert_eq!(
            fs::read_dir(&scratch.0).unwrap().count(),
            1,
            "{stage:?} left a temporary"
        );
    }
}

#[test]
fn failures_after_the_rename_report_written_not_durable() {
    for (stage, nested) in
        [(Stage::DirectorySync, false), (Stage::ParentSync, true)]
    {
        let scratch = Scratch::new();
        let path = if nested {
            scratch.path("new/config.toml")
        } else {
            scratch.path("config.toml")
        };
        let file = ConfigFile::new(&path);
        let base = file.load().unwrap();
        let error = file
            .apply_with(
                &base,
                &[set("font.size", Value::Float(12.0))],
                &accept,
                &hooks(Some(stage), &|_| {}),
            )
            .unwrap_err();
        assert!(
            matches!(error, ApplyError::WrittenNotDurable(_)),
            "{stage:?}: {error}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "[font]\nsize = 12.0\n");
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_retargeted_before_the_rename_restarts_then_conflicts() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new();
    let original = scratch.path("a.toml");
    fs::write(&original, "[font]\nsize = 14.0\n").unwrap();
    let hard = scratch.path("hard.toml");
    fs::hard_link(&original, &hard).unwrap();
    let link = scratch.path("config.toml");
    symlink(&original, &link).unwrap();
    let file = ConfigFile::new(&link);
    let base = file.load().unwrap();
    // A link retargeted to a hard link of the original keeps identity and
    // content; only the pathname changes. The restarted attempt rebases
    // and writes through the new target.
    let retarget = |count: usize| {
        if count == 0 {
            fs::remove_file(&link).unwrap();
            symlink(&hard, &link).unwrap();
        }
    };
    file.apply_with(
        &base,
        &[set("font.size", Value::Float(12.0))],
        &accept,
        &hooks(None, &retarget),
    )
    .unwrap();
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&hard).unwrap(), "[font]\nsize = 12.0\n");
    // A second change during the restarted attempt conflicts without
    // renaming.
    let base = file.load().unwrap();
    let edits = |count: usize| {
        fs::write(
            &hard,
            format!("[font]\nsize = 12.0\nfamily = \"F{count}\"\n"),
        )
        .unwrap();
    };
    let error = file
        .apply_with(
            &base,
            &[set("font.size", Value::Float(10.0))],
            &accept,
            &hooks(None, &edits),
        )
        .unwrap_err();
    assert!(matches!(error, ApplyError::Conflict(_)), "{error}");
    assert_eq!(
        fs::read_to_string(&hard).unwrap(),
        "[font]\nsize = 12.0\nfamily = \"F1\"\n"
    );
    assert_eq!(
        fs::read_dir(&scratch.0).unwrap().count(),
        3,
        "no temporary left"
    );
}

#[test]
fn a_restart_still_syncs_the_parents_it_created() {
    let scratch = Scratch::new();
    let path = scratch.path("new/config.toml");
    let file = ConfigFile::new(&path);
    let base = file.load().unwrap();
    // Another writer creates the file first, so the attempt restarts and
    // rebases onto it after the directory already exists.
    let create = |count: usize| {
        if count == 0 {
            fs::write(&path, "[tabs]\nstyle = \"strip\"\n").unwrap();
        }
    };
    let recorded = hooks(None, &create);
    file.apply_with(
        &base,
        &[set("font.size", Value::Float(12.0))],
        &accept,
        &recorded,
    )
    .unwrap();
    assert_eq!(recorded.rechecks.get(), 2, "the attempt restarted");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "[tabs]\nstyle = \"strip\"\n\n[font]\nsize = 12.0\n"
    );
    let parent_syncs = recorded
        .reached
        .borrow()
        .iter()
        .filter(|stage| **stage == Stage::ParentSync)
        .count();
    assert_eq!(parent_syncs, 1, "{:?}", recorded.reached.borrow());
}

#[cfg(unix)]
#[test]
fn a_link_left_dangling_at_the_recheck_restarts() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new();
    let target = scratch.path("dotfiles.toml");
    fs::write(&target, "[font]\nsize = 14.0\n").unwrap();
    let link = scratch.path("config.toml");
    symlink(&target, &link).unwrap();
    let file = ConfigFile::new(&link);
    let base = file.load().unwrap();
    // The restarted attempt finds the dangling link, as any first attempt
    // would.
    let unlink = |count: usize| {
        if count == 0 {
            fs::remove_file(&target).unwrap();
        }
    };
    let error = file
        .apply_with(
            &base,
            &[set("font.size", Value::Float(12.0))],
            &accept,
            &hooks(None, &unlink),
        )
        .unwrap_err();
    assert!(matches!(error, ApplyError::DanglingSymlink(_)), "{error}");
    assert!(!target.exists());
    assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 1);
    // Dangling during the restarted attempt is a second change.
    fs::write(&target, "[font]\nsize = 14.0\n").unwrap();
    let base = file.load().unwrap();
    let change_then_unlink = |count: usize| {
        if count == 0 {
            fs::write(&target, "[font]\nsize = 14.0\nfamily = \"F\"\n")
                .unwrap();
        } else {
            fs::remove_file(&target).unwrap();
        }
    };
    let error = file
        .apply_with(
            &base,
            &[set("font.size", Value::Float(12.0))],
            &accept,
            &hooks(None, &change_then_unlink),
        )
        .unwrap_err();
    assert!(matches!(error, ApplyError::Conflict(_)), "{error}");
    assert!(!target.exists());
    assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 1);
}

#[test]
fn stale_temporaries_are_removed_and_fresh_ones_kept() {
    let scratch = Scratch::new();
    let file = ConfigFile::new(scratch.path("config.toml"));
    let stale = scratch.path(".config.toml.huterm-tmp-1-1");
    let fresh = scratch.path(".config.toml.huterm-tmp-1-2");
    let unrelated = scratch.path(".other.huterm-tmp-1-1");
    for path in [&stale, &fresh, &unrelated] {
        fs::write(path, "").unwrap();
    }
    let old = SystemTime::now() - Duration::from_mins(11);
    for path in [&stale, &unrelated] {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    let base = file.load().unwrap();
    file.apply(&base, &[set("font.size", Value::Float(12.0))], &accept)
        .unwrap();
    assert!(!stale.exists());
    assert!(fresh.exists());
    assert!(unrelated.exists());
}

#[test]
fn entries_locate_every_entry_and_a_rebased_locator_finds_its_change() {
    let source = "[[keybinding]]\nkey = \"a\"\ncommand = \"new_tab\"\n\n\
                  [[keybinding]]\nkey = \"b\"\ncommand = \"new_tab\"\n";
    let snapshot = snapshot_of(source);
    let entries = snapshot.entries(List::Keybinding);
    assert_eq!(
        entries,
        vec![locator(source, 0), locator(source, 1)],
        "one parse gives what `entry` gives"
    );
    // After the caller set `when` on the second entry, its old locator no
    // longer matches; the rebased one does.
    let changed = rendered(
        source,
        &[Patch::SetEntryField {
            locator: entries[1].clone(),
            field: "when".into(),
            value: Value::String("Terminal".into()),
        }],
    );
    let mut content = entries[1].content().clone();
    content.insert("when".into(), Value::String("Terminal".into()));
    let rebased = entries[1].rebased(1, content);
    assert_eq!(rebased.index(), 1);
    let removed =
        rendered(&changed, &[Patch::RemoveEntry { locator: rebased }]);
    assert_eq!(
        removed,
        "[[keybinding]]\nkey = \"a\"\ncommand = \"new_tab\"\n"
    );
}
