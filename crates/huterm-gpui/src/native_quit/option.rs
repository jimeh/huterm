//! Caller-owned Option dead keys, without starting `AppKit` marked text.

use std::ffi::c_void;

use anyhow::ensure;
use objc::{msg_send, sel, sel_impl};

use super::{Object, application, is_main_thread};

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> *const c_void;
    fn TISGetInputSourceProperty(
        source: *const c_void,
        property: *const c_void,
    ) -> *const c_void;
    static kTISPropertyUnicodeKeyLayoutData: *const c_void;
    fn LMGetKbdType() -> u8;
    fn UCKeyTranslate(
        layout: *const c_void,
        key: u16,
        action: u16,
        modifiers: u32,
        keyboard: u32,
        options: u32,
        state: *mut u32,
        capacity: usize,
        length: *mut usize,
        output: *mut u16,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: *const c_void);
    fn CFEqual(left: *const c_void, right: *const c_void) -> u8;
    fn CFDataGetBytePtr(value: *const c_void) -> *const u8;
}

/// Owns a Copy-rule reference; the raw pointer keeps it on the UI thread.
struct InputSource(*const c_void);

impl Drop for InputSource {
    fn drop(&mut self) {
        // SAFETY: This wrapper owns one non-null Copy-rule reference.
        unsafe { CFRelease(self.0) };
    }
}

#[derive(Default)]
pub(crate) struct OptionComposition {
    source: Option<InputSource>,
    dead_key: u32,
    pending: bool,
}

impl OptionComposition {
    pub(crate) fn clear(&mut self) {
        self.dead_key = 0;
        self.pending = false;
        self.source = None;
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending
    }

    /// Invalidates old state before special-key cancellation as well as text.
    pub(crate) fn refresh_source(&mut self) -> anyhow::Result<()> {
        if !self.is_pending() {
            return Ok(());
        }
        ensure!(
            is_main_thread()?,
            "Option composition requires the main thread"
        );
        // SAFETY: TIS returns a Copy-rule reference on the main thread. Compare
        // it while both references live, then release the temporary source.
        unsafe {
            let source = TISCopyCurrentKeyboardInputSource();
            ensure!(
                !source.is_null(),
                "current keyboard input source is unavailable"
            );
            self.invalidate_source(&InputSource(source));
        }
        Ok(())
    }

    fn invalidate_source(&mut self, source: &InputSource) {
        // SAFETY: Both wrappers own non-null TIS references.
        if self.source.as_ref().is_none_or(|previous| unsafe {
            CFEqual(previous.0, source.0) == 0
        }) {
            self.clear();
        }
    }

    /// None leaves ordinary text and input methods to `AppKit`. Some, including
    /// an empty dead-key result, must consume the native event.
    pub(crate) fn translate_current(
        &mut self,
        window: &gpui::Window,
    ) -> anyhow::Result<Option<String>> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        ensure!(
            is_main_thread()?,
            "Option composition requires the main thread"
        );
        let RawWindowHandle::AppKit(handle) =
            HasWindowHandle::window_handle(window)
                .map_err(|error| {
                    anyhow::anyhow!("native window handle: {error}")
                })?
                .as_raw()
        else {
            self.clear();
            return Ok(None);
        };
        let application = application()?;
        // SAFETY: AppKit owns the current event and the window handle's live
        // NSView. All values are read synchronously on the main thread.
        unsafe {
            let event: *mut Object = msg_send![application, currentEvent];
            if event.is_null() {
                self.clear();
                return Ok(None);
            }
            let event_type: usize = msg_send![event, type];
            let modifiers: usize = msg_send![event, modifierFlags];
            let view = handle.ns_view.as_ptr().cast::<Object>();
            let target: *mut Object = msg_send![view, window];
            let event_window: *mut Object = msg_send![event, window];
            if event_type != 10
                || target.is_null()
                || target != event_window
                || modifiers & ((1 << 18) | (1 << 20)) != 0
            {
                self.clear();
                return Ok(None);
            }
            let option = modifiers & (1 << 19) != 0;
            let key: u16 = msg_send![event, keyCode];
            let source = TISCopyCurrentKeyboardInputSource();
            ensure!(
                !source.is_null(),
                "current keyboard input source is unavailable"
            );
            // NSEvent Shift, Caps Lock and Option map to Carbon modifier bits
            // 1, 2 and 3 after the EventRecord >> 8 conversion.
            let carbon_modifiers = u32::from(modifiers & (1 << 17) != 0) << 1
                | u32::from(modifiers & (1 << 16) != 0) << 2
                | u32::from(option) << 3;
            self.translate(InputSource(source), key, carbon_modifiers, option)
        }
    }

    fn translate(
        &mut self,
        source: InputSource,
        key: u16,
        modifiers: u32,
        option: bool,
    ) -> anyhow::Result<Option<String>> {
        let result = self.translate_layout(source, key, modifiers, option);
        if result.is_err() {
            self.clear();
        }
        result
    }

    fn translate_layout(
        &mut self,
        source: InputSource,
        key: u16,
        modifiers: u32,
        option: bool,
    ) -> anyhow::Result<Option<String>> {
        // SAFETY: Every source owns a live TIS reference. Its layout data is
        // borrowed only while the source lives. Input methods without a Unicode
        // layout stay on the ordinary AppKit path; do not substitute US input.
        unsafe {
            self.invalidate_source(&source);
            let data = TISGetInputSourceProperty(
                source.0,
                kTISPropertyUnicodeKeyLayoutData,
            );
            if data.is_null() || (!option && !self.is_pending()) {
                self.clear();
                return Ok(None);
            }
            let layout = CFDataGetBytePtr(data);
            ensure!(!layout.is_null(), "keyboard layout data is unavailable");
            let keyboard = u32::from(LMGetKbdType());
            let (text, state) = translate_key(
                layout.cast(),
                key,
                modifiers,
                keyboard,
                0,
                self.dead_key,
            )?;
            // UCKeyTranslate's opaque state can remain nonzero after a commit.
            // Compare Space with and without that state to detect a remaining
            // accent. NoDeadKeys prevents the probe from starting a new one;
            // both calls mutate only copies, and probe text never reaches a PTY.
            let pending = if state == 0 {
                false
            } else {
                let (pending_space, _) =
                    translate_key(layout.cast(), 49, 0, keyboard, 1, state)?;
                let (plain_space, _) =
                    translate_key(layout.cast(), 49, 0, keyboard, 1, 0)?;
                pending_space != plain_space
            };
            self.dead_key = if pending { state } else { 0 };
            self.pending = pending;
            self.source = pending.then_some(source);
            Ok(Some(text))
        }
    }
}

/// The caller keeps the OS-owned Unicode layout alive for the entire call.
unsafe fn translate_key(
    layout: *const c_void,
    key: u16,
    modifiers: u32,
    keyboard: u32,
    options: u32,
    mut state: u32,
) -> anyhow::Result<(String, u32)> {
    let mut output = [0_u16; 255];
    let mut length = 0;
    // SAFETY: The caller owns the layout; output/state pointers refer to local
    // buffers of exactly the advertised sizes. Action 0 is kUCKeyActionDown.
    let status = unsafe {
        UCKeyTranslate(
            layout,
            key,
            0,
            modifiers,
            keyboard,
            options,
            &raw mut state,
            output.len(),
            &raw mut length,
            output.as_mut_ptr(),
        )
    };
    ensure!(
        status == 0,
        "keyboard translation failed with OSStatus {status}"
    );
    ensure!(
        length <= output.len(),
        "keyboard translation exceeded its output buffer"
    );
    Ok((String::from_utf16(&output[..length])?, state))
}

#[cfg(test)]
mod tests {
    use super::*;

    static NATIVE_LAYOUT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn TISCreateInputSourceList(
            properties: *const c_void,
            all: u8,
        ) -> *const c_void;
        static kTISPropertyInputSourceID: *const c_void;
    }

    unsafe extern "C" {
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(
            array: *const c_void,
            index: isize,
        ) -> *const c_void;
        fn CFRetain(value: *const c_void) -> *const c_void;
    }

    fn source(id: &str) -> InputSource {
        // SAFETY: Enumerate installed sources without selecting or changing
        // the user's layout. Retain the matching source before releasing the list.
        unsafe {
            let list =
                InputSource(TISCreateInputSourceList(std::ptr::null(), 1));
            assert!(!list.0.is_null());
            for index in 0..CFArrayGetCount(list.0) {
                let value = CFArrayGetValueAtIndex(list.0, index);
                let identifier =
                    TISGetInputSourceProperty(value, kTISPropertyInputSourceID);
                if super::super::native_string(identifier.cast_mut().cast())
                    .unwrap()
                    == id
                {
                    return InputSource(CFRetain(value));
                }
            }
        }
        panic!("installed keyboard layout missing: {id}");
    }

    fn us(
        state: &mut OptionComposition,
        key: u16,
        option: bool,
    ) -> Option<String> {
        state
            .translate(
                source("com.apple.keylayout.US"),
                key,
                u32::from(option) << 3,
                option,
            )
            .unwrap()
    }

    #[test]
    fn option_symbols_and_dead_key_commit_use_the_native_layout() {
        let _guard = NATIVE_LAYOUT_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut state = OptionComposition::default();
        for (key, expected) in [(15, "®"), (8, "ç"), (3, "ƒ")] {
            assert_eq!(us(&mut state, key, true).as_deref(), Some(expected));
        }
        assert_eq!(us(&mut state, 14, true).as_deref(), Some(""));
        assert!(state.is_pending());
        assert_eq!(us(&mut state, 14, false).as_deref(), Some("é"));
        assert!(!state.is_pending());
        assert_eq!(us(&mut state, 14, false), None);
    }

    #[test]
    fn cancellation_prevents_the_next_letter_from_using_a_pending_accent() {
        let _guard = NATIVE_LAYOUT_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut state = OptionComposition::default();
        assert_eq!(us(&mut state, 14, true).as_deref(), Some(""));
        state.clear();
        assert_eq!(us(&mut state, 14, false), None);
    }

    #[test]
    fn switching_layout_discards_the_old_dead_key_state() {
        let _guard = NATIVE_LAYOUT_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut state = OptionComposition::default();
        assert_eq!(us(&mut state, 14, true).as_deref(), Some(""));
        assert_eq!(
            state
                .translate(source("com.apple.keylayout.British"), 14, 0, false)
                .unwrap(),
            None
        );
        assert!(!state.is_pending());
    }
    #[test]
    fn chained_dead_keys_preserve_new_accent_and_completed_keys_end_composition()
     {
        let _guard = NATIVE_LAYOUT_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut state = OptionComposition::default();
        assert_eq!(us(&mut state, 14, true).as_deref(), Some(""));
        assert_eq!(us(&mut state, 32, true).as_deref(), Some("´"));
        assert!(state.is_pending());
        assert_eq!(us(&mut state, 32, false).as_deref(), Some("ü"));
        assert!(!state.is_pending());
        assert_eq!(us(&mut state, 14, true).as_deref(), Some(""));
        assert_eq!(us(&mut state, 14, true).as_deref(), Some("´"));
        assert!(state.is_pending());
        assert_eq!(us(&mut state, 14, false).as_deref(), Some("é"));
        assert!(!state.is_pending());
        assert_eq!(us(&mut state, 14, false), None);
        assert_eq!(us(&mut state, 14, true).as_deref(), Some(""));
        assert_eq!(us(&mut state, 7, false).as_deref(), Some("´x"));
        assert!(!state.is_pending());
    }

    #[test]
    fn native_layout_preserves_shift_caps_and_all_us_option_dead_keys() {
        let _guard = NATIVE_LAYOUT_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut state = OptionComposition::default();
        for (dead, letter, expected) in [
            (14, 14, "é"),
            (32, 32, "ü"),
            (34, 14, "ê"),
            (45, 45, "ñ"),
            (50, 0, "à"),
        ] {
            assert_eq!(us(&mut state, dead, true).as_deref(), Some(""));
            assert_eq!(
                us(&mut state, letter, false).as_deref(),
                Some(expected)
            );
            assert!(!state.is_pending());
        }
        for modifier in [2, 4] {
            assert_eq!(us(&mut state, 14, true).as_deref(), Some(""));
            assert_eq!(
                state
                    .translate(
                        source("com.apple.keylayout.US"),
                        14,
                        modifier,
                        false
                    )
                    .unwrap()
                    .as_deref(),
                Some("É")
            );
            assert!(!state.is_pending());
        }
    }

    #[test]
    fn invalid_native_layout_reports_the_os_error() {
        let _guard = NATIVE_LAYOUT_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: UCKeyTranslate documents null layout as a paramErr input.
        let error = unsafe { translate_key(std::ptr::null(), 14, 0, 0, 0, 0) }
            .unwrap_err();
        assert!(error.to_string().contains("OSStatus -50"));
    }
}
