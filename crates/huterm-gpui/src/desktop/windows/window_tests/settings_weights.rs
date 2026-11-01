//! The terminal weight row over real Settings controls and the patch service.

use huterm_config::FontWeight;
use zed_font_kit::properties::{Properties, Weight};

use super::settings::{
    ScratchConfig, open_settings, settled, with_view, with_window,
};
use super::settings_controls::{
    a11y_click, a11y_node, break_file, keys, visual,
};
use super::*;
use crate::terminal_font::WeightFaces;

fn faces(weights: &[f32]) -> Result<WeightFaces, String> {
    WeightFaces::from_properties(
        &weights
            .iter()
            .map(|weight| Properties {
                weight: Weight(*weight),
                ..Properties::new()
            })
            .collect::<Vec<_>>(),
    )
}
fn multi(_: &str) -> Result<WeightFaces, String> {
    faces(&[300.0, 400.0, 500.0, 700.0])
}
fn one(_: &str) -> Result<WeightFaces, String> {
    faces(&[400.0])
}
fn failed(_: &str) -> Result<WeightFaces, String> {
    Err("test enumeration failure".into())
}

fn without_regular(_: &str) -> Result<WeightFaces, String> {
    faces(&[300.0, 700.0])
}

fn open(
    harness: &mut DesktopHarness,
    file: &ScratchConfig,
    enumerate: fn(&str) -> Result<WeightFaces, String>,
) {
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().applied_terminal_family =
            Some("Test Mono".into());
    });
    open_settings(harness, &file.path());
    with_view(harness, |view, cx| {
        view.set_weight_enumerator(enumerate, cx);
    });
    harness.cx.run_until_parked();
    with_window(harness, |view, window, cx| {
        view.show_section(huterm_config::settings::SectionId::Appearance, cx);
        window.refresh();
    });
    visual(harness).run_until_parked();
}
fn open_menu(harness: &mut DesktopHarness) {
    with_window(
        harness,
        super::super::settings_window::SettingsView::focus_weight,
    );
    keys(harness, "space");
}

#[gpui::test]
fn weight_numeric_value_highlights_by_name_and_same_pick_preserves_the_number(
    cx: &mut TestAppContext,
) {
    let text = "[font]\nweight = 300\n";
    let file = ScratchConfig::new("weight-number", text);
    let mut harness = DesktopHarness::new(cx);
    open(&mut harness, &file, multi);
    let node = a11y_node(&mut harness, "settings-select-font.weight");
    assert_eq!(node["aria"]["value"], "Light (300)");
    open_menu(&mut harness);
    assert_eq!(
        with_view(&mut harness, |view, cx| view.weight_highlight(cx))
            .as_deref(),
        Some("light")
    );
    keys(&harness, "enter");
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), text);
    with_view(&mut harness, |view, cx| {
        view.set("font.weight", Some(toml::Value::Integer(350)), cx);
        assert!(view.error("font.weight").is_some());
    });
    open_menu(&mut harness);
    keys(&harness, "enter");
    assert_eq!(
        with_view(&mut harness, |view, _| view.error("font.weight")),
        None
    );
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), text);
    open_menu(&mut harness);
    with_view(&mut harness, |view, cx| view.pick_menu_item("medium", cx));
    harness.wait_for("weight name saved", |cx| {
        settled(cx) && file.text().contains("\"medium\"")
    });
    assert_eq!(file.text(), "[font]\nweight = \"medium\"\n");
    with_view(&mut harness, |view, cx| view.set("font.weight", None, cx));
    harness.wait_for("weight reset", |cx| {
        settled(cx) && !file.text().contains("weight")
    });
}

#[gpui::test]
fn weight_missing_face_stays_in_the_menu_and_names_what_draws(
    cx: &mut TestAppContext,
) {
    let file =
        ScratchConfig::new("weight-missing", "[font]\nweight = \"black\"\n");
    let mut harness = DesktopHarness::new(cx);
    open(&mut harness, &file, multi);
    let state = with_view(&mut harness, |view, _| view.weight_state());
    assert_eq!(
        state.3,
        Some(format!(
            "{} has no Black weight, so terminals use Bold.",
            state.0
        ))
    );
    open_menu(&mut harness);
    assert!(
        with_view(&mut harness, |view, cx| view.menu_ids(cx))
            .contains(&"black".into())
    );
    assert_eq!(
        with_view(&mut harness, |view, cx| view.weight_highlight(cx))
            .as_deref(),
        Some("black")
    );
}

#[gpui::test]
fn one_weight_disables_pointer_and_keyboard_opening(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("weight-one", "");
    let mut harness = DesktopHarness::new(cx);
    open(&mut harness, &file, one);
    let node = a11y_node(&mut harness, "settings-select-font.weight");
    assert_eq!(node["aria"]["disabled"], true);
    a11y_click(&mut harness, &node);
    open_menu(&mut harness);
    assert!(!with_view(&mut harness, |view, _| view.menu_open()));
    let state = with_view(&mut harness, |view, _| view.weight_state());
    assert_eq!(state.3, Some(format!("{} has one weight.", state.0)));
    assert_eq!(file.text(), "");
}

#[gpui::test]
fn linux_monospace_alias_disables_weight_without_enumerating_and_allows_reset(
    cx: &mut TestAppContext,
) {
    fn concrete_only(family: &str) -> Result<WeightFaces, String> {
        assert_ne!(family, "monospace", "the alias must not enumerate weights");
        multi(family)
    }

    let text = "[font]\nfamily = \"monospace\"\nweight = 300\n";
    let file = ScratchConfig::new("weight-linux-alias", text);
    let mut harness = DesktopHarness::new(cx);
    open(&mut harness, &file, multi);
    open_menu(&mut harness);
    let previous = with_view(&mut harness, |view, _| view.weight_state());
    with_view(&mut harness, |view, cx| {
        cx.global_mut::<Desktop>().applied_terminal_family =
            Some("monospace".into());
        view.scan_linux_weights(concrete_only, cx);
        assert!(!view.menu_open(), "the previous family's menu closes");
        view.weight_completion(&previous.0, previous.1, multi(""), cx);
    });
    harness.cx.run_until_parked();
    let node = a11y_node(&mut harness, "settings-select-font.weight");
    assert_eq!(node["aria"]["disabled"], true);
    assert_eq!(node["aria"]["value"], "Light (300)");
    let state = with_view(&mut harness, |view, _| view.weight_state());
    assert_eq!(state.0, "monospace");
    assert_eq!(
        state.2,
        [FontWeight::parse(&toml::Value::Integer(300)).unwrap()]
    );
    assert_eq!(
        state.3.as_deref(),
        Some(
            "monospace is a font alias, so terminals draw it at its default weight. Choose a font family to set a weight."
        )
    );
    let row = a11y_node(&mut harness, "settings-row-font.weight");
    assert!(
        row["aria"]["description"]
            .as_str()
            .unwrap()
            .ends_with(state.3.as_deref().unwrap()),
        "the alias note appears under the weight row: {row}"
    );
    a11y_click(&mut harness, &node);
    open_menu(&mut harness);
    assert!(!with_view(&mut harness, |view, _| view.menu_open()));
    assert_eq!(file.text(), text, "the numeric weight stays as written");

    let reset = a11y_node(&mut harness, "settings-reset-font.weight");
    a11y_click(&mut harness, &reset);
    harness.wait_for("the alias weight to reset", |cx| {
        settled(cx) && !file.text().contains("weight")
    });
    assert_eq!(file.text(), "[font]\nfamily = \"monospace\"\n");
    // The host's macOS reload resolves monospace to Menlo. Restore the
    // applied Linux alias before checking the reset row on either platform.
    with_view(&mut harness, |view, cx| {
        cx.global_mut::<Desktop>().applied_terminal_family =
            Some("monospace".into());
        view.scan_linux_weights(concrete_only, cx);
    });
    harness.cx.run_until_parked();
    let node = a11y_node(&mut harness, "settings-select-font.weight");
    assert_eq!(node["aria"]["disabled"], true);
    assert_eq!(node["aria"]["value"], "Regular (400)");

    with_view(&mut harness, |view, cx| {
        cx.global_mut::<Desktop>().applied_terminal_family =
            Some("Test Mono".into());
        view.scan_linux_weights(multi, cx);
    });
    harness.cx.run_until_parked();
    let node = a11y_node(&mut harness, "settings-select-font.weight");
    assert_ne!(node["aria"]["disabled"], true);
    assert_eq!(
        with_view(&mut harness, |view, _| view.weight_state()).3,
        None
    );
    open_menu(&mut harness);
    assert!(with_view(&mut harness, |view, _| view.menu_open()));
}

#[gpui::test]
fn weight_enumeration_failure_offers_all_weights_and_stale_results_do_nothing(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("weight-failure", "");
    let mut harness = DesktopHarness::new(cx);
    open(&mut harness, &file, failed);
    let state = with_view(&mut harness, |view, _| view.weight_state());
    assert_eq!(state.2, FontWeight::ALL);
    assert_eq!(state.3, None);
    let node = a11y_node(&mut harness, "settings-select-font.weight");
    assert_ne!(node["aria"]["disabled"], true);
    open_menu(&mut harness);
    for (family, generation) in
        [(&*state.0, state.1 - 1), ("old family", state.1)]
    {
        with_view(&mut harness, |view, cx| {
            view.weight_completion(family, generation, one(family), cx);
        });
        assert_eq!(
            with_view(&mut harness, |view, _| view.weight_state()),
            state
        );
        assert!(with_view(&mut harness, |view, _| view.menu_open()));
    }
    with_view(&mut harness, |view, cx| {
        view.weight_completion(&state.0, state.1, one(&state.0), cx);
    });
    assert!(!with_view(&mut harness, |view, _| view.menu_open()));
}

#[gpui::test]
fn invalid_file_disables_the_weight_select(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("weight-locked", "");
    let mut harness = DesktopHarness::new(cx);
    open(&mut harness, &file, multi);
    break_file(&mut harness, &file, "[font\n");
    let node = a11y_node(&mut harness, "settings-select-font.weight");
    assert_eq!(node["aria"]["disabled"], true);
    open_menu(&mut harness);
    assert!(!with_view(&mut harness, |view, _| view.menu_open()));
}

#[gpui::test]
fn an_invalid_weight_shows_what_the_file_says(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("weight-invalid", "[font]\nweight = 350\n");
    let mut harness = DesktopHarness::new(cx);
    open(&mut harness, &file, without_regular);
    let node = a11y_node(&mut harness, "settings-select-font.weight");
    assert_eq!(node["aria"]["value"], "350");
    assert_eq!(node["aria"]["disabled"], true);
    assert_eq!(
        with_view(&mut harness, |view, _| view.weight_state()).3,
        None
    );
}

fn same_family_rescan(
    cx: &mut TestAppContext,
    name: &str,
    enumerate: fn(&str) -> Result<WeightFaces, String>,
    disabled: bool,
) {
    let file = ScratchConfig::new(name, "[font]\nweight = 900\n");
    let mut harness = DesktopHarness::new(cx);
    open(&mut harness, &file, enumerate);
    if !disabled {
        open_menu(&mut harness);
    }
    let before = with_view(&mut harness, |view, _| view.weight_state());
    with_view(&mut harness, |view, cx| {
        view.set_weight_enumerator(enumerate, cx);
        let pending = view.weight_state();
        assert_eq!(pending.0, before.0);
        assert_eq!(pending.1, before.1 + 1);
        assert_eq!(pending.2, before.2, "options stay while pending");
        assert_eq!(pending.3, before.3, "note stays while pending");
        assert_eq!(view.weight_select_disabled(), disabled);
        assert_eq!(view.menu_open(), !disabled, "menu stays while pending");
    });
    let node = a11y_node(&mut harness, "settings-select-font.weight");
    assert_eq!(node["aria"]["disabled"] == true, disabled);
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        let completed = view.weight_state();
        assert_eq!(completed.2, before.2);
        assert_eq!(completed.3, before.3);
        assert_eq!(
            view.menu_open(),
            !disabled,
            "unchanged options keep the menu"
        );
    });
}

#[gpui::test]
fn weight_same_family_rescan_preserves_options_note_and_open_menu(
    cx: &mut TestAppContext,
) {
    same_family_rescan(cx, "weight-rescan-multi", multi, false);
}

#[gpui::test]
fn weight_same_family_rescan_preserves_one_weight_disabled_state(
    cx: &mut TestAppContext,
) {
    same_family_rescan(cx, "weight-rescan-one", one, true);
}

#[gpui::test]
fn weight_same_family_rescan_preserves_enumeration_failure_fallback(
    cx: &mut TestAppContext,
) {
    same_family_rescan(cx, "weight-rescan-failed", failed, false);
}

#[gpui::test]
fn weight_rescan_shows_current_while_the_latest_applied_family_is_pending(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("weight-rescan", "[font]\nweight = 300\n");
    let mut harness = DesktopHarness::new(cx);
    open(&mut harness, &file, multi);
    open_menu(&mut harness);
    let old = with_view(&mut harness, |view, _| view.weight_state());
    with_view(&mut harness, |view, cx| {
        cx.global_mut::<Desktop>().applied_terminal_family =
            Some("Next family".into());
        view.set_weight_enumerator(multi, cx);
        let pending = view.weight_state();
        assert_eq!(pending.0, "Next family");
        assert_eq!(
            pending.2,
            [FontWeight::parse(&toml::Value::Integer(300)).unwrap()]
        );
        assert_eq!(pending.3, None);
        assert!(!view.menu_open());
        view.weight_completion(&old.0, old.1, one(&old.0), cx);
        assert_eq!(view.weight_state(), pending);
    });
    harness.cx.run_until_parked();
    let latest = with_view(&mut harness, |view, _| view.weight_state());
    assert_eq!(latest.0, "Next family");
    assert_eq!(
        latest
            .2
            .iter()
            .map(|weight| weight.number())
            .collect::<Vec<_>>(),
        [300, 400, 500, 700]
    );
}
