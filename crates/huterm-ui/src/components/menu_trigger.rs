//! Menu trigger: a quiet control that names the current choice and opens a
//! menu of the others, such as the title bar's session control.
//!
//! Anatomy: a leading icon, a label that truncates with an ellipsis at the
//! owner's maximum width, and a small chevron, in a control-height pill
//! with no fill until hovered, pressed, or open. A muted label marks an
//! automatic value beside custom ones. Collapsed, the trigger is a square
//! that shows its icon alone. While its menu is open it takes the owner's
//! open fill, or the hover fill without one, and the full-strength text
//! color.
//!
//! Keyboard: the trigger never takes focus; the owner opens its menu from a
//! command, and the menu takes focus. GPUI's implicit keyboard click is
//! ignored.
//!
//! Pointer: a click runs the owner's action; hover and pressed states show.
//! The owner may stop presses so a title row's move gesture never starts
//! on it.
//!
//! Accessibility: a `Button` named by the owner's accessible name, which may
//! say more than the label, such as "Session: huterm", and reporting whether
//! its menu is expanded. The tooltip shows the owner's text, such as the
//! full label when it is truncated or collapsed, and never while the menu
//! is open.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, Hsla, IntoElement, MouseButton, ParentElement,
    Pixels, Refineable, RenderOnce, SharedString, StyleRefinement, Styled,
    Window, div, prelude::*, px,
};

use crate::click::from_keyboard;
use crate::components::button::Colors;
use crate::components::tooltip::Tooltip;
use crate::tokens::{Tokens, size};
use crate::typography::ui;

/// The icon's size.
pub const ICON_SIZE: f32 = 14.0;
/// The chevron's size.
pub const CHEVRON_SIZE: f32 = 11.0;
/// Padding before the icon.
pub const PADDING_START: f32 = 8.0;
/// Padding after the chevron.
pub const PADDING_END: f32 = 7.0;
/// The space between the icon, the label, and the chevron.
pub const GAP: f32 = 6.0;
/// The label's text size.
pub const TEXT_SIZE: f32 = 12.0;

/// The trigger's width around a label `label` points wide: the padding,
/// icon, gaps, and chevron. Owners that place the trigger measure the label
/// and size it with this, so layout and hit testing agree.
#[must_use]
pub fn width_for_label(label: Pixels) -> Pixels {
    px(PADDING_START + ICON_SIZE + GAP)
        + label
        + px(GAP + CHEVRON_SIZE + PADDING_END)
}

type Action = Rc<dyn Fn(&mut Window, &mut App)>;

/// A menu trigger.
#[derive(IntoElement)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent builder option"
)]
pub struct MenuTrigger {
    id: ElementId,
    accessible_name: SharedString,
    label: SharedString,
    icon: SharedString,
    chevron: SharedString,
    tokens: Tokens,
    colors: Colors,
    /// The label's color when it is muted; `None` draws it in the text
    /// color.
    muted: Option<Hsla>,
    /// The icon and chevron color at rest.
    quiet: Option<Hsla>,
    open: bool,
    open_fill: Option<Hsla>,
    collapsed: bool,
    label_width: Option<Pixels>,
    tooltip: Option<SharedString>,
    occlude: bool,
    stop_presses: bool,
    debug_selector: Option<String>,
    on_click: Option<Action>,
    style: StyleRefinement,
    children: Vec<AnyElement>,
}

impl std::fmt::Debug for MenuTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuTrigger")
            .field("id", &self.id)
            .field("accessible_name", &self.accessible_name)
            .field("label", &self.label)
            .field("open", &self.open)
            .field("collapsed", &self.collapsed)
            .finish_non_exhaustive()
    }
}

impl MenuTrigger {
    /// A trigger showing `label` with the `icon` and `chevron` assets,
    /// named `accessible_name` for assistive technology, in `colors`.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        accessible_name: impl Into<SharedString>,
        label: impl Into<SharedString>,
        icon: impl Into<SharedString>,
        chevron: impl Into<SharedString>,
        tokens: Tokens,
        colors: Colors,
    ) -> Self {
        Self {
            id: id.into(),
            accessible_name: accessible_name.into(),
            label: label.into(),
            icon: icon.into(),
            chevron: chevron.into(),
            tokens,
            colors,
            muted: None,
            quiet: None,
            open: false,
            open_fill: None,
            collapsed: false,
            label_width: None,
            tooltip: None,
            occlude: false,
            stop_presses: false,
            debug_selector: None,
            on_click: None,
            style: StyleRefinement::default(),
            children: Vec::new(),
        }
    }

    /// Draws the label in `color`, such as the dim text of an automatic
    /// name.
    #[must_use]
    pub fn muted(mut self, color: Hsla) -> Self {
        self.muted = Some(color);
        self
    }

    /// Draws the icon and chevron in `color` until hovered or open.
    #[must_use]
    pub fn quiet(mut self, color: Hsla) -> Self {
        self.quiet = Some(color);
        self
    }

    /// Shows the open state while the trigger's menu is open.
    #[must_use]
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Fills the open trigger with `color`, such as the active tab's fill,
    /// instead of the hover fill.
    #[must_use]
    pub fn open_fill(mut self, color: Hsla) -> Self {
        self.open_fill = Some(color);
        self
    }

    /// Shows the icon alone in a square.
    #[must_use]
    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    /// Lays the label out `width` wide, truncating it with an ellipsis when
    /// it needs more.
    #[must_use]
    pub fn label_width(mut self, width: Pixels) -> Self {
        self.label_width = Some(width);
        self
    }

    /// Shows `text` as the tooltip, such as the full label when truncated.
    #[must_use]
    pub fn tooltip(mut self, text: Option<SharedString>) -> Self {
        self.tooltip = text;
        self
    }

    /// Blocks hover and presses to what lies beneath.
    #[must_use]
    pub fn occlude(mut self) -> Self {
        self.occlude = true;
        self
    }

    /// Stops a left press from reaching ancestors, such as a title row's
    /// move gesture.
    #[must_use]
    pub fn stop_presses(mut self) -> Self {
        self.stop_presses = true;
        self
    }

    /// Names the trigger for test and smoke bounds lookups.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl FnOnce() -> String) -> Self {
        self.debug_selector = Some(selector());
        self
    }

    /// Runs `action` on a pointer click.
    #[must_use]
    pub fn on_click(
        mut self,
        action: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(action));
        self
    }
}

impl Styled for MenuTrigger {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for MenuTrigger {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for MenuTrigger {
    fn render(mut self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let colors = self.colors;
        let group: SharedString = format!("menu-trigger-{:?}", self.id).into();
        let quiet = if self.open {
            colors.text
        } else {
            self.quiet.unwrap_or(colors.text)
        };
        let label_color = if self.open {
            colors.text
        } else {
            self.muted.unwrap_or(colors.text)
        };
        let tooltip = self.tooltip.clone().filter(|_| !self.open);
        let height = size::CONTROL_HEIGHT;
        let icon = |path: SharedString, points: f32, color: Hsla| {
            gpui::svg()
                .path(path)
                .flex_none()
                .size(px(points))
                .text_color(color)
                .group_hover(group.clone(), move |style| {
                    style.text_color(colors.text)
                })
        };
        let mut trigger = div()
            .id(self.id.clone())
            .group(group.clone())
            .role(gpui::Role::Button)
            .aria_label(self.accessible_name.clone())
            .aria_expanded(self.open)
            .when_some(tooltip.clone(), |trigger, text| {
                trigger.aria_description(text)
            })
            .when_some(self.debug_selector.take(), |trigger, selector| {
                trigger.debug_selector(|| selector)
            })
            .when(self.occlude, InteractiveElement::occlude)
            .when(self.stop_presses, |trigger| {
                trigger.on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
            })
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .h(height)
            .rounded(px(size::CONTROL_RADIUS))
            .text_size(ui(TEXT_SIZE))
            .whitespace_nowrap()
            .when_some(colors.fill, Styled::bg)
            .when(self.open, |trigger| {
                trigger.bg(self.open_fill.unwrap_or(colors.hover))
            })
            .hover(move |style| style.bg(colors.hover))
            .active(move |style| style.bg(colors.pressed));
        trigger = if self.collapsed {
            trigger.w(height).justify_center().child(icon(
                self.icon.clone(),
                ICON_SIZE,
                quiet,
            ))
        } else {
            trigger
                .gap(px(GAP))
                .pl(px(PADDING_START))
                .pr(px(PADDING_END))
                .child(icon(self.icon.clone(), ICON_SIZE, quiet))
                .child(
                    div()
                        .min_w_0()
                        .when_some(self.label_width, |label, width| {
                            label.w(width).flex_none()
                        })
                        .truncate()
                        .text_color(label_color)
                        .debug_selector(|| "menu-trigger-label".to_owned())
                        .child(self.label.clone()),
                )
                .child(icon(self.chevron.clone(), CHEVRON_SIZE, quiet))
        };
        trigger.style().refine(&std::mem::take(&mut self.style));
        trigger
            .when_some(tooltip, |trigger, text| {
                trigger.tooltip(move |_, cx| {
                    Tooltip::view(text.clone(), tokens, cx)
                })
            })
            .when_some(self.on_click, |trigger, action| {
                trigger.on_click(move |event, window, cx| {
                    if !from_keyboard(event) {
                        action(window, cx);
                    }
                })
            })
            .children(self.children)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        Modifiers, TestAppContext, VisualTestContext, point, prelude::*, px,
    };

    use super::{MenuTrigger, width_for_label};
    use crate::components::button::Colors;
    use crate::testing::{a11y_node, host, tokens};

    fn colors() -> Colors {
        let tokens = tokens();
        Colors {
            fill: None,
            text: tokens.fg,
            border: None,
            hover: tokens.hover,
            pressed: tokens.pressed(),
        }
    }

    fn trigger(
        open: bool,
        collapsed: bool,
        tooltip: Option<&str>,
    ) -> MenuTrigger {
        MenuTrigger::new(
            "session",
            "Session: huterm-release-notes",
            "huterm-release-notes",
            "icons/layers.svg",
            "icons/chevron-down.svg",
            tokens(),
            colors(),
        )
        .open(open)
        .collapsed(collapsed)
        .label_width(px(60.0))
        .tooltip(tooltip.map(Into::into))
    }

    fn hosted(
        cx: &mut TestAppContext,
        build: impl Fn() -> MenuTrigger + 'static,
    ) -> &mut VisualTestContext {
        host(cx, move |_, _| {
            gpui::div().flex().child(build()).into_any_element()
        })
        .1
    }

    #[gpui::test]
    fn a_trigger_reports_its_own_name_its_menu_state_and_its_tooltip(
        cx: &mut TestAppContext,
    ) {
        let cx =
            hosted(cx, || trigger(false, false, Some("huterm-release-notes")));
        cx.update(|window, _| {
            let aria = &a11y_node(window, "session")["aria"];
            assert_eq!(aria["role"], "Button");
            assert_eq!(aria["label"], "Session: huterm-release-notes");
            assert_eq!(aria["expanded"], false);
            assert_eq!(aria["description"], "huterm-release-notes");
        });
    }

    #[gpui::test]
    fn an_open_trigger_reports_its_menu_expanded_without_a_tooltip(
        cx: &mut TestAppContext,
    ) {
        let cx =
            hosted(cx, || trigger(true, false, Some("huterm-release-notes")));
        cx.update(|window, _| {
            let aria = &a11y_node(window, "session")["aria"];
            assert_eq!(aria["expanded"], true);
            // An open menu covers the tooltip, so it is not described.
            assert!(aria["description"].is_null());
        });
    }

    #[gpui::test]
    fn the_label_truncates_at_the_width_its_owner_measured(
        cx: &mut TestAppContext,
    ) {
        let cx = hosted(cx, || trigger(false, false, None));
        let label = cx
            .debug_bounds("menu-trigger-label")
            .expect("the label drew");
        assert_eq!(label.size.width, px(60.0));
        assert_eq!(
            width_for_label(px(60.0)),
            px(8.0 + 14.0 + 6.0 + 60.0 + 6.0 + 11.0 + 7.0)
        );
    }

    #[gpui::test]
    fn a_collapsed_trigger_is_a_square_icon(cx: &mut TestAppContext) {
        let cx = hosted(cx, || {
            trigger(false, true, None).debug_selector(|| "collapsed".to_owned())
        });
        assert!(cx.debug_bounds("menu-trigger-label").is_none());
        let square = cx.debug_bounds("collapsed").expect("the trigger drew");
        assert_eq!(square.size.width, square.size.height);
    }

    #[gpui::test]
    fn a_pointer_click_runs_the_action(cx: &mut TestAppContext) {
        let clicks = Rc::new(Cell::new(0));
        let counted = Rc::clone(&clicks);
        let (_, cx) = host(cx, move |_, _| {
            let counted = Rc::clone(&counted);
            gpui::div()
                .flex()
                .child(
                    trigger(false, false, None)
                        .on_click(move |_, _| counted.set(counted.get() + 1)),
                )
                .into_any_element()
        });
        cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(clicks.get(), 1);
    }
}
