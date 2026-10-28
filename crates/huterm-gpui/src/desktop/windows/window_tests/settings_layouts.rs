//! The Settings window's Pane layouts card through the desktop harness:
//! new, edited, renamed, and deleted user layouts, and the layout cycle.

use huterm_config::settings::SectionId;

use super::super::settings_window::SettingsView;
use super::settings::{
    ScratchConfig, conflicted, settings, settled, with_view, with_window,
};
use super::settings_controls::{a11y_node, focus, keys, show, visual};
use super::*;
use crate::desktop::key_hint::KeyHint;
use crate::keymap::Platform;

const DEV: &str = "panes.layouts.dev";
const WORK: &str = "panes.layouts.work";
const CYCLE: &str = "panes.layout_cycle";

/// Opens Settings on the Window and panes section.
fn open_window_section(harness: &mut DesktopHarness, file: &ScratchConfig) {
    show(harness, file, SectionId::Window);
}

/// Waits for every save to finish and for the file to satisfy `check`.
fn saved(
    harness: &mut DesktopHarness,
    file: &ScratchConfig,
    check: impl Fn(&str) -> bool,
) {
    harness.wait_for("the save", |cx| settled(cx) && check(&file.text()));
}

/// Runs the control an action key names, as Space on it does.
fn press(harness: &mut DesktopHarness, key: &str) {
    with_window(harness, |view, window, cx| view.press(key, window, cx));
    visual(harness).run_until_parked();
}

fn dialog(harness: &mut DesktopHarness) -> &'static str {
    with_view(harness, |view, _| view.dialog_name())
}

/// Waits until the open dialog is `name`, once every save has finished.
fn dialog_shows(harness: &mut DesktopHarness, name: &'static str) {
    harness.wait_for(name, |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.dialog_name() == name)
                    .unwrap_or(false)
            })
    });
    visual(harness).run_until_parked();
}

/// Waits until the running configuration has the layout `name`.
fn applied(harness: &mut DesktopHarness, name: &str) {
    harness.wait_for("the reload", |cx| {
        let desktop = cx.global::<Desktop>();
        !desktop.reloads.reloading()
            && desktop.config.panes.layout(name).is_some()
    });
}

const COMMENTED: &str = "# Mine.\n[tabs]\nposition = \"top\" # keep\n";

#[gpui::test]
fn new_layout_writes_columns_and_focuses_its_template(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("layouts-new", COMMENTED);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.action_handle("lnew").focus(window, cx);
    });
    keys(&harness, "enter");
    assert_eq!(dialog(&mut harness), "new-layout");
    assert_eq!(focus(&mut harness), "dialog:field");
    keys(&harness, "d e v enter");
    saved(&mut harness, &file, |text| text.contains("dev"));
    // Comments elsewhere in the file survive, and the cycle stays as it is.
    assert_eq!(
        file.text(),
        format!("{COMMENTED}\n[panes.layouts]\ndev = \"columns(*)\"\n")
    );
    visual(&harness).run_until_parked();
    assert_eq!(dialog(&mut harness), "none");
    assert_eq!(focus(&mut harness), DEV);
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.field_value(DEV, cx), "columns(*)");
        assert_eq!(view.layout_names(), ["dev"]);
        assert!(!view.cycle_names().contains(&"dev".to_owned()));
    });
    // Typing replaces the whole template.
    let selection =
        with_view(&mut harness, |view, cx| view.field_selection(DEV, cx));
    assert_eq!(selection, 0.."columns(*)".len());
    // Its save applies it, so a binding or the palette can use it at once.
    applied(&mut harness, "dev");
}

#[gpui::test]
fn an_invalid_built_in_or_taken_name_disables_create_and_says_why(
    cx: &mut TestAppContext,
) {
    let original = "[panes.layouts]\npair = \"rows(*)\"\n";
    let file = ScratchConfig::new("layouts-new-names", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    press(&mut harness, "lnew");
    // A blank name only disables Create.
    let create = a11y_node(&mut harness, "settings-lcreate");
    assert_eq!(create["aria"]["disabled"], true, "{create}");
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.layout_name_problem(cx), None);
    });
    for (name, why) in [
        ("Dev", "Use 1 to 32 lowercase letters"),
        ("2dev", "Use 1 to 32 lowercase letters"),
        ("columns", "columns is a built-in layout."),
        ("pair", "A layout named pair exists."),
    ] {
        with_view(&mut harness, |view, cx| view.type_dialog_name(name, cx));
        visual(&harness).run_until_parked();
        let field = a11y_node(&mut harness, "settings-lname");
        let message = field["aria"]["description"].as_str().unwrap_or_default();
        assert!(message.starts_with(why), "{name}: {field}");
        let create = a11y_node(&mut harness, "settings-lcreate");
        assert_eq!(create["aria"]["disabled"], true, "{name}: {create}");
        // Enter in the field writes nothing.
        keys(&harness, "enter");
        assert_eq!(dialog(&mut harness), "new-layout", "{name}");
    }
    with_view(&mut harness, |view, cx| view.type_dialog_name("dev", cx));
    visual(&harness).run_until_parked();
    let create = a11y_node(&mut harness, "settings-lcreate");
    assert!(create["aria"]["disabled"].is_null(), "{create}");
    keys(&harness, "escape");
    assert_eq!(dialog(&mut harness), "none");
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), original);
}

#[gpui::test]
fn a_65th_layout_is_refused(cx: &mut TestAppContext) {
    let original: String = std::iter::once("[panes.layouts]\n".to_owned())
        .chain(
            (0..huterm_config::MAX_LAYOUTS)
                .map(|index| format!("l{index} = \"rows(*)\"\n")),
        )
        .collect();
    let file = ScratchConfig::new("layouts-new-full", &original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    press(&mut harness, "lnew");
    with_view(&mut harness, |view, cx| view.type_dialog_name("dev", cx));
    visual(&harness).run_until_parked();
    let field = a11y_node(&mut harness, "settings-lname");
    let message = field["aria"]["description"].as_str().unwrap_or_default();
    assert!(
        message.starts_with("config.toml holds at most 64"),
        "{field}"
    );
    let create = a11y_node(&mut harness, "settings-lcreate");
    assert_eq!(create["aria"]["disabled"], true, "{create}");
    keys(&harness, "enter");
    assert_eq!(dialog(&mut harness), "new-layout");
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), original);
}

const PAIR: &str = "[panes.layouts]\ndev = \"rows(*)\" # mine\n";

#[gpui::test]
fn a_template_saves_on_enter_or_blur_and_never_invalid(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layouts-template", PAIR);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    // Focus events, such as the blur that saves, reach an active window.
    with_window(&mut harness, |_, window, _| window.activate_window());
    // On its way to 55%, the template passes through 5%: typing writes
    // nothing, and the parser's message follows the text.
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(DEV, window, cx);
        view.type_into(DEV, "columns(pane, )", window, cx);
    });
    visual(&harness).run_until_parked();
    let message = a11y_node(&mut harness, "settings-ltemplate-message-dev");
    assert_eq!(
        message["aria"]["label"],
        "expected `columns`, `rows`, `grid`, `pane`, or `*` at column 15 of \
         the template",
        "{message}"
    );
    with_window(&mut harness, |view, window, cx| {
        view.type_into(DEV, "columns(pane 5", window, cx);
        assert!(view.template_problem("dev", cx).is_some());
        view.type_into(DEV, "columns(pane 55%, rows(*))", window, cx);
        assert_eq!(view.template_problem("dev", cx), None);
    });
    visual(&harness).run_until_parked();
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), PAIR);
    with_view(&mut harness, |view, cx| view.enter(DEV, cx));
    saved(&mut harness, &file, |text| text.contains("55%"));
    assert_eq!(
        file.text(),
        "[panes.layouts]\ndev = \"columns(pane 55%, rows(*))\" # mine\n"
    );
    // Leaving the field saves too.
    let leave = |harness: &mut DesktopHarness| {
        with_window(harness, |view, window, cx| {
            view.action_handle("lren:dev").focus(window, cx);
        });
        visual(harness).run_until_parked();
    };
    with_window(&mut harness, |view, window, cx| {
        view.type_into(DEV, "grid(*)", window, cx);
    });
    leave(&mut harness);
    saved(&mut harness, &file, |text| text.contains("grid"));
    // An invalid template stays a draft, with its message, and Enter or
    // leaving the field does not even try to save it.
    let good = file.text();
    let before = with_view(&mut harness, |view, _| view.writes_finished());
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(DEV, window, cx);
        view.type_into(DEV, "rows(pane,)", window, cx);
        view.enter(DEV, cx);
    });
    visual(&harness).run_until_parked();
    leave(&mut harness);
    with_view(&mut harness, |view, cx| {
        assert!(view.template_problem("dev", cx).is_some());
    });
    assert!(harness.cx.update(settled));
    assert_eq!(
        with_view(&mut harness, |view, _| view.writes_finished()),
        before
    );
    assert_eq!(file.text(), good);
    // Escape puts back the file's template.
    with_view(&mut harness, |view, cx| {
        view.escape(DEV, cx);
        assert_eq!(view.field_value(DEV, cx), "grid(*)");
        assert_eq!(view.template_problem("dev", cx), None);
    });
    harness.cx.run_until_parked();
    assert_eq!(file.text(), good);
}

/// Renames the layout `from` to `to` from its Rename dialog.
fn renaming(
    view: &mut SettingsView,
    from: &str,
    to: &str,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<'_, SettingsView>,
) {
    view.press(&format!("lren:{from}"), window, cx);
    assert_eq!(view.dialog_name(), "rename-layout");
    view.type_dialog_name(to, cx);
    view.answer("lrename", window, cx);
    assert_eq!(
        view.dialog_name(),
        "none",
        "{:?}",
        view.layout_name_problem(cx)
    );
}

fn rename(harness: &mut DesktopHarness, from: &str, to: &str) {
    with_window(harness, |view, window, cx| {
        renaming(view, from, to, window, cx);
    });
}

const USED: &str = "# Mine.\n[panes]\nlayout_cycle = [\"columns\", \"dev\", \"rows\"]\n\n\
                    [panes.layouts]\ndev = \"columns(pane 55%, rows(*))\"\n\
                    pair = \"rows(*)\"\n\n\
                    [[keybinding]]\nkey = \"alt-l\"\ncommand = \"arrange_panes\"\n\
                    args = { layout = \"dev\" } # mine\n\n\
                    [[keybinding]]\nkey = \"alt-p\"\ncommand = \"arrange_panes\"\n\
                    args = { layout = \"pair\" }\n";

#[gpui::test]
fn a_rename_moves_the_template_bindings_and_cycle_in_one_save(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layouts-rename", USED);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    let before = with_view(&mut harness, |view, _| view.writes_finished());
    rename(&mut harness, "dev", "work");
    // The new name shows at once, while the save runs.
    with_view(&mut harness, |view, _| {
        assert_eq!(view.layout_names(), ["pair", "work"]);
        assert_eq!(view.cycle_names(), ["columns", "work", "rows"]);
    });
    saved(&mut harness, &file, |text| text.contains("work"));
    assert_eq!(
        file.text(),
        "# Mine.\n[panes]\nlayout_cycle = [\"columns\", \"work\", \"rows\"]\n\n\
         [panes.layouts]\npair = \"rows(*)\"\n\
         work = \"columns(pane 55%, rows(*))\"\n\n\
         [[keybinding]]\nkey = \"alt-l\"\ncommand = \"arrange_panes\"\n\
         args = { layout = \"work\" } # mine\n\n\
         [[keybinding]]\nkey = \"alt-p\"\ncommand = \"arrange_panes\"\n\
         args = { layout = \"pair\" }\n"
    );
    let after = with_view(&mut harness, |view, _| view.writes_finished());
    assert_eq!(after - before, 1, "one save");
    visual(&harness).run_until_parked();
    assert_eq!(focus(&mut harness), "action:lren:work");
}

#[gpui::test]
fn a_rename_rewrites_a_binding_added_since_settings_read_the_file(
    cx: &mut TestAppContext,
) {
    let original = "[panes.layouts]\ndev = \"rows(*)\"\n";
    let file = ScratchConfig::new("layouts-rename-added", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    // Another editor binds the layout before Settings rereads.
    let binding = "\n[[keybinding]]\nkey = \"alt-l\"\ncommand = \"arrange_panes\"\n\
                   args = { layout = \"dev\" }\n";
    std::fs::write(file.path(), format!("{original}{binding}")).unwrap();
    rename(&mut harness, "dev", "work");
    saved(&mut harness, &file, |text| text.contains("work"));
    assert_eq!(
        file.text(),
        format!(
            "[panes.layouts]\nwork = \"rows(*)\"\n{}",
            binding.replace("\"dev\"", "\"work\"")
        )
    );
}

#[gpui::test]
fn a_rename_that_fails_to_save_changes_nothing_and_says_why(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layouts-rename-conflict", USED);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    // Another editor changes the template before Settings rereads.
    let theirs = USED.replace("55%", "65%");
    std::fs::write(file.path(), &theirs).unwrap();
    rename(&mut harness, "dev", "work");
    dialog_shows(&mut harness, "rename-layout-refused");
    assert_eq!(file.text(), theirs);
    let alert = a11y_node(&mut harness, "settings-rename-layout-refused");
    let why = alert["aria"]["description"].as_str().unwrap_or_default();
    assert!(why.starts_with("config.toml changed"), "{alert}");
    keys(&harness, "enter");
    assert_eq!(dialog(&mut harness), "none");
    assert_eq!(focus(&mut harness), "action:lren:dev");
    with_view(&mut harness, |view, _| {
        assert_eq!(view.layout_names(), ["dev", "pair"]);
        assert_eq!(view.cycle_names(), ["columns", "dev", "rows"]);
    });
}

#[gpui::test]
fn a_rename_refuses_a_binding_it_cannot_rewrite_and_writes_nothing(
    cx: &mut TestAppContext,
) {
    // An inline array of keybindings has no entry to edit in place.
    let original = "keybinding = [{ key = \"alt-l\", command = \
                    \"arrange_panes\", args = { layout = \"dev\" } }]\n\n\
                    [panes.layouts]\ndev = \"rows(*)\"\n";
    let file = ScratchConfig::new("layouts-rename-inline", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    rename(&mut harness, "dev", "work");
    dialog_shows(&mut harness, "rename-layout-refused");
    assert_eq!(file.text(), original);
    let alert = a11y_node(&mut harness, "settings-rename-layout-refused");
    let why = alert["aria"]["description"].as_str().unwrap_or_default();
    assert!(why.contains("keybinding"), "{alert}");
    with_view(&mut harness, |view, _| {
        assert_eq!(view.layout_names(), ["dev"]);
    });
}

#[gpui::test]
fn template_edits_around_a_rename_land_under_the_new_name(
    cx: &mut TestAppContext,
) {
    let original = "[panes.layouts]\ndev = \"rows(*)\"\n";
    let file = ScratchConfig::new("layouts-rename-edits", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        // Saved before the rename, which waits behind it and builds on it.
        view.type_into(DEV, "columns(*)", window, cx);
        view.enter(DEV, cx);
        renaming(view, "dev", "work", window, cx);
        // The new name's field shows what the rename copies; leaving it
        // writes nothing.
        assert_eq!(view.field_shows(WORK, window, cx), "columns(*)");
        view.blur(WORK, cx);
        // Saved after it, under the new name.
        view.type_into(WORK, "grid(*)", window, cx);
        view.enter(WORK, cx);
    });
    harness.wait_for("every save", settled);
    assert_eq!(file.text(), "[panes.layouts]\nwork = \"grid(*)\"\n");
    with_view(&mut harness, |view, _| {
        assert!(!view.conflicted(WORK));
        assert_eq!(view.error(WORK), None);
    });
}

#[gpui::test]
fn a_refused_rename_drops_the_edits_made_under_the_new_name(
    cx: &mut TestAppContext,
) {
    let original = "[panes.layouts]\ndev = \"rows(*)\"\n";
    let file = ScratchConfig::new("layouts-rename-refused-edit", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    let theirs = original.replace("rows", "columns");
    std::fs::write(file.path(), &theirs).unwrap();
    with_window(&mut harness, |view, window, cx| {
        renaming(view, "dev", "work", window, cx);
        // Queued behind the rename; written alone, it would make `work`.
        view.type_into(WORK, "grid(*)", window, cx);
        view.enter(WORK, cx);
    });
    dialog_shows(&mut harness, "rename-layout-refused");
    assert_eq!(file.text(), theirs);
    let alert = a11y_node(&mut harness, "settings-rename-layout-refused");
    let why = alert["aria"]["description"].as_str().unwrap_or_default();
    assert!(
        why.ends_with("Changes made meanwhile that name work were not saved."),
        "{alert}"
    );
    // Leaving the field does not submit the dropped edit again.
    with_view(&mut harness, |view, cx| view.blur(WORK, cx));
    harness.cx.run_until_parked();
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), theirs);
    with_view(&mut harness, |view, _| {
        assert_eq!(view.layout_names(), ["dev"]);
    });
}

#[gpui::test]
fn a_refused_rename_drops_a_deletion_of_the_new_name(cx: &mut TestAppContext) {
    let original = "[panes.layouts]\ndev = \"rows(*)\"\n";
    let file = ScratchConfig::new("layouts-rename-refused-delete", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    // Another editor makes a work of its own, which refuses the rename.
    let theirs = format!("{original}work = \"grid(*)\"\n");
    std::fs::write(file.path(), &theirs).unwrap();
    with_window(&mut harness, |view, window, cx| {
        renaming(view, "dev", "work", window, cx);
        // Queued behind the rename; applied to the file as it is then, it
        // would delete the other editor's work.
        view.press("ldel:work", window, cx);
        assert_eq!(view.dialog_name(), "delete-layout");
        view.answer("ldelyes", window, cx);
        assert_eq!(view.dialog_name(), "none");
    });
    dialog_shows(&mut harness, "rename-layout-refused");
    assert_eq!(file.text(), theirs);
    let alert = a11y_node(&mut harness, "settings-rename-layout-refused");
    let why = alert["aria"]["description"].as_str().unwrap_or_default();
    assert!(
        why.ends_with("Changes made meanwhile that name work were not saved."),
        "{alert}"
    );
    with_view(&mut harness, |view, _| {
        assert_ne!(view.status_text(), "Saved");
    });
}

#[gpui::test]
fn a_conflicted_create_drops_a_deletion_of_its_name(cx: &mut TestAppContext) {
    let original = "[panes.layouts]\ndev = \"rows(*)\"\n";
    let file = ScratchConfig::new("layouts-create-conflict-delete", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    // Another editor makes a work of its own, which the creation conflicts
    // with.
    let theirs = format!("{original}work = \"grid(*)\"\n");
    std::fs::write(file.path(), &theirs).unwrap();
    let before = with_view(&mut harness, |view, _| view.writes_finished());
    with_window(&mut harness, |view, window, cx| {
        view.press("lnew", window, cx);
        view.type_dialog_name("work", cx);
        view.answer("lcreate", window, cx);
        // Queued behind the creation; applied to the file as it is then, it
        // would delete the other editor's work.
        view.press("ldel:work", window, cx);
        assert_eq!(view.dialog_name(), "delete-layout");
        view.answer("ldelyes", window, cx);
        assert_eq!(view.dialog_name(), "none");
    });
    harness.wait_for("the creation's save", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.writes_finished() > before)
                    .unwrap_or(false)
            })
    });
    harness.cx.run_until_parked();
    assert_eq!(file.text(), theirs);
    // The creation's conflict shows on the other editor's work.
    with_view(&mut harness, |view, _| {
        assert_eq!(view.layout_names(), ["dev", "work"]);
        assert!(view.conflicted(WORK));
    });
}

#[gpui::test]
fn a_create_conflicting_after_an_earlier_save_still_drops_its_deletion(
    cx: &mut TestAppContext,
) {
    let original = "[panes.layouts]\ndev = \"rows(*)\"\npair = \"grid(*)\"\n";
    let file = ScratchConfig::new("layouts-create-after-save", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    // Another editor makes a work of its own. The deletion of pair reads
    // the file afresh, so once it lands the window's read holds their work
    // too, and only the creation itself says it was a creation.
    std::fs::write(file.path(), format!("{original}work = \"grid(*)\"\n"))
        .unwrap();
    let before = with_view(&mut harness, |view, _| view.writes_finished());
    with_window(&mut harness, |view, window, cx| {
        view.press("ldel:pair", window, cx);
        view.answer("ldelyes", window, cx);
        view.press("lnew", window, cx);
        view.type_dialog_name("work", cx);
        view.answer("lcreate", window, cx);
        view.press("ldel:work", window, cx);
        assert_eq!(view.dialog_name(), "delete-layout");
        view.answer("ldelyes", window, cx);
        assert_eq!(view.dialog_name(), "none");
    });
    harness.wait_for("the creation's save", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| {
                        view.writes_finished() > before + 1
                    })
                    .unwrap_or(false)
            })
    });
    harness.cx.run_until_parked();
    assert_eq!(
        file.text(),
        "[panes.layouts]\ndev = \"rows(*)\"\nwork = \"grid(*)\"\n"
    );
}

#[gpui::test]
fn delete_asks_first_and_focus_moves_to_the_next_row(cx: &mut TestAppContext) {
    let original = "[panes.layouts]\ndev = \"rows(*)\"\npair = \"grid(*)\"\n";
    let file = ScratchConfig::new("layouts-delete", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.action_handle("ldel:dev").focus(window, cx);
    });
    keys(&harness, "enter");
    assert_eq!(dialog(&mut harness), "delete-layout");
    assert_eq!(focus(&mut harness), "dialog:ldelno");
    let alert = a11y_node(&mut harness, "settings-delete-layout");
    assert_eq!(
        alert["aria"]["label"],
        "Delete the \u{201c}dev\u{201d} layout?"
    );
    keys(&harness, "escape");
    assert_eq!(dialog(&mut harness), "none");
    assert_eq!(focus(&mut harness), "action:ldel:dev");
    assert_eq!(file.text(), original);
    keys(&harness, "enter");
    keys(&harness, "tab enter");
    saved(&mut harness, &file, |text| !text.contains("dev"));
    assert_eq!(file.text(), "[panes.layouts]\npair = \"grid(*)\"\n");
    visual(&harness).run_until_parked();
    // The row went, so focus moves to the same button of the next one.
    assert_eq!(focus(&mut harness), "action:ldel:pair");
    // The last row's Delete hands focus to New layout.
    keys(&harness, "enter");
    keys(&harness, "tab enter");
    saved(&mut harness, &file, |text| !text.contains("pair"));
    visual(&harness).run_until_parked();
    assert_eq!(focus(&mut harness), "action:lnew");
}

#[gpui::test]
fn deleting_a_layout_a_binding_or_the_cycle_uses_is_refused(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layouts-delete-used", USED);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    press(&mut harness, "ldel:dev");
    assert_eq!(dialog(&mut harness), "delete-layout-refused");
    assert_eq!(focus(&mut harness), "dialog:ldelno");
    let alert = a11y_node(&mut harness, "settings-delete-layout");
    let why = alert["aria"]["description"].as_str().unwrap_or_default();
    let key = KeyHint::parse("alt-l", Platform::current()).text();
    assert_eq!(
        why,
        format!(
            "The keybinding {key} and the layout cycle use this layout. \
             Remove it from the cycle, and give the keybinding another \
             layout, first."
        )
    );
    keys(&harness, "enter");
    assert_eq!(dialog(&mut harness), "none");
    assert_eq!(focus(&mut harness), "action:ldel:dev");
    // A binding alone refuses too.
    press(&mut harness, "ldel:pair");
    assert_eq!(dialog(&mut harness), "delete-layout-refused");
    keys(&harness, "enter");
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), USED);
}

/// A `[[keybinding]]` that runs `arrange_panes` with the layout `name`.
fn binding(key: &str, name: &str) -> String {
    format!(
        "\n[[keybinding]]\nkey = \"{key}\"\ncommand = \"arrange_panes\"\n\
         args = {{ layout = \"{name}\" }}\n"
    )
}

#[gpui::test]
fn a_delete_refuses_a_binding_added_since_settings_read_the_file(
    cx: &mut TestAppContext,
) {
    let original = "[panes.layouts]\ndev = \"rows(*)\"\npair = \"grid(*)\"\n";
    let file = ScratchConfig::new("layouts-delete-added", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    // Another editor binds the layout before Settings rereads, so the
    // alert asks rather than refusing up front.
    let theirs = format!("{original}{}", binding("alt-l", "dev"));
    std::fs::write(file.path(), &theirs).unwrap();
    with_window(&mut harness, |view, window, cx| {
        view.press("ldel:dev", window, cx);
        assert_eq!(view.dialog_name(), "delete-layout");
        view.answer("ldelyes", window, cx);
    });
    dialog_shows(&mut harness, "delete-layout-refused");
    assert_eq!(file.text(), theirs);
    let alert = a11y_node(&mut harness, "settings-delete-layout");
    let key = KeyHint::parse("alt-l", Platform::current()).text();
    assert_eq!(
        alert["aria"]["description"],
        format!(
            "The keybinding {key} uses this layout. Give the keybinding \
             another layout, or remove it, first."
        ),
        "{alert}"
    );
    with_view(&mut harness, |view, _| {
        assert_eq!(view.layout_names(), ["dev", "pair"]);
    });
}

#[gpui::test]
fn a_delete_behind_a_rename_to_its_name_is_refused(cx: &mut TestAppContext) {
    let original = format!(
        "[panes.layouts]\ndev = \"rows(*)\"\n{}",
        binding("alt-l", "dev")
    );
    let file = ScratchConfig::new("layouts-delete-renamed", &original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    let release = with_view(&mut harness, |view, _| view.hold_batch());
    rename(&mut harness, "dev", "work");
    let renamed = original.replace("dev", "work");
    harness.wait_for("the rename's write", |_| file.text() == renamed);
    // The rename has not settled here, so the bindings Settings read still
    // name dev, and the deletion waits behind it.
    with_window(&mut harness, |view, window, cx| {
        view.press("ldel:work", window, cx);
        assert_eq!(view.dialog_name(), "delete-layout");
        view.answer("ldelyes", window, cx);
        assert_eq!(view.dialog_name(), "none");
    });
    drop(release);
    dialog_shows(&mut harness, "delete-layout-refused");
    assert_eq!(file.text(), renamed);
    let alert = a11y_node(&mut harness, "settings-delete-layout");
    let why = alert["aria"]["description"].as_str().unwrap_or_default();
    assert!(why.starts_with("The keybinding "), "{alert}");
    with_view(&mut harness, |view, _| {
        assert_eq!(view.layout_names(), ["work"]);
    });
}

#[gpui::test]
fn a_rename_leaves_no_conflict_under_the_old_name(cx: &mut TestAppContext) {
    let original = "[panes.layouts]\ndev = \"rows(*)\"\n";
    let file = ScratchConfig::new("layouts-rename-conflicted", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    // A draft that another editor's change to the template conflicts with.
    with_window(&mut harness, |view, window, cx| {
        view.type_into(DEV, "grid(*)", window, cx);
    });
    let theirs = original.replace("rows", "columns");
    std::fs::write(file.path(), &theirs).unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the draft's conflict", |cx| {
        settled(cx) && conflicted(cx, DEV)
    });
    // Closing while the rename saves waits for it, then closes: the old
    // name has no row left to settle a conflict on.
    let release = with_view(&mut harness, |view, _| view.hold_batch());
    with_window(&mut harness, |view, window, cx| {
        renaming(view, "dev", "work", window, cx);
        view.command(ids::CLOSE_PANE, window, cx).unwrap();
    });
    drop(release);
    harness.wait_for("Settings to close after the rename", |cx| {
        settings(cx).is_none()
    });
    assert_eq!(file.text(), theirs.replace("dev", "work"));
}

#[gpui::test]
fn a_draft_in_a_layout_deleted_elsewhere_leaves_nothing_to_settle(
    cx: &mut TestAppContext,
) {
    let original = "[panes.layouts]\ndev = \"rows(*)\"\npair = \"grid(*)\"\n";
    let file = ScratchConfig::new("layouts-draft-deleted", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.type_into(DEV, "columns(*)", window, cx);
    });
    let theirs = "[panes.layouts]\npair = \"grid(*)\"\n";
    std::fs::write(file.path(), theirs).unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the reload", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.layout_names() == ["pair"])
                    .unwrap_or(false)
            })
    });
    with_view(&mut harness, |view, cx| {
        assert!(!view.conflicted(DEV));
        assert_eq!(view.error(DEV), None);
        assert_eq!(view.field_value(DEV, cx), "");
    });
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::CLOSE_PANE, window, cx).unwrap();
    });
    harness.wait_for("Settings to close", |cx| settings(cx).is_none());
    assert_eq!(file.text(), theirs);
}

#[gpui::test]
fn focus_in_a_template_moves_to_the_next_row_or_new_layout_when_its_layout_goes(
    cx: &mut TestAppContext,
) {
    let original = "[panes.layouts]\nalpha = \"rows(*)\"\ndev = \"grid(*)\"\n\
                    pair = \"columns(*)\"\n";
    let file = ScratchConfig::new("layouts-focus-gone", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(DEV, window, cx);
    });
    visual(&harness).run_until_parked();
    // Another editor deletes the layout whose template has focus.
    let deleted = |harness: &mut DesktopHarness, text: &str, left: &[&str]| {
        std::fs::write(file.path(), text).unwrap();
        with_view(harness, SettingsView::follow_reload);
        harness.wait_for("the reload", |cx| {
            settled(cx)
                && settings(cx).is_some_and(|handle| {
                    handle
                        .read_with(cx, |view, _| view.layout_names() == left)
                        .unwrap_or(false)
                })
        });
        visual(harness).run_until_parked();
    };
    // The next row's template takes it, else New layout, which follows the
    // list, never the previous row's.
    deleted(
        &mut harness,
        "[panes.layouts]\nalpha = \"rows(*)\"\npair = \"columns(*)\"\n",
        &["alpha", "pair"],
    );
    assert_eq!(focus(&mut harness), "panes.layouts.pair");
    deleted(
        &mut harness,
        "[panes.layouts]\nalpha = \"rows(*)\"\n",
        &["alpha"],
    );
    assert_eq!(focus(&mut harness), "action:lnew");
}

#[gpui::test]
fn a_layout_made_behind_an_earlier_save_keeps_its_typed_template(
    cx: &mut TestAppContext,
) {
    let original = "[panes.layouts]\ndev = \"rows(*)\"\npair = \"grid(*)\"\n";
    let file = ScratchConfig::new("layouts-new-behind", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    let release = with_view(&mut harness, |view, _| view.hold_batch());
    with_window(&mut harness, |view, window, cx| {
        view.press("ldel:pair", window, cx);
        view.answer("ldelyes", window, cx);
        // Made behind the deletion, and typed into before either lands.
        view.press("lnew", window, cx);
        view.type_dialog_name("work", cx);
        view.answer("lcreate", window, cx);
        assert_eq!(view.dialog_name(), "none");
        view.type_into(WORK, "rows(*)", window, cx);
    });
    harness.wait_for("the deletion's write", |_| !file.text().contains("pair"));
    // The deletion settles while the creation still waits, so the file it
    // read has no work yet.
    drop(release);
    harness.wait_for("every save", settled);
    assert_eq!(
        file.text(),
        "[panes.layouts]\ndev = \"rows(*)\"\nwork = \"columns(*)\"\n"
    );
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.layout_names(), ["dev", "work"]);
        assert_eq!(view.field_value(WORK, cx), "rows(*)");
    });
}

#[gpui::test]
fn reset_in_a_template_field_writes_nothing(cx: &mut TestAppContext) {
    let original = "[panes.layouts]\ndev = \"grid(*)\"\n";
    let file = ScratchConfig::new("layouts-reset", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    let before = with_view(&mut harness, |view, _| view.writes_finished());
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(DEV, window, cx);
        view.command(ids::SETTINGS_RESET, window, cx).unwrap();
    });
    visual(&harness).run_until_parked();
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), original);
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.writes_finished(), before);
        assert_eq!(view.layout_names(), ["dev"]);
        assert_eq!(view.field_value(DEV, cx), "grid(*)");
    });
}

#[gpui::test]
fn cycle_rows_turn_on_off_and_move_and_keep_one_layout(
    cx: &mut TestAppContext,
) {
    let original = "[panes]\nlayout_cycle = [\"rows\", \"columns\"]\n\n\
                    [panes.layouts]\ndev = \"grid(*)\"\n";
    let file = ScratchConfig::new("layouts-cycle", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    // Members first, in cycle order, then the rest in picker order.
    with_view(&mut harness, |view, _| {
        assert_eq!(
            view.cycle_row_names(),
            ["rows", "columns", "main_left", "main_top", "tiled", "dev"]
        );
    });
    let cycle = |text: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix("layout_cycle = "))
            .unwrap_or_default()
            .to_owned()
    };
    press(&mut harness, "lcyc:dev");
    saved(&mut harness, &file, |text| cycle(text).contains("dev"));
    assert_eq!(cycle(&file.text()), "[\"rows\", \"columns\", \"dev\"]");
    press(&mut harness, "lup:dev");
    saved(&mut harness, &file, |text| {
        cycle(text) == "[\"rows\", \"dev\", \"columns\"]"
    });
    press(&mut harness, "ldown:rows");
    saved(&mut harness, &file, |text| {
        cycle(text) == "[\"dev\", \"rows\", \"columns\"]"
    });
    press(&mut harness, "lcyc:rows");
    saved(&mut harness, &file, |text| {
        cycle(text) == "[\"dev\", \"columns\"]"
    });
    press(&mut harness, "lcyc:columns");
    saved(&mut harness, &file, |text| cycle(text) == "[\"dev\"]");
    // The last member stays, with an alert that says why.
    press(&mut harness, "lcyc:dev");
    assert_eq!(dialog(&mut harness), "cycle-refused");
    let alert = a11y_node(&mut harness, "settings-cycle-refused");
    assert_eq!(
        alert["aria"]["label"],
        "The cycle needs at least one layout."
    );
    keys(&harness, "enter");
    assert_eq!(dialog(&mut harness), "none");
    assert_eq!(focus(&mut harness), "action:lcyc:dev");
    assert!(harness.cx.update(settled));
    assert_eq!(cycle(&file.text()), "[\"dev\"]");
}

#[gpui::test]
fn a_cycle_conflict_offers_use_mine_and_use_file(cx: &mut TestAppContext) {
    let original = "[panes]\nlayout_cycle = [\"columns\", \"rows\"]\n";
    let file = ScratchConfig::new("layouts-cycle-conflict", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    let theirs = "[panes]\nlayout_cycle = [\"rows\", \"columns\"]\n";
    // Another editor reorders the cycle before a change made here saves.
    let conflict = |harness: &mut DesktopHarness, row: &str| {
        std::fs::write(file.path(), theirs).unwrap();
        press(harness, &format!("lcyc:{row}"));
        harness.wait_for("the conflict", |cx| {
            settled(cx) && conflicted(cx, CYCLE)
        });
        visual(harness).run_until_parked();
        let alert = a11y_node(harness, "settings-conflict-panes.layout_cycle");
        assert!(
            alert["aria"]["label"]
                .as_str()
                .is_some_and(|label| label.starts_with("config.toml changed")),
            "{alert}"
        );
        for (id, label) in [
            ("settings-mine-panes.layout_cycle", "Use mine"),
            ("settings-file-panes.layout_cycle", "Use file"),
        ] {
            let button = a11y_node(harness, id);
            assert_eq!(button["aria"]["label"], label, "{button}");
        }
    };
    // Use mine saves the change over the file's.
    conflict(&mut harness, "tiled");
    press(&mut harness, "mine:panes.layout_cycle");
    saved(&mut harness, &file, |text| text.contains("tiled"));
    assert_eq!(
        file.text(),
        "[panes]\nlayout_cycle = [\"columns\", \"rows\", \"tiled\"]\n"
    );
    assert!(!harness.cx.update(|cx| conflicted(cx, CYCLE)));
    // Use file keeps the file's.
    conflict(&mut harness, "main_left");
    press(&mut harness, "file:panes.layout_cycle");
    assert!(!harness.cx.update(|cx| conflicted(cx, CYCLE)));
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), theirs);
    with_view(&mut harness, |view, _| {
        assert_eq!(view.cycle_names(), ["rows", "columns"]);
    });
}

#[gpui::test]
fn tab_reaches_every_control_and_the_arrows_move_along_the_cycle(
    cx: &mut TestAppContext,
) {
    let original = "[panes]\nlayout_cycle = [\"columns\", \"dev\"]\n\n\
                    [panes.layouts]\ndev = \"grid(*)\"\n";
    let file = ScratchConfig::new("layouts-keys", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(DEV, window, cx);
    });
    // Disabled moves, such as the first member's Move up and the others'
    // moves, are no stops.
    let order = [
        DEV,
        "action:lren:dev",
        "action:ldel:dev",
        "action:lnew",
        "action:lcyc:columns",
        "action:ldown:columns",
        "action:lcyc:dev",
        "action:lup:dev",
        "action:lcyc:rows",
        "action:lcyc:main_left",
        "action:lcyc:main_top",
        "action:lcyc:tiled",
    ];
    for expected in order.iter().skip(1) {
        keys(&harness, "tab");
        assert_eq!(focus(&mut harness), *expected);
    }
    for expected in order.iter().rev().skip(1) {
        keys(&harness, "shift-tab");
        assert_eq!(focus(&mut harness), *expected);
    }
    // The arrows move along the cycle's switches, wrapping.
    with_window(&mut harness, |view, window, cx| {
        view.action_handle("lcyc:columns").focus(window, cx);
    });
    for (key, expected) in [
        ("down", "action:lcyc:dev"),
        ("down", "action:lcyc:rows"),
        ("up", "action:lcyc:dev"),
        ("up", "action:lcyc:columns"),
        ("up", "action:lcyc:tiled"),
    ] {
        keys(&harness, key);
        assert_eq!(focus(&mut harness), expected, "after {key}");
    }
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), original);
    // Space turns the focused row on.
    keys(&harness, "space");
    saved(&mut harness, &file, |text| text.contains("tiled"));
    assert!(
        file.text()
            .contains("layout_cycle = [\"columns\", \"dev\", \"tiled\"]"),
        "{}",
        file.text()
    );
    assert_eq!(focus(&mut harness), "action:lcyc:tiled");
}

#[gpui::test]
fn keyboard_focus_on_a_layouts_template_reveals_its_name(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "layouts-reveal-name",
        "[panes.layouts]\ndev = \"grid(*)\"\nsides = \"columns(*)\"\n\
         spare = \"rows(*)\"\n",
    );
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    visual(&harness).simulate_resize(gpui::size(px(860.0), px(420.0)));
    visual(&harness).run_until_parked();
    let label = |harness: &mut DesktopHarness| {
        visual(harness)
            .debug_bounds("settings-label-panes.layouts.dev")
            .expect("dev's name drew")
    };
    // From the bottom, the keyboard reaches spare's template, then backs up
    // through sides' row to dev's template.
    with_view(&mut harness, |view, cx| {
        view.scroll_pane_to(px(100_000.0));
        cx.notify();
    });
    visual(&harness).run_until_parked();
    // F13, which nothing binds, makes the focus move the keyboard's.
    keys(&harness, "f13");
    with_window(&mut harness, |view, window, cx| {
        view.focus_field("panes.layouts.spare", window, cx);
    });
    visual(&harness).run_until_parked();
    let viewport = with_view(&mut harness, |view, _| view.pane_viewport());
    assert!(
        label(&mut harness).top() < viewport.top(),
        "dev's name already showed"
    );
    for _ in 0..6 {
        keys(&harness, "shift-tab");
    }
    assert_eq!(focus(&mut harness), DEV);
    let shown = label(&mut harness);
    assert!(
        shown.top() >= viewport.top() && shown.bottom() <= viewport.bottom(),
        "dev's name at {shown:?} sits outside the pane at {viewport:?}"
    );
}

#[gpui::test]
fn the_name_dialogs_follow_the_dialog_keys(cx: &mut TestAppContext) {
    let original = "[panes.layouts]\ndev = \"grid(*)\"\n";
    let file = ScratchConfig::new("layouts-dialog-keys", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    // Rename starts on the current name, selected, so typing replaces it.
    with_window(&mut harness, |view, window, cx| {
        view.action_handle("lren:dev").focus(window, cx);
    });
    keys(&harness, "enter");
    assert_eq!(dialog(&mut harness), "rename-layout");
    assert_eq!(focus(&mut harness), "dialog:field");
    let caret = with_view(&mut harness, |view, cx| view.dialog_caret(cx));
    assert_eq!(caret.map(|(_, selection)| selection), Some(0..3));
    keys(&harness, "w o r k");
    for (key, expected) in [
        ("tab", "dialog:lrenno"),
        ("tab", "dialog:lrename"),
        ("tab", "dialog:field"),
        ("shift-tab", "dialog:lrename"),
        ("shift-tab", "dialog:lrenno"),
        ("shift-tab", "dialog:field"),
    ] {
        keys(&harness, key);
        assert_eq!(focus(&mut harness), expected, "after {key}");
    }
    // Escape cancels, and focus returns to Rename.
    keys(&harness, "escape");
    assert_eq!(dialog(&mut harness), "none");
    assert_eq!(focus(&mut harness), "action:lren:dev");
    // Enter in the field renames.
    keys(&harness, "enter w o r k enter");
    saved(&mut harness, &file, |text| text.contains("work"));
    assert_eq!(file.text(), "[panes.layouts]\nwork = \"grid(*)\"\n");
    visual(&harness).run_until_parked();
    assert_eq!(dialog(&mut harness), "none");
    assert_eq!(focus(&mut harness), "action:lren:work");
}

#[gpui::test]
fn a_read_only_file_opens_no_layout_dialog(cx: &mut TestAppContext) {
    let original = "[panes.layouts]\ndev = \"grid(*)\"\n";
    let file = ScratchConfig::new("layouts-read-only", original);
    let mut harness = DesktopHarness::new(cx);
    open_window_section(&mut harness, &file);
    let broken = format!("{original}[window\n");
    super::settings_controls::break_file(&mut harness, &file, &broken);
    for (key, id) in [
        ("lnew", "settings-lnew"),
        ("lren:dev", "settings-lren-dev"),
        ("ldel:dev", "settings-ldel-dev"),
        ("lcyc:dev", "settings-lcyc-dev"),
    ] {
        let control = a11y_node(&mut harness, id);
        assert_eq!(control["aria"]["disabled"], true, "{control}");
        press(&mut harness, key);
        assert_eq!(dialog(&mut harness), "none", "{key}");
    }
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), broken);
}
