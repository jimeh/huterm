use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    ClipboardItem, Entity, Modifiers, TestAppContext, VisualTestContext, div,
    point, px,
};

use super::*;
use crate::focus::ring_drawn;
use crate::testing::{a11y_node, host, tokens};

/// A field `width` points wide at the window's origin, and a count of its
/// `Changed` events.
struct Probe {
    input: Entity<Input>,
    changes: Rc<Cell<usize>>,
}

fn field(
    cx: &mut TestAppContext,
    width: f32,
    build: impl FnOnce(Input) -> Input + 'static,
) -> (Probe, &mut VisualTestContext) {
    let input = cx.update(|cx| {
        cx.new(|cx| build(Input::new("name", "Name", tokens(), cx)))
    });
    let changes = Rc::new(Cell::new(0));
    let counted = changes.clone();
    cx.update(|cx| {
        cx.subscribe(&input, move |_, _: &Changed, _| {
            counted.set(counted.get() + 1);
        })
        .detach();
    });
    let drawn = input.clone();
    let (_, cx) = host(cx, move |_, _| {
        div().w(px(width)).child(drawn.clone()).into_any_element()
    });
    (Probe { input, changes }, cx)
}

impl Probe {
    fn text(&self, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| self.input.read(cx).text().to_owned())
    }

    fn update<R>(
        &self,
        cx: &mut VisualTestContext,
        f: impl FnOnce(&mut Input, &mut Context<'_, Input>) -> R,
    ) -> R {
        let result = cx.update(|_, cx| self.input.update(cx, f));
        cx.run_until_parked();
        result
    }

    fn focused(&self, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| {
            self.input.read(cx).focus_handle(cx).is_focused(window)
        })
    }
}

#[gpui::test]
fn a_read_only_field_refuses_every_edit_path(cx: &mut TestAppContext) {
    let (probe, cx) = field(cx, 200.0, |input| input);
    probe.update(cx, |input, cx| {
        input.set_text("keep this", cx);
        input.set_read_only(true, cx);
    });
    let changes = probe.changes.get();
    // A press focuses it: read-only fields stay focusable and selectable.
    cx.simulate_click(point(px(20.0), px(12.0)), Modifiers::none());
    cx.run_until_parked();
    assert!(probe.focused(cx));
    cx.simulate_input("x");
    cx.update(|window, cx| {
        probe.input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "k", None, window, cx);
            input.replace_text_in_range(None, "y", window, cx);
        });
    });
    cx.write_to_clipboard(ClipboardItem::new_string("pasted".to_owned()));
    probe.update(cx, |input, cx| {
        input.paste(cx);
        input.delete_backward(cx);
        input.delete_forward(cx);
        input.delete_word_backward(cx);
        input.delete_word_forward(cx);
        input.delete_to_start(cx);
        input.select_all(cx);
        input.cut(cx);
    });
    assert_eq!(probe.text(cx), "keep this");
    assert_eq!(probe.changes.get(), changes, "no edit was reported");
    // Selecting and copying still work.
    assert_eq!(probe.update(cx, |input, _| input.selection()), 0..9);
    probe.update(cx, Input::copy);
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("keep this")
    );
    // The same typing edits the field once it is editable again.
    probe.update(cx, |input, cx| input.set_read_only(false, cx));
    cx.simulate_input("x");
    cx.run_until_parked();
    assert_eq!(probe.text(cx), "x");
}

#[gpui::test]
fn a_disabled_field_takes_no_focus_and_refuses_edits(cx: &mut TestAppContext) {
    let (probe, cx) = field(cx, 200.0, |input| input);
    probe.update(cx, |input, cx| {
        input.set_text("value", cx);
        input.set_disabled(true, Some("The file is invalid".into()), cx);
    });
    cx.simulate_click(point(px(20.0), px(12.0)), Modifiers::none());
    cx.run_until_parked();
    assert!(!probe.focused(cx));
    cx.write_to_clipboard(ClipboardItem::new_string("pasted".to_owned()));
    probe.update(cx, |input, cx| {
        input.select_all(cx);
        input.paste(cx);
        input.cut(cx);
        input.delete_backward(cx);
    });
    assert_eq!(probe.text(cx), "value");
    assert!(probe.update(cx, |input, _| input.selection().is_empty()));
    cx.update(|window, cx| {
        assert!(!probe.input.read(cx).focus_handle(cx).tab_stop);
        let aria = &a11y_node(window, "name")["aria"];
        assert_eq!(aria["disabled"], true);
        assert_eq!(aria["description"], "The file is invalid");
    });
}

#[gpui::test]
fn long_text_scrolls_to_keep_the_caret_in_view(cx: &mut TestAppContext) {
    let (probe, cx) = field(cx, 90.0, |input| input);
    let text = "a long value that runs well past a narrow field";
    probe.update(cx, |input, cx| input.set_text(text, cx));
    let in_view = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            let (caret, visible) = probe
                .input
                .read(cx)
                .painted_caret()
                .expect("the field painted");
            assert!(
                caret >= visible.left() && caret <= visible.right(),
                "the caret at {caret:?} is outside {visible:?}"
            );
            caret - visible.left()
        })
    };
    // The caret at the end of the text is in view.
    let end = in_view(cx);
    assert!(end > px(40.0), "the caret is at the right: {end:?}");
    // Back at the start, the text scrolls back.
    probe.update(cx, |input, cx| input.move_to_start(false, cx));
    assert_eq!(in_view(cx), px(0.0));
    // Selecting to the end follows the selection's moving end.
    probe.update(cx, |input, cx| input.move_to_end(true, cx));
    assert_eq!(
        probe.update(cx, |input, _| input.selection()),
        0..text.len()
    );
    in_view(cx);
    // A press near the right edge lands near the end of the scrolled text,
    // not where the unscrolled text would be.
    cx.simulate_click(point(px(80.0), px(12.0)), Modifiers::none());
    cx.run_until_parked();
    let cursor = probe.update(cx, |input, _| input.cursor());
    assert!(cursor + 3 >= text.len(), "the press landed at {cursor}");
}

#[gpui::test]
fn cut_copies_the_selection_then_deletes_it(cx: &mut TestAppContext) {
    let (probe, cx) = field(cx, 200.0, |input| input);
    probe.update(cx, |input, cx| input.set_text("hello world", cx));
    // Nothing selected: nothing is cut or copied.
    cx.write_to_clipboard(ClipboardItem::new_string("before".to_owned()));
    let changes = probe.changes.get();
    probe.update(cx, Input::cut);
    assert_eq!(probe.text(cx), "hello world");
    assert_eq!(probe.changes.get(), changes);
    probe.update(cx, |input, cx| {
        input.move_word_left(true, cx);
        input.cut(cx);
    });
    assert_eq!(probe.text(cx), "hello ");
    assert_eq!(probe.changes.get(), changes + 1);
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("world")
    );
}

#[gpui::test]
fn an_owner_write_ends_a_composition(cx: &mut TestAppContext) {
    let compose = |cx: &mut VisualTestContext, probe: &Probe| {
        cx.update(|window, cx| {
            probe.input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(
                    None, "かな", None, window, cx,
                );
            });
        });
    };
    let commit = |cx: &mut VisualTestContext, probe: &Probe| {
        let marked = cx.update(|window, cx| {
            probe
                .input
                .update(cx, |input, cx| input.marked_text_range(window, cx))
        });
        assert_eq!(marked, None, "the owner's write ended the composition");
        cx.update(|window, cx| {
            probe.input.update(cx, |input, cx| {
                input.replace_text_in_range(None, "!", window, cx);
            });
        });
        cx.run_until_parked();
    };
    let (probe, cx) = field(cx, 200.0, |input| input);
    probe.update(cx, |input, cx| input.set_text("hello", cx));
    // A stale composition range would index past the shorter text.
    compose(cx, &probe);
    probe.update(cx, |input, cx| input.set_text("x", cx));
    commit(cx, &probe);
    assert_eq!(probe.text(cx), "x!");
    compose(cx, &probe);
    probe.update(cx, |input, cx| input.replace(0..usize::MAX, "y", cx));
    commit(cx, &probe);
    assert_eq!(probe.text(cx), "y!");
}

#[gpui::test]
fn an_owner_replacement_is_one_line_of_whole_characters(
    cx: &mut TestAppContext,
) {
    // Never painted, so a line break fails here rather than in the text
    // shaper.
    let input = cx.new(|cx| Input::new("unpainted", "Unpainted", tokens(), cx));
    // 2..3 is inside "é", so the range widens to it.
    let (text, cursor) = cx.update(|cx| {
        input.update(cx, |input, cx| {
            input.set_text("aéb", cx);
            input.replace(2..3, "1\n2", cx);
            (input.text().to_owned(), input.cursor())
        })
    });
    assert_eq!((text.as_str(), cursor), ("a1 2b", 4));
}

#[gpui::test]
fn a_read_only_field_draws_no_caret(cx: &mut TestAppContext) {
    let (probe, cx) = field(cx, 200.0, |input| input);
    probe.update(cx, |input, cx| {
        input.set_text("keep this", cx);
        input.set_read_only(true, cx);
    });
    cx.simulate_click(point(px(20.0), px(12.0)), Modifiers::none());
    cx.run_until_parked();
    assert!(probe.focused(cx));
    assert!(!probe.update(cx, |input, _| input.caret_drawn()));
    probe.update(cx, |input, cx| input.set_read_only(false, cx));
    assert!(probe.update(cx, |input, _| input.caret_drawn()));
}

#[gpui::test]
fn the_field_reports_its_role_value_and_states(cx: &mut TestAppContext) {
    let (probe, cx) =
        field(cx, 200.0, |input| input.with_placeholder("Default"));
    cx.update(|window, _| {
        let aria = &a11y_node(window, "name")["aria"];
        assert_eq!(aria["role"], "TextInput");
        assert_eq!(aria["label"], "Name");
        assert_eq!(aria["placeholder"], "Default");
        assert!(aria["read_only"].is_null());
        assert!(aria["invalid"].is_null());
    });
    probe.update(cx, |input, cx| {
        input.set_text("Menlo", cx);
        input.set_read_only(true, cx);
        input.set_invalid(true, cx);
    });
    cx.update(|window, _| {
        let aria = &a11y_node(window, "name")["aria"];
        assert_eq!(aria["value"], "Menlo");
        assert_eq!(aria["read_only"], true);
        assert_eq!(aria["invalid"], "True");
    });
}

#[gpui::test]
fn keyboard_focus_rings_a_framed_field_only(cx: &mut TestAppContext) {
    for (chrome, ringed) in [(Chrome::Framed, true), (Chrome::Bare, false)] {
        let (probe, cx) =
            field(cx, 200.0, move |input| input.with_chrome(chrome));
        let handle = cx.update(|_, cx| probe.input.read(cx).focus_handle(cx));
        cx.simulate_keystrokes("a");
        cx.update(|window, cx| handle.focus(window, cx));
        cx.run_until_parked();
        let drawn = cx.update(|window, cx| ring_drawn(&handle, window, cx));
        assert_eq!(drawn, ringed, "{chrome:?}");
    }
}

#[test]
fn grapheme_navigation_and_deletion_keep_clusters_intact() {
    let mut buffer = TextBuffer::default();
    buffer.replace(0..0, "a👨‍👩‍👧‍👦é");
    let end = buffer.content.len();
    let before_e = buffer.previous_boundary(end);
    assert_eq!(&buffer.content[before_e..], "é");
    buffer.selection = before_e..end;
    buffer.replace(buffer.selection.clone(), "");
    assert_eq!(buffer.content, "a👨‍👩‍👧‍👦");
}

#[test]
fn pointer_indices_snap_to_grapheme_starts_and_keep_the_end() {
    let content = "e\u{301}x";
    assert_eq!(snap_to_grapheme_start(content, 1), 0);
    assert_eq!(snap_to_grapheme_start(content, 3), 3);
    assert_eq!(snap_to_grapheme_start(content, 4), 4);
}

#[test]
fn word_boundaries_delete_forward_and_backward_from_the_cursor() {
    let mut buffer = TextBuffer::default();
    buffer.replace(0..0, "one two three");
    buffer.selection = 4..4;
    buffer.select_to(buffer.next_word_boundary(buffer.cursor()));
    buffer.replace(buffer.selection.clone(), "");
    assert_eq!(buffer.content, "one  three");
    buffer.selection = 4..4;
    buffer.select_to(buffer.previous_word_boundary(buffer.cursor()));
    buffer.replace(buffer.selection.clone(), "");
    assert_eq!(buffer.content, " three");
}

#[test]
fn marked_replacement_and_paste_remain_single_line() {
    let mut buffer = TextBuffer::default();
    buffer.replace_marked(None, "e", Some(0..1));
    buffer.replace_input(None, "é\nnext");
    assert_eq!(buffer.content, "é next");
    assert!(buffer.marked.is_none());
}

#[test]
fn keyboard_selection_tracks_direction() {
    let mut buffer = TextBuffer::default();
    buffer.replace(0..0, "abc");
    buffer.select_to(2);
    buffer.select_to(1);
    assert_eq!(buffer.selection, 1..3);
    assert!(buffer.reversed);
    buffer.select_to(3);
    buffer.select_to(buffer.next_boundary(buffer.cursor()));
    assert_eq!(buffer.selection, 3..3);
    assert!(!buffer.reversed);
}
