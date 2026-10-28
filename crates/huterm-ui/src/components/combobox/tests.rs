use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    Action, App, Bounds, Context, Div, Entity, FocusHandle, Focusable as _,
    KeyBinding, Modifiers, MouseButton, Pixels, Point, Render, ScrollDelta,
    ScrollHandle, ScrollWheelEvent, TestAppContext, TouchPhase,
    VisualTestContext, Window, div, point, prelude::*, px, size,
};

use super::{Combobox, ComboboxEvent};
use crate::components::input::Input;
use crate::components::popover::MARGIN;
use crate::option_list::ListOption;
use crate::testing::{a11y_labelled, a11y_node, a11y_nodes, a11y_send, tokens};

gpui::actions!(
    combobox_test,
    [
        Next, Previous, PageDown, PageUp, First, Last, Confirm, Escape, Toggle
    ]
);

/// The key context the owner's bindings use.
const KEYS: &str = "ComboboxTest";

const FONTS: [&str; 8] = [
    "Menlo",
    "Monaco",
    "Mononoki",
    "Fira Code",
    "Fira Mono",
    "JetBrains Mono",
    "Berkeley Mono",
    "Iosevka",
];

type Log = Rc<RefCell<Vec<String>>>;

/// The suggestions in `fonts` that contain the field's text, ignoring
/// case, as a font family field filters the installed families. Each run
/// counts in `runs`.
fn containing(
    fonts: Vec<String>,
    runs: Rc<Cell<usize>>,
) -> impl Fn(&Input) -> Vec<ListOption> + 'static {
    move |input| {
        runs.set(runs.get() + 1);
        let text = input.text().to_lowercase();
        fonts
            .iter()
            .filter(|font| font.to_lowercase().contains(&text))
            .map(|font| ListOption::new(font.to_lowercase(), font.clone()))
            .collect()
    }
}

/// How a test's owner is set up.
#[derive(Clone, Copy, Default)]
struct Setup {
    auto_highlight: bool,
    /// Forty numbered fonts in place of [`FONTS`].
    many: bool,
    field: Field,
}

/// Where the field sits.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Field {
    /// At the window's top left, 160 points wide.
    #[default]
    TopLeft,
    /// At the window's bottom edge.
    AtBottom,
    /// Across the window's top.
    FullWidth,
}

/// A font family field at the window's top left, or its bottom edge, whose
/// owner routes the test keys to the combobox, applies a pick to the field,
/// and logs what reaches it: picks, its own Enter and Escape, the field's
/// blur, and presses on a control at the right that takes no focus, as a
/// disabled one does. Another control there takes focus, and the root
/// takes it from presses nothing else takes, as a window's root view does.
/// Below them, a scrolling area holds more than it shows.
struct Owner {
    input: Entity<Input>,
    combobox: Entity<Combobox>,
    root: FocusHandle,
    other: FocusHandle,
    log: Log,
    /// How many times the filter ran.
    filtered: Rc<Cell<usize>>,
    scroll: ScrollHandle,
    field: Field,
    /// How far right of the window's edge the field sits.
    shift: Pixels,
}

impl Owner {
    fn new(
        setup: Setup,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let input =
            cx.new(|cx| Input::new("font", "Font family", tokens(), cx));
        let fonts: Vec<String> = if setup.many {
            (0..40).map(|index| format!("Font {index:02}")).collect()
        } else {
            FONTS.iter().map(|font| (*font).to_owned()).collect()
        };
        let filtered = Rc::new(Cell::new(0));
        let runs = filtered.clone();
        let combobox = cx.new(|cx| {
            Combobox::new(
                "font-family",
                "Font family",
                &input,
                tokens(),
                containing(fonts, runs),
                window,
                cx,
            )
            .with_auto_highlight(setup.auto_highlight)
        });
        let log = Log::default();
        let picks = log.clone();
        cx.subscribe(&combobox, move |owner, _, event: &ComboboxEvent, cx| {
            let ComboboxEvent::Picked(id) = event;
            picks.borrow_mut().push(format!("picked:{id}"));
            let label = owner
                .combobox
                .read(cx)
                .options()
                .iter()
                .find(|option| &option.id == id)
                .map(|option| option.label.to_string());
            if let Some(label) = label {
                owner
                    .input
                    .update(cx, |input, cx| input.set_text(label, cx));
            }
        })
        .detach();
        let blurs = log.clone();
        let focus = input.read(cx).focus_handle(cx);
        cx.on_blur(&focus, window, move |_, _, _| {
            blurs.borrow_mut().push("blur".to_owned());
        })
        .detach();
        Self {
            input,
            combobox,
            root: cx.focus_handle(),
            other: cx.focus_handle(),
            log,
            filtered,
            scroll: ScrollHandle::new(),
            field: setup.field,
            shift: px(0.0),
        }
    }

    fn text(&self, cx: &App) -> String {
        self.input.read(cx).text().to_owned()
    }
}

impl Render for Owner {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let viewport = window.viewport_size();
        let top = if self.field == Field::AtBottom {
            viewport.height - px(30.0)
        } else {
            px(0.0)
        };
        let inert_log = self.log.clone();
        let width = if self.field == Field::FullWidth {
            viewport.width
        } else {
            px(160.0)
        };
        let keys = div()
            .key_context(KEYS)
            .absolute()
            .top(top)
            .left(self.shift)
            .w(width)
            .on_action(cx.listener(|owner, _: &Confirm, _, cx| {
                let picked = owner.combobox.update(cx, Combobox::confirm);
                if !picked {
                    let text = owner.text(cx);
                    owner.log.borrow_mut().push(format!("enter:{text}"));
                }
            }))
            .on_action(cx.listener(|owner, _: &Escape, _, cx| {
                let closed = owner.combobox.update(cx, Combobox::close);
                if !closed {
                    owner.log.borrow_mut().push("escape".to_owned());
                }
            }));
        let keys = route::<Next>(keys, Combobox::select_next, cx);
        let keys = route::<Previous>(keys, Combobox::select_previous, cx);
        let keys = route::<PageDown>(keys, Combobox::select_page_down, cx);
        let keys = route::<PageUp>(keys, Combobox::select_page_up, cx);
        let keys = route::<First>(keys, Combobox::select_first, cx);
        let keys = route::<Last>(keys, Combobox::select_last, cx);
        let keys = route::<Toggle>(keys, Combobox::toggle, cx);
        div()
            .size_full()
            .relative()
            .track_focus(&self.root)
            .child(crate::focus::tracker())
            .child(
                keys.child(
                    div()
                        .relative()
                        .child(self.input.clone())
                        .child(self.combobox.clone()),
                ),
            )
            .child(
                div()
                    .id("inert")
                    .absolute()
                    .top(px(200.0))
                    .left(px(300.0))
                    .size(px(40.0))
                    .on_mouse_down(MouseButton::Left, move |_, window, _| {
                        inert_log.borrow_mut().push("inert-down".to_owned());
                        window.prevent_default();
                    }),
            )
            .child(
                div()
                    .id("other")
                    .absolute()
                    .top(px(300.0))
                    .left(px(300.0))
                    .size(px(40.0))
                    .track_focus(&self.other),
            )
            .child(
                div()
                    .id("scroller")
                    .absolute()
                    .top(px(400.0))
                    .left(px(300.0))
                    .w(px(100.0))
                    .h(px(50.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().h(px(300.0))),
            )
    }
}

/// `element` running `act` on the combobox for the test action `A`.
fn route<A: Action>(
    element: Div,
    act: fn(&mut Combobox, &mut Context<'_, Combobox>) -> bool,
    cx: &mut Context<'_, Owner>,
) -> Div {
    element.on_action(cx.listener(move |owner, _: &A, _, cx| {
        owner.combobox.update(cx, |combobox, cx| {
            act(combobox, cx);
        });
    }))
}

/// Types `text` into the focused field and lets the result settle.
fn type_text(cx: &mut VisualTestContext, text: &str) {
    cx.simulate_input(text);
    cx.run_until_parked();
}

/// Presses `keys` and lets the result settle.
fn keys(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
    cx.run_until_parked();
}

struct Probe {
    owner: Entity<Owner>,
}

fn setup(
    cx: &mut TestAppContext,
    setup: Setup,
) -> (Probe, &mut VisualTestContext) {
    let (owner, cx) =
        cx.add_window_view(move |window, cx| Owner::new(setup, window, cx));
    cx.update(|_, cx| {
        let context = Some(KEYS);
        cx.bind_keys([
            KeyBinding::new("down", Next, context),
            KeyBinding::new("up", Previous, context),
            KeyBinding::new("pagedown", PageDown, context),
            KeyBinding::new("pageup", PageUp, context),
            KeyBinding::new("home", First, context),
            KeyBinding::new("end", Last, context),
            KeyBinding::new("enter", Confirm, context),
            KeyBinding::new("escape", Escape, context),
            KeyBinding::new("alt-down", Toggle, context),
        ]);
    });
    // Blur reaches focus listeners only in an active window.
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let probe = Probe { owner };
    probe.focus_field(cx);
    (probe, cx)
}

impl Probe {
    fn focus_field(&self, cx: &mut VisualTestContext) {
        let focus = cx.update(|_, cx| {
            self.owner.read(cx).input.read(cx).focus_handle(cx)
        });
        cx.update(|window, cx| focus.focus(window, cx));
        cx.run_until_parked();
    }

    fn combobox(&self, cx: &mut VisualTestContext) -> Entity<Combobox> {
        cx.update(|_, cx| self.owner.read(cx).combobox.clone())
    }

    fn input(&self, cx: &mut VisualTestContext) -> Entity<Input> {
        cx.update(|_, cx| self.owner.read(cx).input.clone())
    }

    fn open(&self, cx: &mut VisualTestContext) -> bool {
        let combobox = self.combobox(cx);
        cx.read(|cx| combobox.read(cx).is_open())
    }

    fn labels(&self, cx: &mut VisualTestContext) -> Vec<String> {
        let combobox = self.combobox(cx);
        cx.read(|cx| {
            combobox
                .read(cx)
                .options()
                .iter()
                .map(|option| option.label.to_string())
                .collect()
        })
    }

    fn highlighted(&self, cx: &mut VisualTestContext) -> Option<String> {
        let combobox = self.combobox(cx);
        cx.read(|cx| {
            combobox
                .read(cx)
                .highlighted()
                .map(|option| option.label.to_string())
        })
    }

    fn text(&self, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| self.owner.read(cx).text(cx))
    }

    fn field_focused(&self, cx: &mut VisualTestContext) -> bool {
        let input = self.input(cx);
        cx.update(|window, cx| {
            input.read(cx).focus_handle(cx).is_focused(window)
        })
    }

    fn taken(&self, cx: &mut VisualTestContext) -> Vec<String> {
        cx.update(|_, cx| {
            std::mem::take(&mut *self.owner.read(cx).log.borrow_mut())
        })
    }

    /// How many times the filter ran since the last call.
    fn filtered(&self, cx: &mut VisualTestContext) -> usize {
        cx.update(|_, cx| self.owner.read(cx).filtered.replace(0))
    }

    fn list_bounds(&self, cx: &mut VisualTestContext) -> Bounds<Pixels> {
        let combobox = self.combobox(cx);
        cx.read(|cx| combobox.read(cx).list_bounds())
            .expect("the list painted")
    }
}

/// Delivers the frames the window asks for, as a platform's frame source
/// would and GPUI's test mode does not, until it asks for none.
fn deliver_frames(cx: &mut VisualTestContext) {
    for _ in 0..10 {
        cx.run_until_parked();
        if cx.update(Window::simulate_next_frame) == 0 {
            return;
        }
    }
    panic!("the window kept asking for frames");
}

/// Turns the wheel `by` points over `position`.
fn wheel(cx: &mut VisualTestContext, position: Point<Pixels>, by: f32) {
    cx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.0), px(by))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();
}

/// Asserts that the list draws inside the window's margins.
fn assert_inside_margins(probe: &Probe, cx: &mut VisualTestContext) {
    let viewport = cx.update(|window, _| window.viewport_size());
    let list = probe.list_bounds(cx);
    assert!(
        list.left() >= MARGIN && list.right() <= viewport.width - MARGIN,
        "{list:?} in {viewport:?}"
    );
}

#[gpui::test]
fn typing_filters_the_list_while_focus_stays_in_the_field(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(cx, Setup::default());
    assert!(!probe.open(cx));
    type_text(cx, "mo");
    assert!(probe.open(cx), "typing opens the list");
    assert_eq!(
        probe.labels(cx),
        [
            "Monaco",
            "Mononoki",
            "Fira Mono",
            "JetBrains Mono",
            "Berkeley Mono"
        ]
    );
    assert_eq!(probe.highlighted(cx), None, "nothing is highlighted yet");
    assert!(probe.field_focused(cx));
    // Typing continues in the field while the list shows.
    type_text(cx, "no");
    assert_eq!(probe.text(cx), "mono");
    assert!(probe.open(cx));
    assert_eq!(
        probe.labels(cx),
        ["Mononoki", "Fira Mono", "JetBrains Mono", "Berkeley Mono"]
    );
    assert!(probe.field_focused(cx));
    // Text that matches nothing closes the list.
    type_text(cx, "zz");
    assert!(!probe.open(cx));
    assert_eq!(probe.text(cx), "monozz");
}

#[gpui::test]
fn moving_changes_the_highlight_and_never_the_text(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    type_text(cx, "mo");
    keys(cx, "down");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Monaco"));
    keys(cx, "down");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Mononoki"));
    keys(cx, "up up");
    assert_eq!(
        probe.highlighted(cx).as_deref(),
        Some("Berkeley Mono"),
        "Up wraps from the first suggestion"
    );
    keys(cx, "down");
    assert_eq!(
        probe.highlighted(cx).as_deref(),
        Some("Monaco"),
        "Down wraps from the last suggestion"
    );
    keys(cx, "end");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Berkeley Mono"));
    keys(cx, "home");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Monaco"));
    keys(cx, "pagedown");
    assert_eq!(
        probe.highlighted(cx).as_deref(),
        Some("Berkeley Mono"),
        "a page stops at the last suggestion"
    );
    keys(cx, "pageup");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Monaco"));
    assert_eq!(probe.text(cx), "mo");
    assert!(probe.field_focused(cx));
    assert_eq!(probe.taken(cx), Vec::<String>::new());
}

#[gpui::test]
fn down_and_up_open_a_closed_list_at_its_ends(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    type_text(cx, "fira");
    keys(cx, "escape");
    assert!(!probe.open(cx));
    keys(cx, "down");
    assert!(probe.open(cx));
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Fira Code"));
    keys(cx, "escape up");
    assert!(probe.open(cx));
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Fira Mono"));
    // The toggle closes and opens it with no highlight.
    keys(cx, "alt-down");
    assert!(!probe.open(cx));
    keys(cx, "alt-down");
    assert!(probe.open(cx));
    assert_eq!(probe.highlighted(cx), None);
    // Page and end moves leave a closed list closed.
    keys(cx, "escape pagedown home end");
    assert!(!probe.open(cx));
}

#[gpui::test]
fn auto_highlight_lets_confirm_complete_the_first_match(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(
        cx,
        Setup {
            auto_highlight: true,
            ..Setup::default()
        },
    );
    type_text(cx, "mo");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Monaco"));
    // Every edit highlights the new first match.
    type_text(cx, "no");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Mononoki"));
    keys(cx, "enter");
    assert_eq!(probe.taken(cx), ["picked:mononoki"]);
    assert!(!probe.open(cx));
    assert_eq!(probe.text(cx), "Mononoki");
}

#[gpui::test]
fn confirm_without_a_highlight_leaves_any_text_to_the_owner(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(cx, Setup::default());
    type_text(cx, "mo");
    assert!(probe.open(cx));
    keys(cx, "enter");
    assert_eq!(probe.taken(cx), ["enter:mo"], "no suggestion was picked");
    assert!(!probe.open(cx), "confirm closes the list");
    // A name no suggestion has is kept as typed.
    type_text(cx, " Comic");
    assert!(!probe.open(cx));
    keys(cx, "enter");
    assert_eq!(probe.taken(cx), ["enter:mo Comic"]);
    assert_eq!(probe.text(cx), "mo Comic");
}

#[gpui::test]
fn escape_closes_the_list_before_it_reaches_the_owner(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    type_text(cx, "mo");
    keys(cx, "down escape");
    assert!(!probe.open(cx));
    assert_eq!(probe.taken(cx), Vec::<String>::new());
    assert_eq!(probe.text(cx), "mo", "closing writes nothing");
    keys(cx, "escape");
    assert_eq!(probe.taken(cx), ["escape"]);
}

#[gpui::test]
fn an_owner_write_refilters_an_open_list_and_opens_none(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(cx, Setup::default());
    type_text(cx, "mo");
    let input = probe.input(cx);
    let write = |cx: &mut VisualTestContext, text: &'static str| {
        cx.update(|_, cx| {
            input.update(cx, |input, cx| input.set_text(text, cx));
        });
        cx.run_until_parked();
    };
    write(cx, "fira");
    assert!(probe.open(cx));
    assert_eq!(probe.labels(cx), ["Fira Code", "Fira Mono"]);
    // An owner putting back the value after Escape opens nothing.
    keys(cx, "escape");
    write(cx, "Menlo");
    assert!(!probe.open(cx));
}

#[gpui::test]
fn pressing_a_suggestion_picks_it_once_without_a_blur(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    type_text(cx, "mo");
    // An accessibility click presses and releases at the row's center.
    a11y_send(
        cx,
        |window| a11y_labelled(window, "Mononoki"),
        gpui::AccessibleAction::Click,
    );
    assert_eq!(probe.taken(cx), ["picked:mononoki"]);
    assert!(probe.field_focused(cx));
    assert!(!probe.open(cx));
    assert_eq!(probe.text(cx), "Mononoki");
}

#[gpui::test]
fn blur_and_deactivation_close_the_list(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    type_text(cx, "mo");
    let other = cx.update(|_, cx| probe.owner.read(cx).other.clone());
    cx.update(|window, cx| other.focus(window, cx));
    cx.run_until_parked();
    assert!(!probe.open(cx));
    assert_eq!(probe.taken(cx), ["blur"]);
    probe.focus_field(cx);
    type_text(cx, "n");
    assert!(probe.open(cx));
    cx.deactivate_window();
    cx.run_until_parked();
    assert!(!probe.open(cx));
}

#[gpui::test]
fn an_outside_press_closes_the_list_and_still_lands(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    type_text(cx, "mo");
    // A press on the field leaves the list open.
    cx.simulate_click(point(px(20.0), px(12.0)), Modifiers::none());
    cx.run_until_parked();
    assert!(probe.open(cx));
    // A press that moves no focus, so no blur closes the list.
    cx.simulate_click(point(px(320.0), px(220.0)), Modifiers::none());
    cx.run_until_parked();
    assert!(!probe.open(cx));
    assert_eq!(probe.taken(cx), ["inert-down"]);
    assert!(probe.field_focused(cx));
}

#[gpui::test]
fn a_read_only_or_disabled_field_never_shows_suggestions(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(cx, Setup::default());
    type_text(cx, "mo");
    assert!(probe.open(cx));
    let input = probe.input(cx);
    // Turning read-only while open closes the list, and focus stays.
    cx.update(|_, cx| {
        input.update(cx, |input, cx| input.set_read_only(true, cx));
    });
    cx.run_until_parked();
    assert!(!probe.open(cx));
    assert!(probe.field_focused(cx), "the keys still reach the owner");
    keys(cx, "down");
    assert!(!probe.open(cx));
    keys(cx, "alt-down");
    assert!(!probe.open(cx));
    cx.update(|_, cx| {
        input.update(cx, |input, cx| {
            input.set_read_only(false, cx);
            input.set_disabled(true, Some("Set by the profile".into()), cx);
        });
    });
    cx.run_until_parked();
    keys(cx, "down");
    assert!(!probe.open(cx));
    let combobox = probe.combobox(cx);
    let opened = cx.update(|_, cx| combobox.update(cx, Combobox::open));
    assert!(!opened);
}

#[gpui::test]
fn the_field_and_list_report_their_roles_and_states(cx: &mut TestAppContext) {
    let (_, cx) = setup(cx, Setup::default());
    cx.update(|window, _| {
        let field = &a11y_node(window, "font")["aria"];
        assert_eq!(field["role"], "EditableComboBox");
        assert_eq!(field["label"], "Font family");
        assert_eq!(field["expanded"], false);
        assert!(field["description"].is_null(), "{field}");
    });
    type_text(cx, "mo");
    keys(cx, "down down");
    cx.update(|window, _| {
        let field = &a11y_node(window, "font")["aria"];
        assert_eq!(field["value"], "mo");
        assert_eq!(field["expanded"], true);
        assert_eq!(field["description"], "Mononoki");
        let list = &a11y_node(window, "options")["aria"];
        assert_eq!(list["role"], "ListBox");
        assert_eq!(list["label"], "Font family");
        assert_eq!(list["size_of_set"], 5);
        let mut options: Vec<_> = a11y_nodes(window)
            .into_iter()
            .filter(|node| node["aria"]["role"] == "ListBoxOption")
            .map(|node| {
                let aria = &node["aria"];
                (
                    aria["position_in_set"].as_u64(),
                    aria["label"].as_str().map(str::to_owned),
                    aria["selected"] == true,
                )
            })
            .collect();
        options.sort();
        let expected: Vec<_> = (0..)
            .zip(probe_labels())
            .map(|(position, label)| {
                (Some(position), Some(label.to_owned()), label == "Mononoki")
            })
            .collect();
        assert_eq!(options, expected);
    });
    keys(cx, "escape");
    cx.update(|window, _| {
        let field = &a11y_node(window, "font")["aria"];
        assert_eq!(field["expanded"], false);
        assert!(field["description"].is_null(), "{field}");
    });
}

/// The suggestions for "mo".
fn probe_labels() -> [&'static str; 5] {
    [
        "Monaco",
        "Mononoki",
        "Fira Mono",
        "JetBrains Mono",
        "Berkeley Mono",
    ]
}

#[gpui::test]
fn a_moving_pointer_moves_the_one_highlight(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    type_text(cx, "mo");
    keys(cx, "down");
    let combobox = probe.combobox(cx);
    let list = cx
        .read(|cx| combobox.read(cx).list_bounds())
        .expect("the list painted");
    // The third row's center, under the list's border and padding.
    let row = point(list.left() + px(20.0), list.top() + px(1.0 + 4.0 + 70.0));
    cx.simulate_mouse_move(row, None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Fira Mono"));
    cx.update(|window, _| {
        assert_eq!(
            a11y_node(window, "font")["aria"]["description"],
            "Fira Mono"
        );
    });
}

#[gpui::test]
fn a_long_list_scrolls_to_its_highlight_inside_the_window(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(
        cx,
        Setup {
            many: true,
            field: Field::AtBottom,
            ..Setup::default()
        },
    );
    type_text(cx, "font");
    assert_eq!(probe.labels(cx).len(), 40);
    let combobox = probe.combobox(cx);
    let viewport = cx.update(|window, _| window.viewport_size());
    let list = cx
        .read(|cx| combobox.read(cx).list_bounds())
        .expect("the list painted");
    assert!(
        list.top() >= px(0.0) && list.bottom() <= viewport.height,
        "{list:?} in {viewport:?}"
    );
    assert!(
        list.size.height <= px(8.0 * 28.0 + 2.0 * 4.0 + 2.0),
        "eight rows show at most: {list:?}"
    );
    let shows = |cx: &mut VisualTestContext, row: usize| {
        cx.read(|cx| combobox.read(cx).list.shows(row))
    };
    keys(cx, "end");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Font 39"));
    assert!(shows(cx, 39));
    keys(cx, "home pagedown");
    assert_eq!(
        probe.highlighted(cx).as_deref(),
        Some("Font 08"),
        "a page is the eight rows that show"
    );
    assert!(shows(cx, 8));
    keys(cx, "home");
    assert!(shows(cx, 0));
}

#[gpui::test]
fn each_edit_filters_once(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    probe.filtered(cx);
    type_text(cx, "m");
    assert!(probe.open(cx));
    assert_eq!(probe.filtered(cx), 1, "the edit that opens the list");
    type_text(cx, "o");
    assert_eq!(probe.filtered(cx), 1, "an edit while the list shows");
    let input = probe.input(cx);
    cx.update(|_, cx| {
        input.update(cx, |input, cx| input.set_text("fira", cx));
    });
    cx.run_until_parked();
    assert_eq!(probe.labels(cx), ["Fira Code", "Fira Mono"]);
    assert_eq!(probe.filtered(cx), 1, "an owner's write while it shows");
}

#[gpui::test]
fn the_list_narrows_to_fit_a_narrow_window(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    cx.simulate_resize(size(px(180.0), px(400.0)));
    cx.run_until_parked();
    type_text(cx, "mo");
    assert_inside_margins(&probe, cx);
}

#[gpui::test]
fn a_full_width_field_keeps_its_list_inside_the_window(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(
        cx,
        Setup {
            field: Field::FullWidth,
            ..Setup::default()
        },
    );
    type_text(cx, "mo");
    assert_inside_margins(&probe, cx);
}

#[gpui::test]
fn the_list_follows_its_field_when_the_field_moves(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    type_text(cx, "mo");
    deliver_frames(cx);
    assert_eq!(probe.list_bounds(cx).left(), MARGIN);
    // One redraw of the owner moves the field.
    probe.owner.update(cx, |owner, cx| {
        owner.shift = px(50.0);
        cx.notify();
    });
    deliver_frames(cx);
    assert!(probe.open(cx));
    assert_eq!(probe.list_bounds(cx).left(), px(50.0));
}

#[gpui::test]
fn a_wheel_outside_the_list_closes_it_and_still_scrolls(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(cx, Setup::default());
    type_text(cx, "mo");
    wheel(cx, point(px(350.0), px(420.0)), -30.0);
    assert!(!probe.open(cx));
    let offset = cx.update(|_, cx| probe.owner.read(cx).scroll.offset());
    assert_eq!(offset.y, px(-30.0), "the area under the wheel scrolled");
    assert!(probe.field_focused(cx));
}

#[gpui::test]
fn a_wheel_over_the_list_scrolls_it_and_keeps_it_open(cx: &mut TestAppContext) {
    let (probe, cx) = setup(
        cx,
        Setup {
            many: true,
            ..Setup::default()
        },
    );
    type_text(cx, "font");
    let combobox = probe.combobox(cx);
    let shows = |cx: &mut VisualTestContext, row: usize| {
        cx.read(|cx| combobox.read(cx).list.shows(row))
    };
    assert!(shows(cx, 0));
    let list = probe.list_bounds(cx);
    wheel(cx, list.center(), -100.0);
    assert!(probe.open(cx));
    assert!(!shows(cx, 0), "the list scrolled");
}
