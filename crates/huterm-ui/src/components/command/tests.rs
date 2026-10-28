use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Action, Context, Div, Entity, FocusHandle, Focusable as _, KeyBinding,
    Modifiers, Pixels, Render, TestAppContext, VisualTestContext, Window, div,
    point, prelude::*, px,
};

use super::{Command, CommandEvent, HighlightFallback};
use crate::components::input::{Changed, Input};
use crate::option_list::ListOption;
use crate::testing::{a11y_labelled, a11y_node, a11y_nodes, a11y_send, tokens};

gpui::actions!(
    command_test,
    [Next, Previous, PageDown, PageUp, First, Last, Confirm]
);

/// The key context the owner's bindings use.
const KEYS: &str = "CommandTest";

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

/// How a test's owner is set up.
#[derive(Clone, Copy)]
struct Setup {
    /// Forty numbered fonts in place of [`FONTS`], in a list at most
    /// `max_height` points tall.
    many: bool,
    max_height: f32,
    /// The height the owner gives the list, in place of its own.
    height: Option<f32>,
    pointer_highlights: bool,
    fallback: HighlightFallback,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            many: false,
            max_height: 100.0,
            height: None,
            pointer_highlights: true,
            fallback: HighlightFallback::First,
        }
    }
}

/// A search over a command list of fonts, whose owner filters the fonts
/// containing the query, routes the test keys to the list, and logs its
/// events and the search's blur. The root takes focus from presses nothing
/// else takes, as a window's root view does.
struct Owner {
    input: Entity<Input>,
    command: Entity<Command>,
    root: FocusHandle,
    log: Rc<RefCell<Vec<CommandEvent>>>,
    blurs: Rc<RefCell<usize>>,
    height: Option<Pixels>,
}

impl Owner {
    fn new(
        setup: Setup,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let input = cx.new(|cx| Input::new("search", "Search", tokens(), cx));
        let command = cx.new(|cx| {
            let mut command =
                Command::new("fonts", "Fonts", &input, tokens(), cx)
                    .with_pointer_highlight(setup.pointer_highlights)
                    .with_highlight_fallback(setup.fallback);
            if setup.many {
                command.set_max_height(Some(px(setup.max_height)), cx);
            }
            command
        });
        let fonts: Vec<String> = if setup.many {
            (0..40).map(|index| format!("Font {index:02}")).collect()
        } else {
            FONTS.iter().map(|font| (*font).to_owned()).collect()
        };
        let filter = move |query: &str| -> Vec<ListOption> {
            let query = query.to_lowercase();
            fonts
                .iter()
                .filter(|font| font.to_lowercase().contains(&query))
                .map(|font| ListOption::new(font.to_lowercase(), font.clone()))
                .collect()
        };
        let log = Rc::new(RefCell::new(Vec::new()));
        let seen = log.clone();
        cx.subscribe(&command, move |_, _, event: &CommandEvent, _| {
            seen.borrow_mut().push(event.clone());
        })
        .detach();
        let all = filter("");
        command.update(cx, |command, cx| command.set_options(all, cx));
        cx.subscribe(&input, move |owner, input, _: &Changed, cx| {
            let query = input.read(cx).text().to_owned();
            let options = filter(&query);
            owner.command.update(cx, |command, cx| {
                command.set_empty(format!("No fonts match “{query}”"), cx);
                command.set_options(options, cx);
            });
        })
        .detach();
        let blurs = Rc::new(RefCell::new(0));
        let counted = blurs.clone();
        let focus = input.read(cx).focus_handle(cx);
        cx.on_blur(&focus, window, move |_, _, _| {
            *counted.borrow_mut() += 1;
        })
        .detach();
        Self {
            input,
            command,
            root: cx.focus_handle(),
            log,
            blurs,
            height: setup.height.map(px),
        }
    }
}

impl Render for Owner {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let keys = div()
            .key_context(KEYS)
            .w(px(200.0))
            .flex()
            .flex_col()
            .on_action(cx.listener(|owner, _: &Confirm, _, cx| {
                owner.command.update(cx, Command::confirm);
            }));
        let keys = route::<Next>(keys, Command::select_next, cx);
        let keys = route::<Previous>(keys, Command::select_previous, cx);
        let keys = route::<PageDown>(keys, Command::select_page_down, cx);
        let keys = route::<PageUp>(keys, Command::select_page_up, cx);
        let keys = route::<First>(keys, Command::select_first, cx);
        let keys = route::<Last>(keys, Command::select_last, cx);
        let list = match self.height {
            Some(height) => div()
                .h(height)
                .flex()
                .flex_col()
                .child(self.command.clone())
                .into_any_element(),
            None => self.command.clone().into_any_element(),
        };
        div()
            .size_full()
            .track_focus(&self.root)
            .child(crate::focus::tracker())
            .child(keys.child(self.input.clone()).child(list))
    }
}

/// `element` running `act` on the command for the test action `A`.
fn route<A: Action>(
    element: Div,
    act: fn(&mut Command, &mut Context<'_, Command>),
    cx: &mut Context<'_, Owner>,
) -> Div {
    element.on_action(cx.listener(move |owner, _: &A, _, cx| {
        owner.command.update(cx, act);
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

struct Probe {
    owner: Entity<Owner>,
    focus: FocusHandle,
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
        ]);
    });
    // Blur reaches focus listeners only in an active window.
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let focus =
        cx.update(|_, cx| owner.read(cx).input.read(cx).focus_handle(cx));
    cx.update(|window, cx| focus.focus(window, cx));
    cx.run_until_parked();
    let probe = Probe { owner, focus };
    probe.taken(cx);
    (probe, cx)
}

impl Probe {
    fn command(&self, cx: &mut VisualTestContext) -> Entity<Command> {
        cx.update(|_, cx| self.owner.read(cx).command.clone())
    }

    fn labels(&self, cx: &mut VisualTestContext) -> Vec<String> {
        let command = self.command(cx);
        cx.read(|cx| {
            command
                .read(cx)
                .options()
                .iter()
                .map(|option| option.label.to_string())
                .collect()
        })
    }

    fn highlighted(&self, cx: &mut VisualTestContext) -> Option<String> {
        let command = self.command(cx);
        cx.read(|cx| {
            command
                .read(cx)
                .highlighted()
                .map(|option| option.label.to_string())
        })
    }

    fn text(&self, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| self.owner.read(cx).input.read(cx).text().to_owned())
    }

    fn search_focused(&self, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, _| self.focus.is_focused(window))
    }

    fn blurs(&self, cx: &mut VisualTestContext) -> usize {
        cx.update(|_, cx| *self.owner.read(cx).blurs.borrow())
    }

    fn shows(&self, cx: &mut VisualTestContext, row: usize) -> bool {
        let command = self.command(cx);
        cx.read(|cx| command.read(cx).list.shows(row))
    }

    fn taken(&self, cx: &mut VisualTestContext) -> Vec<CommandEvent> {
        cx.update(|_, cx| {
            std::mem::take(&mut *self.owner.read(cx).log.borrow_mut())
        })
    }
}

fn highlighted(id: &str) -> CommandEvent {
    CommandEvent::Highlighted(id.to_owned().into())
}

fn picked(id: &str) -> CommandEvent {
    CommandEvent::Picked(id.to_owned().into())
}

#[gpui::test]
fn the_query_filters_the_items_and_highlights_the_first(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(cx, Setup::default());
    assert_eq!(probe.labels(cx).len(), 8);
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Menlo"));
    type_text(cx, "mo");
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
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Monaco"));
    assert_eq!(probe.taken(cx), [highlighted("monaco")]);
    assert!(probe.search_focused(cx));
}

#[gpui::test]
fn the_highlight_stays_on_its_item_while_it_matches(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    keys(cx, "down down down down");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Fira Mono"));
    probe.taken(cx);
    type_text(cx, "mono");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Fira Mono"));
    assert_eq!(probe.taken(cx), Vec::<CommandEvent>::new());
    type_text(cx, "n");
    assert_eq!(probe.labels(cx), ["Mononoki"]);
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Mononoki"));
    assert_eq!(probe.taken(cx), [highlighted("mononoki")]);
}

#[gpui::test]
fn moves_stop_at_the_ends_and_keep_focus_in_the_search(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(cx, Setup::default());
    keys(cx, "up");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Menlo"));
    assert_eq!(probe.taken(cx), Vec::<CommandEvent>::new());
    keys(cx, "down");
    assert_eq!(probe.taken(cx), [highlighted("monaco")]);
    keys(cx, "end down");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Iosevka"));
    keys(cx, "home");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Menlo"));
    // Every item shows, so a page reaches the last.
    keys(cx, "pagedown");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Iosevka"));
    keys(cx, "pageup");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Menlo"));
    assert_eq!(probe.text(cx), "");
    assert!(probe.search_focused(cx));
}

#[gpui::test]
fn confirm_reports_the_highlighted_item_and_nothing_without_one(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(cx, Setup::default());
    keys(cx, "down enter");
    assert_eq!(probe.taken(cx), [highlighted("monaco"), picked("monaco")]);
    type_text(cx, "zzz");
    probe.taken(cx);
    keys(cx, "enter");
    assert_eq!(probe.taken(cx), Vec::<CommandEvent>::new());
    let command = probe.command(cx);
    let confirmed = cx.update(|_, cx| command.update(cx, Command::confirm));
    assert!(!confirmed);
}

#[gpui::test]
fn no_matches_show_the_empty_state(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    type_text(cx, "zzz");
    assert_eq!(probe.labels(cx), Vec::<String>::new());
    assert_eq!(probe.highlighted(cx), None);
    cx.update(|window, _| {
        let empty = &a11y_node(window, "empty")["aria"];
        assert_eq!(empty["label"], "No fonts match “zzz”");
        assert!(
            a11y_nodes(window)
                .iter()
                .all(|node| node["aria"]["role"] != "ListBox"),
            "the list gives way to the empty state"
        );
        let field = &a11y_node(window, "search")["aria"];
        assert_eq!(field["expanded"], false, "no list shows");
        assert!(field["description"].is_null(), "{field}");
    });
    // Clearing the query brings the list back.
    let input = cx.update(|_, cx| probe.owner.read(cx).input.clone());
    cx.update(|_, cx| input.update(cx, |input, cx| input.set_text("", cx)));
    cx.run_until_parked();
    assert_eq!(probe.labels(cx).len(), 8);
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Menlo"));
    cx.update(|window, _| {
        assert_eq!(a11y_node(window, "search")["aria"]["expanded"], true);
    });
}

#[gpui::test]
fn pressing_an_item_highlights_and_picks_it_in_place(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    a11y_send(
        cx,
        |window| a11y_labelled(window, "Fira Code"),
        gpui::AccessibleAction::Click,
    );
    assert_eq!(
        probe.taken(cx),
        [highlighted("fira code"), picked("fira code")]
    );
    assert!(probe.search_focused(cx));
    assert_eq!(probe.blurs(cx), 0);
}

/// Moves the pointer over the fourth item: below the search's 24
/// points, the list's 4-point padding, and three 28-point rows.
fn hover_fourth(cx: &mut VisualTestContext) {
    let fourth = point(px(40.0), px(24.0 + 4.0 + 3.0 * 28.0 + 14.0));
    cx.simulate_mouse_move(fourth, None, Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn a_moving_pointer_highlights_unless_turned_off(cx: &mut TestAppContext) {
    let (probe, cx) = setup(cx, Setup::default());
    hover_fourth(cx);
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Fira Code"));
    assert_eq!(probe.taken(cx), [highlighted("fira code")]);
}

#[gpui::test]
fn a_list_showing_details_keeps_its_highlight_under_the_pointer(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(
        cx,
        Setup {
            pointer_highlights: false,
            ..Setup::default()
        },
    );
    hover_fourth(cx);
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Menlo"));
    assert_eq!(probe.taken(cx), Vec::<CommandEvent>::new());
}

#[gpui::test]
fn the_search_and_list_report_their_roles_and_states(cx: &mut TestAppContext) {
    let (_, cx) = setup(cx, Setup::default());
    keys(cx, "down");
    cx.update(|window, _| {
        let field = &a11y_node(window, "search")["aria"];
        assert_eq!(field["role"], "EditableComboBox");
        assert_eq!(field["expanded"], true);
        assert_eq!(field["description"], "Monaco");
        let list = &a11y_node(window, "options")["aria"];
        assert_eq!(list["role"], "ListBox");
        assert_eq!(list["label"], "Fonts");
        assert_eq!(list["size_of_set"], 8);
        let selected: Vec<_> = a11y_nodes(window)
            .into_iter()
            .filter(|node| node["aria"]["role"] == "ListBoxOption")
            .filter(|node| node["aria"]["selected"] == true)
            .map(|node| {
                (
                    node["aria"]["label"].clone(),
                    node["aria"]["position_in_set"].clone(),
                )
            })
            .collect();
        assert_eq!(selected, [("Monaco".into(), 1.into())]);
    });
}

#[gpui::test]
fn a_long_list_scrolls_to_its_highlight(cx: &mut TestAppContext) {
    let (probe, cx) = setup(
        cx,
        Setup {
            many: true,
            ..Setup::default()
        },
    );
    let command = probe.command(cx);
    let shows = |cx: &mut VisualTestContext, row: usize| {
        cx.read(|cx| command.read(cx).list.shows(row))
    };
    keys(cx, "end");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Font 39"));
    assert!(shows(cx, 39));
    // A 100-point list shows three whole 28-point rows under its padding.
    keys(cx, "home pagedown");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Font 03"));
    assert!(shows(cx, 3));
    keys(cx, "home");
    assert!(shows(cx, 0));
}

#[gpui::test]
fn a_cleared_fallback_loses_a_vanished_highlight(cx: &mut TestAppContext) {
    let (probe, cx) = setup(
        cx,
        Setup {
            fallback: HighlightFallback::Clear,
            ..Setup::default()
        },
    );
    assert_eq!(probe.highlighted(cx), None, "nothing was highlighted");
    keys(cx, "down down down down down");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Fira Mono"));
    probe.taken(cx);
    type_text(cx, "mono");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Fira Mono"));
    type_text(cx, "n");
    assert_eq!(probe.labels(cx), ["Mononoki"]);
    assert_eq!(probe.highlighted(cx), None, "Fira Mono no longer matches");
    keys(cx, "enter");
    assert_eq!(probe.taken(cx), Vec::<CommandEvent>::new());
    let command = probe.command(cx);
    let confirmed = cx.update(|_, cx| command.update(cx, Command::confirm));
    assert!(!confirmed);
    cx.update(|window, _| {
        let field = &a11y_node(window, "search")["aria"];
        assert!(field["description"].is_null(), "{field}");
    });
    // The owner restarts at the first item, and can clear it again.
    cx.update(|_, cx| command.update(cx, Command::select_first));
    assert_eq!(probe.taken(cx), [highlighted("mononoki")]);
    cx.update(|_, cx| command.update(cx, Command::clear_highlight));
    assert_eq!(probe.highlighted(cx), None);
    assert_eq!(probe.taken(cx), Vec::<CommandEvent>::new());
}

#[gpui::test]
fn a_list_held_below_its_maximum_moves_within_the_height_it_shows(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(
        cx,
        Setup {
            many: true,
            max_height: 140.0,
            height: Some(80.0),
            ..Setup::default()
        },
    );
    keys(cx, "end");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Font 39"));
    assert!(probe.shows(cx, 39));
    // An 80-point list shows two whole 28-point rows under its padding.
    keys(cx, "home pagedown");
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Font 02"));
    assert!(probe.shows(cx, 2));
    // The owner shrinks the list, which reveals the highlight again.
    keys(cx, "end");
    deliver_frames(cx);
    probe.owner.update(cx, |owner, cx| {
        owner.height = Some(px(60.0));
        cx.notify();
    });
    deliver_frames(cx);
    assert!(probe.shows(cx, 39));
}

#[gpui::test]
fn a_kept_highlight_stays_in_view_when_the_rows_shrink(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = setup(
        cx,
        Setup {
            many: true,
            ..Setup::default()
        },
    );
    let command = probe.command(cx);
    let set = |cx: &mut VisualTestContext, options: Vec<ListOption>| {
        cx.update(|_, cx| {
            command.update(cx, |command, cx| command.set_options(options, cx));
        });
        cx.run_until_parked();
    };
    let fonts = |count: usize| -> Vec<ListOption> {
        (0..count)
            .map(|index| {
                let option = ListOption::new(
                    format!("font {index:02}"),
                    format!("Font {index:02}"),
                );
                if index == 39 {
                    option.description("Variable")
                } else {
                    option
                }
            })
            .collect()
    };
    // The last font's description makes every row 40 points tall.
    set(cx, fonts(40));
    cx.update(|_, cx| {
        command.update(cx, |command, cx| command.select("font 20", cx));
    });
    cx.run_until_parked();
    assert!(probe.shows(cx, 20));
    // Without it, rows are 28 points, and the highlight keeps its index.
    set(cx, fonts(39));
    assert_eq!(probe.highlighted(cx).as_deref(), Some("Font 20"));
    assert!(probe.shows(cx, 20));
}
