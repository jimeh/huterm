//! The macOS alert sound and named system sounds, for the audible bell.

use std::collections::HashMap;
use std::ffi::CString;

use objc::rc::StrongPtr;
use objc::runtime::{BOOL, Class, Object};
use objc::{msg_send, sel, sel_impl};

use super::is_main_thread;

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn NSBeep();
}

/// Plays the user's alert sound at the macOS alert volume. It returns at
/// once; off the main thread it plays nothing.
pub(crate) fn beep() {
    if is_main_thread().unwrap_or(false) {
        // SAFETY: NSBeep takes no arguments and runs on the main thread.
        unsafe { NSBeep() };
    }
}

/// Named sounds from `/System/Library/Sounds`, each loaded once and kept,
/// whether it loaded or not.
#[derive(Default)]
pub(crate) struct SystemSounds {
    loaded: HashMap<&'static str, Option<StrongPtr>>,
}

impl SystemSounds {
    /// Plays the sound called `name` at `volume`, from 0 to 1, and returns
    /// at once. `false` when the sound does not load or this is not the
    /// main thread.
    pub(crate) fn play(&mut self, name: &'static str, volume: f32) -> bool {
        if !is_main_thread().unwrap_or(false) {
            return false;
        }
        let Some(sound) = self.loaded.entry(name).or_insert_with(|| load(name))
        else {
            return false;
        };
        // SAFETY: `sound` is a retained NSSound used on the main thread.
        // `play` returns NO while the sound is still playing, which leaves
        // it playing.
        unsafe {
            let _: () = msg_send![**sound, setVolume: volume];
            let _: BOOL = msg_send![**sound, play];
        }
        true
    }
}

/// The retained `NSSound` named `name`, or `None` when it does not load.
fn load(name: &str) -> Option<StrongPtr> {
    let name = CString::new(name).ok()?;
    let string_class = Class::get("NSString")?;
    let sound_class = Class::get("NSSound")?;
    // SAFETY: stringWithUTF8String copies the bytes into an autoreleased
    // NSString; soundNamed returns an autoreleased NSSound or nil, which is
    // retained before the pool drains.
    unsafe {
        let string: *mut Object =
            msg_send![string_class, stringWithUTF8String: name.as_ptr()];
        if string.is_null() {
            return None;
        }
        let sound: *mut Object = msg_send![sound_class, soundNamed: string];
        (!sound.is_null()).then(|| StrongPtr::retain(sound))
    }
}
