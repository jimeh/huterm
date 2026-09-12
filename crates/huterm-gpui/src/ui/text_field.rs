//! Single-line text input with native composition and grapheme editing.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, ElementInputHandler,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, Hsla,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    Render, ShapedLine, SharedString, TextRun, UTF16Selection, Window, canvas,
    div, fill, point, prelude::*, px,
};
use huterm_protocol::{CommandError, CommandInvocation, ids};
use unicode_segmentation::UnicodeSegmentation;

use super::text::{range_from_utf16, range_to_utf16};

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

    fn previous_word_boundary(&self, offset: usize) -> usize {
        self.content[..offset]
            .unicode_word_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        self.content[offset..]
            .unicode_word_indices()
            .next()
            .map_or(self.content.len(), |(index, word)| {
                offset + index + word.len()
            })
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

fn snap_to_grapheme_start(content: &str, index: usize) -> usize {
    let index = index.min(content.len());
    if index == content.len() {
        return index;
    }
    content
        .grapheme_indices(true)
        .map(|(start, _)| start)
        .take_while(|start| *start <= index)
        .last()
        .unwrap_or(0)
}

pub(crate) struct TextField {
    focus: FocusHandle,
    placeholder: String,
    buffer: TextBuffer,
    foreground: Hsla,
    /// The field's last painted bounds and shaped text, for pointer
    /// hit-testing.
    layout: Option<(Bounds<Pixels>, ShapedLine)>,
    /// A pointer selection in progress since a left press in the field.
    dragging: bool,
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
            layout: None,
            dragging: false,
        }
    }

    /// The grapheme offset nearest to a window `position`, from the last
    /// paint.
    fn index_at(&self, position: Point<Pixels>) -> Option<usize> {
        let (bounds, line) = self.layout.as_ref()?;
        let x = position.x - bounds.left() - px(FIELD_PADDING);
        let index = line.closest_index_for_x(x).min(self.buffer.content.len());
        // Glyph indices can fall inside a cluster; snap to its start.
        Some(snap_to_grapheme_start(&self.buffer.content, index))
    }

    fn press(&mut self, event: &MouseDownEvent, cx: &mut Context<'_, Self>) {
        let Some(index) = self.index_at(event.position) else {
            return;
        };
        match event.click_count {
            2 => {
                let word = self
                    .buffer
                    .content
                    .unicode_word_indices()
                    .map(|(start, word)| start..start + word.len())
                    .find(|range| range.contains(&index) || range.end == index);
                match word {
                    Some(range) => {
                        self.buffer.move_to(range.start);
                        self.buffer.select_to(range.end);
                    }
                    None => self.buffer.move_to(index),
                }
            }
            count if count >= 3 => {
                self.buffer.move_to(0);
                self.buffer.select_to(self.buffer.content.len());
            }
            _ if event.modifiers.shift => self.buffer.select_to(index),
            _ => self.buffer.move_to(index),
        }
        self.dragging = true;
        cx.notify();
    }

    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<'_, Self>) {
        if !self.dragging {
            return;
        }
        if let Some(index) = self.index_at(position)
            && index != self.buffer.cursor()
        {
            self.buffer.select_to(index);
            cx.notify();
        }
    }

    fn release(&mut self, cx: &mut Context<'_, Self>) {
        if self.dragging {
            self.dragging = false;
            cx.notify();
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.buffer.content
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.buffer.content.is_empty()
    }

    pub(crate) fn set_placeholder(&mut self, placeholder: impl Into<String>) {
        self.placeholder = placeholder.into();
    }

    pub(crate) fn set_foreground(
        &mut self,
        foreground: Hsla,
        cx: &mut Context<'_, Self>,
    ) {
        self.foreground = foreground;
        cx.notify();
    }

    /// Selects the whole buffer without emitting `Changed`.
    pub(crate) fn select_all_text(&mut self, cx: &mut Context<'_, Self>) {
        self.buffer.selection = 0..self.buffer.content.len();
        self.buffer.reversed = false;
        cx.notify();
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
    /// Moves the cursor to `target`, or extends the selection there when
    /// `select` is set. Cursor changes never emit `Changed`; the text is the
    /// same.
    fn move_cursor(
        &mut self,
        target: usize,
        select: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if select {
            self.buffer.select_to(target);
        } else {
            self.buffer.move_to(target);
        }
        cx.notify();
    }
    fn left(&mut self, select: bool, cx: &mut Context<'_, Self>) {
        let to = if select || self.buffer.selection.is_empty() {
            self.buffer.previous_boundary(self.buffer.cursor())
        } else {
            self.buffer.selection.start
        };
        self.move_cursor(to, select, cx);
    }
    fn right(&mut self, select: bool, cx: &mut Context<'_, Self>) {
        let to = if select || self.buffer.selection.is_empty() {
            self.buffer.next_boundary(self.buffer.cursor())
        } else {
            self.buffer.selection.end
        };
        self.move_cursor(to, select, cx);
    }
    fn home(&mut self, select: bool, cx: &mut Context<'_, Self>) {
        self.move_cursor(0, select, cx);
    }
    fn end(&mut self, select: bool, cx: &mut Context<'_, Self>) {
        self.move_cursor(self.buffer.content.len(), select, cx);
    }
    fn move_word_left(&mut self, select: bool, cx: &mut Context<'_, Self>) {
        let to = if select || self.buffer.selection.is_empty() {
            self.buffer.previous_word_boundary(self.buffer.cursor())
        } else {
            self.buffer.selection.start
        };
        self.move_cursor(to, select, cx);
    }
    fn move_word_right(&mut self, select: bool, cx: &mut Context<'_, Self>) {
        let to = if select || self.buffer.selection.is_empty() {
            self.buffer.next_word_boundary(self.buffer.cursor())
        } else {
            self.buffer.selection.end
        };
        self.move_cursor(to, select, cx);
    }
    fn select_all(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.buffer.selection = 0..self.buffer.content.len();
        self.buffer.reversed = false;
        cx.notify();
    }
    fn backspace(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) {
        if self.buffer.selection.is_empty() {
            self.buffer
                .select_to(self.buffer.previous_boundary(self.buffer.cursor()));
        }
        self.buffer.replace(self.buffer.selection.clone(), "");
        Self::changed(cx);
    }
    fn delete(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) {
        if self.buffer.selection.is_empty() {
            self.buffer
                .select_to(self.buffer.next_boundary(self.buffer.cursor()));
        }
        self.buffer.replace(self.buffer.selection.clone(), "");
        Self::changed(cx);
    }
    fn delete_word_backward(
        &mut self,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.buffer.selection.is_empty() {
            self.buffer.select_to(
                self.buffer.previous_word_boundary(self.buffer.cursor()),
            );
        }
        self.buffer.replace(self.buffer.selection.clone(), "");
        Self::changed(cx);
    }
    fn delete_word_forward(
        &mut self,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.buffer.selection.is_empty() {
            self.buffer.select_to(
                self.buffer.next_word_boundary(self.buffer.cursor()),
            );
        }
        self.buffer.replace(self.buffer.selection.clone(), "");
        Self::changed(cx);
    }
    fn delete_line_start(
        &mut self,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.buffer.selection.is_empty() {
            self.buffer.select_to(0);
        }
        self.buffer.replace(self.buffer.selection.clone(), "");
        Self::changed(cx);
    }
    fn copy(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) {
        if !self.buffer.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.buffer.content[self.buffer.selection.clone()].to_owned(),
            ));
        }
    }
    fn paste(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(text) =
            cx.read_from_clipboard().and_then(|item| item.text())
        {
            self.buffer.replace_input(None, &text);
            Self::changed(cx);
        }
    }

    /// Runs one text-editing catalog command.
    ///
    /// # Errors
    /// Returns [`CommandError::UnknownCommand`] for commands the field does
    /// not own.
    pub(crate) fn run(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<(), CommandError> {
        let select = invocation.bool("select").unwrap_or(false);
        match invocation.id {
            ids::TEXT_DELETE_BACKWARD => self.backspace(window, cx),
            ids::TEXT_DELETE_FORWARD => self.delete(window, cx),
            ids::TEXT_DELETE_WORD_BACKWARD => {
                self.delete_word_backward(window, cx);
            }
            ids::TEXT_DELETE_WORD_FORWARD => {
                self.delete_word_forward(window, cx);
            }
            ids::TEXT_DELETE_LINE_START => self.delete_line_start(window, cx),
            ids::TEXT_MOVE_LEFT => self.left(select, cx),
            ids::TEXT_MOVE_RIGHT => self.right(select, cx),
            ids::TEXT_MOVE_WORD_LEFT => self.move_word_left(select, cx),
            ids::TEXT_MOVE_WORD_RIGHT => self.move_word_right(select, cx),
            ids::TEXT_LINE_START => self.home(select, cx),
            ids::TEXT_LINE_END => self.end(select, cx),
            ids::TEXT_SELECT_ALL => self.select_all(window, cx),
            ids::TEXT_COPY => self.copy(window, cx),
            ids::TEXT_PASTE => self.paste(window, cx),
            other => return Err(CommandError::UnknownCommand(other)),
        }
        Ok(())
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
        let empty = self.buffer.content.is_empty();
        let mut contents = div().flex().items_center().w_full();
        if empty {
            contents = contents
                .child(div().opacity(0.62).child(self.placeholder.clone()));
        } else {
            contents = contents.child(self.buffer.content.clone());
        }
        let input = cx.entity();
        let focus = self.focus.clone();
        let dragging = self.dragging;
        // The caret and selection are painted over one unbroken text run,
        // positioned by measuring it. Splitting the text around an inline
        // caret element would shift the trailing text by the caret's width
        // and lose kerning across the split.
        let overlay = CaretOverlay {
            text: self.buffer.content.clone().into(),
            selection: self.buffer.selection.clone(),
            caret: focused.then_some(self.buffer.cursor()),
            foreground: self.foreground,
        };
        div()
            .relative()
            .w_full()
            .h(px(34.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .track_focus(&self.focus)
            .when(self.buffer.marked.is_some(), |field| {
                field
                    .border_b_1()
                    .border_color(self.foreground.opacity(0.8))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|field, event: &MouseDownEvent, _, cx| {
                    field.press(event, cx);
                }),
            )
            .child(contents)
            .child(
                canvas(
                    move |_, window, _| overlay.shape(window),
                    move |bounds, shaped, window, cx| {
                        shaped.paint(bounds, window);
                        input.update(cx, |field, _| {
                            field.layout = Some((bounds, shaped.line.clone()));
                        });
                        if dragging {
                            // Track the pointer beyond the field while a
                            // press-drag selection is in progress.
                            let mover = input.clone();
                            window.on_mouse_event(
                                move |event: &MouseMoveEvent, phase, _, cx| {
                                    if phase.bubble() {
                                        mover.update(cx, |field, cx| {
                                            field.drag_to(event.position, cx);
                                        });
                                    }
                                },
                            );
                            let releaser = input.clone();
                            window.on_mouse_event(
                                move |_: &MouseUpEvent, phase, _, cx| {
                                    if phase.bubble() {
                                        releaser.update(cx, |field, cx| {
                                            field.release(cx);
                                        });
                                    }
                                },
                            );
                        }
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

const FIELD_PADDING: f32 = 8.0;
const CARET_HEIGHT: f32 = 18.0;

/// What the caret canvas paints over the text run.
struct CaretOverlay {
    text: SharedString,
    selection: Range<usize>,
    caret: Option<usize>,
    foreground: Hsla,
}

/// The overlay after measuring the text with the field's font.
struct ShapedOverlay {
    line: ShapedLine,
    selection: Range<usize>,
    caret: Option<usize>,
    foreground: Hsla,
}

impl CaretOverlay {
    fn shape(self, window: &mut Window) -> ShapedOverlay {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let run = TextRun {
            len: self.text.len(),
            font: style.font(),
            color: self.foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line =
            window
                .text_system()
                .shape_line(self.text, font_size, &[run], None);
        ShapedOverlay {
            line,
            selection: self.selection,
            caret: self.caret,
            foreground: self.foreground,
        }
    }
}

impl ShapedOverlay {
    fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let left = bounds.left() + px(FIELD_PADDING);
        let top = bounds.top() + (bounds.size.height - px(CARET_HEIGHT)) / 2.0;
        let x = |index: usize| left + self.line.x_for_index(index);
        if !self.selection.is_empty() {
            let start = x(self.selection.start);
            let end = x(self.selection.end);
            window.paint_quad(fill(
                Bounds::from_corners(
                    point(start, top),
                    point(end, top + px(CARET_HEIGHT)),
                ),
                self.foreground.opacity(0.18),
            ));
        }
        if let Some(caret) = self.caret {
            let start = x(caret);
            window.paint_quad(fill(
                Bounds::from_corners(
                    point(start, top),
                    point(start + px(1.0), top + px(CARET_HEIGHT)),
                ),
                self.foreground,
            ));
        }
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
}
