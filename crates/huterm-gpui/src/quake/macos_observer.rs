//! Main-thread notifications and an active-only target-screen display link.
use super::*;
use crate::quake::observation::{CHANGED, DISPLAY, Signal};
use objc::{declare::ClassDecl, runtime::Sel};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    sync::OnceLock,
    time::Duration,
};

thread_local! {
    static OBSERVERS: RefCell<HashMap<usize, Rc<Inbox>>> = RefCell::default();
}
struct Inbox {
    native_transition: Cell<bool>,
    signal: Signal,
    clock_epoch: Cell<u64>,
    clock_link: Cell<usize>,
}
#[repr(C)]
struct RateRange {
    minimum: f32,
    maximum: f32,
    preferred: f32,
}
struct Clock {
    link: Retained,
    display: String,
    epoch: u64,
}
impl Drop for Clock {
    fn drop(&mut self) {
        // SAFETY: Main-thread owned CADisplayLink; invalidation releases its target
        // and run-loop registrations before the retained link is released.
        unsafe {
            let _: () = msg_send![self.link.0, invalidate];
        }
    }
}
#[derive(Clone)]
pub(crate) struct Observer(Rc<Observation>);
struct Observation {
    target: Retained,
    inbox: Rc<Inbox>,
    clock: RefCell<Option<Clock>>,
    platform: Platform,
}
impl Drop for Observation {
    fn drop(&mut self) {
        self.inbox.signal.invalidate_frames();
        self.inbox.clock_link.set(0);
        self.clock.borrow_mut().take();
        // SAFETY: Remove only this owned observer from both registration centers.
        unsafe {
            if let Ok(center) = center() {
                let _: () = msg_send![center, removeObserver:self.target.0];
            }
            if let Ok(center) = workspace_center() {
                let _: () = msg_send![center, removeObserver:self.target.0];
            }
        }
        OBSERVERS.with(|entries| {
            entries.borrow_mut().remove(&(self.target.0 as usize))
        });
    }
}
impl Window {
    #[expect(
        clippy::too_many_lines,
        reason = "one explicit notification registration inventory"
    )]
    pub fn observe(
        &self,
        signal: Signal,
        platform: &Platform,
    ) -> anyhow::Result<Observer> {
        // SAFETY: Retained NSWindow and observer live on the main thread. Selectors
        // publish facts only, so synchronous AppKit delivery cannot reenter GPUI.
        unsafe {
            let target: *mut Object = msg_send![observer_class()?, new];
            ensure!(!target.is_null(), "Quake observer unavailable");
            let inbox = Rc::new(Inbox {
                native_transition: Cell::new(false),
                signal,
                clock_epoch: Cell::new(0),
                clock_link: Cell::new(0),
            });
            OBSERVERS.with(|entries| {
                entries
                    .borrow_mut()
                    .insert(target as usize, Rc::clone(&inbox))
            });
            let observer = Observer(Rc::new(Observation {
                target: Retained(target),
                inbox,
                clock: RefCell::new(None),
                platform: platform.clone(),
            }));
            let center = center()?;
            for name in [
                c"NSWindowDidMoveNotification",
                c"NSWindowDidResizeNotification",
                c"NSWindowDidChangeOcclusionStateNotification",
            ] {
                register(
                    center,
                    target,
                    name,
                    sel!(changed:),
                    self.0.native.0,
                )?;
            }
            for name in [
                c"NSWindowDidChangeBackingPropertiesNotification",
                c"NSWindowDidChangeScreenNotification",
            ] {
                register(
                    center,
                    target,
                    name,
                    sel!(displayChanged:),
                    self.0.native.0,
                )?;
            }
            for name in [
                c"NSWindowDidBecomeKeyNotification",
                c"NSWindowDidResignKeyNotification",
                c"NSApplicationDidBecomeActiveNotification",
                c"NSApplicationDidResignActiveNotification",
                c"NSApplicationDidHideNotification",
                c"NSApplicationDidUnhideNotification",
            ] {
                register(
                    center,
                    target,
                    name,
                    sel!(changed:),
                    std::ptr::null_mut(),
                )?;
            }
            register(
                center,
                target,
                c"NSApplicationDidChangeScreenParametersNotification",
                sel!(displayChanged:),
                std::ptr::null_mut(),
            )?;
            for name in [
                c"NSWindowWillEnterFullScreenNotification",
                c"NSWindowWillExitFullScreenNotification",
            ] {
                register(
                    center,
                    target,
                    name,
                    sel!(willTransition:),
                    self.0.native.0,
                )?;
            }
            for name in [
                c"NSWindowDidEnterFullScreenNotification",
                c"NSWindowDidExitFullScreenNotification",
            ] {
                register(
                    center,
                    target,
                    name,
                    sel!(didTransition:),
                    self.0.native.0,
                )?;
            }
            let workspace = workspace_center()?;
            for name in [
                c"NSWorkspaceActiveSpaceDidChangeNotification",
                c"NSWorkspaceScreensDidWakeNotification",
                c"NSWorkspaceDidActivateApplicationNotification",
            ] {
                register(
                    workspace,
                    target,
                    name,
                    sel!(displayChanged:),
                    std::ptr::null_mut(),
                )?;
            }
            observer.0.inbox.signal.notify(DISPLAY);
            Ok(observer)
        }
    }
}
impl Observer {
    #[expect(clippy::unused_self, reason = "shared X11 observation contract")]
    pub fn failed(&self) -> bool {
        false
    }
    pub fn native_idle(&self) -> bool {
        !self.0.inbox.native_transition.get()
    }
    pub fn refresh_period(&self, display: &Display) -> Option<Duration> {
        // SAFETY: Screen is selected afresh from NSScreen.screens on the main thread.
        unsafe {
            let screen = self.screen(display).ok()?;
            let interval: f64 = msg_send![screen, minimumRefreshInterval];
            Duration::try_from_secs_f64(interval)
                .ok()
                .filter(|period| !period.is_zero())
        }
    }
    fn screen(&self, display: &Display) -> anyhow::Result<*mut Object> {
        let displays = self.0.platform.displays()?;
        let index = displays
            .iter()
            .position(|candidate| candidate.id == display.id)
            .context("animation display disconnected")?;
        // SAFETY: No event-loop turn occurs between matching the display and indexing
        // the authoritative screen list; still validate its current count.
        unsafe {
            let screens: *mut Object = msg_send![class("NSScreen")?, screens];
            let count: usize = msg_send![screens, count];
            ensure!(index < count, "animation screen unavailable");
            let screen: *mut Object = msg_send![screens, objectAtIndex:index];
            ensure!(!screen.is_null(), "animation screen unavailable");
            Ok(screen)
        }
    }
    pub fn set_clock(
        &self,
        display: Option<&Display>,
        epoch: u64,
    ) -> anyhow::Result<bool> {
        if self.0.clock.borrow().as_ref().is_some_and(|clock| {
            display.is_some_and(|display| clock.display == display.id)
                && clock.epoch == epoch
        }) {
            return Ok(true);
        }
        self.0.inbox.clock_link.set(0);
        self.0.clock.borrow_mut().take();
        let Some(display) = display else {
            return Ok(false);
        };
        let screen = self.screen(display)?;
        // SAFETY: NSScreen display links are available on supported macOS 14+.
        // The callback writes a generation-scoped signal, never native geometry.
        unsafe {
            let maximum: isize = msg_send![screen, maximumFramesPerSecond];
            ensure!(maximum > 0, "target-screen refresh rate unavailable");
            #[expect(
                clippy::cast_precision_loss,
                reason = "native refresh rate is a small positive integer"
            )]
            let maximum = maximum as f32;
            let runloop: *mut Object =
                msg_send![class("NSRunLoop")?, mainRunLoop];
            let mode: *mut Object = msg_send![class("NSString")?, stringWithUTF8String:c"kCFRunLoopCommonModes".as_ptr()];
            ensure!(
                !runloop.is_null() && !mode.is_null(),
                "display link run loop unavailable"
            );
            let link: *mut Object = msg_send![screen, displayLinkWithTarget:self.0.target.0 selector:sel!(frame:)];
            ensure!(!link.is_null(), "target-screen display link unavailable");
            let link = Retained::new(link);
            let range = RateRange {
                minimum: 1.0,
                maximum,
                preferred: maximum,
            };
            let _: () = msg_send![link.0, setPreferredFrameRateRange:range];
            self.0.inbox.clock_epoch.set(epoch);
            self.0.inbox.clock_link.set(link.0 as usize);
            let _: () = msg_send![link.0, addToRunLoop:runloop forMode:mode];
            *self.0.clock.borrow_mut() = Some(Clock {
                link,
                display: display.id.clone(),
                epoch,
            });
            Ok(true)
        }
    }
}
pub(crate) fn work_area_changed() {
    OBSERVERS.with(|entries| {
        for inbox in entries.borrow().values() {
            inbox.signal.notify(DISPLAY);
        }
    });
}
unsafe fn center() -> anyhow::Result<*mut Object> {
    // SAFETY: Main-thread Foundation singleton.
    unsafe { Ok(msg_send![class("NSNotificationCenter")?, defaultCenter]) }
}
unsafe fn workspace_center() -> anyhow::Result<*mut Object> {
    // SAFETY: Main-thread AppKit singleton.
    unsafe {
        let workspace: *mut Object =
            msg_send![class("NSWorkspace")?, sharedWorkspace];
        Ok(msg_send![workspace, notificationCenter])
    }
}
unsafe fn register(
    center: *mut Object,
    target: *mut Object,
    name: &CStr,
    selector: Sel,
    object: *mut Object,
) -> anyhow::Result<()> {
    // SAFETY: Named Foundation notification and retained observer with matching selector.
    unsafe {
        let name: *mut Object =
            msg_send![class("NSString")?, stringWithUTF8String:name.as_ptr()];
        let _: () = msg_send![center, addObserver:target selector:selector name:name object:object];
    }
    Ok(())
}
fn with_inbox(object: &Object, callback: impl FnOnce(&Inbox)) {
    OBSERVERS.with(|entries| {
        if let Some(inbox) =
            entries.borrow().get(&(std::ptr::from_ref(object) as usize))
        {
            callback(inbox);
        }
    });
}
extern "C" fn changed(object: &Object, _: Sel, _: *mut Object) {
    with_inbox(object, |inbox| inbox.signal.notify(CHANGED));
}
extern "C" fn display_changed(object: &Object, _: Sel, _: *mut Object) {
    with_inbox(object, |inbox| inbox.signal.notify(DISPLAY));
}
extern "C" fn will_transition(object: &Object, _: Sel, _: *mut Object) {
    with_inbox(object, |inbox| {
        inbox.native_transition.set(true);
        inbox.signal.notify(CHANGED);
    });
}
extern "C" fn did_transition(object: &Object, _: Sel, _: *mut Object) {
    with_inbox(object, |inbox| {
        inbox.native_transition.set(false);
        inbox.signal.notify(DISPLAY);
    });
}
extern "C" fn frame(object: &Object, _: Sel, link: *mut Object) {
    with_inbox(object, |inbox| {
        inbox.signal.raw_frame();
        if inbox.clock_link.get() == link as usize {
            inbox.signal.frame(inbox.clock_epoch.get());
        }
    });
}
fn observer_class() -> anyhow::Result<&'static Class> {
    static CLASS: OnceLock<Option<&'static Class>> = OnceLock::new();
    CLASS
        .get_or_init(|| {
            let mut declaration =
                ClassDecl::new("HutermQuakeObserver", Class::get("NSObject")?)?;
            // SAFETY: Every selector has one Objective-C object argument and returns void.
            unsafe {
                for (selector, callback) in [
                    (
                        sel!(changed:),
                        changed as extern "C" fn(&Object, Sel, *mut Object),
                    ),
                    (sel!(displayChanged:), display_changed),
                    (sel!(willTransition:), will_transition),
                    (sel!(didTransition:), did_transition),
                    (sel!(frame:), frame),
                ] {
                    declaration.add_method(selector, callback);
                }
            }
            Some(declaration.register())
        })
        .context("register Quake native observer")
}
