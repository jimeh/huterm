//! Field: one setting's row, after shadcn/ui's Field: its label, any
//! descriptions, its control, and an error.
//!
//! Anatomy: a label column (the label on a title line that may carry
//! elements before and after it, such as a changed marker or a tag; then
//! each description, muted; then the error, in the danger text color),
//! actions beside the column, such as Reset, and the control. Horizontal
//! fields put the control at the trailing edge; vertical ones put it under
//! the label, as a wide picker needs. It pads itself as a row of a
//! [`crate::components::card::Card`]; an owner restyles it through
//! `Styled`.
//!
//! Behavior: none of its own. The control keeps its own focus, keys, and
//! states; an owner whose control is invalid marks it so as well, and
//! disables it while the row is read-only.
//!
//! Accessibility: a `Group` named by the label, whose description is the
//! descriptions and the error, so assistive technology reads them from
//! the row. The control still carries its own name, as every component
//! requires, since GPUI cannot relate it to the label.

use gpui::{
    AnyElement, App, ElementId, IntoElement, RenderOnce, SharedString,
    StyleRefinement, Window, div, prelude::*, px,
};

use crate::tokens::{Tokens, size};

/// A field.
#[derive(IntoElement)]
pub struct Field {
    id: ElementId,
    label: SharedString,
    tokens: Tokens,
    vertical: bool,
    before: Vec<AnyElement>,
    after: Vec<AnyElement>,
    descriptions: Vec<SharedString>,
    error: Option<SharedString>,
    actions: Vec<AnyElement>,
    control: Option<AnyElement>,
    label_selector: Option<String>,
    style: StyleRefinement,
}

impl std::fmt::Debug for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Field")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("descriptions", &self.descriptions)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl Field {
    /// A horizontal field labelled `label`.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        tokens: Tokens,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            tokens,
            vertical: false,
            before: Vec::new(),
            after: Vec::new(),
            descriptions: Vec::new(),
            error: None,
            actions: Vec::new(),
            control: None,
            label_selector: None,
            style: StyleRefinement::default(),
        }
    }

    /// Puts the control under the label, across the field's width.
    #[must_use]
    pub fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }

    /// An element before the label on its line, such as a changed marker.
    #[must_use]
    pub fn before_label(mut self, element: impl IntoElement) -> Self {
        self.before.push(element.into_any_element());
        self
    }

    /// An element after the label on its line, such as a tag.
    #[must_use]
    pub fn after_label(mut self, element: impl IntoElement) -> Self {
        self.after.push(element.into_any_element());
        self
    }

    /// A line under the label explaining the setting; each call adds one.
    #[must_use]
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.descriptions.push(text.into());
        self
    }

    /// The message of an invalid value, under the descriptions.
    #[must_use]
    pub fn error(mut self, message: Option<SharedString>) -> Self {
        self.error = message;
        self
    }

    /// An element beside the label column, such as a Reset button.
    #[must_use]
    pub fn action(mut self, element: impl IntoElement) -> Self {
        self.actions.push(element.into_any_element());
        self
    }

    /// The control the field labels.
    #[must_use]
    pub fn control(mut self, control: impl IntoElement) -> Self {
        self.control = Some(control.into_any_element());
        self
    }

    /// Names the label column for GPUI's `debug_bounds` in tests. Like
    /// GPUI's own `debug_selector`, it calls `selector` only where GPUI
    /// records selectors: in debug builds with `test-support`, which also
    /// enables GPUI's.
    #[must_use]
    pub fn label_debug_selector(
        mut self,
        selector: impl FnOnce() -> String,
    ) -> Self {
        if cfg!(any(test, all(debug_assertions, feature = "test-support"))) {
            self.label_selector = Some(selector());
        }
        self
    }
}

impl Styled for Field {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// A secondary line of `text` in `color`.
fn line(text: SharedString, color: gpui::Hsla) -> gpui::Div {
    div()
        .text_size(size::TEXT_SMALL)
        .text_color(color)
        .child(text)
}

impl RenderOnce for Field {
    fn render(mut self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let description: Vec<&str> = self
            .descriptions
            .iter()
            .chain(&self.error)
            .map(SharedString::as_ref)
            .collect();
        let description = description.join("\n");
        let title = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(size::TEXT_BODY)
            .text_color(tokens.fg)
            .children(self.before)
            .child(self.label.clone())
            .children(self.after);
        let column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.0))
            .child(title)
            .children(
                self.descriptions
                    .into_iter()
                    .map(|text| line(text, tokens.muted)),
            )
            .children(self.error.map(|text| line(text, tokens.danger_text())))
            .when_some(self.label_selector.take(), |column, selector| {
                column.debug_selector(|| selector)
            });
        let head = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .w_full()
            .min_w_0()
            .when(!self.vertical, Styled::flex_1)
            .child(column)
            .children(self.actions);
        let mut field = div()
            .id(self.id)
            .role(gpui::Role::Group)
            .aria_label(self.label)
            .when(!description.is_empty(), |field| {
                field.aria_description(description)
            })
            .flex()
            .gap(px(12.0))
            .px(px(14.0))
            .py(px(10.0))
            .when(self.vertical, |field| field.flex_col().items_start())
            .when(!self.vertical, Styled::items_center)
            .child(head)
            .children(self.control);
        field.style().refine(&std::mem::take(&mut self.style));
        field
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{TestAppContext, div, prelude::*, px};

    use super::Field;
    use crate::testing::{a11y_node, host, tokens};

    #[gpui::test]
    fn a_field_reports_its_label_descriptions_and_error(
        cx: &mut TestAppContext,
    ) {
        let (_, cx) = host(cx, |_, _| {
            Field::new("padding", "Padding", tokens())
                .description("Space around the terminal grid.")
                .description("Applies to new windows.")
                .error(Some("Must be zero or more.".into()))
                .control(div().id("control").size(px(40.0)))
                .into_any_element()
        });
        cx.update(|window, _| {
            let aria = &a11y_node(window, "padding")["aria"];
            assert_eq!(aria["role"], "Group");
            assert_eq!(aria["label"], "Padding");
            assert_eq!(
                aria["description"],
                "Space around the terminal grid.\nApplies to new windows.\n\
                 Must be zero or more."
            );
        });
    }

    #[gpui::test]
    fn a_horizontal_field_puts_its_control_at_the_trailing_edge(
        cx: &mut TestAppContext,
    ) {
        let (_, cx) = host(cx, |_, _| {
            div()
                .w(px(300.0))
                .child(Field::new("font", "Font size", tokens()).control(
                    div().size(px(40.0)).debug_selector(|| "horizontal".into()),
                ))
                .child(
                    Field::new("theme", "Theme", tokens()).vertical().control(
                        div()
                            .size(px(40.0))
                            .debug_selector(|| "vertical".into()),
                    ),
                )
                .into_any_element()
        });
        let horizontal = cx.debug_bounds("horizontal").expect("drawn");
        // Inside the row's 14-point padding.
        assert_eq!(horizontal.right(), px(286.0));
        let vertical = cx.debug_bounds("vertical").expect("drawn");
        assert_eq!(vertical.left(), px(14.0));
    }

    #[gpui::test]
    fn a_label_selector_names_the_label_column(cx: &mut TestAppContext) {
        let (_, cx) = host(cx, |_, _| {
            div()
                .w(px(300.0))
                .child(
                    Field::new("font", "Font size", tokens())
                        .label_debug_selector(|| "label".into())
                        .control(div().size(px(40.0))),
                )
                .into_any_element()
        });
        let label = cx.debug_bounds("label").expect("drawn");
        // From the 14-point padding to the 12-point gap before the control.
        assert_eq!(label.left(), px(14.0));
        assert_eq!(label.right(), px(234.0));
    }
}
