//! Read native occlusion so frame tests do not race `AppKit`'s minimize animation.
#![allow(unsafe_code, unexpected_cfgs)]

use anyhow::ensure;
use objc::runtime::Object;
use objc::{msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub(super) fn occluded(window: &gpui::Window) -> anyhow::Result<bool> {
    const VISIBLE: usize = 1 << 1; // NSWindowOcclusionStateVisible
    let RawWindowHandle::AppKit(handle) =
        HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("{error}"))?
            .as_raw()
    else {
        anyhow::bail!("expected AppKit window");
    };
    // SAFETY: GPUI keeps this NSView alive during the main-thread window update.
    // Both messages only read native state and do not dispatch GPUI callbacks.
    unsafe {
        let view = handle.ns_view.as_ptr().cast::<Object>();
        let native: *mut Object = msg_send![view, window];
        ensure!(!native.is_null(), "no native window");
        let state: usize = msg_send![native, occlusionState];
        Ok(state & VISIBLE == 0)
    }
}
