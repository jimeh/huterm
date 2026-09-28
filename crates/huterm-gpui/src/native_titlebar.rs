//! `AppKit` geometry of the standard window buttons in Huterm's title strip.
#![allow(unsafe_code, unexpected_cfgs)]

use anyhow::{Context as _, ensure};
use gpui::{Bounds, Window};
use objc::runtime::Object;
use objc::{msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// `NSWindowCloseButton`.
const CLOSE_BUTTON: usize = 0;
/// `NSWindowZoomButton`.
const ZOOM_BUTTON: usize = 2;

/// The close button's frame in its title-bar container, in points. Before
/// GPUI repositions the buttons this is `AppKit`'s own placement.
pub(crate) fn close_button_frame(
    window: &Window,
) -> anyhow::Result<Bounds<f64>> {
    let button = standard_button(window, CLOSE_BUTTON)?;
    // SAFETY: `standard_button` returned a live NSButton owned by the window,
    // read synchronously on the main thread.
    Ok(unsafe { msg_send![button, frame] })
}

/// The span from the close button's left edge to the zoom button's right
/// edge, in window points with a top-left origin.
pub(crate) fn buttons_in_window(
    window: &Window,
) -> anyhow::Result<Bounds<f64>> {
    let close = in_window(standard_button(window, CLOSE_BUTTON)?)?;
    let zoom = in_window(standard_button(window, ZOOM_BUTTON)?)?;
    Ok(Bounds {
        origin: close.origin,
        size: gpui::size(
            zoom.origin.x + zoom.size.width - close.origin.x,
            close.size.height,
        ),
    })
}

fn in_window(button: *mut Object) -> anyhow::Result<Bounds<f64>> {
    // SAFETY: The caller passes a live standard button. It and its window stay
    // alive for this synchronous main-thread read; a nil view converts to
    // window base coordinates.
    unsafe {
        let bounds: Bounds<f64> = msg_send![button, bounds];
        let rect: Bounds<f64> = msg_send![
            button,
            convertRect: bounds
            toView: std::ptr::null_mut::<Object>()
        ];
        let native_window: *mut Object = msg_send![button, window];
        ensure!(!native_window.is_null(), "window button has no window");
        let content_view: *mut Object = msg_send![native_window, contentView];
        ensure!(!content_view.is_null(), "window has no content view");
        let content: Bounds<f64> = msg_send![content_view, bounds];
        Ok(Bounds {
            origin: gpui::point(
                rect.origin.x,
                content.size.height - rect.origin.y - rect.size.height,
            ),
            size: rect.size,
        })
    }
}

fn standard_button(
    window: &Window,
    kind: usize,
) -> anyhow::Result<*mut Object> {
    let RawWindowHandle::AppKit(handle) =
        HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("native window handle: {error}"))?
            .as_raw()
    else {
        anyhow::bail!("traffic lights require an AppKit window");
    };
    let view = handle.ns_view.as_ptr().cast::<Object>();
    // SAFETY: The borrowed handle identifies GPUI's live NSView on the main
    // thread; its window owns the standard buttons.
    unsafe {
        let native_window: *mut Object = msg_send![view, window];
        ensure!(!native_window.is_null(), "GPUI view has no window");
        let button: *mut Object =
            msg_send![native_window, standardWindowButton: kind];
        (!button.is_null())
            .then_some(button)
            .context("window has no standard buttons")
    }
}
