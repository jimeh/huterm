//! Which display a Quake window uses: the profile's `display` selector,
//! resolved again on every show from hidden, unless the user moved the window
//! to another display in regular presentation and the displays have not
//! changed since.

use super::Display;

/// A display the user moved the window to in regular presentation. It holds
/// only while the profile names the same selector and the same displays are
/// connected with the same primary, so connecting or disconnecting a display
/// returns the window to the display its profile names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pin {
    display: String,
    selector: String,
    arrangement: Arrangement,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Arrangement {
    ids: Vec<String>,
    primary: Option<String>,
}

impl Arrangement {
    fn of(displays: &[Display]) -> Self {
        let mut ids: Vec<_> =
            displays.iter().map(|display| display.id.clone()).collect();
        ids.sort_unstable();
        Self {
            ids,
            primary: displays
                .iter()
                .find(|display| display.primary)
                .map(|display| display.id.clone()),
        }
    }
}

impl Pin {
    fn holds<'a>(
        &self,
        displays: &'a [Display],
        selector: &str,
    ) -> Option<&'a Display> {
        (self.selector == selector
            && self.arrangement == Arrangement::of(displays))
        .then(|| displays.iter().find(|display| display.id == self.display))
        .flatten()
    }
}

/// Resolves a selector against the connected displays. `point` is the pointer
/// for `pointer`, or the focused window's centre for `active`. A selector
/// that matches nothing, such as a disconnected `id:`, uses the primary
/// display, or the first display when none is primary.
pub(crate) fn select<'a>(
    displays: &'a [Display],
    selector: &str,
    point: Option<(f64, f64)>,
) -> Option<&'a Display> {
    let requested = displays.iter().find(|display| {
        selector.strip_prefix("id:") == Some(display.id.as_str())
            || point.is_some_and(|(x, y)| display.frame.contains(x, y))
    });
    if requested.is_none() && selector.starts_with("id:") {
        eprintln!("Quake display {selector:?} is unavailable; using primary");
    }
    requested
        .or_else(|| displays.iter().find(|display| display.primary))
        .or_else(|| displays.first())
}

/// The display a show from hidden uses: the pinned display while its pin
/// holds, otherwise the selector resolved now. A pin that no longer holds is
/// dropped.
pub(crate) fn show<'a>(
    displays: &'a [Display],
    selector: &str,
    point: Option<(f64, f64)>,
    pin: &mut Option<Pin>,
) -> Option<&'a Display> {
    if let Some(display) =
        pin.as_ref().and_then(|pin| pin.holds(displays, selector))
    {
        return Some(display);
    }
    *pin = None;
    select(displays, selector, point)
}

/// The display after the connected displays change. The window keeps a
/// display that is still connected, refreshing its geometry, so a visible
/// window never jumps; otherwise it moves where a show would place it, with
/// `active` and `pointer` using the primary display. Any change to the
/// arrangement drops a pin, even one the displays later restore.
pub(crate) fn changed<'a>(
    displays: &'a [Display],
    current: &str,
    selector: &str,
    pin: &mut Option<Pin>,
) -> Option<&'a Display> {
    if pin
        .as_ref()
        .is_some_and(|pin| pin.holds(displays, selector).is_none())
    {
        *pin = None;
    }
    displays
        .iter()
        .find(|display| display.id == current)
        .or_else(|| show(displays, selector, None, pin))
}

/// Records the display a window returns to quake presentation on. Only a
/// move to another display pins it; returning on the display it left keeps
/// any earlier pin.
pub(crate) fn moved(
    displays: &[Display],
    from: &str,
    to: &Display,
    selector: &str,
    pin: &mut Option<Pin>,
) {
    if to.id != from {
        *pin = Some(Pin {
            display: to.id.clone(),
            selector: selector.to_owned(),
            arrangement: Arrangement::of(displays),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quake::Rect;

    const BUILT_IN: &str = "built-in";
    const EXTERNAL: &str = "external";
    const SIDE: &str = "side";

    fn display(id: &str, x: f64, primary: bool) -> Display {
        let frame = Rect {
            x,
            y: 0.0,
            width: 1000.0,
            height: 800.0,
        };
        Display {
            id: id.into(),
            frame,
            work: frame,
            primary,
        }
    }

    /// The built-in display at the origin and the external one to its right.
    fn docked() -> Vec<Display> {
        vec![
            display(EXTERNAL, 1000.0, true),
            display(BUILT_IN, 0.0, false),
        ]
    }

    fn undocked() -> Vec<Display> {
        vec![display(BUILT_IN, 0.0, true)]
    }

    /// Both connected, with the built-in display made primary.
    fn swapped() -> Vec<Display> {
        vec![
            display(EXTERNAL, 1000.0, false),
            display(BUILT_IN, 0.0, true),
        ]
    }

    /// A window's placement state as the desktop keeps it.
    struct Window {
        selector: &'static str,
        display: String,
        pin: Option<Pin>,
    }

    impl Window {
        fn open(selector: &'static str, displays: &[Display]) -> Self {
            Self::open_at(selector, displays, None)
        }

        fn open_at(
            selector: &'static str,
            displays: &[Display],
            point: Option<(f64, f64)>,
        ) -> Self {
            let mut pin = None;
            let display = show(displays, selector, point, &mut pin)
                .expect("a display")
                .id
                .clone();
            Self {
                selector,
                display,
                pin,
            }
        }

        fn show(&mut self, displays: &[Display]) -> &str {
            self.show_at(displays, None)
        }

        fn show_at(
            &mut self,
            displays: &[Display],
            point: Option<(f64, f64)>,
        ) -> &str {
            self.display = show(displays, self.selector, point, &mut self.pin)
                .expect("a display")
                .id
                .clone();
            &self.display
        }

        fn change(&mut self, displays: &[Display]) -> &str {
            self.display =
                changed(displays, &self.display, self.selector, &mut self.pin)
                    .expect("a display")
                    .id
                    .clone();
            &self.display
        }

        fn move_to(&mut self, displays: &[Display], id: &str) {
            let to = displays
                .iter()
                .find(|display| display.id == id)
                .expect("a connected display");
            moved(displays, &self.display, to, self.selector, &mut self.pin);
            self.display = id.into();
        }
    }

    #[test]
    fn primary_follows_a_disconnect_and_reconnect() {
        let mut window = Window::open("primary", &docked());
        assert_eq!(window.display, EXTERNAL);
        // Unplugging moves even a hidden window off the missing display.
        assert_eq!(window.change(&undocked()), BUILT_IN);
        assert_eq!(window.show(&undocked()), BUILT_IN);
        assert_eq!(window.change(&docked()), BUILT_IN);
        assert_eq!(
            window.show(&docked()),
            EXTERNAL,
            "the next show uses the reconnected primary display"
        );
    }

    #[test]
    fn primary_changed_while_hidden_applies_on_the_next_show() {
        let mut window = Window::open("primary", &docked());
        let swapped = swapped();
        assert_eq!(window.change(&swapped), EXTERNAL);
        assert_eq!(window.show(&swapped), BUILT_IN);
    }

    #[test]
    fn primary_changed_while_visible_keeps_the_window_until_it_shows_again() {
        let mut window = Window::open("primary", &docked());
        let swapped = swapped();
        assert_eq!(
            window.change(&swapped),
            EXTERNAL,
            "a visible window stays on a display that is still connected"
        );
        assert_eq!(window.show(&swapped), BUILT_IN);
    }

    #[test]
    fn a_visible_window_on_a_disconnected_display_moves_to_the_primary() {
        let mut window = Window::open("primary", &docked());
        assert_eq!(window.change(&undocked()), BUILT_IN);
    }

    #[test]
    fn a_resolution_change_keeps_the_display() {
        let mut window = Window::open("primary", &docked());
        let mut wider = docked();
        wider[0].frame.width = 2000.0;
        assert_eq!(window.change(&wider), EXTERNAL);
    }

    #[test]
    fn id_uses_its_display_whenever_connected_and_otherwise_the_primary() {
        let mut window = Window::open("id:external", &docked());
        assert_eq!(window.display, EXTERNAL);
        assert_eq!(window.change(&undocked()), BUILT_IN);
        assert_eq!(window.show(&undocked()), BUILT_IN);
        assert_eq!(window.show(&docked()), EXTERNAL);
        // A missing ID uses the primary display, not an arbitrary one.
        let mut window = Window::open("id:missing", &docked());
        assert_eq!(window.display, EXTERNAL);
        let side = vec![
            display(SIDE, -1000.0, false),
            display(BUILT_IN, 0.0, true),
            display(EXTERNAL, 1000.0, false),
        ];
        assert_eq!(window.show(&side), BUILT_IN);
    }

    #[test]
    fn without_a_primary_display_the_first_display_is_used() {
        let displays = vec![
            display(BUILT_IN, 0.0, false),
            display(EXTERNAL, 1000.0, false),
        ];
        assert_eq!(Window::open("id:missing", &displays).display, BUILT_IN);
        assert_eq!(Window::open("primary", &displays).display, BUILT_IN);
    }

    #[test]
    fn active_and_pointer_resolve_their_point_on_every_show() {
        for selector in ["active", "pointer"] {
            let mut window =
                Window::open_at(selector, &docked(), Some((500.0, 400.0)));
            assert_eq!(window.display, BUILT_IN, "{selector}");
            assert_eq!(
                window.show_at(&docked(), Some((1500.0, 400.0))),
                EXTERNAL,
                "{selector}"
            );
            // A visible window whose display goes away uses the primary.
            assert_eq!(window.change(&undocked()), BUILT_IN, "{selector}");
            assert_eq!(
                window.show_at(&docked(), Some((1500.0, 400.0))),
                EXTERNAL,
                "{selector}"
            );
            // A point outside every display uses the primary.
            assert_eq!(
                window.show_at(&docked(), Some((-500.0, 400.0))),
                EXTERNAL,
                "{selector}"
            );
        }
    }

    #[test]
    fn a_move_holds_until_the_displays_change() {
        let mut window = Window::open("primary", &docked());
        window.move_to(&docked(), BUILT_IN);
        assert_eq!(window.show(&docked()), BUILT_IN);
        assert_eq!(
            window.show(&docked()),
            BUILT_IN,
            "the move survives hiding"
        );
        // A resolution change is not a display change.
        let mut wider = docked();
        wider[1].frame.width = 2000.0;
        assert_eq!(window.change(&wider), BUILT_IN);
        assert_eq!(window.show(&wider), BUILT_IN);
        window.change(&undocked());
        window.change(&docked());
        assert_eq!(
            window.show(&docked()),
            EXTERNAL,
            "a disconnect drops the move even after the display returns"
        );
    }

    #[test]
    fn a_move_ends_when_a_display_connects_or_the_primary_changes() {
        let mut window = Window::open("primary", &docked());
        window.move_to(&docked(), BUILT_IN);
        let mut more = docked();
        more.push(display(SIDE, -1000.0, false));
        assert_eq!(window.show(&more), EXTERNAL);

        let mut window = Window::open("id:built-in", &docked());
        window.move_to(&docked(), EXTERNAL);
        let swapped = swapped();
        assert_eq!(window.show(&swapped), BUILT_IN);
    }

    #[test]
    fn a_move_ends_when_the_profile_names_another_display() {
        let mut window = Window::open("primary", &docked());
        window.move_to(&docked(), BUILT_IN);
        window.selector = "id:external";
        assert_eq!(window.show(&docked()), EXTERNAL);
        window.selector = "primary";
        assert_eq!(window.show(&docked()), EXTERNAL, "the move stays dropped");
    }

    #[test]
    fn returning_on_the_same_display_does_not_pin_active() {
        let mut window =
            Window::open_at("active", &docked(), Some((500.0, 400.0)));
        window.move_to(&docked(), BUILT_IN);
        assert!(window.pin.is_none());
        assert_eq!(window.show_at(&docked(), Some((1500.0, 400.0))), EXTERNAL);
        window.move_to(&docked(), BUILT_IN);
        assert_eq!(
            window.show_at(&docked(), Some((1500.0, 400.0))),
            BUILT_IN,
            "a move to another display overrides the active display"
        );
    }
}
