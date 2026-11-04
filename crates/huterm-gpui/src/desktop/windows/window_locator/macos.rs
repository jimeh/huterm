//! `AppKit` event-point conversion and fresh-window placement.
#![allow(unsafe_code, unexpected_cfgs)]

use super::Located;
use anyhow::ensure;
use gpui::{Bounds, Pixels, Window, point, px};
use objc::runtime::{BOOL, NO, Object};
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

fn native(window: &Window) -> anyhow::Result<*mut Object> {
    let RawWindowHandle::AppKit(handle) =
        HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("native window handle: {error}"))?
            .as_raw()
    else {
        anyhow::bail!("not an AppKit window")
    };
    // SAFETY: GPUI owns the live NSView, and this synchronous read runs on
    // the main thread while the source window is borrowed.
    let native: *mut Object =
        unsafe { msg_send![handle.ns_view.as_ptr().cast::<Object>(), window] };
    ensure!(!native.is_null(), "view has no window");
    Ok(native)
}

pub(super) fn identity(window: &Window) -> anyhow::Result<u64> {
    let native = native(window)?;
    // SAFETY: live NSWindow on the main thread.
    let number: isize = unsafe { msg_send![native, windowNumber] };
    Ok(u64::try_from(number)?)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "native logical points fit f32"
)]
pub(super) fn locate(
    source: &Window,
    pointer: gpui::Point<Pixels>,
    windows: &[(gpui::WindowId, u64)],
) -> anyhow::Result<Located> {
    let native = native(source)?;
    // SAFETY: All receivers are live AppKit objects read on the main
    // thread. Conversion uses NSWindow base coordinates (bottom-left),
    // while GPUI events use the content view's top-left. No setters run.
    unsafe {
        let view: *mut Object = msg_send![native, contentView];
        ensure!(!view.is_null(), "window has no content view");
        let bounds: Bounds<f64> = msg_send![view, bounds];
        let local = gpui::point(
            f64::from(f32::from(pointer.x)),
            bounds.size.height - f64::from(f32::from(pointer.y)),
        );
        let base: gpui::Point<f64> = msg_send![view, convertPoint: local toView: std::ptr::null_mut::<Object>()];
        let screen: gpui::Point<f64> =
            msg_send![native, convertPointToScreen: base];
        let number: isize = msg_send![class!(NSWindow), windowNumberAtPoint: screen belowWindowWithWindowNumber: 0_isize];
        if number <= 0 {
            return Ok(Located::Outside);
        }
        let app: *mut Object =
            msg_send![class!(NSApplication), sharedApplication];
        let target: *mut Object =
            msg_send![app, windowWithWindowNumber: number];
        if target.is_null() {
            return Ok(Located::Outside);
        }
        let visible: BOOL = msg_send![target, isVisible];
        let miniaturized: BOOL = msg_send![target, isMiniaturized];
        if !super::target_visible(visible != NO, miniaturized != NO, false) {
            return Ok(Located::Outside);
        }
        let native_id = u64::try_from(number)?;
        let Some((id, _)) = windows.iter().find(|(_, id)| *id == native_id)
        else {
            return Ok(Located::Blocked);
        };
        let view: *mut Object = msg_send![target, contentView];
        ensure!(!view.is_null(), "target has no content view");
        let base: gpui::Point<f64> =
            msg_send![target, convertPointFromScreen: screen];
        let local: gpui::Point<f64> = msg_send![view, convertPoint: base fromView: std::ptr::null_mut::<Object>()];
        let bounds: Bounds<f64> = msg_send![view, bounds];
        let local = point(
            px(local.x as f32),
            px((bounds.size.height - local.y) as f32),
        );
        // A native title bar outside GPUI content is still our window and
        // blocks tear-off, but offers no content target.
        Ok(Located::Window(*id, local))
    }
}

pub(super) fn screen_point(
    source: &Window,
    pointer: gpui::Point<Pixels>,
) -> anyhow::Result<gpui::Point<f64>> {
    let native = native(source)?;
    // SAFETY: Synchronous main-thread reads of the source's live views.
    unsafe {
        let view: *mut Object = msg_send![native, contentView];
        ensure!(!view.is_null(), "window has no content view");
        let bounds: Bounds<f64> = msg_send![view, bounds];
        let local = gpui::point(
            f64::from(f32::from(pointer.x)),
            bounds.size.height - f64::from(f32::from(pointer.y)),
        );
        let base: gpui::Point<f64> = msg_send![view, convertPoint: local toView: std::ptr::null_mut::<Object>()];
        Ok(msg_send![native, convertPointToScreen: base])
    }
}

/// Place only a freshly-created destination. Called outside GPUI borrows,
/// since the frame setter can synchronously notify GPUI.
pub(super) fn place(
    number: u64,
    pointer: gpui::Point<f64>,
    grab: gpui::Point<Pixels>,
) -> anyhow::Result<()> {
    // SAFETY: Main thread; NSApplication returns a live window by number.
    // Every possibly-nil receiver is checked before sending messages.
    unsafe {
        let app: *mut Object =
            msg_send![class!(NSApplication), sharedApplication];
        let target: *mut Object =
            msg_send![app, windowWithWindowNumber: isize::try_from(number)?];
        ensure!(!target.is_null(), "destination window disappeared");
        let screens: *mut Object = msg_send![class!(NSScreen), screens];
        let count: usize = msg_send![screens, count];
        let mut work: Option<Bounds<f64>> = None;
        for index in 0..count {
            let screen: *mut Object = msg_send![screens, objectAtIndex: index];
            let frame: Bounds<f64> = msg_send![screen, frame];
            if frame.contains(&pointer) {
                work = Some(msg_send![screen, visibleFrame]);
                break;
            }
        }
        let frame: Bounds<f64> = msg_send![target, frame];
        let content: *mut Object = msg_send![target, contentView];
        ensure!(!content.is_null(), "destination has no content view");
        let bounds: Bounds<f64> = msg_send![content, bounds];
        let title_height = (frame.size.height - bounds.size.height).max(0.0);
        let mut left = pointer.x - f64::from(f32::from(grab.x));
        let mut top = pointer.y + f64::from(f32::from(grab.y)) + title_height;
        if let Some(work) = work {
            let work: Bounds<f64> = work;
            left = left.clamp(
                work.left(),
                (work.right() - frame.size.width).max(work.left()),
            );
            top = top.clamp(
                (work.top() + frame.size.height).min(work.bottom()),
                work.bottom(),
            );
        }
        let _: () =
            msg_send![target, setFrameTopLeftPoint: gpui::point(left, top)];
    }
    Ok(())
}
