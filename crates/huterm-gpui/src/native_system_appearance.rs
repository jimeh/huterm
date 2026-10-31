//! Changes of the application's appearance, `NSApp.effectiveAppearance`,
//! which follows the system's light or dark mode. GPUI reports appearance
//! changes per window only, and a window whose `NSWindow.appearance`
//! follows the theme (`native_appearance`) no longer sees the system's.
#![allow(unsafe_code, unexpected_cfgs)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::OnceLock;

use anyhow::{Context as _, ensure};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};

thread_local! {
    static SENDER: RefCell<Option<async_channel::Sender<()>>> =
        RefCell::default();
}

const KEY_PATH: &std::ffi::CStr = c"effectiveAppearance";

/// Observes the application's appearance until dropped.
pub(crate) struct SystemAppearance {
    target: *mut Object,
}

impl SystemAppearance {
    /// Starts observing. The receiver gets a message after each change;
    /// the observer only sends, so `AppKit`'s synchronous delivery never
    /// reenters GPUI. Read the new value through `App::window_appearance`.
    pub(crate) fn observe()
    -> anyhow::Result<(Self, async_channel::Receiver<()>)> {
        let (sender, receiver) = async_channel::bounded(1);
        // SAFETY: Runs on the main thread, where GPUI created NSApp. The
        // observer is a fresh NSObject this value owns; it stops observing
        // before it is released.
        unsafe {
            let app: *mut Object = msg_send![
                Class::get("NSApplication").context("NSApplication")?,
                sharedApplication
            ];
            ensure!(!app.is_null(), "no shared application");
            let target: *mut Object = msg_send![observer_class()?, new];
            ensure!(!target.is_null(), "appearance observer unavailable");
            SENDER.with(|slot| *slot.borrow_mut() = Some(sender));
            let key: *mut Object = msg_send![
                Class::get("NSString").context("NSString")?,
                stringWithUTF8String: KEY_PATH.as_ptr()
            ];
            let _: () = msg_send![
                app,
                addObserver: target
                forKeyPath: key
                options: 0_usize
                context: std::ptr::null_mut::<c_void>()
            ];
            Ok((Self { target }, receiver))
        }
    }
}

/// Whether the application's appearance is dark. GPUI's
/// `App::window_appearance` reports appearances other than plain Aqua and
/// Dark Aqua as light, such as the high-contrast ones Increase Contrast
/// selects, so this asks `AppKit` for the closer of the two.
#[cfg_attr(test, expect(dead_code, reason = "tests set the mode"))]
pub(crate) fn is_dark() -> anyhow::Result<bool> {
    // SAFETY: Main-thread reads of NSApp's appearance and AppKit's named
    // appearances; every object is AppKit-owned or autoreleased.
    unsafe {
        let string = Class::get("NSString").context("NSString")?;
        let app: *mut Object = msg_send![
            Class::get("NSApplication").context("NSApplication")?,
            sharedApplication
        ];
        ensure!(!app.is_null(), "no shared application");
        let appearance: *mut Object = msg_send![app, effectiveAppearance];
        ensure!(!appearance.is_null(), "no application appearance");
        let aqua: *mut Object = msg_send![
            string,
            stringWithUTF8String: c"NSAppearanceNameAqua".as_ptr()
        ];
        let dark: *mut Object = msg_send![
            string,
            stringWithUTF8String: c"NSAppearanceNameDarkAqua".as_ptr()
        ];
        let names = [aqua, dark];
        let names: *mut Object = msg_send![
            Class::get("NSArray").context("NSArray")?,
            arrayWithObjects: names.as_ptr()
            count: names.len()
        ];
        let best: *mut Object =
            msg_send![appearance, bestMatchFromAppearancesWithNames: names];
        if best.is_null() {
            return Ok(false);
        }
        let matched: bool = msg_send![best, isEqualToString: dark];
        Ok(matched)
    }
}

impl Drop for SystemAppearance {
    fn drop(&mut self) {
        SENDER.with(|slot| slot.borrow_mut().take());
        if self.target.is_null() {
            return;
        }
        // SAFETY: Main-thread observer this value owns, removed from the
        // application before its last reference goes.
        unsafe {
            if let (Some(application), Some(string)) =
                (Class::get("NSApplication"), Class::get("NSString"))
            {
                let app: *mut Object =
                    msg_send![application, sharedApplication];
                let key: *mut Object =
                    msg_send![string, stringWithUTF8String: KEY_PATH.as_ptr()];
                if !app.is_null() {
                    let _: () = msg_send![
                        app,
                        removeObserver: self.target
                        forKeyPath: key
                    ];
                }
            }
            let _: () = msg_send![self.target, release];
        }
    }
}

fn observer_class() -> anyhow::Result<&'static Class> {
    static CLASS: OnceLock<Option<&'static Class>> = OnceLock::new();
    CLASS
        .get_or_init(|| {
            let mut declaration = ClassDecl::new(
                "HutermSystemAppearanceObserver",
                Class::get("NSObject")?,
            )?;
            // SAFETY: The KVO callback's signature: key path, object,
            // change dictionary, and context, returning void.
            unsafe {
                declaration.add_method(
                    sel!(observeValueForKeyPath:ofObject:change:context:),
                    changed
                        as extern "C" fn(
                            &Object,
                            Sel,
                            *mut Object,
                            *mut Object,
                            *mut Object,
                            *mut c_void,
                        ),
                );
            }
            Some(declaration.register())
        })
        .context("register system appearance observer")
}

extern "C" fn changed(
    _: &Object,
    _: Sel,
    _: *mut Object,
    _: *mut Object,
    _: *mut Object,
    _: *mut c_void,
) {
    SENDER.with(|slot| {
        if let Some(sender) = slot.borrow().as_ref() {
            // A full channel already holds a wake for this change.
            let _ = sender.try_send(());
        }
    });
}
