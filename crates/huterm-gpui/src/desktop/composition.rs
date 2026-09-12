//! Pending platform text belongs to the view until `AppKit` commits it.

use std::ops::Range;

use super::TerminalView;
#[cfg(target_os = "macos")]
use super::windows;
#[cfg(any(target_os = "macos", test))]
use crate::ui::text::{range_from_utf16, range_to_utf16, utf16_len};
use gpui::Context;
#[cfg(target_os = "macos")]
use gpui::{App, Bounds, Pixels, Window, point, size};
#[cfg(target_os = "macos")]
use huterm_protocol::TerminalInput;

#[derive(Default)]
pub(super) struct Composition {
    text: String,
    selection: Range<usize>,
}

impl Composition {
    pub(super) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub(super) fn clear(&mut self) -> bool {
        let had_text = !self.is_empty();
        self.text.clear();
        self.selection = 0..0;
        had_text
    }
}

#[cfg(any(target_os = "macos", test))]
impl Composition {
    fn len(&self) -> usize {
        utf16_len(&self.text)
    }

    // AppKit ranges use UTF-16 units; never slice through a UTF-8 scalar or
    // the middle of a surrogate pair when adjusting an external range.
    fn range(&self, range: Range<usize>) -> (Range<usize>, Range<usize>) {
        let bytes = range_from_utf16(&self.text, range);
        let units = range_to_utf16(&self.text, bytes.clone());
        (bytes, units)
    }

    fn replace(&mut self, range: Option<Range<usize>>, text: &str) -> usize {
        let (bytes, units) = self.range(range.unwrap_or(0..self.len()));
        self.text.replace_range(bytes, text);
        units.start
    }

    fn mark(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        let start = self.replace(range, text);
        let length = text.encode_utf16().count();
        let selected = selected.unwrap_or(length..length);
        self.selection = start + selected.start.min(length)
            ..start + selected.end.max(selected.start).min(length);
    }

    fn commit(&mut self, range: Option<Range<usize>>, text: &str) -> String {
        self.replace(range, text);
        self.selection = 0..0;
        std::mem::take(&mut self.text)
    }
}

impl TerminalView {
    pub(super) fn clear_composition(&mut self, cx: &mut Context<'_, Self>) {
        self.clear_option_composition();
        if self.composition.clear() {
            #[cfg(target_os = "macos")]
            {
                let native_window = self.native_window;
                cx.spawn(async move |_, cx| {
                    // Leave the GPUI update before calling AppKit, which can
                    // synchronously call back into the platform input handler.
                    let context = native_window.update(cx, |_, window, cx| {
                        if windows::active_composition(window, cx) {
                            return Ok(None);
                        }
                        crate::native_quit::text_input_context(window).map(Some)
                    });
                    match context {
                        Ok(Ok(Some(context))) => context.discard_marked_text(),
                        Ok(Err(error)) => {
                            eprintln!("Huterm composition cleanup: {error}");
                        }
                        Ok(Ok(None)) | Err(_) => {}
                    }
                })
                .detach();
            }
            cx.notify();
        }
    }
}

#[cfg(target_os = "macos")]
impl TerminalView {
    fn accepts_composed_text(&self, window: &Window, cx: &App) -> bool {
        self.visible
            && !self.exited
            && window.is_window_active()
            && self.focus.is_focused(window)
            && windows::terminal_input_allowed(window, cx)
    }

    fn send_composed_text(&mut self, text: String, cx: &mut Context<'_, Self>) {
        if !text.is_empty() {
            self.enqueue_input(TerminalInput::Text(text));
            self.scroll.bottom();
            self.scroll.invalidate();
            self.start_snapshot_if_needed(cx);
            cx.notify();
        }
    }
}

#[cfg(target_os = "macos")]
impl gpui::EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<String> {
        if !self.accepts_composed_text(window, cx) {
            return None;
        }
        let (bytes, units) = self.composition.range(range);
        *adjusted = Some(units);
        Some(self.composition.text[bytes].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<gpui::UTF16Selection> {
        self.accepts_composed_text(window, cx)
            .then(|| gpui::UTF16Selection {
                range: self.composition.selection.clone(),
                reversed: false,
            })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<Range<usize>> {
        (self.accepts_composed_text(window, cx) && !self.composition.is_empty())
            .then(|| 0..self.composition.len())
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let text = std::mem::take(&mut self.composition.text);
        self.composition.clear();
        if self.accepts_composed_text(window, cx) {
            self.send_composed_text(text, cx);
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.accepts_composed_text(window, cx) {
            let text = self.composition.commit(range, text);
            self.send_composed_text(text, cx);
        } else {
            self.composition.clear();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.accepts_composed_text(window, cx) {
            self.composition.mark(range, text, selected);
        } else {
            self.composition.clear();
        }
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> Option<Bounds<Pixels>> {
        let cursor = self.snapshot.as_ref()?.cursor?;
        Some(Bounds::new(
            bounds.origin
                + point(
                    self.metrics.cell_width * f32::from(cursor.column),
                    self.metrics.cell_height * f32::from(cursor.row),
                ),
            size(self.metrics.cell_width, self.metrics.cell_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_key_is_pending_until_commit_and_sent_only_once() {
        let mut composition = Composition::default();
        composition.mark(None, "´", Some(1..1));
        assert_eq!(composition.text, "´");
        assert_eq!(composition.commit(None, "é"), "é");
        assert!(composition.is_empty());
        assert_eq!(composition.commit(None, "e"), "e");
    }

    #[test]
    fn canceled_composition_cannot_replay_after_policy_or_focus_change() {
        let mut composition = Composition::default();
        composition.mark(None, "´", None);
        assert!(composition.clear());
        assert!(!composition.clear());
        assert_eq!(composition.commit(None, "e"), "e");
    }

    #[test]
    fn utf16_preedit_replacement_preserves_surrogate_pairs() {
        let mut composition = Composition::default();
        composition.mark(None, "a😀z", None);
        assert_eq!(composition.range(2..3), (1..5, 1..3));
        composition.mark(Some(1..3), "é", Some(0..1));
        assert_eq!(composition.text, "aéz");
        assert_eq!(composition.selection, 1..2);
        assert_eq!(composition.commit(Some(1..2), "e"), "aez");
    }
}
