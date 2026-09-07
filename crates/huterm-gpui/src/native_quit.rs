//! Cancellable `AppKit` termination without replacing GPUI's application delegate.

// Objective-C runtime integration is confined to this module. The rest of the
// desktop keeps the workspace's unsafe-code prohibition.
#![allow(unsafe_code)]
// objc 0.2's message macros refer to an obsolete cargo-clippy feature.
#![allow(unexpected_cfgs)]

use std::ffi::CString;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, ensure};
use async_channel::{Receiver, Sender};
use objc::declare::MethodImplementation;
use objc::runtime::{self, Class, NO, Object, Sel, YES};
use objc::{Encode as _, msg_send, sel, sel_impl};

#[path = "native_quit/option.rs"]
mod option;
pub(crate) type OptionComposition = option::OptionComposition;

struct Bridge {
    requests: Sender<()>,
    pending: AtomicBool,
    approved: AtomicBool,
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

/// Installs the native quit veto after GPUI creates its `AppKit` delegate.
///
/// # Errors
/// Returns an error outside the main thread, on repeat installation, or when
/// the loaded delegate no longer supports this integration contract.
pub fn install() -> anyhow::Result<Receiver<()>> {
    ensure!(
        is_main_thread()?,
        "native quit installation requires the main thread"
    );
    ensure!(
        BRIDGE.get().is_none(),
        "native quit bridge is already installed"
    );
    let application = application()?;
    // SAFETY: NSApplication's delegate getter returns a borrowed Objective-C
    // object. Installation occurs on AppKit's main thread while GPUI owns it.
    let delegate: *mut Object = unsafe { msg_send![application, delegate] };
    ensure!(
        !delegate.is_null(),
        "GPUI has not installed its application delegate"
    );
    let selector = Sel::register("applicationShouldTerminate:");
    // SAFETY: The non-null delegate remains owned by GPUI for the process's app
    // lifetime. Class metadata is permanent after Objective-C registration.
    let class = unsafe { (&*delegate).class() };
    ensure!(
        class.name() == "GPUIApplicationDelegate",
        "unexpected application delegate {}",
        class.name()
    );
    ensure!(
        class.instance_method(selector).is_none(),
        "application delegate already implements applicationShouldTerminate:"
    );
    let encoding = CString::new(format!("{}@:@", usize::encode().as_str()))?;
    let (requests, receiver) = async_channel::bounded(1);
    BRIDGE
        .set(Bridge {
            requests,
            pending: AtomicBool::new(false),
            approved: AtomicBool::new(false),
        })
        .map_err(|_| {
            anyhow::anyhow!("native quit bridge was concurrently installed")
        })?;
    let implementation =
        should_terminate as extern "C" fn(&Object, Sel, *mut Object) -> usize;
    // SAFETY: Add only a previously absent method. This preserves GPUI's
    // instance layout and all existing callbacks. The return type matches
    // AppKit's NSUInteger NSApplicationTerminateReply; the sole explicit
    // argument is an NSApplication object. The runtime copies the encoding.
    let added = unsafe {
        runtime::class_addMethod(
            std::ptr::from_ref(class).cast_mut(),
            selector,
            implementation.imp(),
            encoding.as_ptr(),
        )
    };
    ensure!(added != NO, "cannot install applicationShouldTerminate:");
    Ok(receiver)
}

extern "C" fn should_terminate(_: &Object, _: Sel, _: *mut Object) -> usize {
    let Some(bridge) = BRIDGE.get() else {
        return 0;
    };
    if bridge.approved.load(Ordering::Acquire) {
        return 1;
    }
    if !bridge.pending.swap(true, Ordering::AcqRel) {
        // Never invoke GPUI while AppKit's terminate: stack is active. A full
        // queue already carries this request; a closed receiver fails closed.
        let _ = bridge.requests.try_send(());
    }
    0
}

/// Allows a new native request after the user cancels or quit is abandoned.
pub fn cancel_request() {
    if let Some(bridge) = BRIDGE.get() {
        bridge.pending.store(false, Ordering::Release);
    }
}

/// Permits `AppKit` termination after approved capture and cleanup complete.
pub fn allow_termination() {
    if let Some(bridge) = BRIDGE.get() {
        bridge.approved.store(true, Ordering::Release);
    }
}

/// Calls the actual `AppKit` termination entrypoint for the native smoke helper.
/// Invoke this from a foreground task outside an active GPUI App borrow.
///
/// # Errors
/// Returns an error outside the main thread or if `NSApplication` is unavailable.
#[allow(
    dead_code,
    reason = "the standalone native smoke compiles this helper from the production bridge"
)]
pub fn request_termination() -> anyhow::Result<()> {
    ensure!(
        is_main_thread()?,
        "native termination requires the main thread"
    );
    let application = application()?;
    // SAFETY: The receiver is NSApplication, the sender is nullable, and this
    // function checked AppKit's main-thread requirement above.
    unsafe {
        let _: () =
            msg_send![application, terminate: std::ptr::null_mut::<Object>()];
    }
    Ok(())
}

fn application() -> anyhow::Result<*mut Object> {
    let class = Class::get("NSApplication").context("AppKit is unavailable")?;
    // SAFETY: NSApplication's sharedApplication class method returns its
    // process-owned singleton. Callers enforce main-thread access.
    let application: *mut Object =
        unsafe { msg_send![class, sharedApplication] };
    ensure!(
        !application.is_null(),
        "NSApplication returned a null singleton"
    );
    Ok(application)
}

fn is_main_thread() -> anyhow::Result<bool> {
    let class = Class::get("NSThread").context("Foundation is unavailable")?;
    // SAFETY: isMainThread is a parameterless NSThread class method returning
    // Objective-C BOOL, and is documented for use from any thread.
    let main: runtime::BOOL = unsafe { msg_send![class, isMainThread] };
    Ok(main == YES)
}

/// Retained context for one native view. It must stay on `AppKit`'s thread.
pub(crate) struct TextInputContext(*mut Object);

impl TextInputContext {
    /// Cancels the native preedit after GPUI has released its window borrow.
    pub(crate) fn discard_marked_text(self) {
        // SAFETY: The context was retained on the main thread and this non-Send
        // value can only be used there. No GPUI update is active during this
        // call, so synchronous NSTextInputClient callbacks can update views.
        unsafe {
            let _: () = msg_send![self.0, discardMarkedText];
        }
    }
}

impl Drop for TextInputContext {
    fn drop(&mut self) {
        // SAFETY: Balances text_input_context's retain on the same thread.
        unsafe {
            let _: () = msg_send![self.0, release];
        }
    }
}

/// Captures the exact window's native input context while GPUI owns the view.
pub(crate) fn text_input_context(
    window: &gpui::Window,
) -> anyhow::Result<TextInputContext> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    ensure!(
        is_main_thread()?,
        "composition cleanup requires the main thread"
    );
    let RawWindowHandle::AppKit(handle) =
        HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("native window handle: {error}"))?
            .as_raw()
    else {
        anyhow::bail!("composition cleanup requires an AppKit window");
    };
    let view = handle.ns_view.as_ptr().cast::<Object>();
    // SAFETY: The borrowed window handle identifies GPUI's live NSView. Retain
    // the context before leaving that borrow; it is released by the wrapper.
    let context: *mut Object = unsafe { msg_send![view, inputContext] };
    ensure!(
        !context.is_null(),
        "native text input context is unavailable"
    );
    unsafe {
        let _: *mut Object = msg_send![context, retain];
    }
    Ok(TextInputContext(context))
}

/// Shortcut currently installed in an actual `NSMenuItem`.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct MenuShortcut {
    pub(crate) key: String,
    pub(crate) modifiers: usize,
}

/// Reads a uniquely named menu item from this process's `AppKit` menu tree.
pub(crate) fn menu_shortcut(title: &str) -> anyhow::Result<MenuShortcut> {
    ensure!(
        is_main_thread()?,
        "menu inspection requires the main thread"
    );
    let application = application()?;
    // SAFETY: AppKit owns mainMenu and all descendants during this synchronous
    // main-thread walk. No callback mutates the tree while it is borrowed.
    let menu: *mut Object = unsafe { msg_send![application, mainMenu] };
    ensure!(!menu.is_null(), "AppKit main menu is unavailable");
    let mut matches = Vec::new();
    unsafe {
        find_menu_shortcuts(menu, title, &mut matches)?;
    }
    ensure!(
        matches.len() == 1,
        "expected one menu item {title:?}, found {}",
        matches.len()
    );
    matches.pop().context("menu item disappeared")
}

unsafe fn find_menu_shortcuts(
    menu: *mut Object,
    title: &str,
    matches: &mut Vec<MenuShortcut>,
) -> anyhow::Result<()> {
    // SAFETY: The caller guarantees a live NSMenu on the AppKit thread. Item
    // indices are bounded by its count; NSString values are copied immediately.
    unsafe {
        let count: usize = msg_send![menu, numberOfItems];
        for index in 0..count {
            let item: *mut Object = msg_send![menu, itemAtIndex: index];
            ensure!(!item.is_null(), "AppKit returned a null menu item");
            let native_title: *mut Object = msg_send![item, title];
            if native_string(native_title)? == title {
                let key: *mut Object = msg_send![item, keyEquivalent];
                matches.push(MenuShortcut {
                    key: native_string(key)?,
                    modifiers: msg_send![item, keyEquivalentModifierMask],
                });
            }
            let submenu: *mut Object = msg_send![item, submenu];
            if !submenu.is_null() {
                find_menu_shortcuts(submenu, title, matches)?;
            }
        }
    }
    Ok(())
}

unsafe fn native_string(value: *mut Object) -> anyhow::Result<String> {
    ensure!(!value.is_null(), "AppKit returned a null string");
    // SAFETY: The caller supplies a live NSString. UTF8String stays borrowed
    // until the next mutation; copy it before returning to the caller.
    let bytes: *const std::ffi::c_char =
        unsafe { msg_send![value, UTF8String] };
    ensure!(!bytes.is_null(), "AppKit returned a null UTF-8 string");
    Ok(unsafe { std::ffi::CStr::from_ptr(bytes) }
        .to_str()?
        .to_owned())
}
