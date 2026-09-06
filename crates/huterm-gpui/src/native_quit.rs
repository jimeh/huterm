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
