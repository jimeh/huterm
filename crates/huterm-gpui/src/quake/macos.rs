//! Main-thread `AppKit` effects. Window ownership is retained across deferred calls.
#![allow(unsafe_code, unexpected_cfgs)]
#![allow(
    clippy::unnecessary_wraps,
    reason = "platform adapter methods share the fallible X11 contract"
)]
use super::{Display, Rect};
use anyhow::{Context as _, ensure};
use gpui::{Bounds, Window as GpuiWindow, point, size};
use objc::{
    msg_send,
    runtime::{BOOL, Class, NO, Object, YES},
    sel, sel_impl,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    ffi::{CStr, c_void},
    rc::Rc,
};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(options: u32, relative: u32) -> *mut Object;
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> *mut c_void;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFUUIDCreateString(
        allocator: *const c_void,
        uuid: *const c_void,
    ) -> *mut Object;
    fn CFRelease(object: *const c_void);
}

struct Retained(*mut Object);
impl Retained {
    unsafe fn new(object: *mut Object) -> Self {
        // SAFETY: Caller supplies a live main-thread Objective-C object.
        unsafe {
            let _: *mut Object = msg_send![object, retain];
        }
        Self(object)
    }
}
impl Drop for Retained {
    fn drop(&mut self) {
        // SAFETY: Balanced retain; this type is deliberately not Send.
        unsafe {
            let _: () = msg_send![self.0, release];
        }
    }
}
#[derive(Clone)]
pub(crate) struct Platform {
    executor: gpui::ForegroundExecutor,
}
struct Inner {
    native: Retained,
    style: usize,
    shadow: BOOL,
    collection: usize,
    lease: crate::native_fullscreen::QuakeLease,
    executor: gpui::ForegroundExecutor,
}
impl Drop for Inner {
    fn drop(&mut self) {
        let lease = std::mem::take(&mut self.lease);
        let native =
            std::mem::replace(&mut self.native, Retained(std::ptr::null_mut()));
        self.executor
            .spawn(async move {
                if let Err(error) = lease.set(false) {
                    eprintln!("Quake presentation cleanup: {error}");
                }
                drop(native);
            })
            .detach();
    }
}
#[derive(Clone)]
pub(crate) struct Window(Rc<Inner>);
#[derive(Clone)]
pub(crate) struct Focus {
    application: Rc<Retained>,
    pid: i32,
}
impl PartialEq for Focus {
    fn eq(&self, other: &Self) -> bool {
        self.pid == other.pid
    }
}
impl Eq for Focus {}

impl Platform {
    pub fn new(cx: &gpui::App) -> anyhow::Result<Self> {
        // SAFETY: NSThread class getter has no side effects.
        unsafe {
            let main: BOOL = msg_send![class("NSThread")?, isMainThread];
            ensure!(main == YES, "quake requires the main thread");
        }
        Ok(Self {
            executor: cx.foreground_executor().clone(),
        })
    }
    pub fn window(&self, window: &GpuiWindow) -> anyhow::Result<Window> {
        let RawWindowHandle::AppKit(handle) =
            HasWindowHandle::window_handle(window)
                .map_err(|error| {
                    anyhow::anyhow!("native window handle: {error}")
                })?
                .as_raw()
        else {
            anyhow::bail!("quake requires AppKit");
        };
        // SAFETY: Live GPUI NSView on the main thread. Retain its NSWindow
        // before leaving the caller's GPUI borrow.
        unsafe {
            let view = handle.ns_view.as_ptr().cast::<Object>();
            let native: *mut Object = msg_send![view, window];
            ensure!(!native.is_null(), "quake NSWindow unavailable");
            Ok(Window(Rc::new(Inner {
                native: Retained::new(native),
                style: msg_send![native, styleMask],
                shadow: msg_send![native, hasShadow],
                collection: msg_send![native, collectionBehavior],
                lease: crate::native_fullscreen::QuakeLease::default(),
                executor: self.executor.clone(),
            })))
        }
    }
    #[expect(
        clippy::unused_self,
        reason = "platform adapter shares an instance API with X11"
    )]
    pub fn focused(&self) -> anyhow::Result<Option<Focus>> {
        // SAFETY: NSWorkspace and its frontmost NSRunningApplication are
        // main-thread AppKit objects. Retain the return target before yield.
        unsafe {
            let workspace: *mut Object =
                msg_send![class("NSWorkspace")?, sharedWorkspace];
            let application: *mut Object =
                msg_send![workspace, frontmostApplication];
            if application.is_null() {
                return Ok(None);
            }
            Ok(Some(Focus {
                pid: msg_send![application, processIdentifier],
                application: Rc::new(Retained::new(application)),
            }))
        }
    }
    #[expect(
        clippy::unused_self,
        reason = "platform adapter shares an instance API with X11"
    )]
    pub fn is_ours(&self, target: &Focus) -> anyhow::Result<bool> {
        Ok(u32::try_from(target.pid).ok() == Some(std::process::id()))
    }
    #[expect(
        clippy::unused_self,
        reason = "platform adapter shares an instance API with X11"
    )]
    pub fn focus(&self, target: &Focus) -> anyhow::Result<()> {
        // SAFETY: Retained NSRunningApplication; activation does not enter a
        // Huterm window borrow. A terminated application is never reused.
        unsafe {
            let terminated: BOOL =
                msg_send![target.application.0, isTerminated];
            ensure!(terminated == NO, "previous application disappeared");
            let accepted: BOOL =
                msg_send![target.application.0,activateWithOptions: 2usize];
            ensure!(
                accepted == YES,
                "macOS refused previous-application activation"
            );
        }
        Ok(())
    }
    #[expect(
        clippy::unused_self,
        reason = "platform adapter shares an instance API with X11"
    )]
    pub fn displays(&self) -> anyhow::Result<Vec<Display>> {
        // SAFETY: NSScreen list and all getter results are read synchronously
        // on the main thread. CoreGraphics UUIDs are explicitly released.
        unsafe {
            let screens: *mut Object = msg_send![class("NSScreen")?, screens];
            let count: usize = msg_send![screens, count];
            let top = desktop_top()?;
            let mut result = Vec::new();
            for index in 0..count {
                let screen: *mut Object =
                    msg_send![screens,objectAtIndex:index];
                let frame: Bounds<f64> = msg_send![screen, frame];
                let visible: Bounds<f64> = msg_send![screen, visibleFrame];
                let description: *mut Object =
                    msg_send![screen, deviceDescription];
                let number = object_for_key(description, c"NSScreenNumber")?;
                let id: u32 = msg_send![number, unsignedIntValue];
                let uuid = CGDisplayCreateUUIDFromDisplayID(id);
                ensure!(!uuid.is_null(), "display UUID unavailable");
                let uuid_string = CFUUIDCreateString(std::ptr::null(), uuid);
                CFRelease(uuid);
                ensure!(
                    !uuid_string.is_null(),
                    "display UUID text unavailable"
                );
                let string: *const std::ffi::c_char =
                    msg_send![uuid_string, UTF8String];
                let id = CStr::from_ptr(string).to_string_lossy().into_owned();
                CFRelease(uuid_string.cast());
                result.push(Display {
                    id,
                    frame: from_native(frame, top),
                    work: from_native(visible, top),
                    primary: index == 0,
                });
            }
            Ok(result)
        }
    }
    pub fn resolve_display(&self, selector: &str) -> anyhow::Result<Display> {
        let displays = self.displays()?;
        let pointer = || -> anyhow::Result<(f64, f64)> {
            // SAFETY: NSEvent mouseLocation is a main-thread screen point.
            unsafe {
                let point: gpui::Point<f64> =
                    msg_send![class("NSEvent")?, mouseLocation];
                Ok((point.x, desktop_top()? - point.y))
            }
        };
        let point = if selector == "pointer" {
            Some(pointer()?)
        } else if selector == "active" {
            self.active_center()?.or(Some(pointer()?))
        } else {
            None
        };
        if let Some(display) = displays.iter().find(|display| {
            selector.strip_prefix("id:") == Some(display.id.as_str())
                || point.is_some_and(|(x, y)| display.frame.contains(x, y))
        }) {
            return Ok(display.clone());
        }
        if selector.starts_with("id:") {
            eprintln!(
                "Quake display {selector:?} is unavailable; using primary"
            );
        }
        displays
            .into_iter()
            .find(|display| display.primary)
            .context("no display available")
    }
    fn active_center(&self) -> anyhow::Result<Option<(f64, f64)>> {
        let Some(focus) = self.focused()? else {
            return Ok(None);
        };
        // SAFETY: The on-screen CG window dictionaries are toll-free bridged
        // CF collections. Only public owner PID, layer and bounds are read;
        // no titles, screenshots or Accessibility permission are required.
        unsafe {
            let list = CGWindowListCopyWindowInfo(0x1 | 0x10, 0);
            if list.is_null() {
                return Ok(None);
            }
            let list = Retained(list);
            let count: usize = msg_send![list.0, count];
            for index in 0..count {
                let window: *mut Object = msg_send![list.0,objectAtIndex:index];
                let owner = object_for_key(window, c"kCGWindowOwnerPID")?;
                let pid: i32 = msg_send![owner, intValue];
                let layer = object_for_key(window, c"kCGWindowLayer")?;
                let layer: i32 = msg_send![layer, intValue];
                if pid != focus.pid || layer != 0 {
                    continue;
                }
                let bounds = object_for_key(window, c"kCGWindowBounds")?;
                let number = |name| -> anyhow::Result<f64> {
                    let value = object_for_key(bounds, name)?;
                    Ok(msg_send![value, doubleValue])
                };
                return Ok(Some((
                    number(c"X")? + number(c"Width")? / 2.0,
                    number(c"Y")? + number(c"Height")? / 2.0,
                )));
            }
            Ok(None)
        }
    }
    #[expect(
        clippy::unused_self,
        reason = "platform adapter shares an instance API with X11"
    )]
    pub fn supports_fade(&self) -> anyhow::Result<bool> {
        Ok(true)
    }
}
impl Window {
    pub fn inspect(&self) -> anyhow::Result<String> {
        // SAFETY: Read-only native smoke observations of the retained NSWindow.
        unsafe {
            let id: isize = msg_send![self.0.native.0, windowNumber];
            let alpha: f64 = msg_send![self.0.native.0, alphaValue];
            let style: usize = msg_send![self.0.native.0, styleMask];
            let app: *mut Object =
                msg_send![class("NSApplication")?, sharedApplication];
            let options: usize = msg_send![app, presentationOptions];
            Ok(format!(
                "native_id={id}\nopacity={alpha}\ndecorated={}\noptions={options}",
                style & 1 != 0
            ))
        }
    }
    pub fn frame(&self) -> anyhow::Result<Rect> {
        // SAFETY: Retained main-thread NSWindow frame getter.
        unsafe {
            let frame: Bounds<f64> = msg_send![self.0.native.0, frame];
            Ok(from_native(frame, desktop_top()?))
        }
    }
    pub fn active(&self) -> anyhow::Result<bool> {
        // SAFETY: Retained main-thread NSWindow key status getter.
        unsafe {
            let key: BOOL = msg_send![self.0.native.0, isKeyWindow];
            let app: *mut Object =
                msg_send![class("NSApplication")?, sharedApplication];
            let active: BOOL = msg_send![app, isActive];
            Ok(key == YES && active == YES)
        }
    }
    pub fn visible(&self) -> anyhow::Result<bool> {
        // SAFETY: Retained main-thread NSWindow visibility getter.
        unsafe {
            let visible: BOOL = msg_send![self.0.native.0, isVisible];
            Ok(visible == YES)
        }
    }
    pub fn fullscreen(&self) -> anyhow::Result<bool> {
        // SAFETY: Read-only main-thread style getter observes native Spaces too.
        unsafe {
            let style: usize = msg_send![self.0.native.0, styleMask];
            Ok(self.0.lease.held() || style & (1 << 14) != 0)
        }
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "native screen insets become GPUI f32 logical pixels"
    )]
    pub fn safe_area(&self) -> gpui::Edges<gpui::Pixels> {
        // SAFETY: Retained NSWindow and its current NSScreen, read synchronously.
        unsafe {
            let screen: *mut Object = msg_send![self.0.native.0, screen];
            crate::native_fullscreen::screen_safe_area(screen)
                .map(|value| gpui::px(*value as f32))
        }
    }
    pub fn toggle_native_for_smoke(&self) {
        // SAFETY: The smoke schedules this on the foreground executor outside
        // GPUI updates, exercising the actual AppKit Space transition.
        unsafe {
            let _: () = msg_send![self.0.native.0,toggleFullScreen:std::ptr::null_mut::<Object>()];
        }
    }
    pub fn set_fullscreen(&self, enabled: bool) -> anyhow::Result<()> {
        // SAFETY: A native green-button fullscreen session must finish exiting
        // before the quake reducer changes frame or style. The reducer observes
        // both this style bit and the existing AppKit transition observer.
        unsafe {
            let style: usize = msg_send![self.0.native.0, styleMask];
            if !enabled && style & (1 << 14) != 0 {
                let _: () = msg_send![self.0.native.0,toggleFullScreen:std::ptr::null_mut::<Object>()];
            }
        }
        self.0.lease.set(enabled)
    }
    pub fn set_frame(&self, frame: Rect) -> anyhow::Result<()> {
        let rect = Bounds::new(
            point(frame.x, desktop_top()? - frame.y - frame.height),
            size(frame.width, frame.height),
        );
        // SAFETY: Main-thread retained NSWindow with finite validated frame.
        // Callers defer this mutation until GPUI has released its window.
        unsafe {
            let _: () = msg_send![self.0.native.0,setFrame:rect display:YES];
        }
        Ok(())
    }
    pub fn set_quake(&self, enabled: bool) -> anyhow::Result<()> {
        // SAFETY: Keep the window's original style and collection options for
        // regular presentation. Clearing title and resize bits removes chrome;
        // GPUI's NSWindow subclass already supports borderless key windows.
        unsafe {
            let native = self.0.native.0;
            let style = if enabled {
                self.0.style & !(0x1 | 0x2 | 0x4 | 0x8)
            } else {
                self.0.style
            };
            let _: () = msg_send![native,setStyleMask:style];
            let _: () = msg_send![native,setHasShadow:if enabled {NO} else {self.0.shadow}];
            let _: () =
                msg_send![native,setLevel:if enabled {3isize} else {0isize}];
            let _: () = msg_send![native,setCollectionBehavior:if enabled {(self.0.collection & !(0x2 | (1 << 7))) | 0x1 | (1 << 8)} else {self.0.collection}];
        }
        Ok(())
    }
    pub fn opacity(&self, value: f64) -> anyhow::Result<()> {
        // SAFETY: AppKit alphaValue accepts a double in [0,1].
        unsafe {
            let _: () =
                msg_send![self.0.native.0,setAlphaValue:value.clamp(0.0,1.0)];
        }
        Ok(())
    }
    pub fn show(&self) -> anyhow::Result<()> {
        // SAFETY: Deferred main-thread activation, outside all GPUI borrows.
        unsafe {
            let app: *mut Object =
                msg_send![class("NSApplication")?, sharedApplication];
            let _: () = msg_send![app,activateIgnoringOtherApps:YES];
            let _: () = msg_send![self.0.native.0,makeKeyAndOrderFront:std::ptr::null_mut::<Object>()];
        }
        Ok(())
    }
    pub fn hide(&self) -> anyhow::Result<()> {
        self.0.lease.set(false)?;
        // SAFETY: Retained main-thread NSWindow, no application-wide hide.
        unsafe {
            let _: () = msg_send![self.0.native.0,orderOut:std::ptr::null_mut::<Object>()];
        }
        Ok(())
    }
}
fn class(name: &str) -> anyhow::Result<&'static Class> {
    Class::get(name).with_context(|| format!("AppKit class {name}"))
}
unsafe fn object_for_key(
    dictionary: *mut Object,
    key: &CStr,
) -> anyhow::Result<*mut Object> {
    // SAFETY: Caller provides an NSDictionary and static UTF-8 key.
    unsafe {
        let key: *mut Object =
            msg_send![class("NSString")?,stringWithUTF8String:key.as_ptr()];
        let value: *mut Object = msg_send![dictionary,objectForKey:key];
        ensure!(!value.is_null(), "native dictionary key unavailable");
        Ok(value)
    }
}
fn desktop_top() -> anyhow::Result<f64> {
    // SAFETY: NSScreen.screens[0] is the menu-bar display, whose top anchors
    // CoreGraphics top-left global coordinates. Other displays may be above it.
    unsafe {
        let screens: *mut Object = msg_send![class("NSScreen")?, screens];
        let count: usize = msg_send![screens, count];
        ensure!(count > 0, "no screens");
        let first: *mut Object = msg_send![screens,objectAtIndex:0usize];
        let frame: Bounds<f64> = msg_send![first, frame];
        Ok(frame.origin.y + frame.size.height)
    }
}
fn from_native(frame: Bounds<f64>, top: f64) -> Rect {
    Rect {
        x: frame.origin.x,
        y: top - frame.origin.y - frame.size.height,
        width: frame.size.width,
        height: frame.size.height,
    }
}
