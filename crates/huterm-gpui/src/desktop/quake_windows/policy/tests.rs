use super::*;
fn visible(now: Instant) -> (Model, Facts) {
    let target = Rect {
        x: 0.0,
        y: 0.0,
        width: 800.0,
        height: 400.0,
    };
    let model = Model {
        profile: Profile {
            hide_on_focus_loss: false,
            ..Profile::default()
        },
        display: Display {
            id: "test".into(),
            frame: target,
            work: target,
            primary: true,
        },
        regular: false,
        regular_bounds: target,
        target,
        transition: Transition::new(true, now),
        stage: Stage::Idle,
        deadline: now + Duration::from_secs(3),
        activation: Activation {
            seen: true,
            ..Activation::default()
        },
        suppress_blur: now,
        restore_focus: false,
        fade_supported: true,
        recovering: false,
        detach_requested: false,
        observed_fullscreen: false,
        revision: 0,
        next_reapply: now,
        animation_started: false,
        native_paused: false,
    };
    let facts = Facts {
        active: true,
        blurred: false,
        visible: true,
        fullscreen: false,
        native_idle: true,
        frame: target,
        confirming: false,
        busy: false,
    };
    (model, facts)
}
#[test]
#[cfg(target_os = "macos")]
fn settled_refit_preserves_presentation_and_resizes_without_a_frame() {
    for fullscreen in [false, true] {
        let now = Instant::now();
        let (mut model, mut facts) = visible(now);
        model.profile.fullscreen = fullscreen;
        facts.fullscreen = fullscreen;
        facts.active = false;
        model.target.width += 200.0;
        model.refit(now, true);
        let result = model.decide(facts, now, false).unwrap();
        assert_eq!(result.actions, vec![Action::Frame(model.target)]);
        facts.frame = model.target;
        assert!(model.decide(facts, now, false).unwrap().settled);
    }
}
#[test]
fn refit_without_native_permission_keeps_transition_preparation() {
    let now = Instant::now();
    let (mut model, facts) = visible(now);
    model.refit(now, false);
    assert_eq!(
        model.decide(facts, now, false).unwrap().actions,
        vec![Action::Fullscreen(false)]
    );
}
#[test]
fn refit_during_activation_keeps_show_request_and_original_deadline() {
    let now = Instant::now();
    let (mut model, mut facts) = visible(now);
    model.stage = Stage::Activate;
    model.activation.request();
    facts.active = false;
    let deadline = model.deadline;
    let refit_at = now + Duration::from_secs(1);
    model.refit(refit_at, true);
    assert_eq!(model.deadline, deadline);
    assert_eq!(
        model.decide(facts, refit_at, false).unwrap().actions,
        vec![Action::Fullscreen(false)]
    );
    model.decide(facts, refit_at, false).unwrap();
    let result = model.decide(facts, refit_at, true).unwrap();
    assert!(result.actions.contains(&Action::Show));
}
#[test]
fn reversal_requested_during_native_pause_preserves_progress() {
    let now = Instant::now();
    let (mut model, mut facts) = visible(now);
    model.request(false, false, now);
    model.decide(facts, now, false).unwrap();
    model.decide(facts, now, false).unwrap();
    model.decide(facts, now, false).unwrap();
    let sampled = now + Duration::from_millis(30);
    let progress = model
        .decide(facts, sampled, true)
        .unwrap()
        .sample
        .unwrap()
        .progress;
    facts.native_idle = false;
    model.decide(facts, sampled, false).unwrap();
    let requested = sampled + Duration::from_secs(1);
    model.request(true, false, requested);
    assert_eq!(model.deadline, requested + Duration::from_secs(3));
    facts.native_idle = true;
    let resumed = requested + Duration::from_millis(100);
    model.decide(facts, resumed, false).unwrap();
    model.decide(facts, resumed, false).unwrap();
    let result = model.decide(facts, resumed, true).unwrap();
    assert!(
        (result.sample.unwrap().progress - progress).abs() < f64::EPSILON,
        "reversal sampled paused wall time"
    );
    assert_eq!(model.deadline, requested + Duration::from_secs(3));
}
#[test]
fn settled_sample_at_the_deadline_settles_instead_of_failing() {
    let now = Instant::now();
    let (mut model, mut facts) = visible(now);
    model.stage = Stage::SettleVisible;
    let result = model.decide(facts, model.deadline, false);
    assert!(result.is_ok_and(|decision| decision.settled));
    model.stage = Stage::SettleHidden;
    facts.visible = false;
    let result = model.decide(facts, model.deadline, false);
    assert!(result.is_ok_and(|decision| decision.settled));
    model.stage = Stage::SettleVisible;
    facts.visible = true;
    facts.native_idle = false;
    assert!(model.decide(facts, model.deadline, false).is_err());
    model.stage = Stage::Activate;
    facts.native_idle = true;
    assert!(model.decide(facts, model.deadline, false).is_err());
    model.stage = Stage::SettleVisible;
    facts.frame.x += 50.0;
    assert!(model.decide(facts, model.deadline, false).is_err());
}
#[test]
fn quiet_settlement_requests_detach_continuation_without_native_effect() {
    let now = Instant::now();
    let (mut model, facts) = visible(now);
    model.regular = true;
    model.detach_requested = true;
    model.stage = Stage::SettleVisible;
    let result = model.decide(facts, now, false).unwrap();
    assert!(result.actions.is_empty());
    assert_eq!(model.next_deadline(now), None);
    assert!(
        result.continuation,
        "quiet settlement must schedule the detach pass"
    );
}
#[test]
fn native_pause_resume_preserves_progress_and_failure_deadline() {
    let now = Instant::now();
    let (mut model, mut facts) = visible(now);
    model.request(false, false, now);
    model.decide(facts, now, false).unwrap();
    model.decide(facts, now, false).unwrap();
    model.decide(facts, now, false).unwrap();
    let sampled = now + Duration::from_millis(30);
    let progress = model
        .decide(facts, sampled, true)
        .unwrap()
        .sample
        .unwrap()
        .progress;
    let deadline = model.deadline;
    facts.native_idle = false;
    model.decide(facts, sampled, false).unwrap();
    facts.native_idle = true;
    let resumed = model
        .decide(facts, sampled + Duration::from_secs(1), true)
        .unwrap()
        .sample
        .unwrap()
        .progress;
    assert!(
        (resumed - progress).abs() < f64::EPSILON,
        "native pause advanced animation: {progress} -> {resumed}"
    );
    assert_eq!(model.deadline, deadline);
}
#[test]
fn settled_visible_and_hidden_have_no_policy_deadline() {
    let now = Instant::now();
    let (mut model, mut facts) = visible(now);
    assert!(model.decide(facts, now, false).unwrap().actions.is_empty());
    assert_eq!(model.next_deadline(now), None);
    model.transition = Transition::new(false, now);
    facts.visible = false;
    assert!(model.decide(facts, now, false).unwrap().actions.is_empty());
    assert_eq!(model.next_deadline(now), None);
}
#[test]
fn geometry_events_cannot_bypass_retry_floor_or_extend_failure_deadline() {
    let now = Instant::now();
    let (mut model, mut facts) = visible(now);
    model.stage = Stage::SettleVisible;
    model.next_reapply = now + REAPPLY_INTERVAL;
    facts.frame.x = 50.0;
    let failure = model.deadline;
    for millis in 0..16 {
        assert!(
            model
                .decide(facts, now + Duration::from_millis(millis), false)
                .unwrap()
                .actions
                .is_empty()
        );
    }
    assert_eq!(
        model
            .decide(facts, now + REAPPLY_INTERVAL, false)
            .unwrap()
            .actions,
        vec![Action::Frame(model.target)]
    );
    assert_eq!(model.deadline, failure);
    assert!(model.decide(facts, failure, false).is_err());
}
#[test]
fn activation_deadline_advances_independently_of_geometry_retry() {
    let now = Instant::now();
    let (mut model, mut facts) = visible(now);
    model.stage = Stage::SettleVisible;
    model.activation.seen = false;
    model.activation.next_retry = Some(now + Duration::from_millis(100));
    model.next_reapply = now + Duration::from_millis(112);
    facts.active = false;
    let retry = now + Duration::from_millis(100);
    assert_eq!(model.next_deadline(now), Some(retry));
    assert_eq!(
        model.decide(facts, retry, false).unwrap().actions,
        vec![Action::Show]
    );
    assert_eq!(model.next_deadline(retry), Some(model.next_reapply));
    assert!(model.next_deadline(retry).unwrap() > retry);
}
#[test]
fn notifications_do_not_sample_animation_but_endpoint_does_not_require_frames()
{
    let now = Instant::now();
    let (mut model, facts) = visible(now);
    model.request(false, false, now);
    model.decide(facts, now, false).unwrap();
    model.decide(facts, now, false).unwrap();
    assert!(model.decide(facts, now, false).unwrap().sample.is_some());
    let later = now + Duration::from_millis(50);
    assert!(model.decide(facts, later, false).unwrap().sample.is_none());
    assert!(model.decide(facts, later, true).unwrap().sample.is_some());
    let result = model.decide(facts, model.animation_end(), false).unwrap();
    assert!(result.actions.contains(&Action::Hide(false)));
    assert_eq!(model.stage, Stage::SettleHidden);
}
#[test]
fn paused_native_transition_keeps_only_finite_failure_deadline() {
    let now = Instant::now();
    let (mut model, mut facts) = visible(now);
    model.stage = Stage::Animate;
    facts.native_idle = false;
    model.decide(facts, now, true).unwrap();
    assert_eq!(model.next_deadline(now), Some(model.deadline));
}
#[test]
fn blur_waits_for_suppression_and_cleared_close_blocker() {
    let now = Instant::now();
    let (mut model, mut facts) = visible(now);
    model.profile.hide_on_focus_loss = true;
    model.suppress_blur = now + Duration::from_millis(200);
    facts.active = false;
    facts.blurred = true;
    model.decide(facts, now, false).unwrap();
    assert!(model.transition.visible());
    assert_eq!(model.next_deadline(now), Some(model.suppress_blur));
    facts.busy = true;
    model.decide(facts, model.suppress_blur, false).unwrap();
    assert!(model.transition.visible());
    facts.busy = false;
    model.decide(facts, model.suppress_blur, false).unwrap();
    assert!(!model.transition.visible());
}
#[test]
fn nonactivating_panel_does_not_hide_and_reused_earlier_wake_does_not_finish_reversal()
 {
    let now = Instant::now();
    let (mut model, mut facts) = visible(now);
    model.profile.hide_on_focus_loss = true;
    facts.active = false;
    model.decide(facts, now, false).unwrap();
    assert!(model.transition.visible());
    model.request(false, false, now);
    model.decide(facts, now, false).unwrap();
    model.decide(facts, now, false).unwrap();
    model
        .decide(facts, now + Duration::from_millis(100), true)
        .unwrap();
    let old_end = model.animation_end();
    model.request(true, false, now + Duration::from_millis(100));
    model
        .decide(facts, now + Duration::from_millis(100), false)
        .unwrap();
    model
        .decide(facts, now + Duration::from_millis(100), false)
        .unwrap();
    model
        .decide(facts, now + Duration::from_millis(100), false)
        .unwrap();
    assert!(model.animation_end() > old_end);
    assert!(
        model
            .decide(facts, old_end, false)
            .unwrap()
            .sample
            .is_none()
    );
    assert_eq!(model.stage, Stage::Animate);
}

/// A window model holding one quake window for `policy`, published from
/// the policy model the way `sync_quake_visibility` and `step` do.
fn published(policy: &Model) -> crate::desktop::windows::model::WindowModel {
    use crate::desktop::windows::model::{
        QuakeRecord, WindowLayout, WindowModel,
    };
    let window = gpui::WindowId::from(1);
    let mut windows = WindowModel::default();
    windows.open(
        window,
        Some(QuakeRecord {
            profile: "logs".to_owned(),
            visible: false,
        }),
        WindowLayout {
            bounds: gpui::WindowBounds::Windowed(gpui::Bounds::default()),
            sidebar_width: gpui::px(180.0),
        },
    );
    super::super::publish_visibility(&mut windows, window, policy);
    windows
}

fn published_visibility(policy: &Model) -> Option<bool> {
    published(policy)
        .quake_state("logs")
        .map(|(visible, _)| visible)
}

#[test]
fn summon_and_hide_requests_publish_desired_visibility() {
    let now = Instant::now();
    let (mut model, _) = visible(now);
    model.request(false, false, now);
    assert_eq!(published_visibility(&model), Some(false));
    model.request(true, false, now);
    assert_eq!(published_visibility(&model), Some(true));
}

#[test]
fn auto_hide_on_focus_loss_publishes_hidden() {
    let now = Instant::now();
    let (mut model, mut facts) = visible(now);
    model.profile.hide_on_focus_loss = true;
    facts.active = false;
    facts.blurred = true;
    model.decide(facts, model.suppress_blur, false).unwrap();
    assert_eq!(published_visibility(&model), Some(false));
}

#[test]
fn fullscreen_toggle_and_recovery_publish_visible() {
    let now = Instant::now();
    let (mut model, _) = visible(now);
    model.request(false, false, now);
    // `quake_windows::toggle` flips presentation, then shows the window.
    model.regular = !model.regular;
    model.request(true, false, now);
    assert_eq!(published_visibility(&model), Some(true));

    model.request(false, false, now);
    assert_eq!(published_visibility(&model), Some(false));
    // `recover` restarts a failed transition as visible.
    model.transition = Transition::new(true, now);
    assert_eq!(published_visibility(&model), Some(true));
}
