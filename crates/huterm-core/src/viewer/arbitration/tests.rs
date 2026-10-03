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
        let (slot, _) = self
            .registry
            .register_with_wake(None, capabilities, initial)
            .unwrap();
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
fn a_revoked_viewer_cannot_take_control_before_it_is_reconciled() {
    let mut fixture = Fixture::new();
    let revoked = fixture.sized(120);
    let leaving = fixture.sized(100);
    fixture.arbiter.report(&revoked, Report::Focus(true));
    fixture.arbiter.report(&leaving, Report::Focus(true));
    assert_eq!(fixture.arbiter.canonical.0, grid(100));
    // Revoked on another thread after this turn reconciled, then the
    // controller is finalized after its last queued message.
    revoked
        .revoked
        .store(true, std::sync::atomic::Ordering::Release);
    leaving.queue();
    leaving
        .dropped
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(leaving.settled());
    let effects = fixture.arbiter.settled(&leaving, &fixture.registry);
    assert_eq!(effects.resize, None, "the revoked viewer took control");
    assert_eq!(fixture.arbiter.canonical.0, grid(100));
}

#[test]
fn a_dropped_viewer_cannot_take_control_before_it_is_reconciled() {
    let mut fixture = Fixture::new();
    let controller = fixture.sized(100);
    let closing = fixture.sized(120);
    fixture.arbiter.report(&controller, Report::Focus(true));
    assert_eq!(fixture.arbiter.canonical.0, grid(100));
    // Dropped after this turn reconciled, with input and a report queued.
    closing.queue();
    closing
        .dropped
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(fixture.arbiter.typed(&closing).resize.is_none());
    assert!(
        fixture
            .arbiter
            .report(&closing, Report::Focus(true))
            .resize
            .is_none()
    );
    assert_eq!(fixture.arbiter.canonical.0, grid(100));
}

#[test]
fn a_gesture_the_application_stopped_tracking_blocks_no_one_but_ends_on_release()
 {
    let mut fixture = Fixture::new();
    let owner = fixture.sized(80);
    let other = fixture.sized(80);
    let left = MouseButton::Left;
    let admit = |fixture: &mut Fixture, slot: &Slot, action| {
        fixture
            .arbiter
            .admit_mouse(slot, &mouse(action, 1), stamp(None), true)
    };
    assert!(admit(&mut fixture, &owner, MouseAction::Press(left)));
    assert!(fixture.arbiter.gesture_held());
    fixture.arbiter.tracking_disabled();
    assert!(!fixture.arbiter.gesture_held());
    // A view that never saw tracking stop still sends its release, which
    // the application, tracking again, must receive.
    assert!(admit(&mut fixture, &owner, MouseAction::Release(left)));

    assert!(admit(&mut fixture, &owner, MouseAction::Press(left)));
    fixture.arbiter.tracking_disabled();
    // Another viewer's press starts a new gesture; the abandoned owner can
    // neither end it nor get synthetic releases.
    assert!(admit(&mut fixture, &other, MouseAction::Press(left)));
    assert!(!admit(&mut fixture, &owner, MouseAction::Release(left)));
    owner
        .dropped
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(fixture.arbiter.sync(&fixture.registry).releases.is_empty());
    assert!(fixture.arbiter.gesture_held(), "the new gesture remains");
}

#[test]
fn removing_a_viewer_the_arbiter_never_met_still_releases_its_gesture() {
    let mut fixture = Fixture::new();
    let (unmet, _) = fixture
        .registry
        .register_with_wake(None, ViewerCapabilities::ALL, blank())
        .unwrap();
    let left = MouseButton::Left;
    assert!(fixture.arbiter.admit_mouse(
        &unmet,
        &mouse(MouseAction::Press(left), 3),
        stamp(None),
        true
    ));
    unmet
        .dropped
        .store(true, std::sync::atomic::Ordering::Release);
    let effects = fixture.arbiter.sync(&fixture.registry);
    assert_eq!(effects.releases, vec![mouse(MouseAction::Release(left), 3)]);
    assert!(fixture.registry.slots().is_empty());
    assert!(!fixture.arbiter.gesture_held());
}

#[test]
fn pressing_an_untracked_button_again_retires_its_earlier_press() {
    let mut fixture = Fixture::new();
    let owner = fixture.sized(80);
    let admit = |fixture: &mut Fixture, action| {
        fixture.arbiter.admit_mouse(
            &owner,
            &mouse(action, 1),
            stamp(None),
            true,
        )
    };
    let left = MouseButton::Left;
    // The view saw tracking stop and never sent the first release.
    for _ in 0..3 {
        assert!(admit(&mut fixture, MouseAction::Press(left)));
        fixture.arbiter.tracking_disabled();
    }
    assert!(admit(&mut fixture, MouseAction::Press(left)));
    assert!(admit(&mut fixture, MouseAction::Release(left)));
    // Nothing is left to release: one press, however often it restarted.
    assert!(!admit(&mut fixture, MouseAction::Release(left)));
    owner
        .dropped
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(fixture.arbiter.sync(&fixture.registry).releases.is_empty());
}

#[test]
fn an_owner_s_new_press_keeps_its_untracked_buttons_releasable() {
    let mut fixture = Fixture::new();
    let owner = fixture.sized(80);
    let other = fixture.sized(80);
    let admit = |fixture: &mut Fixture, slot: &Slot, action| {
        fixture
            .arbiter
            .admit_mouse(slot, &mouse(action, 1), stamp(None), true)
    };
    let (left, right) = (MouseButton::Left, MouseButton::Right);
    assert!(admit(&mut fixture, &owner, MouseAction::Press(left)));
    fixture.arbiter.tracking_disabled();
    // Tracking is back before the owner's view noticed it stopped, so it
    // presses again while still holding Left.
    assert!(admit(&mut fixture, &owner, MouseAction::Press(right)));
    assert!(
        !admit(&mut fixture, &other, MouseAction::Press(left)),
        "the new press blocks other viewers"
    );
    assert!(admit(&mut fixture, &owner, MouseAction::Release(left)));
    assert!(admit(&mut fixture, &owner, MouseAction::Release(right)));
    assert!(!fixture.arbiter.gesture_held());
    assert!(admit(&mut fixture, &other, MouseAction::Press(left)));
}

#[test]
fn a_refused_press_leaves_an_untracked_gesture_to_its_owner() {
    let mut fixture = Fixture::new();
    let owner = fixture.sized(80);
    let other = fixture.sized(80);
    let left = MouseButton::Left;
    assert!(fixture.arbiter.admit_mouse(
        &owner,
        &mouse(MouseAction::Press(left), 1),
        stamp(None),
        true
    ));
    fixture.arbiter.tracking_disabled();
    // A press computed against an older grid is refused, and must not
    // discard the gesture whose release is still coming.
    let stale = fixture.arbiter.geometry_revision() + 1;
    assert!(!fixture.arbiter.admit_mouse(
        &other,
        &mouse(MouseAction::Press(left), 2),
        stamp(Some(stale)),
        true
    ));
    assert!(fixture.arbiter.admit_mouse(
        &owner,
        &mouse(MouseAction::Release(left), 1),
        stamp(None),
        true
    ));
}

#[test]
fn other_viewers_never_move_an_untracked_gesture_s_release() {
    let mut fixture = Fixture::new();
    let owner = fixture.sized(80);
    let other = fixture.sized(80);
    let left = MouseButton::Left;
    assert!(fixture.arbiter.admit_mouse(
        &owner,
        &mouse(MouseAction::Press(left), 1),
        stamp(None),
        true
    ));
    fixture.arbiter.tracking_disabled();
    assert!(fixture.arbiter.admit_mouse(
        &other,
        &mouse(MouseAction::Motion(None), 9),
        stamp(None),
        true
    ));
    owner
        .dropped
        .store(true, std::sync::atomic::Ordering::Release);
    let releases = fixture.arbiter.sync(&fixture.registry).releases;
    assert_eq!(releases, vec![mouse(MouseAction::Release(left), 1)]);
}

#[test]
fn a_repeated_focus_gain_is_activity() {
    let mut fixture = Fixture::new();
    let first = fixture.sized(100);
    let second = fixture.sized(120);
    fixture.arbiter.report(&first, Report::Focus(true));
    fixture.arbiter.report(&second, Report::Focus(true));
    assert_eq!(fixture.arbiter.canonical.0, grid(120));
    // The first view blurred and refocused while its blur was queued, so
    // the runtime sees one more gain from an already focused viewer.
    let effects = fixture.arbiter.report(&first, Report::Focus(true));
    assert_eq!(effects.resize, Some((grid(100), cell())));
    assert_eq!(effects.focus, None, "the application stays focused");
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
    let focus = |fixture: &mut Fixture, slot: &Arc<Slot>, focused| {
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
        fixture.arbiter.admit_mouse(
            slot,
            &mouse(action, column),
            stamp(None),
            true,
        )
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
                stamp(Some(stale)),
                true
            ),
            "{action:?}"
        );
    }
    assert!(fixture.arbiter.admit_mouse(
        &second,
        &mouse(MouseAction::Press(left), 1),
        stamp(Some(current)),
        true
    ));
    // The grid changes mid-gesture; motion and the release still count.
    assert!(fixture.arbiter.typed(&first).resize.is_some());
    assert!(fixture.arbiter.geometry_revision() > current);
    assert!(fixture.arbiter.admit_mouse(
        &second,
        &mouse(MouseAction::Motion(Some(left)), 2),
        stamp(Some(current)),
        true
    ));
    assert!(fixture.arbiter.admit_mouse(
        &second,
        &mouse(MouseAction::Release(left), 2),
        stamp(Some(current)),
        true
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
            stamp(None),
            true
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
        stamp(None),
        true
    ));
}

#[test]
fn typing_returns_to_live_only_without_a_later_scroll_from_the_same_viewer() {
    let fixture = Fixture::new();
    // The bookkeeping lives on the slot, so it holds before the owner
    // thread reconciles a viewer: neither slot is ever synced here.
    let register = |capabilities| {
        fixture
            .registry
            .register_with_wake(None, capabilities, blank())
            .unwrap()
            .0
    };
    let viewer = register(ViewerCapabilities::ALL);
    let typed_before_scroll = InputStamp {
        scrolls: 0,
        geometry: None,
    };
    viewer.scrolled(1);
    assert!(!viewer.return_to_live(typed_before_scroll));
    assert!(!viewer.scroll_superseded(1));
    assert!(viewer.return_to_live(InputStamp {
        scrolls: 2,
        geometry: None,
    }));
    // Scrolls sent before that typing can no longer move the viewport.
    assert!(viewer.scroll_superseded(2));
    assert!(!viewer.scroll_superseded(3));
    let watcher = register(ViewerCapabilities {
        viewport: false,
        ..ViewerCapabilities::ALL
    });
    assert!(
        !watcher.return_to_live(typed_before_scroll),
        "typing moves the shared viewport only with viewport control"
    );
}

#[test]
fn initial_focus_needs_input_or_size() {
    let mut fixture = Fixture::new();
    let focused = Initial {
        focused: true,
        geometry: None,
        presentation: None,
    };
    let (effects_only, effects) = fixture.viewer(
        ViewerCapabilities {
            host_effects: true,
            ..ViewerCapabilities::NONE
        },
        focused,
    );
    assert!(effects.is_empty());
    assert_eq!(
        effects_only
            .activity
            .load(std::sync::atomic::Ordering::Acquire),
        0,
        "no activity to outrank other host-effect recipients"
    );
}
