//! Rows the user changes in Settings stay where they painted while rows
//! around them come and go, through `huterm_ui`'s `RowAnchor`. Each test
//! reads where rows painted in every frame drawn after the change, so a
//! correction one frame late fails it.

use std::collections::HashMap;

use gpui::{
    ElementId, Modifiers, Pixels, ScrollDelta, ScrollEvent, TouchPhase,
};
use huterm_config::settings::SectionId;
use huterm_ui::components::scroll_area::HOLD_QUIET;

use super::settings::{ScratchConfig, settings, settled, with_view};
use super::settings_controls::{a11y_click, a11y_node, show, visual};
use super::*;

/// Where each anchored row painted, by key, frame by frame.
type Frames = Vec<HashMap<ElementId, Pixels>>;

fn key(name: &str) -> ElementId {
    ElementId::Name(name.to_owned().into())
}

/// Shows `section` in a short window, scrolled to its foot, and records
/// where its rows paint from then on.
fn at_the_foot(
    harness: &mut DesktopHarness,
    file: &ScratchConfig,
    section: SectionId,
) {
    show(harness, file, section);
    visual(harness).simulate_resize(gpui::size(px(860.0), px(420.0)));
    visual(harness).run_until_parked();
    to_the_foot(harness);
    with_view(harness, |view, _| view.pane_anchor().record_frames());
}

fn to_the_foot(harness: &mut DesktopHarness) {
    with_view(harness, |view, cx| {
        view.scroll_pane_to(px(100_000.0));
        cx.notify();
    });
    visual(harness).run_until_parked();
    assert!(scrolled(harness) > px(0.0), "the section fits the window");
}

fn frames(harness: &mut DesktopHarness) -> Frames {
    with_view(harness, |view, _| view.pane_anchor().recorded_frames())
}

fn scrolled(harness: &mut DesktopHarness) -> Pixels {
    with_view(harness, |view, _| view.pane_scrolled())
}

/// Whether the last frame drew the row under `name`.
fn drew(cx: &mut App, name: &str) -> bool {
    settings(cx).is_some_and(|handle| {
        handle
            .read_with(cx, |view, _| {
                view.pane_anchor()
                    .recorded_frames()
                    .last()
                    .is_some_and(|frame| frame.contains_key(&key(name)))
            })
            .unwrap_or(false)
    })
}

/// Asserts that `held` painted at one top, within half a point, in every
/// recorded frame, and that `marker` drew in the first frame and not the
/// last when `goes`, or the other way round.
fn held_through(frames: &Frames, held: &str, marker: &str, goes: bool) {
    let drawn =
        |frame: &HashMap<ElementId, Pixels>| frame.contains_key(&key(marker));
    assert_eq!(drawn(&frames[0]), goes, "{marker} before the change");
    assert_eq!(
        drawn(frames.last().unwrap()),
        !goes,
        "{marker} after the change"
    );
    let tops: Vec<Option<Pixels>> = frames
        .iter()
        .map(|frame| frame.get(&key(held)).copied())
        .collect();
    let first = tops[0].unwrap_or_else(|| panic!("{held} drew: {tops:?}"));
    for top in &tops {
        let top = top.unwrap_or_else(|| panic!("{held} drew: {tops:?}"));
        assert!(
            (top - first).abs() <= px(0.5),
            "{held} moved from {first:?}: {tops:?}"
        );
    }
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn turning_the_audible_bell_off_at_the_foot_keeps_its_switch_in_place(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("anchor-bell", "");
    let mut harness = DesktopHarness::new(cx);
    at_the_foot(&mut harness, &file, SectionId::Terminal);
    let before = scrolled(&mut harness);
    let switch = a11y_node(&mut harness, "settings-switch-terminal.bell.audio");
    a11y_click(&mut harness, &switch);
    // Bell sound and Bell volume go once the save's reload lands.
    harness.wait_for("the save's reload", |cx| {
        settled(cx) && !drew(cx, "row:terminal.bell.sound")
    });
    visual(&harness).run_until_parked();
    assert!(file.text().contains("audio = false"), "{}", file.text());
    let frames = frames(&mut harness);
    held_through(
        &frames,
        "row:terminal.bell.audio",
        "row:terminal.bell.volume",
        true,
    );
    assert_eq!(scrolled(&mut harness), before);
}

#[gpui::test]
fn showing_advanced_settings_keeps_the_button_in_place(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("anchor-advanced", "");
    let mut harness = DesktopHarness::new(cx);
    at_the_foot(&mut harness, &file, SectionId::Terminal);
    let before = scrolled(&mut harness);
    let button = a11y_node(&mut harness, "settings-advanced");
    a11y_click(&mut harness, &button);
    harness.wait_for("the save", |cx| {
        settled(cx) && drew(cx, "row:terminal.term")
    });
    visual(&harness).run_until_parked();
    // The rows came above the button, so the pane scrolled down by them.
    held_through(
        &frames(&mut harness),
        "advanced",
        "row:terminal.term",
        false,
    );
    assert!(scrolled(&mut harness) > before);
    // The wheel releases the button.
    with_view(&mut harness, |view, _| {
        assert!(view.pane_anchor().hold(key("advanced")));
    });
    let viewport = with_view(&mut harness, |view, _| view.pane_viewport());
    let mut pointer = visual(&harness);
    pointer.simulate_event(ScrollEvent {
        position: viewport.center(),
        delta: ScrollDelta::Pixels(gpui::point(px(0.0), px(30.0))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        ..Default::default()
    });
    pointer.run_until_parked();
    assert!(!with_view(&mut harness, |view, _| view
        .pane_anchor()
        .is_held()));
}

/// The lowest Keybindings command whose row shows whole, expanded.
fn expand_the_lowest_command(harness: &mut DesktopHarness) -> String {
    let viewport = with_view(harness, |view, _| view.pane_viewport());
    let command = frames(harness)
        .last()
        .unwrap()
        .iter()
        .filter_map(|(id, top)| match id {
            ElementId::Name(name) => name
                .strip_prefix("kcmd:")
                .filter(|_| *top + px(40.0) <= viewport.bottom())
                .map(|command| (command.to_owned(), *top)),
            _ => None,
        })
        .max_by(|a, b| a.1.as_f32().total_cmp(&b.1.as_f32()))
        .map(|(command, _)| command)
        .expect("a command row shows");
    let trigger = format!("settings-kcmd-{command}");
    let node = a11y_node(harness, &trigger);
    a11y_click(harness, &node);
    assert_eq!(a11y_node(harness, &trigger)["aria"]["expanded"], true);
    command
}

fn held(harness: &mut DesktopHarness) -> bool {
    with_view(harness, |view, _| view.pane_anchor().is_held())
}

#[gpui::test]
fn collapsing_a_command_at_the_foot_keeps_its_row_in_place(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("anchor-command", "");
    let mut harness = DesktopHarness::new(cx);
    at_the_foot(&mut harness, &file, SectionId::Keybindings);
    let command = expand_the_lowest_command(&mut harness);
    let trigger = format!("settings-kcmd-{command}");
    // With its shortcuts open at the foot, closing them shrinks the
    // content under the row.
    to_the_foot(&mut harness);
    let before = scrolled(&mut harness);
    with_view(&mut harness, |view, _| view.pane_anchor().record_frames());
    let node = a11y_node(&mut harness, &trigger);
    a11y_click(&mut harness, &node);
    assert_eq!(a11y_node(&mut harness, &trigger)["aria"]["expanded"], false);
    let row = format!("kcmd:{command}");
    let frames = frames(&mut harness);
    let tops: Vec<_> =
        frames.iter().map(|frame| frame.get(&key(&row))).collect();
    assert!(tops.len() > 1, "no frame drew after the click");
    for top in &tops {
        assert_eq!(*top, tops[0], "{row} moved: {tops:?}");
    }
    assert_eq!(scrolled(&mut harness), before);
}

#[gpui::test]
fn a_control_outside_every_held_row_releases_the_hold(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("anchor-outside", "");
    let mut harness = DesktopHarness::new(cx);
    // A warning gives the pane a notice above every row.
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>()
            .config_report
            .refuse(vec!["cannot register ctrl-alt-t: rejected".to_owned()]);
    });
    at_the_foot(&mut harness, &file, SectionId::Terminal);
    harness.wait_for("the first read", settled);
    to_the_foot(&mut harness);
    let before = scrolled(&mut harness);
    with_view(&mut harness, |view, _| {
        assert!(view.pane_anchor().hold(key("advanced")));
    });
    // Its details open above the held row, which must not scroll the pane
    // to keep that row put.
    with_window_view(&mut harness, |view, window, cx| {
        view.press("notice:details", window, cx);
    });
    assert!(!held(&mut harness));
    assert_eq!(scrolled(&mut harness), before);
}

#[gpui::test]
fn recording_a_shortcut_keeps_add_shortcut_in_place(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("anchor-add-shortcut", "");
    let mut harness = DesktopHarness::new(cx);
    at_the_foot(&mut harness, &file, SectionId::Keybindings);
    let command = expand_the_lowest_command(&mut harness);
    to_the_foot(&mut harness);
    let shortcuts = |frame: &HashMap<ElementId, Pixels>| {
        let prefix = format!("kshortcut:{command}:");
        frame
            .keys()
            .filter(|id| {
                matches!(id, ElementId::Name(name) if name.starts_with(&prefix))
            })
            .count()
    };
    let before = shortcuts(frames(&mut harness).last().unwrap());
    with_view(&mut harness, |view, _| view.pane_anchor().record_frames());
    with_window_view(&mut harness, |view, window, cx| {
        view.press(&format!("kadd:{command}"), window, cx);
    });
    let window: gpui::AnyWindowHandle =
        harness.cx.update(|cx| settings(cx).unwrap().into());
    for stroke in ["ctrl-k", "ctrl-y"] {
        harness.cx.dispatch_keystroke(
            window,
            gpui::Keystroke::parse_with_os(
                stroke,
                crate::keymap::Platform::current().os(),
            )
            .unwrap(),
        );
    }
    harness.wait_for("the new shortcut's row", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| {
                        view.pane_anchor()
                            .recorded_frames()
                            .last()
                            .is_some_and(|frame| shortcuts(frame) > before)
                    })
                    .unwrap_or(false)
            })
    });
    visual(&harness).run_until_parked();
    assert!(file.text().contains("ctrl-k ctrl-y"), "{}", file.text());
    // The new row came above the buttons, which stayed where they were
    // pressed in every frame.
    let row = key(&format!("kbuttons:{command}"));
    let tops: Vec<_> = frames(&mut harness)
        .iter()
        .map(|frame| frame.get(&row).copied())
        .collect();
    let first = tops[0].expect("the buttons drew");
    for top in &tops {
        let top = top.unwrap_or_else(|| panic!("the buttons drew: {tops:?}"));
        assert!((top - first).abs() <= px(0.5), "they moved: {tops:?}");
    }
}

#[gpui::test]
fn deleting_the_layout_at_the_foot_keeps_the_rows_above_in_place(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "anchor-layout",
        "[panes.layouts]\ndev = \"rows(*)\"\nwork = \"columns(*)\"\n",
    );
    let mut harness = DesktopHarness::new(cx);
    at_the_foot(&mut harness, &file, SectionId::Panes);
    let before = scrolled(&mut harness);
    with_window_view(&mut harness, |view, window, cx| {
        view.press("ldel:work", window, cx);
        view.answer("ldelyes", window, cx);
    });
    harness.wait_for("the save", |cx| settled(cx) && !drew(cx, "layout:work"));
    visual(&harness).run_until_parked();
    assert!(!file.text().contains("work"), "{}", file.text());
    // The held row went, so the offset and the rows above it stay.
    held_through(&frames(&mut harness), "layout:dev", "layout:work", true);
    assert_eq!(scrolled(&mut harness), before);
}

fn with_window_view(
    harness: &mut DesktopHarness,
    update: impl FnOnce(
        &mut super::super::settings_window::SettingsView,
        &mut Window,
        &mut gpui::Context<'_, super::super::settings_window::SettingsView>,
    ),
) {
    super::settings::with_window(harness, update);
    visual(harness).run_until_parked();
}

/// An action whose write lands after a frame has drawn, as Restore defaults
/// does, keeps its row held until then: rows above it that go with the write
/// leave the buttons where they were pressed.
#[gpui::test]
fn restoring_defaults_holds_the_buttons_until_its_write_lands(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "anchor-restore",
        "[[keybinding]]\nkey = \"ctrl-k ctrl-y\"\ncommand = \"next_tab\"\n",
    );
    let mut harness = DesktopHarness::new(cx);
    at_the_foot(&mut harness, &file, SectionId::Keybindings);
    with_window_view(&mut harness, |view, window, cx| {
        view.press("kcmd:next_tab", window, cx);
    });
    // The buttons a little below the pane's top, with the added shortcut's
    // row above them.
    let row = key("kbuttons:next_tab");
    let top = frames(&mut harness).last().unwrap()[&row];
    let viewport = with_view(&mut harness, |view, _| view.pane_viewport());
    with_view(&mut harness, |view, cx| {
        let scrolled = view.pane_scrolled();
        view.scroll_pane_to(scrolled + top - viewport.top() - px(100.0));
        cx.notify();
    });
    visual(&harness).run_until_parked();
    with_view(&mut harness, |view, _| view.pane_anchor().record_frames());
    super::settings::with_window(&mut harness, |view, window, cx| {
        view.press("kreset:next_tab", window, cx);
    });
    // A frame draws while the write is still on its way.
    visual(&harness).update(Window::simulate_next_frame);
    assert!(!harness.cx.update(settled), "the write already landed");
    assert!(held(&mut harness), "the hold settled before its write");
    harness.wait_for("the restore", |cx| {
        settled(cx) && !drew(cx, "kshortcut:next_tab:u0")
    });
    visual(&harness).run_until_parked();
    assert!(!file.text().contains("ctrl-y"), "{}", file.text());
    let tops: Vec<_> = frames(&mut harness)
        .iter()
        .map(|frame| frame.get(&row).copied())
        .collect();
    let first = tops[0].expect("the buttons drew");
    for top in &tops {
        let top = top.unwrap_or_else(|| panic!("the buttons drew: {tops:?}"));
        assert!((top - first).abs() <= px(0.5), "they moved: {tops:?}");
    }
}

/// A write that fails on another editor's change rereads the file, and the
/// rows that read changes still find the held row in place.
#[gpui::test]
fn a_conflicted_restore_holds_the_buttons_until_its_reread_lands(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "anchor-conflict",
        "[[keybinding]]\nkey = \"ctrl-k ctrl-y\"\ncommand = \"next_tab\"\n",
    );
    let mut harness = DesktopHarness::new(cx);
    at_the_foot(&mut harness, &file, SectionId::Keybindings);
    with_window_view(&mut harness, |view, window, cx| {
        view.press("kcmd:next_tab", window, cx);
    });
    let row = key("kbuttons:next_tab");
    let top = frames(&mut harness).last().unwrap()[&row];
    let viewport = with_view(&mut harness, |view, _| view.pane_viewport());
    with_view(&mut harness, |view, cx| {
        let scrolled = view.pane_scrolled();
        view.scroll_pane_to(scrolled + top - viewport.top() - px(100.0));
        cx.notify();
    });
    visual(&harness).run_until_parked();
    with_view(&mut harness, |view, _| view.pane_anchor().record_frames());
    // Another editor removes the shortcut before Restore defaults writes.
    std::fs::write(file.path(), "").unwrap();
    super::settings::with_window(&mut harness, |view, window, cx| {
        view.press("kreset:next_tab", window, cx);
    });
    harness.wait_for("the reread", |cx| {
        settled(cx) && !drew(cx, "kshortcut:next_tab:u0")
    });
    visual(&harness).run_until_parked();
    let tops: Vec<_> = frames(&mut harness)
        .iter()
        .map(|frame| frame.get(&row).copied())
        .collect();
    let first = tops[0].expect("the buttons drew");
    for top in &tops {
        let top = top.unwrap_or_else(|| panic!("the buttons drew: {tops:?}"));
        assert!((top - first).abs() <= px(0.5), "they moved: {tops:?}");
    }
}

/// A hold is about layout, not the file: it lasts through the save and the
/// reload its change starts, and ends once the row has needed no
/// correction for `HOLD_QUIET`, so later changes move the pane as usual.
#[gpui::test]
fn a_hold_ends_once_its_row_stays_put_for_hold_quiet(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("anchor-quiet", "");
    let mut harness = DesktopHarness::new(cx);
    at_the_foot(&mut harness, &file, SectionId::Terminal);
    let button = a11y_node(&mut harness, "settings-advanced");
    a11y_click(&mut harness, &button);
    harness.wait_for("the save's reload", |cx| {
        settled(cx) && drew(cx, "row:terminal.term")
    });
    visual(&harness).run_until_parked();
    assert!(held(&mut harness), "the save's reload ended the hold");
    harness.cx.executor().advance_clock(HOLD_QUIET);
    with_view(&mut harness, |_, cx| cx.notify());
    visual(&harness).run_until_parked();
    assert!(!held(&mut harness), "a quiet row stays held");
}

/// A section that fits the window has no room to scroll, so the rows
/// "Show advanced settings" adds above its button take the space the hold
/// keeps below, and the button stays where it was pressed.
#[gpui::test]
fn showing_advanced_settings_in_a_section_that_fits_keeps_the_button_in_place(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("anchor-advanced-fits", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Scrollback);
    visual(&harness).simulate_resize(gpui::size(px(860.0), px(900.0)));
    visual(&harness).run_until_parked();
    let max = with_view(&mut harness, |view, _| view.pane_max_scroll());
    assert_eq!(max, px(0.0), "the section does not fit the window");
    with_view(&mut harness, |view, _| view.pane_anchor().record_frames());
    let button = a11y_node(&mut harness, "settings-advanced");
    a11y_click(&mut harness, &button);
    harness.wait_for("the save", |cx| {
        settled(cx) && drew(cx, "row:terminal.scrollback_bytes")
    });
    visual(&harness).run_until_parked();
    held_through(
        &frames(&mut harness),
        "advanced",
        "row:terminal.scrollback_bytes",
        false,
    );
}

/// Space a hold left below the rows goes when a search then narrows them,
/// so the results show instead of a blank pane with them scrolled away.
#[gpui::test]
fn searching_keybindings_after_a_hold_shows_the_results(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("anchor-key-search", "");
    let mut harness = DesktopHarness::new(cx);
    at_the_foot(&mut harness, &file, SectionId::Keybindings);
    let command = expand_the_lowest_command(&mut harness);
    to_the_foot(&mut harness);
    // Collapsing at the foot leaves space below the rows.
    let trigger = format!("settings-kcmd-{command}");
    let node = a11y_node(&mut harness, &trigger);
    a11y_click(&mut harness, &node);
    harness.cx.executor().advance_clock(HOLD_QUIET);
    with_view(&mut harness, |_, cx| cx.notify());
    visual(&harness).run_until_parked();
    assert!(!held(&mut harness));
    with_view(&mut harness, |view, cx| view.search_keybindings("zoom", cx));
    visual(&harness).run_until_parked();
    let viewport = with_view(&mut harness, |view, _| view.pane_viewport());
    let shown = frames(&mut harness)
        .last()
        .unwrap()
        .iter()
        .any(|(id, top)| {
            matches!(id, ElementId::Name(name) if name.starts_with("kcmd:"))
                && *top >= viewport.top()
                && *top < viewport.bottom()
        });
    assert!(shown, "no result shows: {:?}", frames(&mut harness).last());
}

/// A reorder moves its own row, so it releases a hold an earlier change
/// left rather than scrolling the pane after the rows it moves.
#[gpui::test]
fn reordering_the_cycle_right_after_a_change_leaves_the_pane_still(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "anchor-reorder",
        "[panes]\nlayout_cycle = [\"columns\", \"rows\"]\n",
    );
    let mut harness = DesktopHarness::new(cx);
    at_the_foot(&mut harness, &file, SectionId::Panes);
    with_window_view(&mut harness, |view, window, cx| {
        view.press("lcyc:main_left", window, cx);
    });
    harness.wait_for("the cycle's save", settled);
    visual(&harness).run_until_parked();
    assert!(file.text().contains("main_left"), "{}", file.text());
    assert!(held(&mut harness), "the toggle's hold already ended");
    let before = scrolled(&mut harness);
    with_window_view(&mut harness, |view, window, cx| {
        view.press("lup:main_left", window, cx);
    });
    harness.wait_for("the move's save", settled);
    visual(&harness).run_until_parked();
    assert!(
        file.text()
            .contains("[\"columns\", \"main_left\", \"rows\"]"),
        "{}",
        file.text()
    );
    assert!(!held(&mut harness));
    assert_eq!(scrolled(&mut harness), before);
}

/// A field commit that saves nothing, as a blur after pressing another
/// control makes, leaves the hold on the control the user pressed.
#[gpui::test]
fn an_unchanged_field_commit_keeps_the_pressed_controls_hold(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("anchor-blur", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Scrollback);
    harness.wait_for("the first read", settled);
    with_window_view(&mut harness, |view, window, cx| {
        view.press("advanced:pane", window, cx);
    });
    let holds = |harness: &mut DesktopHarness| {
        with_view(harness, |view, _| format!("{:?}", view.pane_anchor()))
    };
    assert!(
        holds(&mut harness).contains("\"advanced\""),
        "{}",
        holds(&mut harness)
    );
    with_view(&mut harness, |view, cx| {
        view.blur("terminal.scrollback_rows", cx);
    });
    visual(&harness).run_until_parked();
    assert!(
        holds(&mut harness).contains("\"advanced\""),
        "{}",
        holds(&mut harness)
    );
}
