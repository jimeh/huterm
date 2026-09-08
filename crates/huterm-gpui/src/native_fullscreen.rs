//! AppKit mechanics for upstream GPUI's "simple fullscreen", called non-native
//! fullscreen by Huterm. Replace these mechanics after a published GPUI upgrade
//! passes docs/plans/fullscreen-modes.md, without keeping two lease owners.
// Basic saved-frame/style and app-wide lease design follows GPUI at
// 242fe31a399f695103dd0cfe7dcef33a59fbf596 (Apache-2.0). Huterm preserves unrelated
// style bits and adds deferred restore, display recovery and operation guards.
#![allow(unsafe_code, unexpected_cfgs)]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::OnceLock;

use anyhow::{Context as _, ensure};
use gpui::{Bounds, Window};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel, YES};
use objc::{msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::fullscreen::native_policy::{
    Display, Leases, OperationGate, PresentationLease, needs_recovery,
    restore_frame,
};
use crate::fullscreen::{Effect, NativeEvent, Operation};

const NATIVE: usize = 1 << 14;
const TITLED: usize = 1;
const RESIZABLE: usize = 1 << 3;

pub(crate) enum Event {
    Native(NativeEvent),
    State(bool, bool),
    Complete(u64, bool),
    Failed(u64, String),
    Recover,
}

#[derive(Default)]
struct Inbox {
    events: RefCell<VecDeque<Event>>,
    screen_changed: Cell<bool>,
    native_transition: Cell<bool>,
    gate: OperationGate,
}

thread_local! {
    static OBSERVERS: RefCell<HashMap<usize, Rc<Inbox>>> = RefCell::default();
    static LEASES: RefCell<Leases> = RefCell::default();
}

struct Retained(*mut Object);
impl Retained {
    unsafe fn retain(object: *mut Object) -> Self {
        // SAFETY: All callers pass a live main-thread AppKit object, or nil.
        unsafe {
            let _: *mut Object = msg_send![object, retain];
        }
        Self(object)
    }
}
impl Drop for Retained {
    fn drop(&mut self) {
        // SAFETY: This !Send wrapper balances its retain on the AppKit thread.
        unsafe {
            let _: () = msg_send![self.0, release];
        }
    }
}

struct Saved {
    content: Bounds<f64>,
    display: Display,
    style: usize,
    responder: Retained,
    lease: PresentationLease,
    complete: bool,
}

struct Inner {
    window: Retained,
    observer: Retained,
    inbox: Rc<Inbox>,
    saved: RefCell<Option<Saved>>,
    registered: Cell<bool>,
    executor: gpui::ForegroundExecutor,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // View destruction can happen inside an App update. Move native
        // resources to a foreground turn before touching presentation options.
        let observer = std::mem::replace(
            &mut self.observer,
            Retained(std::ptr::null_mut()),
        );
        let window =
            std::mem::replace(&mut self.window, Retained(std::ptr::null_mut()));
        let registered = self.registered.replace(false);
        let mut saved = self.saved.get_mut().take();
        self.executor
            .spawn(async move {
                unregister(&observer, registered);
                if let Some(saved) = saved.as_mut()
                    && let Err(error) = release_lease(saved)
                {
                    eprintln!("Fullscreen drop presentation cleanup: {error}");
                }
                drop((saved, observer, window));
            })
            .detach();
    }
}

impl Inner {
    fn cleanup(&self) {
        if self.registered.replace(false) {
            unregister(&self.observer, true);
        }
        if let Some(saved) = self.saved.borrow_mut().as_mut()
            && let Err(error) = release_lease(saved)
        {
            eprintln!("Fullscreen close presentation cleanup: {error}");
            return;
        }
        self.saved.borrow_mut().take();
    }
}

#[derive(Clone)]
pub(crate) struct Adapter(Rc<Inner>);

impl Adapter {
    pub fn preflight(&self) -> anyhow::Result<()> {
        ensure!(!self.0.inbox.gate.closing(), "native window is closing");
        // SAFETY: Read-only main-thread inspection of the retained NSWindow.
        unsafe {
            let screen: *mut Object = msg_send![self.0.window.0, screen];
            display(screen)?;
        }
        Ok(())
    }

    pub fn inspect(&self) -> anyhow::Result<String> {
        // SAFETY: Smoke getters use the same exact retained native window.
        unsafe {
            let window = self.0.window.0;
            let style: usize = msg_send![window, styleMask];
            let frame: Bounds<f64> = msg_send![window, frame];
            let content: Bounds<f64> =
                msg_send![window, contentRectForFrameRect: frame];
            let screen: *mut Object = msg_send![window, screen];
            let screen = display(screen)?;
            let responder: *mut Object = msg_send![window, firstResponder];
            let options: usize = msg_send![application()?, presentationOptions];
            let simple = self
                .0
                .saved
                .borrow()
                .as_ref()
                .is_some_and(|saved| saved.complete);
            Ok(format!(
                "style={style}\nframe={}\ncontent={}\nscreen={}\nresponder={}\noptions={options}\nsimple={simple}",
                native_rect(frame),
                native_rect(content),
                native_rect(screen.frame),
                responder as usize
            ))
        }
    }

    pub fn new(window: &Window, cx: &gpui::App) -> anyhow::Result<Self> {
        // SAFETY: This constructor runs on GPUI's main thread. Validate before
        // retaining the exact NSWindow belonging to the provided NSView.
        unsafe {
            let main: objc::runtime::BOOL = msg_send![
                Class::get("NSThread").context("NSThread")?,
                isMainThread
            ];
            ensure!(main == YES, "fullscreen adapter requires the main thread");
            let RawWindowHandle::AppKit(handle) =
                HasWindowHandle::window_handle(window)?.as_raw()
            else {
                anyhow::bail!("fullscreen requires AppKit");
            };
            let view = handle.ns_view.as_ptr().cast::<Object>();
            let native: *mut Object = msg_send![view, window];
            ensure!(!native.is_null(), "native window is unavailable");
            let observer: *mut Object = msg_send![observer_class()?, new];
            ensure!(!observer.is_null(), "fullscreen observer is unavailable");
            let inbox = Rc::new(Inbox::default());
            OBSERVERS.with(|observers| {
                observers
                    .borrow_mut()
                    .insert(observer as usize, Rc::clone(&inbox));
            });
            let adapter = Self(Rc::new(Inner {
                window: Retained::retain(native),
                observer: Retained(observer),
                inbox,
                saved: RefCell::default(),
                registered: Cell::new(true),
                executor: cx.foreground_executor().clone(),
            }));
            let center: *mut Object = msg_send![
                Class::get("NSNotificationCenter")
                    .context("NSNotificationCenter")?,
                defaultCenter
            ];
            for (name, selector, object) in [
                (
                    c"NSWindowWillEnterFullScreenNotification",
                    sel!(willEnter:),
                    native,
                ),
                (
                    c"NSWindowDidEnterFullScreenNotification",
                    sel!(didEnter:),
                    native,
                ),
                (
                    c"NSWindowWillExitFullScreenNotification",
                    sel!(willExit:),
                    native,
                ),
                (
                    c"NSWindowDidExitFullScreenNotification",
                    sel!(didExit:),
                    native,
                ),
                (
                    c"NSWindowDidChangeScreenNotification",
                    sel!(screenChanged:),
                    native,
                ),
                (
                    c"NSApplicationDidChangeScreenParametersNotification",
                    sel!(screenParameters:),
                    std::ptr::null_mut(),
                ),
            ] {
                let name: *mut Object = msg_send![Class::get("NSString").context("NSString")?, stringWithUTF8String: name.as_ptr()];
                let _: () = msg_send![center, addObserver: observer selector: selector name: name object: object];
            }
            Ok(adapter)
        }
    }

    pub fn drain(&self) -> Vec<Event> {
        if self.0.inbox.screen_changed.replace(false)
            && self.0.saved.borrow().is_some()
        {
            match self.display_invalid() {
                Ok(true) | Err(_) => self.emit(Event::Recover),
                Ok(false) => {}
            }
        }
        self.0.inbox.events.borrow_mut().drain(..).collect()
    }

    pub fn cancel(&self, generation: u64) {
        self.0.inbox.gate.cancel(generation);
    }
    pub fn close_gate(&self) {
        self.0.inbox.gate.close();
    }
    pub fn close(&self) {
        self.close_gate();
        self.0.cleanup();
    }
    pub fn schedule_close(&self) {
        self.close_gate();
        let adapter = self.clone();
        self.0
            .executor
            .spawn(async move {
                adapter.close();
            })
            .detach();
    }
    fn emit(&self, event: Event) {
        self.0.inbox.events.borrow_mut().push_back(event);
    }
    fn valid(&self, operation: Operation) -> bool {
        self.0.inbox.gate.valid(operation)
    }
    pub fn reserve(&self, operation: Operation) {
        self.0.inbox.gate.reserve(operation);
    }

    /// Called only by a foreground task after leaving GPUI's update borrow.
    pub fn begin(&self, operation: Operation) {
        if !self.valid(operation) {
            return;
        }
        let result = match operation.effect {
            Effect::EnterNonNative => self.enter(),
            Effect::ExitNonNative => self.restore_style(),
            Effect::ToggleNative => return,
        };
        if let Err(error) = result {
            self.0.inbox.gate.cancel(operation.generation + 1);
            self.emit(Event::Failed(operation.generation, error.to_string()));
            // Retain recovery ownership until the same exit sequence succeeds.
            if operation.effect == Effect::EnterNonNative
                && self.0.saved.borrow().is_some()
            {
                self.emit(Event::Recover);
            }
        }
    }

    /// A later main-loop turn lets style/presentation changes settle first.
    pub fn finish(&self, operation: Operation) {
        if !self.valid(operation) {
            return;
        }
        let result = match operation.effect {
            Effect::EnterNonNative => self.fill_screen(),
            Effect::ExitNonNative => self.restore_geometry(),
            Effect::ToggleNative => return,
        };
        if !self.valid(operation) {
            return;
        }
        self.0.inbox.gate.complete(operation);
        match result {
            Ok(()) => self.emit(Event::Complete(
                operation.generation,
                self.0.saved.borrow().is_some(),
            )),
            Err(error) => {
                self.emit(Event::Failed(
                    operation.generation,
                    error.to_string(),
                ));
                if operation.effect == Effect::EnterNonNative
                    && self.0.saved.borrow().is_some()
                {
                    self.emit(Event::Recover);
                }
            }
        }
    }

    fn enter(&self) -> anyhow::Result<()> {
        if self.0.saved.borrow().is_some() {
            return Ok(());
        }
        let window = self.0.window.0;
        // SAFETY: Main-thread retained window; getters and validated style and
        // presentation values follow AppKit's documented types. Notifications
        // only touch the separate Inbox, never the borrowed Saved state.
        unsafe {
            let style: usize = msg_send![window, styleMask];
            ensure!(style & NATIVE == 0, "native fullscreen is active");
            let screen: *mut Object = msg_send![window, screen];
            let display = display(screen)?;
            let frame: Bounds<f64> = msg_send![window, frame];
            let content: Bounds<f64> =
                msg_send![window, contentRectForFrameRect: frame];
            let responder: *mut Object = msg_send![window, firstResponder];
            self.0.saved.replace(Some(Saved {
                content,
                display,
                style,
                responder: Retained::retain(responder),
                lease: PresentationLease::default(),
                complete: false,
            }));
            self.emit(Event::State(true, false));
            let app = application()?;
            let options: usize = msg_send![app, presentationOptions];
            let next = LEASES.with(|leases| {
                self.0
                    .saved
                    .borrow_mut()
                    .as_mut()
                    .context("saved fullscreen state")?
                    .lease
                    .acquire(&mut leases.borrow_mut(), options)
                    .map_err(anyhow::Error::msg)
            })?;
            let _: () = msg_send![app, setPresentationOptions: next];
            let _: () =
                msg_send![window, setStyleMask: style & !(TITLED | RESIZABLE)];
        }
        self.emit(Event::State(true, true));
        Ok(())
    }

    fn fill_screen(&self) -> anyhow::Result<()> {
        let window = self.0.window.0;
        let mut saved = self.0.saved.borrow_mut();
        let saved = saved
            .as_mut()
            .context("non-native saved state is unavailable")?;
        // SAFETY: This is a later foreground turn with no GPUI update borrowed.
        unsafe {
            let style: usize = msg_send![window, styleMask];
            ensure!(
                style & NATIVE == 0 && !self.0.inbox.native_transition.get(),
                "external native fullscreen interrupted entry"
            );
            let screen: *mut Object = msg_send![window, screen];
            let current = display(screen)?;
            ensure!(
                current.id == saved.display.id
                    && current.frame == saved.display.frame,
                "display changed during fullscreen entry"
            );
            let _: () = msg_send![window, setFrame: current.frame display: YES];
            let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null_mut::<Object>()];
            restore_responder(window, &saved.responder)?;
            let actual: Bounds<f64> = msg_send![window, frame];
            ensure!(actual == current.frame, "window did not fill its display");
            saved.complete = true;
        }
        Ok(())
    }

    fn restore_style(&self) -> anyhow::Result<()> {
        let mut saved = self.0.saved.borrow_mut();
        let Some(saved) = saved.as_mut() else {
            return Ok(());
        };
        // SAFETY: Exact original mask from this retained window. Native mode
        // must finish exiting before non-native recovery can mutate the mask.
        unsafe {
            let style: usize = msg_send![self.0.window.0, styleMask];
            ensure!(
                style & NATIVE == 0 && !self.0.inbox.native_transition.get(),
                "native fullscreen must exit before recovery"
            );
            let _: () = msg_send![self.0.window.0, setStyleMask: saved.style];
        }
        release_lease(saved)?;
        self.emit(Event::State(true, false));
        Ok(())
    }

    fn restore_geometry(&self) -> anyhow::Result<()> {
        let mut saved = self.0.saved.borrow_mut();
        let Some(state) = saved.as_ref() else {
            return Ok(());
        };
        ensure!(
            !state.lease.held(),
            "presentation lease restoration is pending"
        );
        let window = self.0.window.0;
        // SAFETY: Deferred until after style/lease restoration. The pure helper
        // clamps the titled frame, including its decoration above the content.
        unsafe {
            let style: usize = msg_send![window, styleMask];
            ensure!(
                style & NATIVE == 0 && !self.0.inbox.native_transition.get(),
                "native fullscreen interrupted restoration"
            );
            let screen: *mut Object = msg_send![window, screen];
            let current = display(screen).ok();
            let saved_frame: Bounds<f64> =
                msg_send![window, frameRectForContentRect: state.content];
            let frame = restore_frame(
                saved_frame,
                state.display,
                &displays()?,
                current.map(|display| display.id),
            )
            .context("no display available for fullscreen restoration")?;
            let _: () = msg_send![window, setFrame: frame display: YES];
            let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null_mut::<Object>()];
            restore_responder(window, &state.responder)?;
            let actual: Bounds<f64> = msg_send![window, frame];
            ensure!(
                actual == frame,
                "window restoration did not reach saved bounds"
            );
        }
        saved.take();
        self.emit(Event::State(false, false));
        Ok(())
    }

    fn display_invalid(&self) -> anyhow::Result<bool> {
        let saved = self.0.saved.borrow();
        let Some(saved) = saved.as_ref() else {
            return Ok(false);
        };
        // SAFETY: Main-thread read-only inspection of the retained window.
        unsafe {
            let screen: *mut Object = msg_send![self.0.window.0, screen];
            let frame: Bounds<f64> = msg_send![self.0.window.0, frame];
            Ok(needs_recovery(
                saved.display,
                display(screen).ok(),
                frame,
                &displays()?,
            ))
        }
    }
}

fn native_rect(rect: Bounds<f64>) -> String {
    format!(
        "{},{},{},{}",
        rect.origin.x, rect.origin.y, rect.size.width, rect.size.height
    )
}

fn unregister(observer: &Retained, registered: bool) {
    if !registered {
        return;
    }
    // SAFETY: Exact observer owned here, removal leaves GPUI's delegate alone.
    unsafe {
        if let Some(class) = Class::get("NSNotificationCenter") {
            let center: *mut Object = msg_send![class, defaultCenter];
            let _: () = msg_send![center, removeObserver: observer.0];
        }
    }
    OBSERVERS.with(|observers| {
        observers.borrow_mut().remove(&(observer.0 as usize));
    });
}

fn release_lease(saved: &mut Saved) -> anyhow::Result<()> {
    if !saved.lease.held() {
        return Ok(());
    }
    // SAFETY: Main-thread NSApplication and a validated complete option set.
    unsafe {
        let app = application()?;
        let options: usize = msg_send![app, presentationOptions];
        let next = LEASES
            .with(|leases| {
                saved.lease.release(&mut leases.borrow_mut(), options)
            })
            .map_err(anyhow::Error::msg)?;
        let _: () = msg_send![app, setPresentationOptions: next];
    }
    Ok(())
}

unsafe fn restore_responder(
    window: *mut Object,
    responder: &Retained,
) -> anyhow::Result<()> {
    if responder.0.is_null() {
        return Ok(());
    }
    // SAFETY: Both objects are retained on the AppKit thread by this adapter.
    let restored: objc::runtime::BOOL =
        unsafe { msg_send![window, makeFirstResponder: responder.0] };
    ensure!(restored == YES, "cannot restore window first responder");
    Ok(())
}

fn application() -> anyhow::Result<*mut Object> {
    // SAFETY: Only called on the main thread by the !Send adapter.
    let app = unsafe {
        msg_send![
            Class::get("NSApplication").context("NSApplication")?,
            sharedApplication
        ]
    };
    Ok(app)
}

unsafe fn display(screen: *mut Object) -> anyhow::Result<Display> {
    ensure!(!screen.is_null(), "window display is unavailable");
    // SAFETY: Callers pass NSScreen from AppKit; dictionary key and NSNumber
    // selectors match the documented NSScreenNumber value.
    unsafe {
        let frame: Bounds<f64> = msg_send![screen, frame];
        let visible: Bounds<f64> = msg_send![screen, visibleFrame];
        let description: *mut Object = msg_send![screen, deviceDescription];
        let key: *mut Object = msg_send![Class::get("NSString").context("NSString")?, stringWithUTF8String: c"NSScreenNumber".as_ptr()];
        let number: *mut Object = msg_send![description, objectForKey: key];
        ensure!(!number.is_null(), "display identity is unavailable");
        let id: u32 = msg_send![number, unsignedIntValue];
        Ok(Display { id, frame, visible })
    }
}

unsafe fn displays() -> anyhow::Result<Vec<Display>> {
    // SAFETY: NSScreen owns this main-thread array, read synchronously.
    unsafe {
        let screens: *mut Object =
            msg_send![Class::get("NSScreen").context("NSScreen")?, screens];
        let count: usize = msg_send![screens, count];
        (0..count)
            .map(|index| {
                let screen: *mut Object =
                    msg_send![screens, objectAtIndex: index];
                display(screen)
            })
            .collect()
    }
}

fn observer_class() -> anyhow::Result<&'static Class> {
    static CLASS: OnceLock<Option<&'static Class>> = OnceLock::new();
    CLASS
        .get_or_init(|| {
            let mut class = ClassDecl::new(
                "HutermFullscreenObserver",
                Class::get("NSObject")?,
            )?;
            // SAFETY: Selectors take one notification object and return void. The
            // callbacks enqueue plain values, without calling GPUI or AppKit.
            unsafe {
                for (selector, callback) in [
                    (
                        sel!(willEnter:),
                        will_enter as extern "C" fn(&Object, Sel, *mut Object),
                    ),
                    (sel!(didEnter:), did_enter),
                    (sel!(willExit:), will_exit),
                    (sel!(didExit:), did_exit),
                    (sel!(screenChanged:), screen_changed),
                    (sel!(screenParameters:), screen_parameters),
                ] {
                    class.add_method(selector, callback);
                }
            }
            Some(class.register())
        })
        .context("cannot register fullscreen observer")
}

fn enqueue(observer: &Object, event: Option<NativeEvent>) {
    OBSERVERS.with(|observers| {
        if let Some(inbox) = observers
            .borrow()
            .get(&(std::ptr::from_ref(observer) as usize))
        {
            if let Some(event) = event {
                inbox.native_transition.set(matches!(
                    event,
                    NativeEvent::WillEnter | NativeEvent::WillExit
                ));
                if inbox.native_transition.get() {
                    inbox.gate.native_will();
                }
                inbox.events.borrow_mut().push_back(Event::Native(event));
            } else {
                inbox.screen_changed.set(true);
            }
        }
    });
}
extern "C" fn will_enter(observer: &Object, _: Sel, _: *mut Object) {
    enqueue(observer, Some(NativeEvent::WillEnter));
}
extern "C" fn did_enter(observer: &Object, _: Sel, _: *mut Object) {
    enqueue(observer, Some(NativeEvent::DidEnter));
}
extern "C" fn will_exit(observer: &Object, _: Sel, _: *mut Object) {
    enqueue(observer, Some(NativeEvent::WillExit));
}
extern "C" fn did_exit(observer: &Object, _: Sel, _: *mut Object) {
    enqueue(observer, Some(NativeEvent::DidExit));
}
extern "C" fn screen_changed(observer: &Object, _: Sel, _: *mut Object) {
    OBSERVERS.with(|observers| {
        if let Some(inbox) = observers
            .borrow()
            .get(&(std::ptr::from_ref(observer) as usize))
            && let Some(generation) = inbox.gate.display_changed()
        {
            inbox.events.borrow_mut().push_back(Event::Failed(
                generation,
                "display changed during fullscreen entry".to_owned(),
            ));
            inbox.events.borrow_mut().push_back(Event::Recover);
        }
    });
    enqueue(observer, None);
}
extern "C" fn screen_parameters(observer: &Object, _: Sel, _: *mut Object) {
    enqueue(observer, None);
}
