use std::sync::Arc;

use huterm_protocol::{
    CellSize, GridSize, InputStamp, MAX_GRID_CELLS, Modifiers, MouseAction,
    MouseButton, MouseInput, MousePosition, Rgb, RuntimeId, SizingPolicy,
    TerminalId, TerminalPresentation, ViewerCapabilities, ViewerId,
    WheelDirection,
};

use super::{
    Arbiter, Candidate, Effects, ManualSize, Report, select_controller,
    select_size,
};
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
        id: ViewerId::new(id),
        activity,
        order,
        geometry: (grid(80), cell()),
    }
}

fn controller(candidates: &[Candidate]) -> Option<ViewerId> {
    select_controller(candidates).map(|candidate| candidate.id)
}

#[test]
fn latest_controller_follows_activity_then_registration_order() {
    assert_eq!(controller(&[]), None);
    assert_eq!(
        controller(&[candidate(1, 5, 1), candidate(2, 3, 2)]),
        Some(ViewerId::new(1))
    );
    assert_eq!(
        controller(&[candidate(1, 0, 1), candidate(2, 0, 2)]),
        Some(ViewerId::new(2)),
        "ties go to the latest registration"
    );
}

#[test]
fn each_policy_sizes_differently_sized_viewers_deterministically() {
    let sized = |id, activity, columns, rows, width, height| Candidate {
        geometry: (
            GridSize::clamped(columns, rows),
            CellSize { width, height },
        ),
        ..candidate(id, activity, id)
    };
    // The most recently active viewer is neither the smallest nor the
    // largest in either dimension, and its cell is the narrowest.
    let wide = sized(1, 1, 200, 20, 16, 32);
    let tall = sized(2, 0, 60, 70, 8, 34);
    let active = sized(3, 2, 100, 40, 7, 14);
    for candidates in [[wide, tall, active], [active, tall, wide]] {
        assert_eq!(
            select_size(SizingPolicy::Latest, &candidates),
            Some((
                GridSize::clamped(100, 40),
                CellSize {
                    width: 7,
                    height: 14
                }
            ))
        );
        let densest = CellSize {
            width: 16,
            height: 34,
        };
        assert_eq!(
            select_size(SizingPolicy::Smallest, &candidates),
            Some((GridSize::clamped(60, 20), densest)),
            "columns and rows aggregate separately; the cell is the largest"
        );
        assert_eq!(
            select_size(SizingPolicy::Largest, &candidates),
            Some((GridSize::clamped(200, 70), densest))
        );
        assert_eq!(select_size(SizingPolicy::Manual, &candidates), None);
    }
    for policy in [
        SizingPolicy::Latest,
        SizingPolicy::Smallest,
        SizingPolicy::Largest,
        SizingPolicy::Manual,
    ] {
        assert_eq!(select_size(policy, &[]), None, "{policy:?} keeps size");
    }
}

#[test]
fn the_largest_policy_keeps_the_grid_within_the_cell_limit() {
    // Each grid fits the limit, but the widest and the tallest together do
    // not.
    let wide = Candidate {
        geometry: (GridSize::clamped(4000, 200), cell()),
        ..candidate(1, 0, 1)
    };
    let tall = Candidate {
        geometry: (GridSize::clamped(200, 4000), cell()),
        ..candidate(2, 0, 2)
    };
    let (grid, _) = select_size(SizingPolicy::Largest, &[wide, tall]).unwrap();
    assert!(grid.cells() <= MAX_GRID_CELLS);
    assert_eq!(grid.columns, grid.rows, "the clamp keeps the aspect");
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
    assert_eq!(fixture.arbiter.sync(&fixture.registry).releases, []);
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
    assert_eq!(fixture.arbiter.sync(&fixture.registry).releases, []);
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
fn held_button_motion_needs_a_gesture_its_viewer_owns() {
    let mut fixture = Fixture::new();
    let first = fixture.sized(80);
    let second = fixture.sized(90);
    let stale = fixture.arbiter.geometry_revision();
    fixture.arbiter.report(&first, Report::Focus(true));
    fixture.arbiter.report(&second, Report::Focus(true));
    let current = fixture.arbiter.geometry_revision();
    assert!(current > stale);
    let mut admit = |slot: &Slot, action, geometry| {
        fixture.arbiter.admit_mouse(
            slot,
            &mouse(action, 1),
            stamp(Some(geometry)),
            true,
        )
    };
    let left = MouseButton::Left;
    // A refused press leaves its view dragging; the application never saw
    // the press, so it must not see the drag.
    assert!(!admit(&second, MouseAction::Press(left), stale));
    assert!(!admit(&second, MouseAction::Motion(Some(left)), current));
    assert!(admit(&second, MouseAction::Motion(None), current));
    // After the owner's gesture ends, its drag motion is refused too.
    assert!(admit(&first, MouseAction::Press(left), current));
    assert!(admit(&first, MouseAction::Release(left), current));
    assert!(!admit(&first, MouseAction::Motion(Some(left)), current));
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

fn set_policy(
    fixture: &mut Fixture,
    slot: &Slot,
    policy: SizingPolicy,
) -> Effects {
    fixture.arbiter.report(slot, Report::Policy(policy))
}

#[test]
fn smallest_and_largest_ignore_focus_and_typing() {
    for (policy, expected) in [
        (SizingPolicy::Smallest, grid(100)),
        (SizingPolicy::Largest, grid(120)),
    ] {
        let mut fixture = Fixture::new();
        let first = fixture.sized(100);
        let second = fixture.sized(120);
        let effects = set_policy(&mut fixture, &first, policy);
        assert_eq!(
            effects.resize.map(|(grid, _)| grid),
            (expected != grid(120)).then_some(expected),
            "{policy:?} takes effect when it is set"
        );
        assert_eq!(fixture.arbiter.canonical.0, expected);
        let revision = fixture.arbiter.geometry_revision();
        for effects in [
            fixture.arbiter.report(&first, Report::Focus(true)),
            fixture.arbiter.typed(&second),
            fixture.arbiter.report(&second, Report::Focus(true)),
            fixture.arbiter.report(&second, Report::Focus(false)),
            fixture.arbiter.typed(&first),
        ] {
            assert_eq!(effects.resize, None, "{policy:?}");
        }
        assert_eq!(fixture.arbiter.geometry_revision(), revision);
    }
}

#[test]
fn smallest_follows_every_participant_s_geometry_and_departure() {
    let mut fixture = Fixture::new();
    let first = fixture.sized(100);
    let second = fixture.sized(120);
    set_policy(&mut fixture, &first, SizingPolicy::Smallest);
    // A viewer that has not focused or typed still constrains the size.
    let effects = fixture
        .arbiter
        .report(&second, Report::Geometry(grid(90), cell()));
    assert_eq!(effects.resize, Some((grid(90), cell())));
    // A viewer without `size` and one that has reported no geometry take
    // no part.
    let (_, effects) = fixture.viewer(
        ViewerCapabilities {
            input: true,
            ..ViewerCapabilities::NONE
        },
        Initial {
            focused: true,
            geometry: Some((grid(10), cell())),
            presentation: None,
        },
    );
    assert_eq!(effects.resize, None);
    let (_, effects) = fixture.viewer(ViewerCapabilities::ALL, blank());
    assert_eq!(effects.resize, None);
    // Dropping the smallest viewer grows the terminal at once.
    second
        .dropped
        .store(true, std::sync::atomic::Ordering::Release);
    let effects = fixture.arbiter.sync(&fixture.registry);
    assert_eq!(effects.resize, Some((grid(100), cell())));
}

#[test]
fn presentation_follows_the_latest_viewer_under_every_policy() {
    let presentation = |red| TerminalPresentation {
        background: Rgb {
            red,
            green: 0,
            blue: 0,
        },
        ..TerminalPresentation::default()
    };
    for policy in [
        SizingPolicy::Latest,
        SizingPolicy::Smallest,
        SizingPolicy::Largest,
        SizingPolicy::Manual,
    ] {
        let mut fixture = Fixture::new();
        let first = fixture.sized(100);
        let second = fixture.sized(120);
        // Typed in, so `manual` no longer follows the views' sizes.
        fixture.arbiter.typed(&second);
        set_policy(&mut fixture, &first, policy);
        fixture.arbiter.presentation(&first, presentation(1));
        let effects = fixture.arbiter.presentation(&second, presentation(2));
        assert_eq!(effects.presentation, Some(presentation(2)), "{policy:?}");
        let effects = fixture.arbiter.report(&first, Report::Focus(true));
        assert_eq!(effects.presentation, Some(presentation(1)), "{policy:?}");
    }
}

#[test]
fn the_manual_policy_keeps_the_size_until_one_is_set() {
    let mut fixture = Fixture::new();
    let first = fixture.sized(100);
    let second = fixture.sized(120);
    // A terminal someone has typed in has settled.
    fixture.arbiter.typed(&second);
    let effects = set_policy(&mut fixture, &first, SizingPolicy::Manual);
    assert_eq!(effects.resize, None, "the current size stays");
    for effects in [
        fixture.arbiter.report(&first, Report::Focus(true)),
        fixture.arbiter.typed(&first),
        fixture
            .arbiter
            .report(&second, Report::Geometry(grid(60), cell())),
    ] {
        assert_eq!(effects.resize, None);
    }
    assert_eq!(fixture.arbiter.canonical.0, grid(120));
    // Returning to a viewer-driven policy resizes at once.
    let effects = set_policy(&mut fixture, &second, SizingPolicy::Smallest);
    assert_eq!(effects.resize, Some((grid(60), cell())));
}

#[test]
fn a_manual_size_overrides_every_policy_until_it_is_cleared() {
    let mut fixture = Fixture::new();
    let first = fixture.sized(100);
    let second = fixture.sized(120);
    let large = CellSize {
        width: 16,
        height: 32,
    };
    fixture
        .arbiter
        .report(&second, Report::Geometry(GridSize::clamped(120, 30), large));
    // An omitted dimension and the cell size come from the requesting
    // viewer's own geometry.
    let effects = fixture.arbiter.report(
        &first,
        Report::Manual(Some(ManualSize {
            columns: Some(70),
            rows: None,
        })),
    );
    assert_eq!(effects.resize, Some((GridSize::clamped(70, 24), cell())));
    for effects in [
        fixture.arbiter.report(&second, Report::Focus(true)),
        set_policy(&mut fixture, &second, SizingPolicy::Largest),
        fixture
            .arbiter
            .report(&first, Report::Geometry(grid(200), cell())),
    ] {
        assert_eq!(effects.resize, None, "the manual size holds");
    }
    // A second request replaces the first.
    let effects = fixture.arbiter.report(
        &second,
        Report::Manual(Some(ManualSize {
            columns: None,
            rows: Some(10),
        })),
    );
    assert_eq!(effects.resize, Some((GridSize::clamped(120, 10), large)));
    // Clearing returns to the policy, which the viewers' latest geometry
    // decides.
    let effects = fixture.arbiter.report(&first, Report::Manual(None));
    assert_eq!(effects.resize, Some((GridSize::clamped(200, 30), large)));
}

#[test]
fn a_manual_size_from_a_viewer_without_geometry_keeps_the_canonical_rest() {
    let mut fixture = Fixture::new();
    let (unsized_viewer, _) = fixture.viewer(ViewerCapabilities::ALL, blank());
    let effects = fixture.arbiter.report(
        &unsized_viewer,
        Report::Manual(Some(ManualSize {
            columns: None,
            rows: Some(5),
        })),
    );
    assert_eq!(effects.resize, Some((GridSize::clamped(80, 5), cell())));
}

#[test]
fn the_manual_policy_follows_views_until_the_terminal_is_typed_in() {
    let mut fixture = Fixture::new();
    // The view that spawned the terminal starts with an estimate, and its
    // window can lay out more than once before it settles, as a Quake
    // window does at the regular size before its frame.
    let view = fixture.sized(60);
    set_policy(&mut fixture, &view, SizingPolicy::Manual);
    for columns in [100, 120] {
        let effects = fixture
            .arbiter
            .report(&view, Report::Geometry(grid(columns), cell()));
        assert_eq!(effects.resize, Some((grid(columns), cell())));
    }
    // Typing in another view moves control and resizes first, then the
    // terminal keeps that size. Focus alone settles nothing.
    fixture.arbiter.report(&view, Report::Focus(true));
    let other = fixture.sized(90);
    let effects = fixture.arbiter.typed(&other);
    assert_eq!(effects.resize, Some((grid(90), cell())));
    for effects in [
        fixture
            .arbiter
            .report(&other, Report::Geometry(grid(70), cell())),
        fixture.arbiter.typed(&view),
    ] {
        assert_eq!(effects.resize, None);
    }
}
