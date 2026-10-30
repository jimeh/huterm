//! Input: a single-line text field, after shadcn/ui's Input, `NSTextField`,
//! and `GtkEntry`, with native composition (IME) and grapheme editing.
//!
//! Anatomy: one line of text, or a placeholder while empty, with a caret
//! and a selection painted over it. [`Chrome::Framed`] draws the control's
//! own border and focus ring; [`Chrome::Grouped`] fills an
//! [`InputGroup`](super::input_group::InputGroup), which draws them; and
//! [`Chrome::Bare`] leaves them to an owner such as a palette's search bar.
//! Line breaks typed, pasted, or set become spaces.
//!
//! Keyboard: the field takes focus; text arrives through the platform's
//! input handler, so composition works. The field binds no keys: its owner
//! routes editing commands to its methods ([`Input::move_left`] and its
//! siblings, the deletions, [`Input::select_all`], [`Input::cut`],
//! [`Input::copy`], and [`Input::paste`]).
//!
//! Pointer: a press places the caret, Shift extends the selection, a
//! double press selects a word, and a triple press selects everything. A
//! drag selects and keeps tracking outside the field until the release. A
//! right press keeps a selection it lands on and otherwise places the
//! caret, as macOS does, then focuses the field and dispatches
//! [`ContextMenu`] from it, so the owner that handles that action opens
//! the field's menu; the field draws none itself.
//!
//! Scrolling: text wider than the field scrolls horizontally, by the
//! smallest distance that keeps the caret, which is the selection's moving
//! end, in view.
//!
//! Focus: the owner reads the field's handle through `Focusable`. A framed
//! field shows the focus ring for keyboard focus.
//!
//! States: read-only keeps the field focusable, selectable, and copyable,
//! draws no caret, as a selectable `NSTextField` does not, and refuses
//! typing, composition, paste, cut, and deletion. Disabled
//! takes no focus from a press, asks for no context menu, drops the tab
//! stop, refuses every edit and
//! selection, dims the field, and shows its reason as a tooltip. Invalid
//! draws a framed field's border in the danger color. Both refusals are
//! user edits only: the owner's [`Input::set_text`] and [`Input::replace`]
//! still apply.
//!
//! Accessibility: a `TextInput` named by its label, reporting its text as
//! its value, its placeholder, its read-only, disabled, and invalid states,
//! and a disabled reason as its description. A field that drives a list of
//! suggestions ([`Input::set_list_state`]), such as a combobox's or a
//! command's search, is an `EditableComboBox` instead, reporting whether the
//! list shows and the highlighted suggestion as its description. GPUI
//! exposes no caret or selection ranges, so assistive technology reads the
//! value but may not move through it by character.
//!
//! Events: [`Changed`] follows every change of the text, and [`Edited`]
//! comes first when the user made it, so an owner can tell typing from its
//! own writes. Ending a composition leaves the text as it is and emits
//! neither. [`Moved`] follows a caret or selection move that leaves the
//! text as it is, such as an arrow key or a press.
//!
//! Reference: WAI-ARIA's textbox and editable combobox, and `NSTextField`
//! for editing and scrolling.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, Div, ElementId,
    ElementInputHandler, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Render, ShapedLine, SharedString, Stateful, TextAlign,
    TextRun, UTF16Selection, WeakEntity, Window, canvas, div, fill, point,
    prelude::*, px,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::a11y::States;
use crate::components::tooltip::Tooltip;
use crate::focus;
use crate::text::{range_from_utf16, range_to_utf16};
use crate::tokens::{Tokens, size};
use crate::typography::ui;

/// Emitted whenever the text changes, by the user or the owner.
#[derive(Clone, Debug)]
pub struct Changed;

/// Emitted before [`Changed`] when the user changed the text: typing,
/// composing, pasting, cutting, or deleting. The owner's
/// [`Input::set_text`] and [`Input::replace`] emit only [`Changed`].
#[derive(Clone, Debug)]
pub struct Edited;

/// Emitted when the caret or the selection moves and the text stays the
/// same, so an owner whose suggestions depend on the caret can follow it.
#[derive(Clone, Debug)]
pub struct Moved;

/// Asks the field's owner to open the field's context menu. A right press
/// dispatches it from the field, after focusing the field and placing the
/// caret, so it reaches the nearest handler on the field's focus path.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = huterm_ui, no_json)]
pub struct ContextMenu {
    /// The field the menu edits.
    pub field: WeakEntity<Input>,
    /// Where the press landed, in window coordinates.
    pub position: Point<Pixels>,
}

/// What a field reports while it drives a list of suggestions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListState {
    /// Whether the list shows.
    pub expanded: bool,
    /// The highlighted suggestion's label, reported as the field's
    /// description, since GPUI honors an active descendant only inside the
    /// focused node.
    pub active: Option<SharedString>,
}

/// Who draws the field's border and focus ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Chrome {
    /// The field draws them: a control-height outlined box.
    #[default]
    Framed,
    /// An [`InputGroup`](super::input_group::InputGroup) draws them; the
    /// field fills the group's height between its addons.
    Grouped,
    /// The owner draws them around a 34-point-tall field, as the palette's
    /// search bar does.
    Bare,
}

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

/// What the field last painted, for pointer hit-testing and scrolling.
struct Layout {
    bounds: Bounds<Pixels>,
    line: ShapedLine,
    /// How far the text is scrolled left, in points.
    scroll: Pixels,
    /// Whether the caret was drawn.
    #[cfg(feature = "test-support")]
    caret: bool,
}

/// A single-line text field.
pub struct Input {
    id: ElementId,
    label: SharedString,
    focus: FocusHandle,
    placeholder: SharedString,
    buffer: TextBuffer,
    tokens: Tokens,
    chrome: Chrome,
    read_only: bool,
    /// The disabled reason, empty for none, while disabled.
    disabled: Option<SharedString>,
    invalid: bool,
    /// The list the field drives, if any.
    list: Option<ListState>,
    layout: Option<Layout>,
    /// A pointer selection in progress since a left press in the field.
    dragging: bool,
}

impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Input")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("text", &self.buffer.content)
            .field("read_only", &self.read_only)
            .field("disabled", &self.disabled)
            .field("invalid", &self.invalid)
            .finish_non_exhaustive()
    }
}

impl Input {
    /// An empty, framed field named `label`.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        tokens: Tokens,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            focus: cx.focus_handle(),
            placeholder: SharedString::default(),
            buffer: TextBuffer::default(),
            tokens,
            chrome: Chrome::Framed,
            read_only: false,
            disabled: None,
            invalid: false,
            list: None,
            layout: None,
            dragging: false,
        }
    }

    /// The field with `placeholder` shown while it is empty.
    #[must_use]
    pub fn with_placeholder(
        mut self,
        placeholder: impl Into<SharedString>,
    ) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// The field drawn with `chrome`.
    #[must_use]
    pub fn with_chrome(mut self, chrome: Chrome) -> Self {
        self.chrome = chrome;
        self
    }

    /// Who draws the field's border and focus ring.
    #[must_use]
    pub fn chrome(&self) -> Chrome {
        self.chrome
    }

    /// The field's accessible name.
    #[must_use]
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// The field's text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.buffer.content
    }

    /// Whether the field has no text.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffer.content.is_empty()
    }

    /// The text shown while the field is empty.
    #[must_use]
    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }

    /// Replaces the text shown while the field is empty.
    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>) {
        self.placeholder = placeholder.into();
    }

    /// The colors the field draws with.
    #[must_use]
    pub fn tokens(&self) -> Tokens {
        self.tokens
    }

    /// Recolors the field, as after a theme change; unchanged tokens are
    /// ignored, so an owner may call this on every render.
    pub fn set_tokens(&mut self, tokens: Tokens, cx: &mut Context<'_, Self>) {
        if self.tokens == tokens {
            return;
        }
        self.tokens = tokens;
        cx.notify();
    }

    /// Whether the field refuses edits but stays focusable and selectable.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Makes the field read-only, or editable again.
    pub fn set_read_only(
        &mut self,
        read_only: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if self.read_only != read_only {
            self.read_only = read_only;
            cx.notify();
        }
    }

    /// Whether the field is disabled.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled.is_some()
    }

    /// Disables the field, with `reason` as its tooltip and accessible
    /// description, or enables it again when `disabled` is false. A disabled
    /// field is not a tab stop; enabling it makes it one again.
    pub fn set_disabled(
        &mut self,
        disabled: bool,
        reason: Option<SharedString>,
        cx: &mut Context<'_, Self>,
    ) {
        let next = disabled.then(|| reason.unwrap_or_default());
        if self.disabled == next {
            return;
        }
        let _ = self.focus.clone().tab_stop(!disabled);
        self.disabled = next;
        cx.notify();
    }

    /// Whether the field's value fails validation.
    #[must_use]
    pub fn is_invalid(&self) -> bool {
        self.invalid
    }

    /// Marks the field's value as failing validation, or not.
    pub fn set_invalid(&mut self, invalid: bool, cx: &mut Context<'_, Self>) {
        if self.invalid != invalid {
            self.invalid = invalid;
            cx.notify();
        }
    }

    /// What the field reports about the list it drives, if any.
    #[must_use]
    pub fn list_state(&self) -> Option<&ListState> {
        self.list.as_ref()
    }

    /// Makes the field report the list it drives, or a plain text field
    /// again with `None`; an unchanged state is ignored.
    pub fn set_list_state(
        &mut self,
        list: Option<ListState>,
        cx: &mut Context<'_, Self>,
    ) {
        if self.list != list {
            self.list = list;
            cx.notify();
        }
    }

    /// The caret's byte offset: the selection's moving end.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.buffer.cursor()
    }

    /// The selected byte range, empty at the caret.
    #[must_use]
    pub fn selection(&self) -> Range<usize> {
        self.buffer.selection.clone()
    }

    /// Replaces `range` with `text` and puts the caret after it, as typing
    /// over a selection does. The range widens to the characters it
    /// touches, and ends a composition in progress. The owner's edit, so it
    /// applies even when the field refuses the user's.
    pub fn replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let content = &self.buffer.content;
        let start = content.floor_char_boundary(range.start);
        let end = content.ceil_char_boundary(range.end).max(start);
        let text = text.replace(['\r', '\n'], " ");
        self.buffer.content.replace_range(start..end, &text);
        self.buffer.marked = None;
        self.buffer.move_to(start + text.len());
        Self::changed(cx);
    }

    /// Replaces the text and puts the caret at its end, ending a
    /// composition in progress. The owner's edit, so it applies even when
    /// the field refuses the user's.
    pub fn set_text(
        &mut self,
        text: impl Into<String>,
        cx: &mut Context<'_, Self>,
    ) {
        self.buffer.content = text.into().replace(['\r', '\n'], " ");
        self.buffer.marked = None;
        let end = self.buffer.content.len();
        self.buffer.move_to(end);
        Self::changed(cx);
    }

    /// Moves the caret one grapheme left, or to the selection's start;
    /// with `select`, extends the selection instead.
    pub fn move_left(&mut self, select: bool, cx: &mut Context<'_, Self>) {
        let to = if select || self.buffer.selection.is_empty() {
            self.buffer.previous_boundary(self.buffer.cursor())
        } else {
            self.buffer.selection.start
        };
        self.move_cursor(to, select, cx);
    }

    /// Moves the caret one grapheme right, or to the selection's end; with
    /// `select`, extends the selection instead.
    pub fn move_right(&mut self, select: bool, cx: &mut Context<'_, Self>) {
        let to = if select || self.buffer.selection.is_empty() {
            self.buffer.next_boundary(self.buffer.cursor())
        } else {
            self.buffer.selection.end
        };
        self.move_cursor(to, select, cx);
    }

    /// Moves the caret to the start of the previous word.
    pub fn move_word_left(&mut self, select: bool, cx: &mut Context<'_, Self>) {
        let to = if select || self.buffer.selection.is_empty() {
            self.buffer.previous_word_boundary(self.buffer.cursor())
        } else {
            self.buffer.selection.start
        };
        self.move_cursor(to, select, cx);
    }

    /// Moves the caret to the end of the next word.
    pub fn move_word_right(
        &mut self,
        select: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let to = if select || self.buffer.selection.is_empty() {
            self.buffer.next_word_boundary(self.buffer.cursor())
        } else {
            self.buffer.selection.end
        };
        self.move_cursor(to, select, cx);
    }

    /// Moves the caret to the start of the text.
    pub fn move_to_start(&mut self, select: bool, cx: &mut Context<'_, Self>) {
        self.move_cursor(0, select, cx);
    }

    /// Moves the caret to the end of the text.
    pub fn move_to_end(&mut self, select: bool, cx: &mut Context<'_, Self>) {
        self.move_cursor(self.buffer.content.len(), select, cx);
    }

    /// Selects the whole text. Selection is not an edit, so it emits no
    /// [`Changed`].
    pub fn select_all(&mut self, cx: &mut Context<'_, Self>) {
        if self.is_disabled() {
            return;
        }
        let before = self.selection_state();
        self.buffer.selection = 0..self.buffer.content.len();
        self.buffer.reversed = false;
        self.moved(&before, cx);
    }

    /// Deletes the selection, or the grapheme before the caret.
    pub fn delete_backward(&mut self, cx: &mut Context<'_, Self>) {
        self.delete_to(|buffer| buffer.previous_boundary(buffer.cursor()), cx);
    }

    /// Deletes the selection, or the grapheme after the caret.
    pub fn delete_forward(&mut self, cx: &mut Context<'_, Self>) {
        self.delete_to(|buffer| buffer.next_boundary(buffer.cursor()), cx);
    }

    /// Deletes the selection, or back to the previous word's start.
    pub fn delete_word_backward(&mut self, cx: &mut Context<'_, Self>) {
        self.delete_to(
            |buffer| buffer.previous_word_boundary(buffer.cursor()),
            cx,
        );
    }

    /// Deletes the selection, or forward to the next word's end.
    pub fn delete_word_forward(&mut self, cx: &mut Context<'_, Self>) {
        self.delete_to(|buffer| buffer.next_word_boundary(buffer.cursor()), cx);
    }

    /// Deletes the selection, or back to the start of the text.
    pub fn delete_to_start(&mut self, cx: &mut Context<'_, Self>) {
        self.delete_to(|_| 0, cx);
    }

    /// Copies the selection to the clipboard. A read-only field copies; a
    /// disabled one does not.
    pub fn copy(&mut self, cx: &mut Context<'_, Self>) {
        if self.is_disabled() || self.buffer.selection.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.buffer.content[self.buffer.selection.clone()].to_owned(),
        ));
    }

    /// Copies the selection to the clipboard, then deletes it. Nothing
    /// happens when nothing is selected or the field refuses edits.
    pub fn cut(&mut self, cx: &mut Context<'_, Self>) {
        if !self.editable() || self.buffer.selection.is_empty() {
            return;
        }
        self.copy(cx);
        self.buffer.replace(self.buffer.selection.clone(), "");
        Self::edited(cx);
    }

    /// Replaces the selection with the clipboard's text.
    pub fn paste(&mut self, cx: &mut Context<'_, Self>) {
        if !self.editable() {
            return;
        }
        if let Some(text) =
            cx.read_from_clipboard().and_then(|item| item.text())
        {
            self.buffer.replace_input(None, &text);
            Self::edited(cx);
        }
    }

    /// The caret's window x and the bounds the text is visible in, as last
    /// painted, so tests can check the caret stays in view.
    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn painted_caret(&self) -> Option<(Pixels, Bounds<Pixels>)> {
        let layout = self.layout.as_ref()?;
        let visible = text_area(layout.bounds, px(FIELD_PADDING));
        let x = visible.left() + layout.line.x_for_index(self.buffer.cursor())
            - layout.scroll;
        Some((x, visible))
    }

    /// Whether the last paint drew the caret.
    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn caret_drawn(&self) -> bool {
        self.layout.as_ref().is_some_and(|layout| layout.caret)
    }

    /// Whether the user may change the text.
    fn editable(&self) -> bool {
        !self.read_only && self.disabled.is_none()
    }

    fn changed(cx: &mut Context<'_, Self>) {
        cx.emit(Changed);
        cx.notify();
    }

    /// Reports a change the user made.
    fn edited(cx: &mut Context<'_, Self>) {
        cx.emit(Edited);
        Self::changed(cx);
    }

    /// Deletes the selection, or from the caret to `to` when nothing is
    /// selected.
    fn delete_to(
        &mut self,
        to: impl FnOnce(&TextBuffer) -> usize,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.editable() {
            return;
        }
        if self.buffer.selection.is_empty() {
            let target = to(&self.buffer);
            self.buffer.select_to(target);
        }
        self.buffer.replace(self.buffer.selection.clone(), "");
        Self::edited(cx);
    }

    /// The selection and which end the caret is at.
    fn selection_state(&self) -> (Range<usize>, bool) {
        (self.buffer.selection.clone(), self.buffer.reversed)
    }

    /// Redraws after a caret or selection change, reporting [`Moved`] when
    /// it differs from `before`.
    fn moved(&self, before: &(Range<usize>, bool), cx: &mut Context<'_, Self>) {
        if self.selection_state() != *before {
            cx.emit(Moved);
        }
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
        if self.is_disabled() {
            return;
        }
        let before = self.selection_state();
        if select {
            self.buffer.select_to(target);
        } else {
            self.buffer.move_to(target);
        }
        self.moved(&before, cx);
    }

    /// The grapheme offset nearest to a window `position`, from the last
    /// paint.
    fn index_at(&self, position: Point<Pixels>) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let x = position.x - layout.bounds.left() - px(FIELD_PADDING)
            + layout.scroll;
        let index = layout
            .line
            .closest_index_for_x(x)
            .min(self.buffer.content.len());
        // Glyph indices can fall inside a cluster; snap to its start.
        Some(snap_to_grapheme_start(&self.buffer.content, index))
    }

    fn press(&mut self, event: &MouseDownEvent, cx: &mut Context<'_, Self>) {
        let Some(index) = self.index_at(event.position) else {
            return;
        };
        let before = self.selection_state();
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
        self.moved(&before, cx);
    }

    /// A right press: a selection under the pointer stays, and anywhere
    /// else the caret moves there. Then the field takes focus and asks its
    /// owner for the menu; the dispatch waits for this update to end, so
    /// the owner can read the field.
    fn context_press(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.on_selection(event.position)
            && let Some(index) = self.index_at(event.position)
        {
            let before = self.selection_state();
            self.buffer.move_to(index);
            self.moved(&before, cx);
        }
        window.focus(&self.focus, cx);
        window.dispatch_action(
            Box::new(ContextMenu {
                field: cx.entity().downgrade(),
                position: event.position,
            }),
            cx,
        );
    }

    /// Whether a window `position` lies over the painted selection.
    fn on_selection(&self, position: Point<Pixels>) -> bool {
        let selection = &self.buffer.selection;
        let Some(layout) =
            self.layout.as_ref().filter(|_| !selection.is_empty())
        else {
            return false;
        };
        let x = position.x - layout.bounds.left() - px(FIELD_PADDING)
            + layout.scroll;
        (layout.line.x_for_index(selection.start)
            ..=layout.line.x_for_index(selection.end))
            .contains(&x)
    }

    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<'_, Self>) {
        if !self.dragging {
            return;
        }
        if let Some(index) = self.index_at(position)
            && index != self.buffer.cursor()
        {
            let before = self.selection_state();
            self.buffer.select_to(index);
            self.moved(&before, cx);
        }
    }

    fn release(&mut self, cx: &mut Context<'_, Self>) {
        if self.dragging {
            self.dragging = false;
            cx.notify();
        }
    }
}

impl EventEmitter<Changed> for Input {}

impl EventEmitter<Edited> for Input {}

impl EventEmitter<Moved> for Input {}

impl Focusable for Input {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for Input {
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
        // The composed text stays as it is, so this is no change of the
        // text: an owner must not take it for its own write.
        self.buffer.marked = None;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.editable() {
            return;
        }
        self.buffer.replace_input(range, text);
        Self::edited(cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.editable() {
            return;
        }
        self.buffer.replace_marked(range, text, selected);
        Self::edited(cx);
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

impl Input {
    /// The canvas that paints the text, caret, and selection, records what
    /// it painted, and takes the platform's text input.
    fn text_canvas(
        &self,
        focused: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement + use<> {
        let input = cx.entity();
        let focus = self.focus.clone();
        let dragging = self.dragging;
        // The text, caret, and selection are painted from one unbroken text
        // run, positioned by measuring it and scrolled to keep the caret in
        // view. Splitting the text around an inline caret element would
        // shift the trailing text by the caret's width and lose kerning
        // across the split.
        let overlay = Overlay {
            text: self.buffer.content.clone().into(),
            selection: self.buffer.selection.clone(),
            cursor: self.buffer.cursor(),
            caret: focused && !self.read_only,
            marked: self
                .buffer
                .marked
                .clone()
                .filter(|_| self.chrome != Chrome::Bare),
            scroll: self
                .layout
                .as_ref()
                .map_or(px(0.0), |layout| layout.scroll),
            padding: px(FIELD_PADDING),
            foreground: self.tokens.fg,
        };
        canvas(
            move |bounds, window, _| overlay.shape(bounds, window),
            move |bounds, shaped, window, cx| {
                shaped.paint(window, cx);
                input.update(cx, |field, _| {
                    field.layout = Some(Layout {
                        bounds,
                        line: shaped.line.clone(),
                        scroll: shaped.scroll,
                        #[cfg(feature = "test-support")]
                        caret: shaped.caret,
                    });
                });
                if dragging {
                    // Track the pointer beyond the field while a press-drag
                    // selection is in progress.
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
        .inset_0()
    }

    /// `field` sized and outlined for the field's [`Chrome`].
    fn chrome_style(&self, field: Stateful<Div>) -> Stateful<Div> {
        let tokens = self.tokens;
        match self.chrome {
            Chrome::Framed => field
                .w_full()
                .h(size::CONTROL_HEIGHT)
                .rounded(px(size::CONTROL_RADIUS))
                .border_1()
                .border_color(if self.invalid {
                    tokens.danger
                } else {
                    tokens.line_strong
                })
                .bg(tokens.bg)
                .text_size(size::TEXT_BODY),
            Chrome::Grouped => field
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .text_size(size::TEXT_BODY),
            // A bare field underlines itself while composing; the other
            // chromes underline the marked text.
            Chrome::Bare => field
                .w_full()
                .h(ui(34.0))
                .when(self.buffer.marked.is_some(), |field| {
                    field.border_b_1().border_color(tokens.fg.opacity(0.8))
                }),
        }
    }
}

impl Render for Input {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let tokens = self.tokens;
        let enabled = self.disabled.is_none();
        let reason = self.disabled.clone().filter(|reason| !reason.is_empty());
        // A disabled reason describes the field before any suggestion.
        let active = self
            .list
            .as_ref()
            .and_then(|list| list.active.clone())
            .filter(|_| reason.is_none());
        let text = self.text_canvas(self.focus.is_focused(window), cx);
        let field = div()
            .id(self.id.clone())
            .role(if self.list.is_some() {
                gpui::Role::EditableComboBox
            } else {
                gpui::Role::TextInput
            })
            .aria_label(self.label.clone())
            .aria_value(SharedString::from(self.buffer.content.clone()))
            .when(!self.placeholder.is_empty(), |field| {
                field.aria_placeholder(self.placeholder.clone())
            })
            .when_some(self.list.as_ref(), |field, list| {
                field.aria_expanded(list.expanded)
            })
            .when_some(active, StatefulInteractiveElement::aria_description)
            .when_some(reason, |field, reason| {
                field
                    .aria_description(reason.clone())
                    .tooltip(move |_, cx| {
                        Tooltip::view(reason.clone(), tokens, cx)
                    })
            })
            .relative()
            .px(px(FIELD_PADDING))
            .flex()
            .items_center()
            .overflow_hidden()
            .track_focus(&self.focus)
            .map(|field| self.chrome_style(field))
            // A group dims itself and its addons together.
            .when(!enabled && self.chrome != Chrome::Grouped, |field| {
                field.opacity(0.5)
            })
            .when(!enabled, |field| {
                field
                    .on_mouse_down(MouseButton::Left, |_, window, _| {
                        window.prevent_default();
                    })
                    .on_mouse_down(MouseButton::Right, |_, window, _| {
                        window.prevent_default();
                    })
            })
            .when(enabled, |field| {
                field
                    .cursor(gpui::CursorStyle::IBeam)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|field, event: &MouseDownEvent, _, cx| {
                            field.press(event, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(
                            |field, event: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                field.context_press(event, window, cx);
                            },
                        ),
                    )
            })
            .when(self.buffer.content.is_empty(), |field| {
                field.child(
                    div()
                        .text_color(tokens.muted)
                        .whitespace_nowrap()
                        .child(self.placeholder.clone()),
                )
            })
            .child(text);
        let field = if self.chrome == Chrome::Framed {
            focus::ring(field, &self.focus, tokens, window, cx)
        } else {
            field
        };
        States {
            disabled: !enabled,
            read_only: self.read_only,
            invalid: self.invalid,
            ..States::default()
        }
        .apply(field)
    }
}

const FIELD_PADDING: f32 = 8.0;
const CARET_HEIGHT: f32 = 18.0;
const CARET_WIDTH: f32 = 1.0;

/// The part of a field's `bounds` its text shows in.
fn text_area(bounds: Bounds<Pixels>, padding: Pixels) -> Bounds<Pixels> {
    Bounds::from_corners(
        point(bounds.left() + padding, bounds.top()),
        point(
            (bounds.right() - padding).max(bounds.left() + padding),
            bounds.bottom(),
        ),
    )
}

/// How far to scroll text of `width` so the caret at `caret` shows in a
/// `visible`-wide area, moving the least from `scroll`.
fn scroll_to_caret(
    scroll: Pixels,
    caret: Pixels,
    width: Pixels,
    visible: Pixels,
) -> Pixels {
    let room = (visible - px(CARET_WIDTH)).max(px(0.0));
    let mut scroll = scroll.min((width - room).max(px(0.0)));
    if caret - scroll > room {
        scroll = caret - room;
    }
    if caret < scroll {
        scroll = caret;
    }
    scroll.max(px(0.0))
}

/// What the field's canvas paints: its text with the caret and selection.
struct Overlay {
    text: SharedString,
    selection: Range<usize>,
    cursor: usize,
    /// Whether to draw the caret: the field has focus and is editable.
    caret: bool,
    /// The composition range, underlined unless the chrome marks it.
    marked: Option<Range<usize>>,
    scroll: Pixels,
    padding: Pixels,
    foreground: Hsla,
}

/// The overlay after measuring the text with the field's font.
struct Shaped {
    line: ShapedLine,
    line_height: Pixels,
    visible: Bounds<Pixels>,
    scroll: Pixels,
    selection: Range<usize>,
    cursor: usize,
    caret: bool,
    marked: Option<Range<usize>>,
    foreground: Hsla,
}

impl Overlay {
    fn shape(self, bounds: Bounds<Pixels>, window: &mut Window) -> Shaped {
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
        let visible = text_area(bounds, self.padding);
        let scroll = scroll_to_caret(
            self.scroll,
            line.x_for_index(self.cursor),
            line.width,
            visible.size.width,
        );
        Shaped {
            line,
            line_height: window.line_height(),
            visible,
            scroll,
            selection: self.selection,
            cursor: self.cursor,
            caret: self.caret,
            marked: self.marked,
            foreground: self.foreground,
        }
    }
}

impl Shaped {
    fn paint(&self, window: &mut Window, cx: &mut App) {
        let visible = self.visible;
        let caret_height = px(CARET_HEIGHT).min(visible.size.height - px(6.0));
        let top = visible.top() + (visible.size.height - caret_height) / 2.0;
        let x = |index: usize| {
            visible.left() + self.line.x_for_index(index) - self.scroll
        };
        // The caret may sit on the area's right edge, so the mask keeps
        // its width.
        let mask = ContentMask {
            bounds: Bounds::from_corners(
                visible.origin,
                point(visible.right() + px(CARET_WIDTH), visible.bottom()),
            ),
        };
        window.with_content_mask(Some(mask), |window| {
            if !self.selection.is_empty() {
                window.paint_quad(fill(
                    Bounds::from_corners(
                        point(x(self.selection.start), top),
                        point(x(self.selection.end), top + caret_height),
                    ),
                    self.foreground.opacity(0.18),
                ));
            }
            let origin = point(
                visible.left() - self.scroll,
                visible.top() + (visible.size.height - self.line_height) / 2.0,
            );
            let _ = self.line.paint(
                origin,
                self.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
            if let Some(marked) = &self.marked {
                let bottom = top + caret_height;
                window.paint_quad(fill(
                    Bounds::from_corners(
                        point(x(marked.start), bottom - px(1.0)),
                        point(x(marked.end), bottom),
                    ),
                    self.foreground.opacity(0.8),
                ));
            }
            if self.caret {
                let start = x(self.cursor);
                window.paint_quad(fill(
                    Bounds::from_corners(
                        point(start, top),
                        point(start + px(CARET_WIDTH), top + caret_height),
                    ),
                    self.foreground,
                ));
            }
        });
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests;
