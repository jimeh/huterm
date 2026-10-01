use std::sync::Arc;

use huterm_protocol::{
    CellSize, GridSize, InputStamp, Modifiers, MouseAction, MouseButton,
    MouseInput, MousePosition, RuntimeId, TerminalId, TerminalPresentation,
    ViewerCapabilities, WheelDirection,
};

use super::{Arbiter, Candidate, Effects, Report, select_controller};
use crate::viewer::{Initial, Registry, Slot};

fn cell() -> CellSize {
    CellSize {
        width: 8,
        height: 16,
    }
}

fn grid(columns: u16) -> GridSize {
    GridSize::clamped(columns, 24)
}

struct Fixture {
    registry: Registry,
    arbiter: Arbiter,
}

impl Fixture {
    fn new() -> Self {
        Self {
            registry: Registry::new(
                TerminalId::new(1),
                RuntimeId::new(0),
                Arc::new(crate::wake::Wake::default()),
            ),
            arbiter: Arbiter::new(
                grid(80),
                cell(),
                TerminalPresentation::default(),
            ),
        }
    }

    /// Registers a viewer and lets the arbiter see it.
    fn viewer(
        &mut self,
        capabilities: ViewerCapabilities,
        initial: Initial,
    ) -> (Arc<Slot>, Effects) {
        let (slot, _) =
            self.registry.register(None, capabilities, initial).unwrap();
        let effects = self.arbiter.sync(&self.registry);
        (slot, effects)
    }

    fn sized(&mut self, columns: u16) -> Arc<Slot> {
        self.viewer(
            ViewerCapabilities::ALL,
            Initial {
                focused: false,
                geometry: Some((grid(columns), cell())),
                presentation: None,
            },
        )
        .0
    }
}

fn blank() -> Initial {
    Initial {
        focused: false,
        geometry: None,
        presentation: None,
    }
}

fn candidate(id: u64, activity: u64, order: u64) -> Candidate {
    Candidate {
        id: huterm_protocol::ViewerId::new(id),
        activity,
        order,
    }
}

#[test]
fn latest_controller_follows_activity_then_registration_order() {
    assert_eq!(select_controller([]), None);
    assert_eq!(
        select_controller([candidate(1, 5, 1), candidate(2, 3, 2)]),
        Some(huterm_protocol::ViewerId::new(1))
    );
    assert_eq!(
        select_controller([candidate(1, 0, 1), candidate(2, 0, 2)]),
        Some(huterm_protocol::ViewerId::new(2)),
        "ties go to the latest registration"
    );
}

#[test]
fn focus_moves_control_and_resizes_once_to_the_new_controller() {
    let mut fixture = Fixture::new();
    let first = fixture.sized(100);
    let second = fixture.sized(120);
    // With no activity yet, the latest registration controls.
    assert_eq!(fixture.arbiter.canonical.0, grid(120));
    let revision = fixture.arbiter.geometry_revision();

    let effects = fixture.arbiter.report(&first, Report::Focus(true));
    assert_eq!(effects.resize, Some((grid(100), cell())));
    assert_eq!(fixture.arbiter.geometry_revision(), revision + 1);

    // Geometry from the viewer that does not control changes nothing.
    let effects = fixture
        .arbiter
        .report(&second, Report::Geometry(grid(90), cell()));
    assert_eq!(effects.resize, None);

    // Typing in the other viewer moves control before its input is written.
    let effects = fixture.arbiter.typed(&second);
    assert_eq!(effects.resize, Some((grid(90), cell())));
}

#[test]
fn hidden_viewers_keep_control_until_another_viewer_acts() {
    let mut fixture = Fixture::new();
    let shown = fixture.sized(100);
    let hidden = fixture.sized(120);
    fixture.arbiter.report(&hidden, Report::Focus(true));
    fixture.arbiter.report(&hidden, Report::Focus(false));
    // Losing focus or being hidden does not hand control to an idle viewer.
    let effects = fixture
        .arbiter
        .report(&hidden, Report::Geometry(grid(110), cell()));
    assert_eq!(effects.resize, Some((grid(110), cell())));
    let effects = fixture.arbiter.report(&shown, Report::Focus(true));
    assert_eq!(effects.resize, Some((grid(100), cell())));
}

#[test]
fn a_single_viewer_always_controls_size_and_presentation() {
    let mut fixture = Fixture::new();
    let only = fixture.sized(80);
    let effects = fixture
        .arbiter
        .report(&only, Report::Geometry(grid(90), cell()));
    assert_eq!(effects.resize, Some((grid(90), cell())));
    let mut presentation = TerminalPresentation::default();
    presentation.cursor.red = 9;
    let effects = fixture.arbiter.presentation(&only, presentation.clone());
    assert_eq!(effects.presentation, Some(presentation.clone()));
    assert!(fixture.arbiter.presentation(&only, presentation).is_empty());
}

#[test]
fn viewers_without_size_or_geometry_never_control() {
    let mut fixture = Fixture::new();
    let (reader, _) = fixture.viewer(
        ViewerCapabilities {
            size: false,
            ..ViewerCapabilities::ALL
        },
        Initial {
            focused: true,
            geometry: Some((grid(50), cell())),
            presentation: None,
        },
    );
    let (no_geometry, _) = fixture.viewer(ViewerCapabilities::ALL, blank());
    assert!(fixture.arbiter.typed(&reader).resize.is_none());
    assert!(fixture.arbiter.typed(&no_geometry).resize.is_none());
    // With no candidate, size and presentation stay as they are.
    assert_eq!(fixture.arbiter.canonical.0, grid(80));
}

#[test]
fn a_dropped_viewer_leaves_candidacy_at_once_and_finalizes_after_its_queue() {
    let mut fixture = Fixture::new();
    let remaining = fixture.sized(100);
    let closing = fixture.sized(120);
    fixture.arbiter.report(&closing, Report::Focus(true));
    assert_eq!(fixture.arbiter.canonical.0, grid(120));

    closing.queue();
    closing
        .dropped
        .store(true, std::sync::atomic::Ordering::Release);
    let effects = fixture.arbiter.sync(&fixture.registry);
    assert_eq!(
        effects.resize,
        Some((grid(100), cell())),
        "control moves before the dropped viewer's queued input runs"
    );
    assert_eq!(fixture.registry.slots().len(), 2, "not finalized yet");

    // Its queued input still makes it active, but it cannot take control.
    assert!(fixture.arbiter.typed(&closing).resize.is_none());
    assert!(closing.settled());
    let effects = fixture.arbiter.settled(&closing, &fixture.registry);
    assert_eq!(effects.focus, Some(false), "finalization reports focus out");
    assert_eq!(fixture.registry.slots().len(), 1);
    assert!(fixture.arbiter.typed(&remaining).resize.is_none());
}

#[test]
fn revocation_and_drop_together_finalize_once() {
    let mut fixture = Fixture::new();
    let (slot, effects) = fixture.viewer(
        ViewerCapabilities::ALL,
        Initial {
            focused: true,
            geometry: None,
            presentation: None,
        },
    );
    assert_eq!(effects.focus, Some(true));
    slot.dropped
        .store(true, std::sync::atomic::Ordering::Release);
    fixture.registry.revoke_all();
    let effects = fixture.arbiter.sync(&fixture.registry);
    assert_eq!(effects.focus, Some(false));
    assert!(fixture.arbiter.sync(&fixture.registry).is_empty());
    assert!(fixture.registry.slots().is_empty());
}

#[test]
fn focus_reports_follow_the_count_of_focused_input_viewers() {
    let mut fixture = Fixture::new();
    let first = fixture.sized(80);
    let second = fixture.sized(80);
    let (sizer, _) = fixture.viewer(
        ViewerCapabilities {
            input: false,
            ..ViewerCapabilities::ALL
        },
        blank(),
    );
    let focus = |fixture: &mut Fixture, slot: &Slot, focused| {
        fixture.arbiter.report(slot, Report::Focus(focused)).focus
    };
    assert_eq!(focus(&mut fixture, &first, true), Some(true));
    assert_eq!(focus(&mut fixture, &second, true), None);
    assert_eq!(focus(&mut fixture, &first, false), None);
    assert_eq!(focus(&mut fixture, &second, false), Some(false));
    // Moving between windows can report out, then in.
    assert_eq!(focus(&mut fixture, &second, true), Some(true));
    assert_eq!(focus(&mut fixture, &second, false), Some(false));
    assert_eq!(focus(&mut fixture, &first, true), Some(true));
    // A viewer without input never counts, but its focus moves control.
    assert_eq!(focus(&mut fixture, &sizer, true), None);
    assert_eq!(focus(&mut fixture, &first, true), None, "already focused");
}

fn mouse(action: MouseAction, column: u32) -> MouseInput {
    MouseInput {
        position: MousePosition { column, row: 0 },
        action,
        modifiers: Modifiers::default(),
    }
}

fn stamp(geometry: Option<u64>) -> InputStamp {
    InputStamp {
        scrolls: 0,
        geometry,
    }
}

#[test]
fn the_first_pressing_viewer_owns_the_gesture_until_its_release() {
    let mut fixture = Fixture::new();
    let first = fixture.sized(80);
    let second = fixture.sized(80);
    let mut admit = |slot: &Slot, action, column| {
        fixture
            .arbiter
            .admit_mouse(slot, &mouse(action, column), stamp(None))
    };
    let left = MouseButton::Left;
    assert!(admit(&first, MouseAction::Press(left), 1));
    assert!(!admit(&second, MouseAction::Press(left), 2));
    assert!(!admit(&second, MouseAction::Motion(None), 2));
    assert!(!admit(&second, MouseAction::Wheel(WheelDirection::Up), 2));
    assert!(!admit(&second, MouseAction::Release(left), 2));
    assert!(admit(&first, MouseAction::Motion(Some(left)), 3));
    assert!(!admit(&first, MouseAction::Release(MouseButton::Right), 3));
    assert!(admit(&first, MouseAction::Release(left), 3));
    // With no gesture held, hover and wheel come from any viewer.
    assert!(admit(&second, MouseAction::Motion(None), 4));
    assert!(admit(&second, MouseAction::Wheel(WheelDirection::Down), 4));
    assert!(!admit(&second, MouseAction::Release(left), 4), "unmatched");
}

#[test]
fn stale_geometry_discards_new_reports_but_never_owned_releases() {
    let mut fixture = Fixture::new();
    let first = fixture.sized(80);
    let second = fixture.sized(90);
    let stale = fixture.arbiter.geometry_revision();
    // A resize there and back still changes the revision.
    fixture.arbiter.report(&first, Report::Focus(true));
    fixture.arbiter.report(&second, Report::Focus(true));
    assert!(fixture.arbiter.geometry_revision() > stale + 1);
    let current = fixture.arbiter.geometry_revision();
    let left = MouseButton::Left;
    for action in [
        MouseAction::Press(left),
        MouseAction::Motion(None),
        MouseAction::Wheel(WheelDirection::Up),
    ] {
        assert!(
            !fixture.arbiter.admit_mouse(
                &second,
                &mouse(action, 1),
                stamp(Some(stale))
            ),
            "{action:?}"
        );
    }
    assert!(fixture.arbiter.admit_mouse(
        &second,
        &mouse(MouseAction::Press(left), 1),
        stamp(Some(current))
    ));
    // The grid changes mid-gesture; motion and the release still count.
    fixture.arbiter.report(&first, Report::Focus(true));
    assert!(fixture.arbiter.admit_mouse(
        &second,
        &mouse(MouseAction::Motion(Some(left)), 2),
        stamp(Some(current))
    ));
    assert!(fixture.arbiter.admit_mouse(
        &second,
        &mouse(MouseAction::Release(left), 2),
        stamp(Some(current))
    ));
}

#[test]
fn finalizing_the_gesture_owner_releases_its_buttons_where_it_left_them() {
    let mut fixture = Fixture::new();
    let owner = fixture.sized(80);
    let other = fixture.sized(80);
    for action in [
        MouseAction::Press(MouseButton::Left),
        MouseAction::Press(MouseButton::Right),
        MouseAction::Motion(Some(MouseButton::Right)),
    ] {
        assert!(fixture.arbiter.admit_mouse(
            &owner,
            &mouse(action, 7),
            stamp(None)
        ));
    }
    owner
        .dropped
        .store(true, std::sync::atomic::Ordering::Release);
    let effects = fixture.arbiter.sync(&fixture.registry);
    assert_eq!(
        effects.releases,
        [
            mouse(MouseAction::Release(MouseButton::Left), 7),
            mouse(MouseAction::Release(MouseButton::Right), 7),
        ]
    );
    assert!(fixture.arbiter.admit_mouse(
        &other,
        &mouse(MouseAction::Press(MouseButton::Left), 1),
        stamp(None)
    ));
}

#[test]
fn typing_returns_to_live_only_without_a_later_scroll_from_the_same_viewer() {
    let mut fixture = Fixture::new();
    let viewer = fixture.sized(80);
    let typed_before_scroll = InputStamp {
        scrolls: 0,
        geometry: None,
    };
    fixture.arbiter.scrolled(&viewer, 1);
    assert!(
        !fixture
            .arbiter
            .may_return_to_live(&viewer, typed_before_scroll)
    );
    assert!(fixture.arbiter.may_return_to_live(
        &viewer,
        InputStamp {
            scrolls: 1,
            geometry: None,
        }
    ));
    let (watcher, _) = fixture.viewer(
        ViewerCapabilities {
            viewport: false,
            ..ViewerCapabilities::ALL
        },
        blank(),
    );
    assert!(
        !fixture
            .arbiter
            .may_return_to_live(&watcher, typed_before_scroll),
        "typing moves the shared viewport only with viewport control"
    );
}

#[test]
fn a_refused_send_does_not_leave_a_viewer_queued() {
    let fixture = Fixture::new();
    let (slot, _) = fixture
        .registry
        .register(None, ViewerCapabilities::ALL, blank())
        .unwrap();
    slot.queue();
    assert!(slot.settled(), "the refusal settles the only message");
    assert_eq!(slot.queued(), 0);
}
