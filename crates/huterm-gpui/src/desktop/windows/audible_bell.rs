//! The audible bell under `[terminal.bell] audio`: at most one sound per
//! [`BELL_THROTTLE`] across every terminal, played through a sink on the
//! `Desktop`, so tests install a recording sink and nothing plays.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::App;
use huterm_config::{BellConfig, BellSound};

use super::Desktop;

/// The shortest gap between two bell sounds, across every terminal.
pub(super) const BELL_THROTTLE: Duration = Duration::from_millis(200);

/// Plays bell sounds.
pub(super) trait BellSink {
    /// Plays `sound` at `volume`, from 0 to 1, and returns at once.
    fn play(&self, sound: BellSound, volume: f32);
}

/// Admits at most one sound per [`BELL_THROTTLE`].
#[derive(Debug, Default)]
pub(super) struct BellThrottle {
    last: Option<Instant>,
}

impl BellThrottle {
    /// Whether a sound may play at `now`, which then starts a new gap.
    pub(super) fn admit(&mut self, now: Instant) -> bool {
        if self.last.is_some_and(|last| {
            now.saturating_duration_since(last) < BELL_THROTTLE
        }) {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// The desktop's audible bell: its sink, its throttle, and the clock the
/// throttle reads.
pub(super) struct AudibleBell {
    sink: Rc<dyn BellSink>,
    throttle: BellThrottle,
    clock: Rc<dyn Fn() -> Instant>,
}

impl AudibleBell {
    /// A bell on the platform's sink: `AppKit` on macOS, and silence on
    /// Linux and in tests.
    pub(super) fn platform() -> Self {
        #[cfg(target_os = "macos")]
        let sink: Rc<dyn BellSink> = if cfg!(test) {
            Rc::new(Silent)
        } else {
            Rc::new(AppKitSink::default())
        };
        #[cfg(not(target_os = "macos"))]
        let sink: Rc<dyn BellSink> = Rc::new(Silent);
        Self::with_clock(sink, Rc::new(Instant::now))
    }

    pub(super) fn with_clock(
        sink: Rc<dyn BellSink>,
        clock: Rc<dyn Fn() -> Instant>,
    ) -> Self {
        Self {
            sink,
            throttle: BellThrottle::default(),
            clock,
        }
    }

    fn ring(&mut self, bell: BellConfig) {
        if bell.audio && self.throttle.admit((self.clock)()) {
            self.sink.play(bell.sound, bell.volume);
        }
    }
}

/// Plays the bell sound the configuration asks for once the current update
/// returns, so a terminal's snapshot update never waits on playback.
pub(in crate::desktop) fn ring(cx: &mut App) {
    cx.defer(|cx| {
        if !cx.has_global::<Desktop>() {
            return;
        }
        let desktop = cx.global_mut::<Desktop>();
        let bell = desktop.config.terminal.bell;
        desktop.audible_bell.ring(bell);
    });
}

/// Plays nothing.
struct Silent;

impl BellSink for Silent {
    fn play(&self, _: BellSound, _: f32) {}
}

/// Plays through `AppKit`: the alert sound through `NSBeep`, and named
/// sounds through a cached `NSSound`. A named sound that does not load
/// falls back to the alert sound and is logged once.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct AppKitSink {
    sounds: std::cell::RefCell<crate::native_quit::sound::SystemSounds>,
    failed: std::cell::RefCell<std::collections::HashSet<BellSound>>,
}

#[cfg(target_os = "macos")]
impl BellSink for AppKitSink {
    fn play(&self, sound: BellSound, volume: f32) {
        let Some(name) = sound.system_sound_name() else {
            crate::native_quit::sound::beep();
            return;
        };
        if !self.sounds.borrow_mut().play(name, volume) {
            if self.failed.borrow_mut().insert(sound) {
                eprintln!(
                    "Huterm could not load the {name} bell sound; playing \
                     the alert sound instead"
                );
            }
            crate::native_quit::sound::beep();
        }
    }
}

/// A sink that records what it was asked to play.
#[cfg(test)]
#[derive(Default)]
pub(super) struct RecordingSink {
    pub(super) played: std::cell::RefCell<Vec<(BellSound, f32)>>,
}

#[cfg(test)]
impl BellSink for RecordingSink {
    fn play(&self, sound: BellSound, volume: f32) {
        self.played.borrow_mut().push((sound, volume));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn the_throttle_admits_one_sound_per_gap() {
        let start = Instant::now();
        let mut throttle = BellThrottle::default();
        assert!(throttle.admit(start));
        assert!(!throttle.admit(start));
        assert!(!throttle.admit(start + Duration::from_millis(199)));
        assert!(throttle.admit(start + BELL_THROTTLE));
        // A refused bell does not extend the gap.
        assert!(
            !throttle.admit(start + BELL_THROTTLE + Duration::from_millis(100))
        );
        assert!(throttle.admit(start + BELL_THROTTLE * 2));
    }

    #[test]
    fn a_bell_plays_its_sound_only_when_audio_is_on_and_the_clock_allows() {
        let sink = Rc::new(RecordingSink::default());
        let start = Instant::now();
        let now = Rc::new(Cell::new(start));
        let clock = Rc::clone(&now);
        let mut bell =
            AudibleBell::with_clock(sink.clone(), Rc::new(move || clock.get()));
        let config = BellConfig {
            audio: true,
            sound: BellSound::Glass,
            volume: 0.25,
            ..BellConfig::default()
        };
        bell.ring(BellConfig {
            audio: false,
            ..config
        });
        bell.ring(config);
        bell.ring(config);
        now.set(start + BELL_THROTTLE);
        bell.ring(BellConfig {
            sound: BellSound::System,
            ..config
        });
        assert_eq!(
            *sink.played.borrow(),
            [(BellSound::Glass, 0.25), (BellSound::System, 0.25)]
        );
    }
}
