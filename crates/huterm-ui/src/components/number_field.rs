//! Number field: a field for a number in a range, with stepper buttons,
//! after React Aria's and Base UI's number fields, GNOME's `AdwSpinRow`,
//! and an `NSTextField` with an `NSStepper`.
//!
//! Anatomy: an [`InputGroup`] holding the number as text, an optional unit
//! as a text addon, then decrement and increment buttons, as `AdwSpinRow`
//! places them. The value may be shown scaled, such as a fraction as a
//! percent, rounded to a number of decimals.
//!
//! Keyboard: the field binds no keys. The owner routes text editing to
//! [`NumberField::input`], and its commands to [`NumberField::increment`]
//! and [`NumberField::decrement`] (Up and Down), [`NumberField::page_up`]
//! and [`NumberField::page_down`] (Page Up and Page Down), and
//! [`NumberField::to_min`] and [`NumberField::to_max`] (Home and End).
//! Typed text commits on [`NumberField::commit`] (Enter) and when the field
//! loses focus, unless the owner commits on blur itself
//! ([`NumberField::with_commit_on_blur`]), as an owner that keeps a draft
//! across a press elsewhere does; [`NumberField::revert`] (Escape) puts
//! back the value.
//!
//! Values: every change the user makes is clamped to the range, and steps
//! move along its grid ([`Steps`]): a value off the grid steps to the next
//! grid value first. Committed text that does not parse as a number
//! restores the last valid value; a number out of range is clamped. Typed
//! numbers are rounded to the shown decimals, so the value is the number
//! the field shows, and are not snapped to the grid. A value the owner
//! sets is shown as is, even off the grid or out of range. An empty
//! field steps to the minimum going up and the maximum going down, as
//! React Aria's does, unless the owner says otherwise with
//! [`NumberField::with_empty_step`], as one whose range is open at an end
//! does.
//!
//! Pointer: the steppers step once per click, committing typed text
//! first, and keep focus in the field. A stepper at its bound is disabled,
//! judged by the typed number while it parses, so typing 50 into a field at
//! its maximum of 100 enables the increment stepper.
//!
//! States: disabled and read-only follow [`Input`]'s and refuse every
//! change, from the keyboard, the steppers, and assistive technology; both
//! disable the steppers. Invalid draws the group's border in the danger
//! color.
//!
//! Accessibility: a `SpinButton` named by the label, reporting the shown
//! number as its numeric value with the scaled minimum, maximum, and step,
//! and its text as its value; Increment and Decrement actions step it. The
//! field inside is a `TextInput` with the same name, and the steppers are
//! buttons named Decrease and Increase.
//!
//! Reference: React Aria's `useNumberField`, Base UI's Number Field, and
//! WAI-ARIA's spinbutton.

use gpui::{
    AccessibleAction, App, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, IntoElement, Render, SharedString, Subscription,
    Window, div, prelude::*,
};

use crate::a11y::States;
use crate::components::button::{Button, Variant};
use crate::components::input::{Chrome, Input};
use crate::components::input_group::{Addon, InputGroup};
use crate::steps::Steps;
use crate::tokens::Tokens;

/// Emitted with the new value whenever the user changes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Changed(pub f64);

/// How a value shows as text: multiplied by `scale` and rounded to at most
/// `decimals` places, without trailing zeros.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Format {
    /// The factor between the value and the number shown, such as 100 for
    /// a fraction shown as a percent.
    pub scale: f64,
    /// The most decimal places shown.
    pub decimals: u8,
}

impl Default for Format {
    fn default() -> Self {
        Self {
            scale: 1.0,
            decimals: 2,
        }
    }
}

impl Format {
    /// `value` as text.
    #[must_use]
    pub fn text(self, value: f64) -> String {
        let text =
            format!("{:.*}", usize::from(self.decimals), value * self.scale);
        let text = if text.contains('.') {
            text.trim_end_matches('0').trim_end_matches('.')
        } else {
            &text
        };
        // Rounding can leave a negative zero.
        if text == "-0" {
            "0".to_owned()
        } else {
            text.to_owned()
        }
    }

    /// The value `text` shows, if it is a finite number, rounded to the
    /// decimals shown so it shows as typed.
    #[must_use]
    pub fn parse(self, text: &str) -> Option<f64> {
        let number: f64 = text.trim().parse().ok()?;
        (number.is_finite() && self.scale != 0.0)
            .then(|| self.round(number) / self.scale)
    }

    /// `number`, in shown units, rounded to the decimals shown.
    fn round(self, number: f64) -> f64 {
        let factor = 10_f64.powi(i32::from(self.decimals));
        let rounded = (number * factor).round() / factor;
        if rounded.is_finite() { rounded } else { number }
    }
}

/// Where a step from an empty field goes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum EmptyStep {
    /// The minimum going up and the maximum going down, as React Aria's
    /// field does.
    #[default]
    Ends,
    /// One step from this value, as though the field held it, such as a
    /// setting's default.
    From(f64),
    /// This value, clamped, in either direction, such as the bound of a
    /// range open at its other end.
    To(f64),
}

/// A number field.
pub struct NumberField {
    id: ElementId,
    label: SharedString,
    input: Entity<Input>,
    tokens: Tokens,
    steps: Steps,
    format: Format,
    value: Option<f64>,
    unit: Option<SharedString>,
    empty_step: EmptyStep,
    commit_on_blur: bool,
    _blur: Subscription,
}

impl std::fmt::Debug for NumberField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NumberField")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("steps", &self.steps)
            .field("format", &self.format)
            .field("value", &self.value)
            .finish_non_exhaustive()
    }
}

impl NumberField {
    /// An empty field named `label` over `steps`, showing values with
    /// `format`.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        steps: Steps,
        format: Format,
        tokens: Tokens,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let id = id.into();
        let label = label.into();
        let input = cx.new(|cx| {
            Input::new(child_id(&id, "field"), label.clone(), tokens, cx)
                .with_chrome(Chrome::Grouped)
        });
        let focus = input.read(cx).focus_handle(cx);
        // Blur runs inside a draw, where a change would not show until the
        // next input; deferring lets the commit draw.
        let blur = cx.on_blur(&focus, window, |field, window, cx| {
            if field.commit_on_blur {
                cx.defer_in(window, |field, _, cx| field.commit(cx));
            }
        });
        Self {
            id,
            label,
            input,
            tokens,
            steps,
            format,
            value: None,
            unit: None,
            empty_step: EmptyStep::Ends,
            commit_on_blur: true,
            _blur: blur,
        }
    }

    /// The field with `unit`, such as `pt`, after the number.
    #[must_use]
    pub fn with_unit(mut self, unit: impl Into<SharedString>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// The field stepping from empty as `empty_step` says.
    #[must_use]
    pub fn with_empty_step(mut self, empty_step: EmptyStep) -> Self {
        self.empty_step = empty_step;
        self
    }

    /// The field committing typed text when it loses focus, as it does by
    /// default, or leaving that to the owner.
    #[must_use]
    pub fn with_commit_on_blur(mut self, commit: bool) -> Self {
        self.commit_on_blur = commit;
        self
    }

    /// The text field inside, for routing text editing to it and setting
    /// its placeholder.
    #[must_use]
    pub fn input(&self) -> &Entity<Input> {
        &self.input
    }

    /// The value, or `None` while the field has none.
    #[must_use]
    pub fn value(&self) -> Option<f64> {
        self.value
    }

    /// The range and grid the field steps along.
    #[must_use]
    pub fn steps(&self) -> Steps {
        self.steps
    }

    /// Shows `value` as the owner's: unclamped, emitting nothing. Typed
    /// text is kept while the value is unchanged.
    pub fn set_value(
        &mut self,
        value: Option<f64>,
        cx: &mut Context<'_, Self>,
    ) {
        if self.value == value {
            return;
        }
        self.value = value;
        self.show_value(cx);
    }

    /// Recolors the field, as after a theme change.
    pub fn set_tokens(&mut self, tokens: Tokens, cx: &mut Context<'_, Self>) {
        if self.tokens == tokens {
            return;
        }
        self.tokens = tokens;
        cx.notify();
        self.input
            .update(cx, |input, cx| input.set_tokens(tokens, cx));
    }

    /// Disables the field, with `reason` as its tooltip, or enables it.
    pub fn set_disabled(
        &mut self,
        disabled: bool,
        reason: Option<SharedString>,
        cx: &mut Context<'_, Self>,
    ) {
        self.input
            .update(cx, |input, cx| input.set_disabled(disabled, reason, cx));
        cx.notify();
    }

    /// Makes the field read-only, or editable again.
    pub fn set_read_only(
        &mut self,
        read_only: bool,
        cx: &mut Context<'_, Self>,
    ) {
        self.input
            .update(cx, |input, cx| input.set_read_only(read_only, cx));
        cx.notify();
    }

    /// Marks the value as failing validation, or not.
    pub fn set_invalid(&mut self, invalid: bool, cx: &mut Context<'_, Self>) {
        self.input
            .update(cx, |input, cx| input.set_invalid(invalid, cx));
        cx.notify();
    }

    /// One step up.
    pub fn increment(&mut self, cx: &mut Context<'_, Self>) {
        self.step(Steps::increment, Steps::min, cx);
    }

    /// One step down.
    pub fn decrement(&mut self, cx: &mut Context<'_, Self>) {
        self.step(Steps::decrement, Steps::max, cx);
    }

    /// One page up.
    pub fn page_up(&mut self, cx: &mut Context<'_, Self>) {
        self.step(Steps::page_up, Steps::min, cx);
    }

    /// One page down.
    pub fn page_down(&mut self, cx: &mut Context<'_, Self>) {
        self.step(Steps::page_down, Steps::max, cx);
    }

    /// The minimum.
    pub fn to_min(&mut self, cx: &mut Context<'_, Self>) {
        self.jump(Steps::min, cx);
    }

    /// The maximum.
    pub fn to_max(&mut self, cx: &mut Context<'_, Self>) {
        self.jump(Steps::max, cx);
    }

    /// Takes the typed number as the value, rounded to the shown decimals
    /// and clamped to the range, or puts back the last valid value when the
    /// text is not a number. Text that still shows the value changes
    /// nothing, so a value rounded for display keeps its precision.
    pub fn commit(&mut self, cx: &mut Context<'_, Self>) {
        if !self.editable(cx) {
            return;
        }
        let text = self.input.read(cx).text().trim().to_owned();
        if self.value.map(|value| self.format.text(value)).as_deref()
            == Some(text.as_str())
        {
            return;
        }
        match self.format.parse(&text) {
            Some(value) => self.change(self.steps.clamp(value), cx),
            None => self.revert(cx),
        }
    }

    /// Puts back the value's text, dropping what was typed.
    pub fn revert(&mut self, cx: &mut Context<'_, Self>) {
        let shown = self.shown();
        if self.input.read(cx).text() != shown {
            self.show_value(cx);
        }
    }

    /// The value a step starts from: the typed number, clamped, while it
    /// parses and differs from the value's text, or else the value.
    fn draft(&self, cx: &App) -> Option<f64> {
        let text = self.input.read(cx).text().trim();
        if self.value.map(|value| self.format.text(value)).as_deref()
            == Some(text)
        {
            return self.value;
        }
        self.format
            .parse(text)
            .map(|value| self.steps.clamp(value))
            .or(self.value)
    }

    /// Whether the user may change the value.
    fn editable(&self, cx: &App) -> bool {
        let input = self.input.read(cx);
        !input.is_disabled() && !input.is_read_only()
    }

    /// Commits typed text, then moves the value with `step`. While there
    /// is none, the field's [`EmptyStep`] decides, and its `Ends` go to
    /// `end` of the range.
    fn step(
        &mut self,
        step: impl FnOnce(Steps, f64) -> f64,
        end: impl FnOnce(Steps) -> f64,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.editable(cx) {
            return;
        }
        self.commit(cx);
        let next = match (self.value, self.empty_step) {
            (Some(value), _) | (None, EmptyStep::From(value)) => {
                step(self.steps, value)
            }
            (None, EmptyStep::To(value)) => self.steps.clamp(value),
            (None, EmptyStep::Ends) => end(self.steps),
        };
        self.change(next, cx);
    }

    /// Commits typed text, then moves the value to `end` of the range,
    /// empty or not.
    fn jump(
        &mut self,
        end: impl FnOnce(Steps) -> f64,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.editable(cx) {
            return;
        }
        self.commit(cx);
        self.change(end(self.steps), cx);
    }

    /// Takes `value` as the user's change, emitting [`Changed`] when it
    /// differs, and shows it.
    fn change(&mut self, value: f64, cx: &mut Context<'_, Self>) {
        if self.value != Some(value) {
            self.value = Some(value);
            cx.emit(Changed(value));
        }
        self.show_value(cx);
    }

    fn shown(&self) -> String {
        self.value
            .map(|value| self.format.text(value))
            .unwrap_or_default()
    }

    fn show_value(&mut self, cx: &mut Context<'_, Self>) {
        let text = self.shown();
        self.input.update(cx, |input, cx| input.set_text(text, cx));
        cx.notify();
    }
}

impl EventEmitter<Changed> for NumberField {}

impl Focusable for NumberField {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl Render for NumberField {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let tokens = self.tokens;
        let editable = self.editable(cx);
        let (disabled, read_only) = {
            let input = self.input.read(cx);
            (input.is_disabled(), input.is_read_only())
        };
        let scale = self.format.scale;
        let steps = self.steps;
        // The input is a view inside this one, so typing redraws the field
        // and the steppers follow the typed number.
        let draft = self.draft(cx);
        let at_min = draft.is_some_and(|value| value <= steps.min());
        let at_max = draft.is_some_and(|value| value >= steps.max());
        let stepper = |id: &'static str,
                       label: &'static str,
                       glyph: &'static str,
                       blocked: bool,
                       cx: &mut Context<'_, Self>| {
            let button = Button::new(child_id(&self.id, id), label, tokens)
                .variant(Variant::Ghost)
                .icon(div().child(glyph))
                .icon_only();
            if !editable || blocked {
                return button.disabled(None);
            }
            let field = cx.entity().downgrade();
            button.on_click(move |_, cx| {
                let _ = field.update(cx, |field, cx| {
                    if id == "increment" {
                        field.increment(cx);
                    } else {
                        field.decrement(cx);
                    }
                });
            })
        };
        let decrement = stepper("decrement", "Decrease", "−", at_min, cx);
        let increment = stepper("increment", "Increase", "+", at_max, cx);
        let group =
            InputGroup::new(child_id(&self.id, "group"), &self.input, tokens)
                .when_some(self.unit.clone(), |group, unit| {
                    group.trailing(Addon::Text(unit))
                })
                .trailing(Addon::Button(decrement))
                .trailing(Addon::Button(increment));
        let (to_increment, to_decrement) =
            (cx.entity().downgrade(), cx.entity().downgrade());
        let spin = div()
            .id(self.id.clone())
            .role(gpui::Role::SpinButton)
            .aria_label(self.label.clone())
            .aria_value(SharedString::from(self.shown()))
            .aria_min_numeric_value(steps.min() * scale)
            .aria_max_numeric_value(steps.max() * scale)
            .aria_numeric_value_step(steps.step() * scale)
            // The number shown, so assistive technology reads what the
            // field displays.
            .when_some(self.value, |spin, value| {
                let shown = self.format.text(value).parse();
                spin.aria_numeric_value(shown.unwrap_or(value * scale))
            })
            .w_full()
            .when(editable, |spin| {
                spin.on_a11y_action(
                    AccessibleAction::Increment,
                    move |_, _, cx| {
                        let _ = to_increment.update(cx, Self::increment);
                    },
                )
                .on_a11y_action(
                    AccessibleAction::Decrement,
                    move |_, _, cx| {
                        let _ = to_decrement.update(cx, Self::decrement);
                    },
                )
            })
            .child(group);
        States {
            disabled,
            read_only,
            ..States::default()
        }
        .apply(spin)
    }
}

/// The id of `parent`'s part named `name`.
fn child_id(parent: &ElementId, name: &'static str) -> ElementId {
    ElementId::NamedChild(std::sync::Arc::new(parent.clone()), name.into())
}

#[cfg(all(test, feature = "test-support"))]
mod tests;
