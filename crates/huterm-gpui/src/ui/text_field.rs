//! Single-line text input with native composition and grapheme editing.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, ElementInputHandler,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, Hsla, KeyBinding,
    Pixels, Render, UTF16Selection, Window, canvas, div, prelude::*, px,
};
use unicode_segmentation::UnicodeSegmentation;

use super::text::{range_from_utf16, range_to_utf16};

gpui::actions!(
    huterm_text_field,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Copy,
        Paste
    ]
);

pub(crate) fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("backspace", Backspace, Some("PaletteText")),
        KeyBinding::new("delete", Delete, Some("PaletteText")),
        KeyBinding::new("left", Left, Some("PaletteText")),
        KeyBinding::new("right", Right, Some("PaletteText")),
        KeyBinding::new("shift-left", SelectLeft, Some("PaletteText")),
        KeyBinding::new("shift-right", SelectRight, Some("PaletteText")),
        KeyBinding::new("home", Home, Some("PaletteText")),
        KeyBinding::new("end", End, Some("PaletteText")),
        KeyBinding::new("cmd-a", SelectAll, Some("PaletteText")),
        KeyBinding::new("ctrl-a", SelectAll, Some("PaletteText")),
        KeyBinding::new("cmd-c", Copy, Some("PaletteText")),
        KeyBinding::new("ctrl-c", Copy, Some("PaletteText")),
        KeyBinding::new("ctrl-shift-c", Copy, Some("PaletteText")),
        KeyBinding::new("cmd-v", Paste, Some("PaletteText")),
        KeyBinding::new("ctrl-v", Paste, Some("PaletteText")),
        KeyBinding::new("ctrl-shift-v", Paste, Some("PaletteText")),
    ]
}

#[derive(Clone, Debug)]
pub(crate) struct Changed;

#[derive(Clone, Debug, Default)]
struct TextBuffer {
    content: String,
    selection: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
}

impl TextBuffer {
    fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    fn move_to(&mut self, offset: usize) {
        self.selection = offset..offset;
        self.reversed = false;
    }

    fn select_to(&mut self, offset: usize) {
        let anchor = if self.reversed {
            self.selection.end
        } else {
            self.selection.start
        };
        if offset < anchor {
            self.selection = offset..anchor;
            self.reversed = true;
        } else {
            self.selection = anchor..offset;
            self.reversed = false;
        }
    }

    fn replace(&mut self, range: Range<usize>, text: &str) {
        let text: String = text
            .chars()
            .map(|ch| if matches!(ch, '\n' | '\r') { ' ' } else { ch })
            .collect();
        self.content.replace_range(range.clone(), &text);
        let cursor = range.start + text.len();
        self.selection = cursor..cursor;
        self.reversed = false;
        self.marked = None;
    }

    fn replace_input(&mut self, range: Option<Range<usize>>, text: &str) {
        let range = range
            .map(|range| range_from_utf16(&self.content, range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.clone());
        self.replace(range, text);
    }

    fn replace_marked(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        let range = range
            .map(|range| range_from_utf16(&self.content, range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.clone());
        let start = range.start;
        self.replace(range, text);
        if !text.is_empty() {
            self.marked = Some(start..start + text.len());
        }
        let selected = selected.unwrap_or_else(|| {
            text.encode_utf16().count()..text.encode_utf16().count()
        });
        let selected = range_from_utf16(text, selected);
        self.selection = start + selected.start..start + selected.end;
    }
}

pub(crate) struct TextField {
    focus: FocusHandle,
    placeholder: String,
    buffer: TextBuffer,
    foreground: Hsla,
}

impl TextField {
    pub(crate) fn new(
        placeholder: impl Into<String>,
        foreground: Hsla,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            placeholder: placeholder.into(),
            buffer: TextBuffer::default(),
            foreground,
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.buffer.content
    }

    pub(crate) fn set_text(
        &mut self,
        text: impl Into<String>,
        cx: &mut Context<'_, Self>,
    ) {
        self.buffer.content = text.into().replace(['\r', '\n'], " ");
        let end = self.buffer.content.len();
        self.buffer.move_to(end);
        cx.emit(Changed);
        cx.notify();
    }

    fn changed(cx: &mut Context<'_, Self>) {
        cx.emit(Changed);
        cx.notify();
    }
    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<'_, Self>) {
        let to = if self.buffer.selection.is_empty() {
            self.buffer.previous_boundary(self.buffer.cursor())
        } else {
            self.buffer.selection.start
        };
        self.buffer.move_to(to);
        Self::changed(cx);
    }
    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<'_, Self>) {
        let to = if self.buffer.selection.is_empty() {
            self.buffer.next_boundary(self.buffer.cursor())
        } else {
            self.buffer.selection.end
        };
        self.buffer.move_to(to);
        Self::changed(cx);
    }
    fn select_left(
        &mut self,
        _: &SelectLeft,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let to = self.buffer.previous_boundary(self.buffer.cursor());
        self.buffer.select_to(to);
        Self::changed(cx);
    }
    fn select_right(
        &mut self,
        _: &SelectRight,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let to = self.buffer.next_boundary(self.buffer.cursor());
        self.buffer.select_to(to);
        Self::changed(cx);
    }
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.buffer.move_to(0);
        Self::changed(cx);
    }
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.buffer.move_to(self.buffer.content.len());
        Self::changed(cx);
    }
    fn select_all(
        &mut self,
        _: &SelectAll,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.buffer.selection = 0..self.buffer.content.len();
        self.buffer.reversed = false;
        Self::changed(cx);
    }
    fn backspace(
        &mut self,
        _: &Backspace,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.buffer.selection.is_empty() {
            self.buffer
                .select_to(self.buffer.previous_boundary(self.buffer.cursor()));
        }
        self.buffer.replace(self.buffer.selection.clone(), "");
        Self::changed(cx);
    }
    fn delete(
        &mut self,
        _: &Delete,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.buffer.selection.is_empty() {
            self.buffer
                .select_to(self.buffer.next_boundary(self.buffer.cursor()));
        }
        self.buffer.replace(self.buffer.selection.clone(), "");
        Self::changed(cx);
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<'_, Self>) {
        if !self.buffer.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.buffer.content[self.buffer.selection.clone()].to_owned(),
            ));
        }
    }
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(text) =
            cx.read_from_clipboard().and_then(|item| item.text())
        {
            self.buffer.replace_input(None, &text);
            Self::changed(cx);
        }
    }
}

impl EventEmitter<Changed> for TextField {}
impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> Option<String> {
        let bytes = range_from_utf16(&self.buffer.content, range);
        *adjusted = Some(range_to_utf16(&self.buffer.content, bytes.clone()));
        Some(self.buffer.content[bytes].to_owned())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: range_to_utf16(
                &self.buffer.content,
                self.buffer.selection.clone(),
            ),
            reversed: self.buffer.reversed,
        })
    }
    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> Option<Range<usize>> {
        self.buffer
            .marked
            .clone()
            .map(|range| range_to_utf16(&self.buffer.content, range))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.buffer.marked = None;
        Self::changed(cx);
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.buffer.replace_input(range, text);
        Self::changed(cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.buffer.replace_marked(range, text, selected);
        Self::changed(cx);
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
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

impl Render for TextField {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let mut contents = div().flex().items_center().w_full();
        if self.buffer.content.is_empty() {
            if focused {
                contents = contents
                    .child(div().w(px(1.0)).h(px(18.0)).bg(self.foreground));
            }
            contents = contents
                .child(div().opacity(0.62).child(self.placeholder.clone()));
        } else {
            let selection = self.buffer.selection.clone();
            let before = self.buffer.content[..selection.start].to_owned();
            let selected = self.buffer.content[selection.clone()].to_owned();
            let after = self.buffer.content[selection.end..].to_owned();
            contents = contents.child(before);
            if focused && self.buffer.reversed {
                contents = contents
                    .child(div().w(px(1.0)).h(px(18.0)).bg(self.foreground));
            }
            if !selected.is_empty() {
                contents = contents.child(
                    div().bg(self.foreground.opacity(0.18)).child(selected),
                );
            }
            if focused && !self.buffer.reversed {
                contents = contents
                    .child(div().w(px(1.0)).h(px(18.0)).bg(self.foreground));
            }
            contents = contents.child(after);
        }
        let input = cx.entity();
        let focus = self.focus.clone();
        div()
            .relative()
            .w_full()
            .h(px(34.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .key_context("PaletteText")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .when(self.buffer.marked.is_some(), |field| {
                field
                    .border_b_1()
                    .border_color(self.foreground.opacity(0.8))
            })
            .child(contents)
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, input),
                            cx,
                        );
                    },
                )
                .absolute()
                .inset_0(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
