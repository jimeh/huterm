use super::*;
use std::collections::HashSet;

fn text(name: &str, value: &str) -> CommandArgument {
    CommandArgument::new(name, CommandValue::Text(value.into()))
}

#[test]
fn catalog_entries_are_unique_and_described() {
    let mut seen = HashSet::new();
    for spec in catalog() {
        assert!(seen.insert(spec.id), "duplicate id {}", spec.id);
        assert!(!spec.title.trim().is_empty(), "{} lacks a title", spec.id);
        assert!(
            !spec.description.trim().is_empty(),
            "{} lacks a description",
            spec.id
        );
        assert_eq!(lookup(spec.id.as_str()), Some(spec));
        assert!(
            matches!(
                spec.context,
                None | Some("Palette" | "confirming" | "menu" | "notices")
            ),
            "{} has unknown context {:?}",
            spec.id,
            spec.context
        );
        for argument in spec.args {
            let Some(group) = argument.group() else {
                continue;
            };
            let members = spec
                .args
                .iter()
                .filter(|member| member.group() == Some(group))
                .collect::<Vec<_>>();
            assert!(
                members.len() >= 2,
                "{} group {group} has fewer than two members",
                spec.id
            );
            assert!(
                members.iter().any(|member| member.prompt),
                "{} group {group} has no prompted member",
                spec.id
            );
        }
    }
    assert_eq!(lookup("unbind"), None);
}

#[test]
fn unknown_command_is_rejected() {
    let bogus = CommandId::new("bogus");
    assert_eq!(
        validate(&CommandInvocation::new(bogus, Vec::new())),
        Err(CommandError::UnknownCommand(bogus))
    );
}

#[test]
fn open_command_palette_is_argument_free_and_window_scoped() {
    let invocation =
        CommandInvocation::new(ids::OPEN_COMMAND_PALETTE, Vec::new());
    let spec = validate(&invocation).unwrap();
    assert_eq!(spec.scope, CommandScope::Window);
    assert!(spec.args.is_empty());
}

#[test]
fn argument_names_must_be_declared_and_unique() {
    assert_eq!(
        validate(&CommandInvocation::new(
            ids::NEW_TAB,
            vec![CommandArgument::new("index", CommandValue::Integer(1))]
        )),
        Err(CommandError::UnknownArgument {
            command: ids::NEW_TAB,
            name: "index".into()
        })
    );
    assert_eq!(
        validate(&CommandInvocation::new(
            ids::RENAME_TAB,
            vec![text("name", "one"), text("name", "two")]
        )),
        Err(CommandError::DuplicateArgument {
            command: ids::RENAME_TAB,
            name: "name".into()
        })
    );
}

#[test]
fn required_arguments_are_enforced_and_optional_ones_may_be_omitted() {
    let tab = TabId::new(7);
    assert_eq!(
        validate(&CommandInvocation::new(
            ids::RENAME_TAB,
            vec![CommandArgument::new("tab", CommandValue::Tab(tab))]
        )),
        Err(CommandError::MissingArgument {
            command: ids::RENAME_TAB,
            name: "name"
        })
    );
    let trailing_omitted =
        CommandInvocation::new(ids::RENAME_TAB, vec![text("name", "work")]);
    assert_eq!(
        validate(&trailing_omitted).map(|s| s.id),
        Ok(ids::RENAME_TAB)
    );
    assert_eq!(trailing_omitted.text("name"), Some("work"));
    assert_eq!(trailing_omitted.tab("tab"), None);
    let reordered = CommandInvocation::new(
        ids::RENAME_TAB,
        vec![
            CommandArgument::new("tab", CommandValue::Tab(tab)),
            text("name", "work"),
        ],
    );
    assert!(validate(&reordered).is_ok());
    assert_eq!(reordered.tab("tab"), Some(tab));
    assert_eq!(reordered.text("tab"), None);

    let by_index = CommandInvocation::new(
        ids::SELECT_TAB,
        vec![CommandArgument::new("index", CommandValue::Integer(3))],
    );
    assert!(validate(&by_index).is_ok());
    let by_tab = CommandInvocation::new(
        ids::SELECT_TAB,
        vec![CommandArgument::new("tab", CommandValue::Tab(tab))],
    );
    assert!(validate(&by_tab).is_ok());
}

#[test]
fn argument_types_and_integer_ranges_are_checked() {
    assert_eq!(
        validate(&CommandInvocation::new(
            ids::SELECT_TAB,
            vec![text("index", "1")]
        )),
        Err(CommandError::ArgumentType {
            command: ids::SELECT_TAB,
            name: "index",
            expected: ArgumentKind::Integer { min: 1, max: 9 }
        })
    );
    for (value, ok) in [(0, false), (1, true), (9, true), (10, false)] {
        let result = validate(&CommandInvocation::new(
            ids::SELECT_TAB,
            vec![CommandArgument::new("index", CommandValue::Integer(value))],
        ));
        if ok {
            assert!(result.is_ok(), "index {value}");
        } else {
            assert_eq!(
                result,
                Err(CommandError::ArgumentRange {
                    command: ids::SELECT_TAB,
                    name: "index",
                    value,
                    min: 1,
                    max: 9
                })
            );
        }
    }
}

#[test]
fn validate_supplied_accepts_partial_invocations_and_rejects_bad_values() {
    let bare = CommandInvocation::new(ids::SELECT_TAB, Vec::new());
    assert_eq!(
        validate_supplied(&bare).map(|spec| spec.id),
        Ok(ids::SELECT_TAB)
    );

    assert_eq!(
        validate_supplied(&CommandInvocation::new(
            ids::SELECT_TAB,
            vec![CommandArgument::new("index", CommandValue::Integer(10))]
        )),
        Err(CommandError::ArgumentRange {
            command: ids::SELECT_TAB,
            name: "index",
            value: 10,
            min: 1,
            max: 9,
        })
    );
    assert_eq!(
        validate_supplied(&CommandInvocation::new(
            ids::SELECT_TAB,
            vec![text("index", "1")]
        )),
        Err(CommandError::ArgumentType {
            command: ids::SELECT_TAB,
            name: "index",
            expected: ArgumentKind::Integer { min: 1, max: 9 },
        })
    );

    let conflicting = CommandInvocation::new(
        ids::SELECT_TAB,
        vec![
            CommandArgument::new("tab", CommandValue::Tab(TabId::new(2))),
            CommandArgument::new("index", CommandValue::Integer(1)),
        ],
    );
    let conflict = CommandError::ConflictingArguments {
        command: ids::SELECT_TAB,
        group: "target",
        names: vec!["index", "tab"],
    };
    assert_eq!(validate_supplied(&conflicting), Err(conflict.clone()));
    assert_eq!(validate(&conflicting), Err(conflict));

    assert_eq!(
        validate_supplied(&CommandInvocation::new(
            ids::SELECT_TAB,
            vec![CommandArgument::new("position", CommandValue::Integer(1))]
        )),
        Err(CommandError::UnknownArgument {
            command: ids::SELECT_TAB,
            name: "position".into(),
        })
    );
}

#[test]
fn validate_requires_exactly_one_group_member() {
    assert_eq!(
        validate(&CommandInvocation::new(ids::SELECT_TAB, Vec::new())),
        Err(CommandError::MissingArgument {
            command: ids::SELECT_TAB,
            name: "tab",
        })
    );
    assert!(
        validate(&CommandInvocation::new(
            ids::SELECT_TAB,
            vec![CommandArgument::new("index", CommandValue::Integer(3))]
        ))
        .is_ok()
    );
    assert!(
        validate(&CommandInvocation::new(
            ids::SELECT_TAB,
            vec![CommandArgument::new(
                "tab",
                CommandValue::Tab(TabId::new(3))
            )]
        ))
        .is_ok()
    );
}

#[test]
fn missing_prompted_lists_only_what_a_client_must_collect() {
    let select_tab = lookup(ids::SELECT_TAB.as_str()).unwrap();
    let bare_select_tab = CommandInvocation::new(ids::SELECT_TAB, Vec::new());
    assert_eq!(
        select_tab
            .missing_prompted(&bare_select_tab)
            .iter()
            .map(|argument| argument.name)
            .collect::<Vec<_>>(),
        vec!["tab"]
    );
    let select_by_index = CommandInvocation::new(
        ids::SELECT_TAB,
        vec![CommandArgument::new("index", CommandValue::Integer(2))],
    );
    assert!(select_tab.missing_prompted(&select_by_index).is_empty());

    let rename_tab = lookup(ids::RENAME_TAB.as_str()).unwrap();
    assert_eq!(
        rename_tab
            .missing_prompted(&CommandInvocation::new(
                ids::RENAME_TAB,
                Vec::new()
            ))
            .iter()
            .map(|argument| argument.name)
            .collect::<Vec<_>>(),
        vec!["name"]
    );

    let toggle_quake = lookup(ids::TOGGLE_QUAKE.as_str()).unwrap();
    assert!(
        toggle_quake
            .missing_prompted(&CommandInvocation::new(
                ids::TOGGLE_QUAKE,
                Vec::new()
            ))
            .is_empty()
    );
}

#[test]
fn quake_profile_arguments_carry_text() {
    assert!(
        validate(&CommandInvocation::new(
            ids::TOGGLE_QUAKE,
            vec![text("profile", "logs")]
        ))
        .is_ok()
    );
    assert_eq!(
        validate(&CommandInvocation::new(
            ids::TOGGLE_QUAKE,
            vec![CommandArgument::new("profile", CommandValue::Integer(1))]
        )),
        Err(CommandError::ArgumentType {
            command: ids::TOGGLE_QUAKE,
            name: "profile",
            expected: ArgumentKind::QuakeProfile,
        })
    );
}

#[test]
fn palette_commands_are_palette_scoped_and_context_bound() {
    let expected = [
        ids::PALETTE_SELECT_NEXT,
        ids::PALETTE_SELECT_PREVIOUS,
        ids::PALETTE_PAGE_DOWN,
        ids::PALETTE_PAGE_UP,
        ids::PALETTE_CONFIRM,
        ids::PALETTE_BACK,
        ids::PALETTE_POP,
        ids::PALETTE_EXPAND,
        ids::PALETTE_NEXT_SLOT,
        ids::PALETTE_PREVIOUS_SLOT,
        ids::TEXT_DELETE_BACKWARD,
        ids::TEXT_DELETE_FORWARD,
        ids::TEXT_DELETE_WORD_BACKWARD,
        ids::TEXT_DELETE_WORD_FORWARD,
        ids::TEXT_DELETE_LINE_START,
        ids::TEXT_MOVE_LEFT,
        ids::TEXT_MOVE_RIGHT,
        ids::TEXT_MOVE_WORD_LEFT,
        ids::TEXT_MOVE_WORD_RIGHT,
        ids::TEXT_LINE_START,
        ids::TEXT_LINE_END,
        ids::TEXT_SELECT_ALL,
        ids::TEXT_COPY,
        ids::TEXT_PASTE,
    ];
    for id in expected {
        let spec = lookup(id.as_str()).unwrap();
        assert_eq!(spec.scope, CommandScope::Palette, "{id}");
        assert_eq!(spec.context, Some("Palette"), "{id}");
        assert!(only_select_argument(spec), "{id}");
    }
    for spec in catalog().iter().filter(|spec| {
        spec.id.as_str().starts_with("palette_")
            || spec.id.as_str().starts_with("text_")
    }) {
        assert_eq!(spec.scope, CommandScope::Palette, "{}", spec.id);
        assert_eq!(spec.context, Some("Palette"), "{}", spec.id);
        assert!(only_select_argument(spec), "{}", spec.id);
    }
}

/// Palette commands take no prompted arguments; movement commands may
/// take the unprompted `select` flag.
fn only_select_argument(spec: &CommandSpec) -> bool {
    spec.args.iter().all(|argument| {
        argument.name == "select"
            && argument.kind == ArgumentKind::Bool
            && argument.required == Requirement::Optional
            && !argument.prompt
    })
}

/// A command that cannot run bare needs a prompted argument so the palette
/// can collect it. Commands whose arguments are all optional run
/// immediately on their defaults and may leave every argument unprompted.
#[test]
fn non_palette_commands_needing_arguments_offer_a_prompted_argument() {
    for spec in catalog().iter().filter(|spec| {
        spec.scope != CommandScope::Palette
            && spec
                .args
                .iter()
                .any(|argument| argument.required != Requirement::Optional)
    }) {
        assert!(
            spec.args.iter().any(|argument| argument.prompt),
            "command `{}` needs arguments but prompts for none",
            spec.id
        );
    }
}

#[test]
fn dialog_menu_and_notice_commands_are_window_scoped_and_context_bound() {
    let expected = [
        ("confirming", ids::DIALOG_CONFIRM),
        ("confirming", ids::DIALOG_CANCEL),
        ("confirming", ids::DIALOG_FOCUS_NEXT),
        ("confirming", ids::DIALOG_FOCUS_PREVIOUS),
        ("menu", ids::MENU_SELECT_NEXT),
        ("menu", ids::MENU_SELECT_PREVIOUS),
        ("menu", ids::MENU_SELECT_FIRST),
        ("menu", ids::MENU_SELECT_LAST),
        ("menu", ids::MENU_SELECT_RIGHT),
        ("menu", ids::MENU_SELECT_LEFT),
        ("menu", ids::MENU_CONFIRM),
        ("menu", ids::MENU_CLOSE),
        ("notices", ids::NOTICE_NEXT),
        ("notices", ids::NOTICE_PREVIOUS),
        ("notices", ids::NOTICE_RUN_ACTION),
        ("notices", ids::NOTICE_DISMISS),
    ];
    for (context, id) in expected {
        let spec = lookup(id.as_str()).unwrap();
        assert_eq!(spec.scope, CommandScope::Window, "{id}");
        assert_eq!(spec.context, Some(context), "{id}");
        assert!(spec.args.is_empty(), "{id}");
        assert!(validate(&CommandInvocation::new(id, Vec::new())).is_ok());
    }
    for spec in catalog().iter().filter(|spec| {
        spec.id.as_str().starts_with("dialog_")
            || spec.id.as_str().starts_with("menu_")
            || spec.id.as_str().starts_with("notice_")
    }) {
        assert!(
            expected.iter().any(|(_, id)| *id == spec.id),
            "{} is not covered above",
            spec.id
        );
    }
    // Entry points into those contexts are unconditional so they can be
    // bound and listed anywhere.
    for id in [ids::OPEN_MENU, ids::FOCUS_NOTICES, ids::DISMISS_ALL_NOTICES] {
        let spec = validate(&CommandInvocation::new(id, Vec::new())).unwrap();
        assert_eq!(spec.scope, CommandScope::Window, "{id}");
        assert_eq!(spec.context, None, "{id}");
        assert!(spec.args.is_empty(), "{id}");
    }
}

#[test]
fn tab_targeted_commands_take_an_unprompted_optional_tab() {
    let tab = TabId::new(4);
    for id in [
        ids::CLOSE_TAB,
        ids::CLOSE_OTHER_TABS,
        ids::CLOSE_TABS_AFTER,
        ids::COPY_TAB_DIRECTORY,
        ids::OPEN_TAB_DIRECTORY,
    ] {
        let bare = CommandInvocation::new(id, Vec::new());
        let spec = validate(&bare).unwrap();
        assert_eq!(spec.scope, CommandScope::Window, "{id}");
        assert_eq!(spec.context, None, "{id}");
        assert_eq!(
            spec.args,
            &[ArgumentSpec {
                name: "tab",
                kind: ArgumentKind::Tab,
                required: Requirement::Optional,
                prompt: false,
            }],
            "{id}"
        );
        // The palette runs these immediately on the active tab.
        assert!(spec.missing_prompted(&bare).is_empty(), "{id}");

        let targeted = CommandInvocation::new(
            id,
            vec![CommandArgument::new("tab", CommandValue::Tab(tab))],
        );
        assert!(validate(&targeted).is_ok(), "{id}");
        assert_eq!(targeted.tab("tab"), Some(tab));
        assert_eq!(
            validate(&CommandInvocation::new(
                id,
                vec![CommandArgument::new("tab", CommandValue::Integer(1))]
            )),
            Err(CommandError::ArgumentType {
                command: id,
                name: "tab",
                expected: ArgumentKind::Tab,
            }),
            "{id}"
        );
    }
}

#[test]
fn recent_tab_command_validates() {
    let recent = CommandInvocation::new(ids::SELECT_RECENT_TAB, Vec::new());
    let spec = validate(&recent).unwrap();
    assert_eq!(spec.scope, CommandScope::Window);
    assert!(spec.args.is_empty());
    assert_eq!(
        validate(&CommandInvocation::new(
            ids::SELECT_RECENT_TAB,
            vec![CommandArgument::new(
                "tab",
                CommandValue::Tab(TabId::new(1))
            )]
        )),
        Err(CommandError::UnknownArgument {
            command: ids::SELECT_RECENT_TAB,
            name: "tab".into(),
        })
    );
}

#[test]
fn errors_display_their_context() {
    let error = CommandError::ArgumentRange {
        command: ids::SELECT_TAB,
        name: "index",
        value: 12,
        min: 1,
        max: 9,
    };
    assert_eq!(
        error.to_string(),
        "command `select_tab` argument `index` is 12; expected 1 to 9"
    );
    let conflict = CommandError::ConflictingArguments {
        command: ids::SELECT_TAB,
        group: "target",
        names: vec!["index", "tab"],
    };
    assert_eq!(
        conflict.to_string(),
        "command `select_tab` accepts only one of `index`, `tab`"
    );
    let source: &dyn std::error::Error = &error;
    assert!(source.source().is_none());
}
