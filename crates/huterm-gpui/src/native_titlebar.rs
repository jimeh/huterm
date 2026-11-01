//! `AppKit` geometry of the standard window buttons in Huterm's title strip,
//! and of the window's rounded corners.
#![allow(unsafe_code, unexpected_cfgs)]

use anyhow::{Context as _, ensure};
use gpui::{Bounds, Window};
use objc::runtime::{BOOL, NO, Object};
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// `NSWindowCloseButton`.
const CLOSE_BUTTON: usize = 0;
/// `NSWindowZoomButton`.
const ZOOM_BUTTON: usize = 2;

/// The native title bar outside the content rectangle, in window points.
#[expect(
    clippy::cast_possible_truncation,
    reason = "native point sizes fit f32"
)]
pub(crate) fn height(window: &Window) -> anyhow::Result<f32> {
    let native = native_window(window)?;
    // SAFETY: The live NSWindow is read synchronously on the main thread.
    let (frame, content): (Bounds<f64>, Bounds<f64>) = unsafe {
        let frame: Bounds<f64> = msg_send![native, frame];
        let content: Bounds<f64> =
            msg_send![native, contentRectForFrameRect: frame];
        (frame, content)
    };
    Ok((frame.size.height - content.size.height).max(0.0) as f32)
}

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

/// The window's top and bottom corner radii in points, from `NSWindow`'s
/// private `_topCornerRadius` and `_bottomCornerRadius`, or `None` when
/// this macOS answers neither. Fails for a window without an `AppKit`
/// handle, as in GPUI tests.
pub(crate) fn corner_radii(
    window: &Window,
) -> anyhow::Result<Option<(f64, f64)>> {
    let native_window = native_window(window)?;
    // SAFETY: `native_window` is GPUI's live NSWindow, read synchronously on
    // the main thread. Each private selector is sent only after the window
    // says it responds to it, and both return a CGFloat.
    unsafe {
        let top: BOOL = msg_send![
            native_window,
            respondsToSelector: sel!(_topCornerRadius)
        ];
        let bottom: BOOL = msg_send![
            native_window,
            respondsToSelector: sel!(_bottomCornerRadius)
        ];
        if top == NO || bottom == NO {
            return Ok(None);
        }
        let top: f64 = msg_send![native_window, _topCornerRadius];
        let bottom: f64 = msg_send![native_window, _bottomCornerRadius];
        Ok(Some((top, bottom)))
    }
}

/// The running macOS release's major version, such as 26.
pub(crate) fn macos_major_version() -> u32 {
    /// `NSOperatingSystemVersion`; only its layout matters past `major`.
    #[repr(C)]
    struct Version {
        major: isize,
        _minor: isize,
        _patch: isize,
    }
    // SAFETY: `processInfo` returns the process's shared, live instance;
    // `operatingSystemVersion` returns the struct above by value, which the
    // `objc` crate sends through `objc_msgSend_stret` where the ABI needs it.
    let version: Version = unsafe {
        let info: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
        msg_send![info, operatingSystemVersion]
    };
    u32::try_from(version.major).unwrap_or(0)
}

fn standard_button(
    window: &Window,
    kind: usize,
) -> anyhow::Result<*mut Object> {
    let native_window = native_window(window)?;
    // SAFETY: `native_window` is GPUI's live NSWindow on the main thread; it
    // owns the standard buttons.
    unsafe {
        let button: *mut Object =
            msg_send![native_window, standardWindowButton: kind];
        (!button.is_null())
            .then_some(button)
            .context("window has no standard buttons")
    }
}

/// GPUI's `NSWindow` for `window`.
fn native_window(window: &Window) -> anyhow::Result<*mut Object> {
    let RawWindowHandle::AppKit(handle) =
        HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("native window handle: {error}"))?
            .as_raw()
    else {
        anyhow::bail!("window geometry requires an AppKit window");
    };
    let view = handle.ns_view.as_ptr().cast::<Object>();
    // SAFETY: The borrowed handle identifies GPUI's live NSView on the main
    // thread.
    unsafe {
        let native_window: *mut Object = msg_send![view, window];
        ensure!(!native_window.is_null(), "GPUI view has no window");
        Ok(native_window)
    }
}
