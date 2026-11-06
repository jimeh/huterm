//! `AppKit` styling, placement, and readback of the drag preview window.
//! Setters run outside GPUI borrows, since `AppKit` reports frame and style
//! changes to GPUI synchronously.
#![allow(unsafe_code, unexpected_cfgs)]

use anyhow::ensure;
use gpui::Bounds;
use objc::runtime::{BOOL, NO, Object, YES};
use objc::{class, msg_send, sel, sel_impl};

const BORDERLESS: usize = 0;
const NONACTIVATING_PANEL: usize = 1 << 7;
const IGNORES_CYCLE: usize = 1 << 6;

/// The live preview window by number. Main thread only.
unsafe fn window(number: u64) -> anyhow::Result<*mut Object> {
    // SAFETY: The caller is on the main thread; NSApplication returns a
    // live window or nil, which is checked.
    unsafe {
        let app: *mut Object =
            msg_send![class!(NSApplication), sharedApplication];
        let window: *mut Object =
            msg_send![app, windowWithWindowNumber: isize::try_from(number)?];
        ensure!(!window.is_null(), "drag preview window disappeared");
        Ok(window)
    }
}

/// GPUI gives a `PopUp` `Titled | FullSizeContentView`; the preview is a
/// borderless, nonactivating panel that ignores the mouse, stays out of
/// window cycling and the Window menu, and draws its own shadow. GPUI's
/// popup level and `CanJoinAllSpaces | FullScreenAuxiliary` stay.
pub(super) fn configure(number: u64) -> anyhow::Result<()> {
    // SAFETY: Main thread, outside GPUI borrows; every receiver is live.
    unsafe {
        let window = window(number)?;
        let _: () =
            msg_send![window, setStyleMask: BORDERLESS | NONACTIVATING_PANEL];
        for button in 0_usize..3 {
            let control: *mut Object =
                msg_send![window, standardWindowButton: button];
            if !control.is_null() {
                let _: () = msg_send![control, setHidden: YES];
            }
        }
        let _: () = msg_send![window, setIgnoresMouseEvents: YES];
        let _: () = msg_send![window, setHasShadow: NO];
        let _: () = msg_send![window, setExcludedFromWindowsMenu: YES];
        let _: () = msg_send![window, setHidesOnDeactivate: NO];
        let behavior: usize = msg_send![window, collectionBehavior];
        let _: () =
            msg_send![window, setCollectionBehavior: behavior | IGNORES_CYCLE];
    }
    Ok(())
}

/// Places the preview at `frame` and orders it front without making it
/// key, or orders it out without a frame. Returns whether it is visible.
pub(super) fn apply(
    number: u64,
    frame: Option<Bounds<f64>>,
) -> anyhow::Result<bool> {
    // SAFETY: Main thread, outside GPUI borrows; the window is live.
    unsafe {
        let window = window(number)?;
        if let Some(frame) = frame {
            let _: () = msg_send![window, setFrame: frame display: YES];
            let _: () =
                msg_send![window, orderFront: std::ptr::null_mut::<Object>()];
        } else {
            let _: () =
                msg_send![window, orderOut: std::ptr::null_mut::<Object>()];
        }
        let visible: BOOL = msg_send![window, isVisible];
        Ok(visible != NO)
    }
}

/// The visible frame of the screen holding `pointer`.
pub(super) fn visible_frame(pointer: gpui::Point<f64>) -> Option<Bounds<f64>> {
    // SAFETY: Synchronous main-thread reads of NSScreen.
    unsafe {
        let screens: *mut Object = msg_send![class!(NSScreen), screens];
        if screens.is_null() {
            return None;
        }
        let count: usize = msg_send![screens, count];
        (0..count).find_map(|index| {
            let screen: *mut Object = msg_send![screens, objectAtIndex: index];
            let frame: Bounds<f64> = msg_send![screen, frame];
            frame
                .contains(&pointer)
                .then(|| msg_send![screen, visibleFrame])
        })
    }
}

/// Whether the preview is on screen, and its screen frame.
pub(super) fn readback(number: u64) -> Option<(bool, Bounds<f64>)> {
    // SAFETY: Read-only main-thread getters on a live window.
    unsafe {
        let window = window(number).ok()?;
        let visible: BOOL = msg_send![window, isVisible];
        let frame: Bounds<f64> = msg_send![window, frame];
        Some((visible != NO, frame))
    }
}

/// The key window's number, if any window is key.
pub(super) fn key_window() -> Option<u64> {
    // SAFETY: Read-only main-thread getters; nil is checked.
    unsafe {
        let app: *mut Object =
            msg_send![class!(NSApplication), sharedApplication];
        let window: *mut Object = msg_send![app, keyWindow];
        if window.is_null() {
            return None;
        }
        let number: isize = msg_send![window, windowNumber];
        u64::try_from(number).ok()
    }
}
