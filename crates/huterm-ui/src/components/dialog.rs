//! Dialog: a modal surface over the owner's content, such as About,
//! Licenses, or a rename prompt. An
//! [`AlertDialog`](super::alert_dialog::AlertDialog) is the same shell for
//! a question that interrupts, such as a delete confirmation.
//!
//! Anatomy: a scrim filling the area the owner mounts it in, dimming it and
//! occluding hover, presses, and wheel events beneath, and rounded with
//! [`Dialog::scrim_corners`] when that area is, with a raised panel
//! centered on it: a title, an optional description, a body of the owner's
//! children (such as an input group and an inline error), and a footer of
//! [`Button`]s, right-aligned in the order given. The owner mounts the
//! dialog as the last child of the area it covers, in normal flow rather
//! than GPUI's `deferred` layer, while its [`Modal`] is open; the panel is
//! 420 points wide unless the owner restyles it through `Styled`. The panel
//! is never taller than that area: a tall body shrinks and clips, so the
//! footer stays inside the panel, and a body that can outgrow it puts its
//! long content in a [`ScrollArea`](super::scroll_area::ScrollArea) made
//! with [`fill`](super::scroll_area::ScrollArea::fill), or another
//! `flex_1().min_h_0()` child that scrolls. An owner whose content has its
//! own header and footer, such as an About panel's hero or a footer with a
//! button at its leading edge, makes the dialog [`Dialog::bare`]: the
//! panel then draws neither the title and description nor body padding,
//! and the owner's children fill the body, before the footer of any
//! buttons added with [`Dialog::button`].
//!
//! Focus: the dialog's controls are the body controls the owner declares
//! with [`Dialog::focusable`], then its enabled footer buttons. On its
//! first frame after [`Modal::open`], focus moves to the owner's
//! [`Dialog::initial_focus`] unless that is a disabled button, or else the
//! first control. Tab and Shift-Tab cycle through the controls, wrapping at
//! either end, so focus never reaches what the dialog covers; a press on
//! the panel's background moves no focus. Each frame rereads which buttons
//! are disabled, so a button leaves the cycle while disabled and rejoins it
//! once enabled. The modal remembers the control that last held focus,
//! and [`Modal::last_focused`] gives it to an owner returning focus to the
//! dialog from elsewhere in the window, such as a toast, which falls back
//! to its initial control when that one has left the dialog.
//! [`Modal::close`] returns focus to the target given to [`Modal::open`];
//! an owner that wants focus elsewhere, such as after a delete removed the
//! target, focuses it afterwards.
//!
//! Keyboard: the panel carries [`KEY_CONTEXT`] and nothing inside it adds a
//! context, so a control keeps only the context its owner gives it. The
//! owner binds its commands under that context and routes them: its focus
//! commands (Tab and Shift-Tab by default) to [`Modal::focus_next`] and
//! [`Modal::focus_previous`], its cancel command (Escape) to its own
//! dismissal with [`Dismissal::Escape`], and its confirm command to the
//! focused button's action. Like every [`Button`], the footer buttons
//! ignore GPUI's implicit keyboard click, so only routed commands act on
//! keys. A text field in the body needs its owner's text-editing context,
//! which is deeper than [`KEY_CONTEXT`] and so wins for the arrows and the
//! other editing keys. The owner then routes that context's own Tab,
//! Shift-Tab, confirm, and cancel commands as it routes the dialog's: to
//! [`Modal::focus_next`] and [`Modal::focus_previous`], the dialog's
//! confirm action, and [`Dismissal::Escape`], so Tab never leaves the
//! dialog.
//!
//! Dismissal: every close has a [`Dismissal`] reason, and the owner decides
//! whether to close and where focus goes. The owner mounts
//! [`Modal::guard`] in its root view on every frame, open or not, as it
//! mounts a popover's guard. While the dialog is open, its capture-phase
//! listeners stop every press and wheel event on the scrim outside the
//! panel, by their last painted bounds, so nothing beneath acts or takes
//! focus, and report such a press as [`Dismissal::OutsidePress`]: the
//! owner may close or keep the dialog. A press or wheel event outside the
//! scrim, such as on a window frame around the area the dialog covers, is
//! not the dialog's: it passes through, and nothing is reported. Before
//! the dialog's first frame, the guard stops a press anywhere in the
//! window but reports none, since nothing was shown to dismiss. Releases
//! are never stopped, so a gesture that began before the dialog opened
//! still ends.
//! The owner routes window deactivation and its own state changes, as for
//! a popover; a dialog normally stays open while its window is inactive,
//! as `AppKit`'s sheets and libadwaita's dialogs do, and keeps its focus
//! for the window's return.
//!
//! Limitation: the guard stops every press on the scrim outside the
//! panel's bounds, so a popover opened from the body, such as a select's
//! menu, takes no presses where it extends past the panel. No dialog holds
//! one yet.
//!
//! Accessibility: a modal `Dialog` named by its title and described by its
//! description; its controls report themselves.
//!
//! Reference: Radix's Dialog and WAI-ARIA's modal dialog pattern.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Corners, DispatchPhase, Div, ElementId, Empty,
    FocusHandle, IntoElement, MouseDownEvent, Pixels, Point, Refineable,
    RenderOnce, ScrollWheelEvent, SharedString, StyleRefinement, Window,
    canvas, div, prelude::*, px,
};

use crate::a11y::States;
use crate::components::button::Button;
use crate::components::surface::surface;
use crate::focus;
use crate::tokens::{Tokens, size};

/// The key context the dialog's panel carries.
pub const KEY_CONTEXT: &str = "confirming";

/// The panel's width unless the owner restyles it.
const WIDTH: Pixels = px(420.0);

/// Why a dialog is asked to close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dismissal {
    /// The owner's cancel command, Escape by default.
    Escape,
    /// A press on the scrim outside the panel. An alert dialog never
    /// reports one.
    OutsidePress,
    /// The window deactivated.
    Deactivated,
    /// The owner's state changed, such as a reload or a removed row.
    OwnerChanged,
}

#[derive(Debug, Default)]
struct State {
    open: bool,
    /// Where focus goes on close.
    return_to: Option<FocusHandle>,
    /// Focus moves into the dialog on its next frame.
    focus_pending: bool,
    /// The controls Tab cycles through, from the last frame.
    controls: Vec<FocusHandle>,
    /// The control that last held focus since opening.
    last_focused: Option<FocusHandle>,
    /// Whether the guard reports outside presses: a dialog's are, an alert
    /// dialog's never, and none before the first frame.
    reports_outside: bool,
    /// The panel's bounds from its last paint.
    panel: Option<Bounds<Pixels>>,
    /// The scrim's bounds from its last paint.
    scrim: Option<Bounds<Pixels>>,
}

impl State {
    /// Whether the guard stops a press or wheel event at `position`: one
    /// on an open dialog's scrim outside its panel, or anywhere before the
    /// dialog first paints.
    fn guards(&self, position: Point<Pixels>) -> bool {
        self.open
            && self.scrim.is_none_or(|scrim| scrim.contains(&position))
            && !self.panel.is_some_and(|panel| panel.contains(&position))
    }
}

/// One owner's dialog: its open state, the focus it holds and returns, and
/// the bounds its guard tests. Clones share the state.
#[derive(Clone, Debug, Default)]
pub struct Modal {
    state: Rc<RefCell<State>>,
}

impl Modal {
    /// A closed dialog.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the dialog, which takes focus on its first frame and returns
    /// it to `return_to` on close.
    pub fn open(&self, return_to: &FocusHandle) {
        let mut state = self.state.borrow_mut();
        state.open = true;
        state.return_to = Some(return_to.clone());
        state.focus_pending = true;
        state.last_focused = None;
        state.reports_outside = false;
        state.panel = None;
        state.scrim = None;
    }

    /// Closes the dialog and focuses the return target; `false` when it was
    /// not open.
    pub fn close(&self, window: &mut Window, cx: &mut App) -> bool {
        let return_to = {
            let mut state = self.state.borrow_mut();
            if !state.open {
                return false;
            }
            state.open = false;
            state.focus_pending = false;
            state.controls.clear();
            state.last_focused = None;
            state.panel = None;
            state.scrim = None;
            state.return_to.take()
        };
        if let Some(target) = return_to {
            target.focus(window, cx);
        }
        true
    }

    /// Whether it is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.state.borrow().open
    }

    /// The panel's bounds from its last paint; `None` while closed or
    /// before the panel first paints.
    #[must_use]
    pub fn panel_bounds(&self) -> Option<Bounds<Pixels>> {
        self.state.borrow().panel
    }

    /// The control that last held focus while the dialog was open, if it
    /// is still one of the dialog's controls; `None` while closed or
    /// before any control has held focus.
    #[must_use]
    pub fn last_focused(&self) -> Option<FocusHandle> {
        let state = self.state.borrow();
        state
            .last_focused
            .clone()
            .filter(|focus| state.controls.contains(focus))
    }

    /// Moves focus to the next control, wrapping to the first; from outside
    /// the controls, to the first.
    pub fn focus_next(&self, window: &mut Window, cx: &mut App) {
        self.cycle(true, window, cx);
    }

    /// Moves focus to the previous control, wrapping to the last; from
    /// outside the controls, to the last.
    pub fn focus_previous(&self, window: &mut Window, cx: &mut App) {
        self.cycle(false, window, cx);
    }

    fn cycle(&self, forward: bool, window: &mut Window, cx: &mut App) {
        let target = {
            let state = self.state.borrow();
            if !state.open {
                return;
            }
            let controls = &state.controls;
            let count = controls.len();
            let next = match controls
                .iter()
                .position(|control| control.is_focused(window))
            {
                Some(current) if forward => (current + 1) % count,
                Some(current) => (current + count - 1) % count,
                None if forward => 0,
                None => count.saturating_sub(1),
            };
            controls.get(next).cloned()
        };
        if let Some(target) = target {
            target.focus(window, cx);
            self.state.borrow_mut().last_focused = Some(target);
        }
    }

    /// The capture-phase press and wheel guard, which stops presses and
    /// wheel events on the scrim outside the panel, and anywhere before the
    /// dialog's first frame, and lets those outside the scrim through.
    /// Mount it in the owner's root view on every frame, open or not, after
    /// the window-level listeners every press must reach; `on_dismiss`
    /// receives [`Dismissal::OutsidePress`] for a press on the scrim, and
    /// the owner closes the dialog or keeps it.
    #[must_use]
    pub fn guard<F>(&self, on_dismiss: F) -> impl IntoElement + use<F>
    where
        F: Fn(Dismissal, &mut Window, &mut App) + 'static,
    {
        let state = self.state.clone();
        let on_dismiss = Rc::new(on_dismiss);
        canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                let presses = state.clone();
                let on_dismiss = on_dismiss.clone();
                window.on_mouse_event(
                    move |event: &MouseDownEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        let reports = {
                            let state = presses.borrow();
                            if !state.guards(event.position) {
                                return;
                            }
                            state.reports_outside
                        };
                        // The focus tracker would miss a stopped press.
                        focus::note_pointer_press(window, cx);
                        cx.stop_propagation();
                        window.prevent_default();
                        if reports {
                            on_dismiss(Dismissal::OutsidePress, window, cx);
                        }
                    },
                );
                let wheels = state.clone();
                window.on_mouse_event(
                    move |event: &ScrollWheelEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture
                            && wheels.borrow().guards(event.position)
                        {
                            cx.stop_propagation();
                        }
                    },
                );
            },
        )
        .absolute()
        .size(px(0.0))
    }

    /// Records a frame's controls, the one of them with focus, and the
    /// outside-press policy, and whether this is the first frame since
    /// opening, which moves focus in.
    fn frame(
        &self,
        controls: Vec<FocusHandle>,
        reports_outside: bool,
        window: &Window,
    ) -> bool {
        let mut state = self.state.borrow_mut();
        if let Some(focused) =
            controls.iter().find(|control| control.is_focused(window))
        {
            state.last_focused = Some(focused.clone());
        }
        state.controls = controls;
        state.reports_outside = reports_outside;
        std::mem::take(&mut state.focus_pending)
    }
}

/// A dialog's panel over its scrim, drawn while its [`Modal`] is open.
#[derive(IntoElement)]
pub struct Dialog {
    id: ElementId,
    title: SharedString,
    description: Option<SharedString>,
    modal: Modal,
    tokens: Tokens,
    alert: bool,
    focusables: Vec<FocusHandle>,
    buttons: Vec<(Button, FocusHandle)>,
    initial_focus: Option<FocusHandle>,
    bare: bool,
    scrim_corners: Corners<Pixels>,
    style: StyleRefinement,
    children: Vec<AnyElement>,
}

impl std::fmt::Debug for Dialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dialog")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("alert", &self.alert)
            .finish_non_exhaustive()
    }
}

impl Dialog {
    /// A dialog titled `title`, open while `modal` is.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        modal: &Modal,
        tokens: Tokens,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: None,
            modal: modal.clone(),
            tokens,
            alert: false,
            focusables: Vec::new(),
            buttons: Vec::new(),
            initial_focus: None,
            bare: false,
            scrim_corners: Corners::default(),
            style: StyleRefinement::default(),
            children: Vec::new(),
        }
    }

    /// The shell an alert dialog draws: an `AlertDialog` node whose outside
    /// presses are stopped but never reported.
    pub(crate) fn alert(mut self) -> Self {
        self.alert = true;
        self
    }

    /// A sentence under the title, which also describes the dialog to
    /// assistive technology.
    #[must_use]
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }

    /// Declares a focusable control in the body, such as a field, in Tab
    /// order: body controls come before the footer buttons. Leave out a
    /// control while it cannot take focus.
    #[must_use]
    pub fn focusable(mut self, focus: &FocusHandle) -> Self {
        self.focusables.push(focus.clone());
        self
    }

    /// A footer button, focused through `focus`, after the buttons already
    /// added.
    #[must_use]
    pub fn button(mut self, button: Button, focus: &FocusHandle) -> Self {
        self.buttons.push((button, focus.clone()));
        self
    }

    /// The control that takes focus on opening, instead of the first. It
    /// must be a declared control: a body control or a footer button.
    #[must_use]
    pub fn initial_focus(mut self, focus: &FocusHandle) -> Self {
        self.initial_focus = Some(focus.clone());
        self
    }

    /// The owner draws the panel's content: the dialog draws no title,
    /// description, or body padding, and the children fill the body
    /// edge to edge. The title and description still name and describe
    /// the dialog to assistive technology, so the owner shows them in its
    /// own header. Buttons the owner draws in its content are declared
    /// with [`Dialog::focusable`].
    #[must_use]
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    /// Rounds the scrim's corners to match an area with rounded corners,
    /// so the scrim tints nothing outside it.
    #[must_use]
    pub fn scrim_corners(mut self, corners: Corners<Pixels>) -> Self {
        self.scrim_corners = corners;
        self
    }
}

impl Styled for Dialog {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Dialog {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Dialog {
    /// Records this frame's controls with the modal and, on the first
    /// frame since opening, moves focus in.
    fn hold_focus(&self, window: &mut Window, cx: &mut App) {
        debug_assert!(
            !self.focusables.is_empty() || !self.buttons.is_empty(),
            "a dialog needs a control to hold focus"
        );
        // A disabled button takes no focus, so it leaves the cycle until it
        // is enabled again.
        let controls: Vec<FocusHandle> = self
            .focusables
            .iter()
            .chain(
                self.buttons
                    .iter()
                    .filter(|(button, _)| !button.is_disabled())
                    .map(|(_, focus)| focus),
            )
            .cloned()
            .collect();
        // With every control disabled, focus still enters the dialog, on its
        // first button, which tracks focus while disabled, so keys never
        // reach what the scrim covers.
        let initial = self
            .initial_focus
            .clone()
            .filter(|chosen| controls.contains(chosen))
            .or_else(|| controls.first().cloned())
            .or_else(|| self.buttons.first().map(|(_, focus)| focus.clone()));
        if self.modal.frame(controls, !self.alert, window)
            && let Some(target) = initial
        {
            // Focus moves after the frame, as the guide requires for focus
            // changes made while drawing.
            let modal = self.modal.clone();
            window.defer(cx, move |window, cx| {
                if modal.is_open() {
                    target.focus(window, cx);
                }
            });
        }
    }
}

/// The title and, under it, the description.
fn header(
    title: SharedString,
    description: Option<SharedString>,
    tokens: Tokens,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .px(px(16.0))
        .pt(px(14.0))
        .pb(px(12.0))
        .child(
            div()
                .text_size(size::TEXT_TITLE)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title),
        )
        .when_some(description, |header, text| {
            header.child(
                div()
                    .text_size(size::TEXT_CONTROL)
                    .text_color(tokens.muted)
                    .child(text),
            )
        })
}

/// The buttons, right-aligned on the footer fill, each focused through its
/// handle.
fn footer(buttons: Vec<(Button, FocusHandle)>, tokens: Tokens) -> Option<Div> {
    (!buttons.is_empty()).then(|| {
        div()
            .flex()
            .justify_end()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(10.0))
            .border_t_1()
            .border_color(tokens.line)
            .bg(tokens.footer())
            .children(
                buttons
                    .into_iter()
                    .map(|(button, focus)| button.focus(&focus)),
            )
    })
}

impl RenderOnce for Dialog {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !self.modal.is_open() {
            return Empty.into_any_element();
        }
        self.hold_focus(window, cx);
        let tokens = self.tokens;
        let modal = self.modal.clone();
        let probe = canvas(
            |_, _, _| {},
            move |bounds, (), _, _| {
                modal.state.borrow_mut().panel = Some(bounds);
            },
        )
        .absolute()
        .inset_0();
        let modal = self.modal.clone();
        let scrim_probe = canvas(
            |_, _, _| {},
            move |bounds, (), _, _| {
                modal.state.borrow_mut().scrim = Some(bounds);
            },
        )
        .absolute()
        .inset_0();
        let corners = self.scrim_corners;
        // The body shrinks rather than pushing the footer past the panel's
        // bottom, where the guard would stop presses on its buttons.
        let bare = self.bare;
        let body = (!self.children.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .when(!bare, |body| body.gap(px(8.0)).px(px(16.0)).pb(px(14.0)))
                .children(self.children)
        });
        let mut panel = surface(tokens, size::PANEL_RADIUS)
            .id(self.id)
            .role(if self.alert {
                gpui::Role::AlertDialog
            } else {
                gpui::Role::Dialog
            })
            .aria_label(self.title.clone())
            .when_some(self.description.clone(), |panel, text| {
                panel.aria_description(text)
            })
            .key_context(KEY_CONTEXT)
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .w(WIDTH)
            .max_w_full()
            .max_h_full()
            .text_size(size::TEXT_BODY)
            .text_color(tokens.fg)
            .when(!bare, |panel| {
                panel.child(header(self.title, self.description, tokens))
            })
            .children(body)
            .children(footer(self.buttons, tokens))
            .child(probe);
        // The owner's style applies over the dialog's.
        panel.style().refine(&self.style);
        let panel = States {
            modal: true,
            ..States::default()
        }
        .apply(panel);
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(tokens.scrim())
            .rounded_tl(corners.top_left)
            .rounded_tr(corners.top_right)
            .rounded_br(corners.bottom_right)
            .rounded_bl(corners.bottom_left)
            .flex()
            .items_center()
            .justify_center()
            .p(px(16.0))
            .child(scrim_probe)
            .child(panel)
            .into_any_element()
    }
}

#[cfg(all(test, feature = "test-support"))]
pub(crate) mod tests;
