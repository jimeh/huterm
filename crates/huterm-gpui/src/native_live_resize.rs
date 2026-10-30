//! Whether the user is still dragging a window's edge: `AppKit`'s
//! will-start and did-end live-resize notifications for one `NSWindow`.
//! GPUI reports each new size but not when the drag ends, and nothing at
//! all while the pointer rests mid-drag.
#![allow(unsafe_code, unexpected_cfgs)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::OnceLock;

use anyhow::{Context as _, ensure};
use gpui::Window;
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

thread_local! {
    static OBSERVERS: RefCell<HashMap<usize, async_channel::Sender<bool>>> =
        RefCell::default();
}

/// Observes one window's live resizes until dropped.
pub(crate) struct LiveResize {
    target: *mut Object,
}

impl LiveResize {
    /// Starts observing `window`. The receiver gets `true` when a live
    /// resize starts and `false` when it ends; the selectors only send, so
    /// `AppKit`'s synchronous delivery never reenters GPUI.
    pub(crate) fn observe(
        window: &Window,
    ) -> anyhow::Result<(Self, async_channel::Receiver<bool>)> {
        let RawWindowHandle::AppKit(handle) =
            HasWindowHandle::window_handle(window)
                .map_err(|error| {
                    anyhow::anyhow!("native window handle: {error}")
                })?
                .as_raw()
        else {
            anyhow::bail!("live resize requires an AppKit window");
        };
        let view = handle.ns_view.as_ptr().cast::<Object>();
        let (sender, receiver) = async_channel::unbounded();
        // SAFETY: The borrowed handle names GPUI's live NSView on the main
        // thread. The observer is a fresh NSObject owned by `LiveResize`,
        // which removes it from the center before releasing it.
        unsafe {
            let native: *mut Object = msg_send![view, window];
            ensure!(!native.is_null(), "GPUI view has no window");
            let target: *mut Object = msg_send![observer_class()?, new];
            ensure!(!target.is_null(), "live resize observer unavailable");
            let observer = Self { target };
            OBSERVERS.with(|entries| {
                entries.borrow_mut().insert(target as usize, sender);
            });
            let center: *mut Object = msg_send![
                Class::get("NSNotificationCenter")
                    .context("NSNotificationCenter")?,
                defaultCenter
            ];
            for (name, selector) in [
                (
                    c"NSWindowWillStartLiveResizeNotification",
                    sel!(willStartLiveResize:),
                ),
                (
                    c"NSWindowDidEndLiveResizeNotification",
                    sel!(didEndLiveResize:),
                ),
            ] {
                register(center, target, name, selector, native)?;
            }
            Ok((observer, receiver))
        }
    }
}

impl Drop for LiveResize {
    fn drop(&mut self) {
        OBSERVERS.with(|entries| {
            entries.borrow_mut().remove(&(self.target as usize));
        });
        if self.target.is_null() {
            return;
        }
        // SAFETY: Main-thread observer this value owns; it leaves the
        // center before its last reference goes.
        unsafe {
            if let Some(class) = Class::get("NSNotificationCenter") {
                let center: *mut Object = msg_send![class, defaultCenter];
                let _: () = msg_send![center, removeObserver: self.target];
            }
            let _: () = msg_send![self.target, release];
        }
    }
}

unsafe fn register(
    center: *mut Object,
    target: *mut Object,
    name: &CStr,
    selector: Sel,
    object: *mut Object,
) -> anyhow::Result<()> {
    // SAFETY: A named Foundation notification for a live window, with an
    // observer whose class implements `selector`.
    unsafe {
        let name: *mut Object = msg_send![
            Class::get("NSString").context("NSString")?,
            stringWithUTF8String: name.as_ptr()
        ];
        let _: () = msg_send![
            center,
            addObserver: target
            selector: selector
            name: name
            object: object
        ];
    }
    Ok(())
}

fn observer_class() -> anyhow::Result<&'static Class> {
    static CLASS: OnceLock<Option<&'static Class>> = OnceLock::new();
    CLASS
        .get_or_init(|| {
            let mut declaration = ClassDecl::new(
                "HutermLiveResizeObserver",
                Class::get("NSObject")?,
            )?;
            // SAFETY: Each selector takes one notification and returns void.
            unsafe {
                declaration.add_method(
                    sel!(willStartLiveResize:),
                    will_start as extern "C" fn(&Object, Sel, *mut Object),
                );
                declaration.add_method(
                    sel!(didEndLiveResize:),
                    did_end as extern "C" fn(&Object, Sel, *mut Object),
                );
            }
            Some(declaration.register())
        })
        .context("register live resize observer")
}

fn send(object: &Object, live: bool) {
    OBSERVERS.with(|entries| {
        if let Some(sender) =
            entries.borrow().get(&(std::ptr::from_ref(object) as usize))
        {
            let _ = sender.try_send(live);
        }
    });
}

extern "C" fn will_start(object: &Object, _: Sel, _: *mut Object) {
    send(object, true);
}

extern "C" fn did_end(object: &Object, _: Sel, _: *mut Object) {
    send(object, false);
}
