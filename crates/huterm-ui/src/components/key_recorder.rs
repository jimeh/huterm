//! `KeyRecorder`: a button that records a new keyboard shortcut, such as a
//! keybinding's key or a global shortcut.
//!
//! Anatomy: a [`KeyRecorder`] entity holds the transient recording state;
//! a [`KeyRecorderButton`] draws it each frame with the owner's current
//! shortcut, which the component never caches. The button shows that
//! shortcut as [`kbd`] glyphs, or a placeholder such as "Add shortcut"
//! when there is none, in an outlined control-height pill. While recording
//! it takes the accent border and shows the strokes so far, or "Press
//! keys". The owner decides the glyphs: [`KeyRecorder::new`] takes the
//! function that turns a shortcut into them, since the crate has no notion
//! of platforms or key names. [`KeyRecorder::hint`] says what recording
//! waits for, for the owner to show under its row.
//!
//! Recording: a pointer click, or the activation command the owner routes
//! to [`KeyRecorder::start`] (Space and Return by default), starts it and
//! focuses the button. The recorder must see raw keys, so it binds no
//! catalog commands: the owner registers one keystroke interceptor and
//! forwards each [`KeystrokeEvent`] to [`KeyRecorder::intercept`], which
//! says whether it took the key, so the owner stops its propagation and no
//! binding, menu, or field acts on it. Scope the interceptor to the owner's
//! window, since GPUI calls it for keystrokes in every window. Feed the
//! recorder that started last, even after it stops, so it can swallow what
//! follows its recording.
//!
//! Keyboard while recording:
//!
//! | Key | Effect |
//! | --- | --- |
//! | Escape | Cancels at any point. |
//! | Return | Saves the strokes so far; before any stroke it is a stroke. |
//! | A modifier alone | Ignored. |
//! | A held key's repeat | Ignored, so a held key records one stroke. |
//! | Any other key | Records a stroke. |
//!
//! A recording ends with its last stroke ([`KeyRecorder::with_max_strokes`],
//! two by default, for a chord) or [`CHORD_TIMEOUT`] after the latest one,
//! GPUI's own chord timeout. It reports [`KeyRecorderEvent::Recorded`] with
//! the strokes in GPUI's keystroke syntax, joined by spaces. Afterwards the
//! recorder swallows the repeats of a key still held until a fresh press,
//! so a held shortcut acts nowhere else. A Return or Space pressed just
//! after the timeout saved, meant to save, is swallowed too rather than
//! starting another recording, but only as the first key after it: once
//! the user has pressed another, such as Tab to a field, or pressed the
//! pointer outside the button, Return and Space reach whatever has focus.
//!
//! Dismissal: Escape, focus leaving the button, window deactivation, which
//! GPUI reports as focus leaving, and a press outside the button cancel,
//! reporting [`KeyRecorderEvent::Canceled`]. A press on the button while it
//! records starts over.
//!
//! Focus: the owner supplies the focus handle, so its tab order and focus
//! recovery keep working. The ring shows for keyboard focus only.
//!
//! States: a disabled button takes no focus from a press, does not record,
//! and shows its reason as a tooltip. Keyboard activation arrives through
//! the owner, which checks its own state before calling `start`.
//!
//! Accessibility: a `Button` named by its label, reporting the shortcut,
//! or the strokes so far, as its value, the hint as its description while
//! recording, the disabled reason otherwise, and its disabled state.
//!
//! Reference: macOS's shortcut fields in System Settings and GNOME's
//! keyboard shortcut dialog, which take the next keystroke and cancel on
//! Escape.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, Context, Div, ElementId, Entity, EventEmitter, FocusHandle,
    IntoElement, KeystrokeEvent, MouseButton, Refineable, RenderOnce,
    SharedString, Stateful, StyleRefinement, Styled, Subscription, Window, div,
    prelude::*, px,
};

use crate::a11y::States;
use crate::click::from_keyboard;
use crate::components::kbd::{self, Glyph};
use crate::components::tooltip::Tooltip;
use crate::focus;
use crate::tokens::{Tokens, size};

/// How long a recording waits for a chord's next stroke: GPUI's own chord
/// timeout.
pub const CHORD_TIMEOUT: Duration = Duration::from_secs(1);

/// What a recorder reports to its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyRecorderEvent {
    /// Recording started.
    Started,
    /// Recording finished with these strokes, in GPUI's keystroke syntax and
    /// joined by spaces, such as `cmd-k cmd-t`.
    Recorded(String),
    /// Recording ended without a shortcut.
    Canceled,
}

/// Turns a shortcut in GPUI's keystroke syntax into the glyphs to draw.
type Format = Rc<dyn Fn(&str) -> Vec<Glyph>>;

/// One recording in progress.
struct Recording {
    strokes: Vec<String>,
    /// Cancels on blur, which window deactivation also reports.
    _blur: Subscription,
}

/// A key recorder's transient state: whether it records, and the strokes so
/// far.
pub struct KeyRecorder {
    focus: FocusHandle,
    format: Format,
    max_strokes: usize,
    recording: Option<Recording>,
    /// Bumped by every start, so a blur ends only the recording it left.
    starts: u64,
    /// Bumped by every stroke, so only the latest stroke's timer ends the
    /// recording.
    generation: u64,
    /// A recording ended; repeats of a key still held are swallowed until a
    /// fresh press.
    swallow_repeats: bool,
    /// When the chord timeout last saved.
    timed_out_at: Option<Instant>,
}

impl std::fmt::Debug for KeyRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyRecorder")
            .field("max_strokes", &self.max_strokes)
            .field("strokes", &self.strokes())
            .finish_non_exhaustive()
    }
}

impl EventEmitter<KeyRecorderEvent> for KeyRecorder {}

impl KeyRecorder {
    /// A recorder that takes focus through the owner's `focus` handle and
    /// draws shortcuts as `format` spells them.
    #[must_use]
    pub fn new(
        focus: FocusHandle,
        format: impl Fn(&str) -> Vec<Glyph> + 'static,
    ) -> Self {
        Self {
            focus,
            format: Rc::new(format),
            max_strokes: 2,
            recording: None,
            starts: 0,
            generation: 0,
            swallow_repeats: false,
            timed_out_at: None,
        }
    }

    /// Ends each recording after `strokes` strokes, such as one for a
    /// global shortcut, which cannot be a chord.
    #[must_use]
    pub fn with_max_strokes(mut self, strokes: usize) -> Self {
        self.max_strokes = strokes.max(1);
        self
    }

    /// The focus handle the button takes focus through.
    #[must_use]
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Whether a recording is in progress.
    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// The strokes recorded so far; empty when not recording.
    #[must_use]
    pub fn strokes(&self) -> &[String] {
        self.recording
            .as_ref()
            .map_or(&[], |recording| recording.strokes.as_slice())
    }

    /// What the recording waits for, while recording.
    #[must_use]
    pub fn hint(&self) -> Option<SharedString> {
        let recording = self.recording.as_ref()?;
        Some(if recording.strokes.is_empty() {
            "Press the new shortcut. Esc cancels.".into()
        } else {
            let next = if self.max_strokes == 2 {
                "a second key"
            } else {
                "another key"
            };
            format!(
                "Saves in a moment. Press {next} for a chord, Return to \
                 save now, or Esc to cancel."
            )
            .into()
        })
    }

    /// Starts recording, focusing the button, or starts over when already
    /// recording.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.starts += 1;
        self.generation += 1;
        let start = self.starts;
        self.timed_out_at = None;
        self.focus.focus(window, cx);
        // Focus listeners run inside a draw, where a redraw request is lost.
        let blur = cx.on_blur(&self.focus, window, move |_, window, cx| {
            cx.defer_in(window, move |recorder, _, cx| {
                if recorder.starts == start {
                    recorder.cancel(cx);
                }
            });
        });
        self.recording = Some(Recording {
            strokes: Vec::new(),
            _blur: blur,
        });
        cx.emit(KeyRecorderEvent::Started);
        cx.notify();
    }

    /// Cancels the recording; `false` when none was in progress.
    pub fn cancel(&mut self, cx: &mut Context<'_, Self>) -> bool {
        if self.recording.take().is_none() {
            return false;
        }
        self.swallow_repeats = true;
        cx.emit(KeyRecorderEvent::Canceled);
        cx.notify();
        true
    }

    /// Takes a keystroke from the owner's interceptor, before any binding
    /// sees it. Returns whether the recorder took it, so the owner stops
    /// its propagation.
    pub fn intercept(
        &mut self,
        event: &KeystrokeEvent,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if self.recording.is_none() {
            return self.intercept_idle(event, cx);
        }
        if event.is_held {
            return true;
        }
        let keystroke = &event.keystroke;
        let plain = !keystroke.modifiers.modified();
        let empty = self.strokes().is_empty();
        match keystroke.key.as_str() {
            "escape" if plain => {
                self.cancel(cx);
                return true;
            }
            "enter" if plain && !empty => {
                self.finish(cx);
                return true;
            }
            "shift" | "control" | "alt" | "platform" | "function" => {
                return true;
            }
            _ => {}
        }
        let Some(recording) = &mut self.recording else {
            return true;
        };
        recording
            .strokes
            .push(keystroke.unparse(cx.platform_info().os));
        if recording.strokes.len() >= self.max_strokes {
            self.finish(cx);
            return true;
        }
        self.generation += 1;
        let generation = self.generation;
        cx.spawn(async move |recorder, cx| {
            cx.background_executor().timer(CHORD_TIMEOUT).await;
            let _ = recorder.update(cx, |recorder, cx| {
                if recorder.generation == generation && recorder.is_recording()
                {
                    recorder.finish(cx);
                    recorder.timed_out_at =
                        Some(cx.background_executor().now());
                }
            });
        })
        .detach();
        cx.notify();
        true
    }

    /// Takes a keystroke while not recording: the repeats of a key held
    /// since the last recording ended, and a Return or Space meant to save
    /// that lands just after the chord timeout saved, as the first key
    /// pressed since.
    fn intercept_idle(
        &mut self,
        event: &KeystrokeEvent,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if event.is_held {
            return self.swallow_repeats;
        }
        self.swallow_repeats = false;
        let keystroke = &event.keystroke;
        let meant_to_save = !keystroke.modifiers.modified()
            && matches!(keystroke.key.as_str(), "enter" | "space")
            && self.timed_out_at.is_some_and(|at| {
                cx.background_executor().now().duration_since(at)
                    < CHORD_TIMEOUT / 2
            });
        // Any key ends the window: only the first one can be meant to save.
        self.timed_out_at = None;
        meant_to_save
    }

    /// A pointer press outside the button cancels a recording and, as a
    /// keystroke does, ends the window in which a Return or Space after a
    /// timeout save is swallowed: the press may have moved focus to the
    /// control that key is meant for.
    fn pressed_outside(&mut self, cx: &mut Context<'_, Self>) {
        self.timed_out_at = None;
        self.cancel(cx);
    }

    fn finish(&mut self, cx: &mut Context<'_, Self>) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        self.swallow_repeats = true;
        let event = if recording.strokes.is_empty() {
            KeyRecorderEvent::Canceled
        } else {
            KeyRecorderEvent::Recorded(recording.strokes.join(" "))
        };
        cx.emit(event);
        cx.notify();
    }
}

/// A key recorder's button, drawn each frame with the owner's shortcut.
#[derive(IntoElement)]
pub struct KeyRecorderButton {
    id: ElementId,
    label: SharedString,
    recorder: Entity<KeyRecorder>,
    tokens: Tokens,
    shortcut: Option<SharedString>,
    placeholder: SharedString,
    disabled: Option<SharedString>,
    key_context: Option<SharedString>,
    style: StyleRefinement,
}

impl std::fmt::Debug for KeyRecorderButton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyRecorderButton")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("shortcut", &self.shortcut)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

impl KeyRecorderButton {
    /// The button for `recorder`, named `label`.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        recorder: &Entity<KeyRecorder>,
        tokens: Tokens,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            recorder: recorder.clone(),
            tokens,
            shortcut: None,
            placeholder: SharedString::default(),
            disabled: None,
            key_context: None,
            style: StyleRefinement::default(),
        }
    }

    /// The current shortcut in GPUI's keystroke syntax, if any.
    #[must_use]
    pub fn shortcut(
        mut self,
        shortcut: Option<impl Into<SharedString>>,
    ) -> Self {
        self.shortcut = shortcut.map(Into::into);
        self
    }

    /// The text shown when there is no shortcut, such as "Add shortcut".
    #[must_use]
    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// Disables the button; `reason`, when given, shows as its tooltip.
    #[must_use]
    pub fn disabled(mut self, reason: Option<SharedString>) -> Self {
        self.disabled = Some(reason.unwrap_or_default());
        self
    }

    /// The key context the button carries, such as the owner's control
    /// context that its activation command binds under.
    #[must_use]
    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = Some(context.into());
        self
    }
}

impl Styled for KeyRecorderButton {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for KeyRecorderButton {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let enabled = self.disabled.is_none();
        let recorder = self.recorder.read(cx);
        let focus = recorder.focus.clone().tab_stop(enabled);
        let recording = recorder.is_recording();
        let strokes = recorder.strokes().join(" ");
        let shown = if recording {
            Some(strokes.as_str()).filter(|strokes| !strokes.is_empty())
        } else {
            self.shortcut.as_deref()
        }
        .map(|shortcut| (recorder.format)(shortcut));
        let description = if recording {
            recorder.hint()
        } else {
            self.disabled.clone().filter(|reason| !reason.is_empty())
        };
        let color = if recording {
            tokens.accent_text()
        } else {
            tokens.fg
        };
        let content =
            content(shown.as_deref(), recording, &self.placeholder, color);
        let value = shown.as_deref().map(kbd::text).unwrap_or_default();
        let mut button = div()
            .id(self.id.clone())
            .role(gpui::Role::Button)
            .aria_label(self.label.clone())
            .aria_value(value)
            .when_some(description, |button, text| {
                button.aria_description(text)
            })
            .track_focus(&focus)
            .when_some(self.key_context.take(), |button, context| {
                button.key_context(context.as_ref())
            })
            .flex()
            .flex_none()
            .items_center()
            .overflow_hidden()
            .h(size::CONTROL_HEIGHT)
            .px(px(8.0))
            .rounded(px(size::CONTROL_RADIUS))
            .border_1()
            .border_color(tokens.line_strong)
            .text_size(size::TEXT_CONTROL)
            .text_color(color);
        // The owner's style applies over the default look; recording's
        // accent border applies over both.
        button.style().refine(&std::mem::take(&mut self.style));
        let button = button
            .when(recording, |button| button.border_color(tokens.accent))
            .when_some(
                self.disabled.clone().filter(|reason| !reason.is_empty()),
                |button, text| {
                    button.tooltip(move |_, cx| {
                        Tooltip::view(text.clone(), tokens, cx)
                    })
                },
            )
            .map(|button| interactions(button, &self.recorder, tokens, enabled))
            .child(content);
        let button = focus::ring(button, &focus, tokens, window, cx);
        States {
            disabled: !enabled,
            ..States::default()
        }
        .apply(button)
    }
}

/// What the button shows: the strokes so far while recording, else the
/// shortcut, each as `glyphs`, or a prompt or the placeholder without one.
fn content(
    glyphs: Option<&[Glyph]>,
    recording: bool,
    placeholder: &SharedString,
    color: gpui::Hsla,
) -> gpui::Div {
    match (glyphs, recording) {
        (Some(glyphs), true) => div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(kbd::glyphs(glyphs, px(12.0), color))
            .child(div().text_size(size::TEXT_SMALL).child("…")),
        (Some(glyphs), false) => kbd::glyphs(glyphs, px(12.0), color),
        (None, true) => div().text_size(size::TEXT_SMALL).child("Press keys"),
        (None, false) => div().child(placeholder.clone()),
    }
}

/// `button`'s pointer behavior: hover and pressed states and a click that
/// starts recording, or, when disabled, its dimming and a press guard; and
/// for both, a press outside it that cancels a recording and ends the
/// guard after a timeout save.
fn interactions(
    button: Stateful<Div>,
    recorder: &Entity<KeyRecorder>,
    tokens: Tokens,
    enabled: bool,
) -> Stateful<Div> {
    let button = if enabled {
        let (hover, pressed) = (tokens.hover, tokens.pressed());
        let starts = recorder.clone();
        button
            .hover(move |style| style.bg(hover))
            .active(move |style| style.bg(pressed))
            .on_click(move |event, window, cx| {
                if !from_keyboard(event) {
                    starts.update(cx, |recorder, cx| {
                        recorder.start(window, cx);
                    });
                }
            })
    } else {
        button
            .opacity(0.5)
            .on_mouse_down(MouseButton::Left, |_, window, _| {
                window.prevent_default();
            })
    };
    let cancels = recorder.clone();
    button.on_mouse_down_out(move |_, _, cx| {
        let recorder = cancels.read(cx);
        if recorder.is_recording() || recorder.timed_out_at.is_some() {
            cancels.update(cx, KeyRecorder::pressed_outside);
        }
    })
}

#[cfg(all(test, feature = "test-support"))]
mod tests;
