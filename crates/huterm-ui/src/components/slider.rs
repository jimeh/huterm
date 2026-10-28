//! Slider: a horizontal control that picks a value in a range by position,
//! after shadcn/ui's Slider, `NSSlider`, and `GtkScale`, such as the Quake
//! window's width as a percent.
//!
//! Anatomy: a track filled in the accent up to a round thumb, and an
//! optional readout of the value after it, such as "50%".
//!
//! Values: the slider moves along its [`Steps`] grid, clamped to the range,
//! so it never reports a value outside it. A value the owner gives off the
//! grid draws where it lies, and the first step moves it to the next grid
//! value in that direction.
//!
//! Keyboard: the slider takes focus and binds no keys. The owner routes its
//! commands through [`Steps`]: [`Steps::increment`] and
//! [`Steps::decrement`] for the arrows, [`Steps::page_up`] and
//! [`Steps::page_down`] for Page Up and Page Down, and [`Steps::min`] and
//! [`Steps::max`] for Home and End, and writes the result.
//!
//! Pointer: a press on the track moves the thumb there and takes focus; a
//! drag keeps moving it, also outside the slider, until the release. Each
//! position snaps to the grid, and the owner hears only changes.
//!
//! States: disabled takes no focus from a press, drops the tab stop, does
//! not move, dims the slider, and shows its reason as a tooltip. Disabling
//! ends a drag in progress; re-enabling does not resume it.
//!
//! Accessibility: a horizontal `Slider` named by its label, reporting its
//! numeric value, minimum, maximum, and step, the readout as its value, and
//! its disabled state; Increment and Decrement actions step it.
//!
//! Reference: WAI-ARIA's slider pattern and Radix's Slider.

use std::rc::Rc;

use gpui::{
    AccessibleAction, App, Bounds, Div, ElementId, Entity, FocusHandle,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, RenderOnce, SharedString, Window, canvas, div, prelude::*, px,
    relative,
};

use crate::a11y::States;
use crate::components::tooltip::Tooltip;
use crate::focus;
use crate::steps::Steps;
use crate::tokens::{Tokens, size};

type Change = Rc<dyn Fn(f64, &mut Window, &mut App)>;

/// The thumb's diameter.
const THUMB: f32 = 14.0;
/// The track's thickness.
const TRACK: f32 = 4.0;

/// What a slider remembers between frames: the track's painted bounds, for
/// mapping the pointer, and the drag in progress.
#[derive(Default)]
struct Drag {
    track: Option<Bounds<Pixels>>,
    dragging: bool,
    /// The value the drag last reported, so a move within one grid cell
    /// reports nothing.
    reported: Option<f64>,
}

/// A slider.
#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    label: SharedString,
    value: f64,
    steps: Steps,
    focus: FocusHandle,
    tokens: Tokens,
    readout: Option<SharedString>,
    disabled: Option<SharedString>,
    key_context: Option<SharedString>,
    on_change: Option<Change>,
}

impl std::fmt::Debug for Slider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Slider")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("value", &self.value)
            .field("steps", &self.steps)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

impl Slider {
    /// A slider named `label` at `value` along `steps`, focused through
    /// `focus`.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        value: f64,
        steps: Steps,
        focus: &FocusHandle,
        tokens: Tokens,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            value,
            steps,
            focus: focus.clone(),
            tokens,
            readout: None,
            disabled: None,
            key_context: None,
            on_change: None,
        }
    }

    /// The value as text, drawn after the track and reported as the
    /// slider's value, such as "50%".
    #[must_use]
    pub fn readout(mut self, text: impl Into<SharedString>) -> Self {
        self.readout = Some(text.into());
        self
    }

    /// Disables the slider; `reason`, when given, shows as its tooltip and
    /// accessible description.
    #[must_use]
    pub fn disabled(mut self, reason: Option<SharedString>) -> Self {
        self.disabled = Some(reason.unwrap_or_default());
        self
    }

    /// The key context the slider carries, such as the owner's control
    /// context that its step commands bind under.
    #[must_use]
    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = Some(context.into());
        self
    }

    /// Called with each new value the pointer or assistive technology
    /// picks.
    #[must_use]
    pub fn on_change(
        mut self,
        change: impl Fn(f64, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(change));
        self
    }
}

/// The grid value under the pointer at `x` along `track`.
fn value_at(steps: Steps, track: Bounds<Pixels>, x: Pixels) -> f64 {
    let width = f64::from(track.size.width / px(1.0));
    if width <= 0.0 {
        return steps.min();
    }
    steps.at_fraction(f64::from((x - track.left()) / px(1.0)) / width)
}

/// Reports `value` through `change` unless the drag already did.
fn report(
    drag: &Entity<Drag>,
    value: f64,
    change: &Change,
    window: &mut Window,
    cx: &mut App,
) {
    let fresh = drag.update(cx, |drag, _| {
        let fresh = drag.reported != Some(value);
        drag.reported = Some(value);
        fresh
    });
    if fresh {
        change(value, window, cx);
    }
}

/// Whether a step from `from` landed somewhere else. Grid values are
/// exact, so equality means the step was clamped at a bound.
#[expect(clippy::float_cmp, reason = "grid values are exact")]
fn moved(from: f64, to: f64) -> bool {
    from != to
}

/// Ends the drag in progress, if any.
fn release(drag: &Entity<Drag>, cx: &mut App) {
    drag.update(cx, |drag, cx| {
        if drag.dragging || drag.reported.is_some() {
            drag.dragging = false;
            drag.reported = None;
            cx.notify();
        }
    });
}

/// The canvas that records the track's painted bounds and follows a drag
/// beyond the slider until the release, as a scrollbar thumb does. Its
/// window-level listeners exist on every frame and read the live drag
/// state, so a release anywhere ends a drag that began after the last
/// paint.
fn drag_tracker(
    drag: &Entity<Drag>,
    steps: Steps,
    change: Option<Change>,
) -> impl IntoElement + use<> {
    let drag = drag.clone();
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| {
            drag.update(cx, |drag, _| drag.track = Some(bounds));
            let ended = drag.clone();
            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                if phase.bubble() {
                    release(&ended, cx);
                }
            });
            let Some(change) = change.clone() else {
                return;
            };
            let follow = drag.clone();
            window.on_mouse_event(
                move |event: &MouseMoveEvent, phase, window, cx| {
                    let (dragging, track) = {
                        let drag = follow.read(cx);
                        (drag.dragging, drag.track)
                    };
                    let Some(track) = track.filter(|_| dragging) else {
                        return;
                    };
                    if phase.bubble() && event.dragging() {
                        let value = value_at(steps, track, event.position.x);
                        report(&follow, value, &change, window, cx);
                    }
                },
            );
        },
    )
    .absolute()
    .inset_0()
}

impl Slider {
    /// The track, filled up to the thumb, in a press area as tall as the
    /// control. The thumb overhangs the track's ends by its radius.
    fn area(
        &self,
        drag: &Entity<Drag>,
        change: Option<Change>,
        focus: &FocusHandle,
        window: &Window,
        cx: &mut App,
    ) -> Div {
        let (tokens, steps, value) = (self.tokens, self.steps, self.value);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a fraction from 0 to 1 fits an f32"
        )]
        let fraction = steps.fraction(value) as f32;
        let thumb = div()
            .absolute()
            .top(px((TRACK - THUMB) / 2.0))
            .left(relative(fraction))
            .ml(px(-THUMB / 2.0))
            .size(px(THUMB))
            .rounded_full()
            .bg(tokens.accent)
            .border_2()
            .border_color(tokens.bg);
        let track = div()
            .relative()
            .h(px(TRACK))
            .w_full()
            .rounded_full()
            .bg(tokens.line_strong)
            .child(
                div()
                    .h_full()
                    .w(relative(fraction))
                    .rounded_full()
                    .bg(tokens.accent),
            )
            .child(focus::ring(thumb, focus, tokens, window, cx))
            .child(drag_tracker(drag, steps, change.clone()));
        div()
            .flex_1()
            .h_full()
            .flex()
            .items_center()
            .px(px(THUMB / 2.0))
            .child(track)
            // A press and release within one frame ends before the
            // window-level release listener exists.
            .on_mouse_up(MouseButton::Left, {
                let drag = drag.clone();
                move |_: &MouseUpEvent, _, cx| release(&drag, cx)
            })
            .when_some(change, |area, change| {
                let drag = drag.clone();
                area.on_mouse_down(
                    MouseButton::Left,
                    move |event: &MouseDownEvent, window, cx| {
                        let Some(track) = drag.read(cx).track else {
                            return;
                        };
                        drag.update(cx, |drag, cx| {
                            drag.dragging = true;
                            drag.reported = Some(value);
                            cx.notify();
                        });
                        let picked = value_at(steps, track, event.position.x);
                        report(&drag, picked, &change, window, cx);
                    },
                )
            })
    }
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let enabled = self.disabled.is_none();
        let (steps, value) = (self.steps, self.value);
        let focus = self.focus.clone().tab_stop(enabled);
        let reason = self.disabled.clone().filter(|reason| !reason.is_empty());
        let drag = window.use_keyed_state(
            ElementId::NamedChild(
                std::sync::Arc::new(self.id.clone()),
                "drag".into(),
            ),
            cx,
            |_, _| Drag::default(),
        );
        // Disabling ends a drag, so re-enabling does not resume it.
        if !enabled {
            drag.update(cx, |drag, _| {
                drag.dragging = false;
                drag.reported = None;
            });
        }
        let change = self.on_change.clone().filter(|_| enabled);
        let area = self.area(&drag, change.clone(), &focus, window, cx);
        let slider =
            div()
                .id(self.id)
                .role(gpui::Role::Slider)
                .aria_label(self.label)
                .aria_orientation(gpui::accesskit::Orientation::Horizontal)
                .aria_numeric_value(value)
                .aria_min_numeric_value(steps.min())
                .aria_max_numeric_value(steps.max())
                .aria_numeric_value_step(steps.step())
                .when_some(self.readout.clone(), |slider, readout| {
                    slider.aria_value(readout)
                })
                .when_some(reason, |slider, reason| {
                    slider.aria_description(reason.clone()).tooltip(
                        move |_, cx| Tooltip::view(reason.clone(), tokens, cx),
                    )
                })
                .track_focus(&focus)
                .when_some(self.key_context, |slider, context| {
                    slider.key_context(context.as_ref())
                })
                .flex()
                .items_center()
                .gap(px(10.0))
                .w_full()
                .h(size::CONTROL_HEIGHT)
                .child(area)
                .when_some(self.readout, |slider, readout| {
                    slider.child(
                        div()
                            .flex_none()
                            .text_size(size::TEXT_CONTROL)
                            .text_color(tokens.muted)
                            .child(readout),
                    )
                })
                .when(!enabled, |slider| {
                    slider.opacity(0.5).on_mouse_down(
                        MouseButton::Left,
                        |_, window, _| {
                            window.prevent_default();
                        },
                    )
                })
                .when_some(change, |slider, change| {
                    let (increase, decrease) = (change.clone(), change);
                    slider
                        .on_a11y_action(
                            AccessibleAction::Increment,
                            move |_, window, cx| {
                                let next = steps.increment(value);
                                if moved(value, next) {
                                    increase(next, window, cx);
                                }
                            },
                        )
                        .on_a11y_action(
                            AccessibleAction::Decrement,
                            move |_, window, cx| {
                                let next = steps.decrement(value);
                                if moved(value, next) {
                                    decrease(next, window, cx);
                                }
                            },
                        )
                });
        States {
            disabled: !enabled,
            ..States::default()
        }
        .apply(slider)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests;
