//! Modifier pane drags under `[panes] drag_modifier`: which modifiers are
//! held, whether the pointer rests where a press would grab a pane, when
//! the pane title badges show, and whether a press grabs. Everything here
//! is pure, driven by the window with the inputs it samples and instants
//! the caller supplies.

use std::time::{Duration, Instant};

use gpui::MouseButton;
use huterm_config::PaneDragModifiers;

use crate::ui::animation::AnimationSchedule;

/// How long the combination stays held before each pane's title badge
/// shows, so a pane keybinding such as Cmd-Control-arrow flashes none.
pub(super) const GRAB_BADGE_DELAY: Duration = Duration::from_millis(150);

/// The held modifiers and what they arm in one window.
#[derive(Debug, Default)]
pub(super) struct PaneGrab {
    /// The modifiers held, as `config::modifier_bits` numbers them. GPUI
    /// overwrites `Window::modifiers` from the macOS Control-left remap,
    /// which clears Control; this record keeps it.
    held: u8,
    /// When `held` last changed.
    changed: Option<Instant>,
    /// A key press or a window activation came since `held` changed, so
    /// the badges stay hidden until it changes again.
    suppressed: bool,
    /// The pointer rests over a pane a press would grab.
    armed: bool,
    /// Whether the badges show, as of the last update.
    badges: bool,
}

impl PaneGrab {
    pub(super) fn held(&self) -> u8 {
        self.held
    }

    /// Records the modifiers held at `now`. Returns whether what the
    /// window draws changed.
    pub(super) fn record(&mut self, held: u8, now: Instant) -> bool {
        if held != self.held {
            self.held = held;
            self.changed = Some(now);
            self.suppressed = false;
        }
        self.tick(now)
    }

    /// Hides the badges until the held modifiers change, as a key press
    /// or a window activation does.
    pub(super) fn suppress(&mut self, now: Instant) -> bool {
        self.suppressed = true;
        self.tick(now)
    }

    /// Sets whether the pointer rests over a pane a press would grab.
    pub(super) fn arm(&mut self, armed: bool, now: Instant) -> bool {
        let changed = std::mem::replace(&mut self.armed, armed) != armed;
        self.tick(now) | changed
    }

    pub(super) fn armed(&self) -> bool {
        self.armed
    }

    pub(super) fn badges(&self) -> bool {
        self.badges
    }

    /// Shows the badges once the delay has passed. Returns whether they
    /// changed.
    pub(super) fn tick(&mut self, now: Instant) -> bool {
        let badges = self.badge_deadline().is_some_and(|at| now >= at);
        std::mem::replace(&mut self.badges, badges) != badges
    }

    /// When the badges show while the pointer stays armed.
    fn badge_deadline(&self) -> Option<Instant> {
        self.changed
            .filter(|_| self.armed && !self.suppressed)
            .map(|at| at + GRAB_BADGE_DELAY)
    }

    /// The deadline at which the badges show, while they wait for it.
    pub(super) fn schedule(&self) -> AnimationSchedule {
        match self.badge_deadline().filter(|_| !self.badges) {
            Some(at) => AnimationSchedule::at(at),
            None => AnimationSchedule::IDLE,
        }
    }
}

/// Control's bit, as `config::modifier_bits` numbers it.
const CONTROL: u8 = 2;

/// The bit a held record gives Fn, which no combination names, so nothing
/// matches while it is held.
const FUNCTION: u8 = 16;

/// `modifiers` as a held record.
pub(super) fn held_bits(modifiers: gpui::Modifiers) -> u8 {
    crate::config::modifier_bits(modifiers).unwrap_or(FUNCTION)
}

/// Whether `keystroke` is a modifier pressed and released alone, which GPUI
/// reports as a keystroke of its own.
pub(super) fn modifier_only(keystroke: &gpui::Keystroke) -> bool {
    matches!(
        keystroke.key.as_str(),
        "shift" | "control" | "alt" | "platform" | "function"
    )
}

/// How a press on a pane's grab layer starts a pane drag.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GrabPress {
    /// A Left press with exactly the combination.
    Direct,
    /// The Right press macOS GPUI makes of a Control-left press, with
    /// Control cleared from its modifiers. Its release may come as Right
    /// or, once Control is let go first, as Left.
    Remapped,
}

/// Whether a `button` press carrying `modifiers` grabs a pane while `held`
/// is the record of held modifiers. On macOS a combination with Control
/// arrives as a Right press without it; only the record, which still holds
/// Control, tells that from a Right press with the other modifiers.
pub(super) fn grab_press(
    button: MouseButton,
    modifiers: Option<u8>,
    held: u8,
    combination: PaneDragModifiers,
    macos: bool,
) -> Option<GrabPress> {
    let modifiers = modifiers?;
    match button {
        MouseButton::Left if combination.matches_bits(modifiers) => {
            Some(GrabPress::Direct)
        }
        MouseButton::Right
            if macos
                && combination.has_ctrl()
                && modifiers == combination.bits() & !CONTROL
                && combination.matches_bits(held) =>
        {
            Some(GrabPress::Remapped)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: u8 = 1;
    const SHIFT: u8 = 8;
    const CMD_CTRL: u8 = CMD | CONTROL;

    fn combination(value: &str) -> PaneDragModifiers {
        PaneDragModifiers::parse(value, true).unwrap()
    }

    #[test]
    fn badges_show_after_the_delay_while_armed_and_hide_on_release() {
        let now = Instant::now();
        let mut grab = PaneGrab::default();
        grab.record(CMD_CTRL, now);
        // Arming shows the open hand at once; the badges wait.
        assert!(grab.arm(true, now));
        assert!(grab.armed() && !grab.badges());
        assert_eq!(
            grab.schedule().deadline,
            Some(now + GRAB_BADGE_DELAY),
            "the delay wakes the window"
        );
        let early = now + GRAB_BADGE_DELAY / 2;
        assert!(!grab.tick(early));
        assert!(grab.tick(now + GRAB_BADGE_DELAY));
        assert!(grab.badges());
        assert_eq!(grab.schedule().deadline, None);
        // Releasing a modifier hides them, and the delay starts again from
        // the next change.
        let released = now + Duration::from_millis(400);
        assert!(grab.record(CMD, released));
        assert!(!grab.badges());
        grab.record(CMD_CTRL, released + Duration::from_millis(10));
        assert_eq!(
            grab.schedule().deadline,
            Some(released + Duration::from_millis(10) + GRAB_BADGE_DELAY)
        );
        // Disarming, as leaving the panes does, hides them too.
        grab.tick(released + Duration::from_secs(1));
        assert!(grab.badges());
        assert!(grab.arm(false, released + Duration::from_secs(1)));
        assert!(!grab.armed() && !grab.badges());
        assert_eq!(grab.schedule().deadline, None);
    }

    #[test]
    fn a_key_or_activation_hides_the_badges_until_the_modifiers_change() {
        let now = Instant::now();
        let mut grab = PaneGrab::default();
        grab.record(CMD_CTRL, now);
        grab.arm(true, now);
        // A pane keybinding pressed with the combination held, inside the
        // delay, shows no badge at all.
        grab.suppress(now + Duration::from_millis(50));
        assert!(!grab.tick(now + Duration::from_secs(1)));
        assert!(!grab.badges() && grab.armed());
        assert_eq!(grab.schedule().deadline, None);
        // Recording the same modifiers again, as a pointer event does,
        // keeps them hidden.
        grab.record(CMD_CTRL, now + Duration::from_secs(2));
        grab.tick(now + Duration::from_secs(3));
        assert!(!grab.badges());
        // A change, even a release and a fresh press, restarts the delay.
        let later = now + Duration::from_secs(4);
        grab.record(CMD, later);
        grab.record(CMD_CTRL, later);
        grab.tick(later + GRAB_BADGE_DELAY);
        assert!(grab.badges());
        // An activation while they show hides them at once.
        assert!(grab.suppress(later + Duration::from_secs(1)));
        assert!(!grab.badges());
    }

    #[test]
    fn a_left_press_grabs_with_exactly_the_combination() {
        let cmd_alt = combination("cmd-alt");
        let press = |button, modifiers| {
            grab_press(button, modifiers, 0, cmd_alt, false)
        };
        assert_eq!(
            press(MouseButton::Left, Some(CMD | 4)),
            Some(GrabPress::Direct)
        );
        assert_eq!(press(MouseButton::Left, Some(CMD)), None);
        assert_eq!(press(MouseButton::Left, Some(CMD | 4 | SHIFT)), None);
        // Fn held, which no setting names, never grabs.
        assert_eq!(press(MouseButton::Left, None), None);
        assert_eq!(press(MouseButton::Right, Some(CMD | 4)), None);
        assert_eq!(press(MouseButton::Middle, Some(CMD | 4)), None);
    }

    #[test]
    fn a_remapped_control_press_grabs_only_while_control_is_recorded() {
        let cmd_ctrl = combination("cmd-ctrl");
        // macOS turns Cmd-Control-left into a Right press carrying Cmd.
        assert_eq!(
            grab_press(MouseButton::Right, Some(CMD), CMD_CTRL, cmd_ctrl, true),
            Some(GrabPress::Remapped)
        );
        // A physical Cmd-right-click: the pointer events since Control was
        // let go recorded Cmd alone, so it stays a right-click.
        assert_eq!(
            grab_press(MouseButton::Right, Some(CMD), CMD, cmd_ctrl, true),
            None
        );
        // Linux has no remap, and its Right presses never grab.
        assert_eq!(
            grab_press(
                MouseButton::Right,
                Some(CMD),
                CMD_CTRL,
                cmd_ctrl,
                false
            ),
            None
        );
        // A combination without Control has no remapped press.
        let cmd_shift = combination("cmd-shift");
        assert_eq!(
            grab_press(
                MouseButton::Right,
                Some(CMD | SHIFT),
                CMD | SHIFT,
                cmd_shift,
                true
            ),
            None
        );
        // Extra modifiers on the remapped press do not match.
        assert_eq!(
            grab_press(
                MouseButton::Right,
                Some(CMD | SHIFT),
                CMD_CTRL | SHIFT,
                cmd_ctrl,
                true
            ),
            None
        );
    }
}
