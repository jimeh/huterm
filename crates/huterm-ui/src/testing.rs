//! Helpers for the component contract tests: a host window, the
//! accessibility tree it last drew, and a test keymap that stands in for an
//! owner's commands.
//!
//! Test windows activate accessibility when they open (the vendored GPUI's
//! `test-window-accessibility` patch), so every drawn frame has a tree. A
//! missing tree fails the test rather than skipping its assertions.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Axis, Context, FocusHandle, IntoElement, KeyBinding,
    Render, TestAppContext, VisualTestContext, Window, div, prelude::*, px,
};
use serde_json::Value;

use crate::roving::{Arrow, Group, Move, move_focus};
use crate::scrollbar::ScrollbarColors;
use crate::tokens::{OverlayColors, Tokens};

gpui::actions!(
    component_test,
    [
        /// Up, as an owner's move command.
        Up,
        /// Down.
        Down,
        /// Left.
        Left,
        /// Right.
        Right,
        /// Home.
        Home,
        /// End.
        End,
        /// Space, as an owner's activation command.
        Activate,
    ]
);

/// The key context the test keymap binds under.
pub(crate) const KEYS: &str = "ComponentTest";

/// Binds the arrows, Home, End, and Space to the test actions under
/// [`KEYS`], as an owner's keymap binds its commands. Elements that carry
/// that context then receive them while they have focus.
pub(crate) fn bind_keys(cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        let context = Some(KEYS);
        cx.bind_keys([
            KeyBinding::new("up", Up, context),
            KeyBinding::new("down", Down, context),
            KeyBinding::new("left", Left, context),
            KeyBinding::new("right", Right, context),
            KeyBinding::new("home", Home, context),
            KeyBinding::new("end", End, context),
            KeyBinding::new("space", Activate, context),
        ]);
    });
}

/// `element` routing the test move keys as the owner of a roving group
/// does: the arrows along `axis` (every arrow with none), Home, and End
/// move focus among `handles` through [`move_focus`], with the selection
/// and enabled state `live` reports when the key arrives, and `select`
/// hears the index to select.
pub(crate) fn route_moves<E: InteractiveElement>(
    element: E,
    handles: &[FocusHandle],
    axis: Option<Axis>,
    live: impl Fn() -> (Option<usize>, bool) + 'static,
    select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> E {
    let handles: Rc<[FocusHandle]> = handles.into();
    let (live, select) = (Rc::new(live), Rc::new(select));
    let route = move |step: Option<Move>| {
        let (handles, live, select) =
            (handles.clone(), live.clone(), select.clone());
        move |window: &mut Window, cx: &mut App| {
            let (selected, enabled) = live();
            let group = Group {
                handles: &handles,
                selected,
                enabled,
            };
            if let Some(index) =
                step.and_then(|step| move_focus(group, step, window, cx))
            {
                select(index, window, cx);
            }
        }
    };
    let arrow = |arrow| route(Move::along(axis, arrow));
    let (up, down, left, right) = (
        arrow(Arrow::Up),
        arrow(Arrow::Down),
        arrow(Arrow::Left),
        arrow(Arrow::Right),
    );
    let (home, end) = (route(Some(Move::First)), route(Some(Move::Last)));
    element
        .on_action(move |_: &Up, window, cx| up(window, cx))
        .on_action(move |_: &Down, window, cx| down(window, cx))
        .on_action(move |_: &Left, window, cx| left(window, cx))
        .on_action(move |_: &Right, window, cx| right(window, cx))
        .on_action(move |_: &Home, window, cx| home(window, cx))
        .on_action(move |_: &End, window, cx| end(window, cx))
}

/// Dark tokens for tests.
pub(crate) fn tokens() -> Tokens {
    Tokens::new(OverlayColors {
        foreground: gpui::hsla(0.0, 0.0, 0.9, 1.0),
        background: gpui::hsla(0.0, 0.0, 0.1, 1.0),
        selection: gpui::hsla(0.6, 0.5, 0.4, 1.0),
        accent: gpui::hsla(0.6, 0.8, 0.6, 1.0),
        danger: gpui::hsla(0.0, 0.8, 0.6, 1.0),
        warning: gpui::hsla(0.12, 0.8, 0.6, 1.0),
        scrollbar: ScrollbarColors {
            thumb: gpui::hsla(0.0, 0.0, 0.9, 0.5),
            track: gpui::hsla(0.0, 0.0, 0.9, 0.1),
        },
    })
}

/// What a [`Host`] renders.
type Content = Box<dyn Fn(&mut Window, &mut Context<'_, Host>) -> AnyElement>;

/// A root view that renders whatever its closure returns.
pub(crate) struct Host {
    render: Content,
}

impl Render for Host {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        div()
            .size(px(400.0))
            .child(crate::focus::tracker())
            .child((self.render)(window, cx))
    }
}

/// Opens a window whose root renders `render`, drawn once.
pub(crate) fn host(
    cx: &mut TestAppContext,
    render: impl Fn(&mut Window, &mut Context<'_, Host>) -> AnyElement + 'static,
) -> (gpui::Entity<Host>, &mut VisualTestContext) {
    let (view, cx) = cx.add_window_view(|_, _| Host {
        render: Box::new(render),
    });
    cx.run_until_parked();
    (view, cx)
}

/// The accessibility nodes the window's last frame reported.
pub(crate) fn a11y_nodes(window: &Window) -> Vec<Value> {
    let json = window
        .debug_a11y_tree_json()
        .expect("test windows report an accessibility tree for every frame");
    let tree: Value =
        serde_json::from_str(&json).expect("the tree dump is JSON");
    tree["nodes"]
        .as_object()
        .expect("the tree dump lists its nodes")
        .values()
        .cloned()
        .collect()
}

/// The node the element named `id` reported, panicking with the tree when
/// it reported none. The dump spells element ids in their `Debug` form.
pub(crate) fn a11y_node(window: &Window, id: &str) -> Value {
    let nodes = a11y_nodes(window);
    let spelled = format!("{:?}", gpui::ElementId::Name(id.to_owned().into()));
    nodes
        .iter()
        .find(|node| node["element_id"].as_str() == Some(spelled.as_str()))
        .cloned()
        .unwrap_or_else(|| {
            panic!("no accessibility node for `{id}` in {nodes:#?}")
        })
}

/// The node assistive technology follows inside the focused element, such
/// as a radio group's selected radio, if any.
pub(crate) fn a11y_active_descendant(window: &Window) -> Option<Value> {
    let json = window
        .debug_a11y_tree_json()
        .expect("test windows report an accessibility tree for every frame");
    let tree: Value =
        serde_json::from_str(&json).expect("the tree dump is JSON");
    let key = tree["active_descendant_focus"].as_str()?;
    Some(tree["nodes"][key].clone())
}

/// The node labelled `label`, for elements whose ids are child ids,
/// panicking with the tree when no node carries that label.
pub(crate) fn a11y_labelled(window: &Window, label: &str) -> Value {
    let nodes = a11y_nodes(window);
    nodes
        .iter()
        .find(|node| node["aria"]["label"].as_str() == Some(label))
        .cloned()
        .unwrap_or_else(|| {
            panic!("no accessibility node labelled `{label}` in {nodes:#?}")
        })
}

/// Sends `action` to the node the element named `id` reported, as an
/// assistive technology would, and lets the result settle.
pub(crate) fn a11y_action(
    cx: &mut VisualTestContext,
    id: &str,
    action: gpui::AccessibleAction,
) {
    a11y_send(cx, |window| a11y_node(window, id), action);
}

/// Sends `action` to the node `find` returns from the last frame. GPUI
/// performs a `Click` with no listener as a pointer press at the node's
/// center.
pub(crate) fn a11y_send(
    cx: &mut VisualTestContext,
    find: impl FnOnce(&Window) -> Value,
    action: gpui::AccessibleAction,
) {
    cx.update(|window, cx| {
        let node = find(window);
        let target = node["accesskit_id"]
            .as_str()
            .and_then(|id| id.parse().ok())
            .expect("every node reports its AccessKit id");
        window.simulate_a11y_action(
            gpui::accesskit::ActionRequest {
                action,
                target_tree: gpui::accesskit::TreeId::ROOT,
                target_node: gpui::accesskit::NodeId(target),
                data: None,
            },
            cx,
        );
    });
    cx.run_until_parked();
}
