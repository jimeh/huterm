//! `AppKit` mechanics for upstream GPUI's "simple fullscreen", called non-native
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
use objc::runtime::{Class, NO, Object, Sel, YES};
use objc::{msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::fullscreen::native_policy::{
    Display, DisplayChange, Leases, OperationGate, PresentationLease,
    display_change, restore_frame,
};
use crate::fullscreen::{Effect, NativeEvent, Operation};

const NATIVE: usize = 1 << 14;
const TITLED: usize = 1;
const RESIZABLE: usize = 1 << 3;

pub(crate) enum Event {
    Native(NativeEvent),
    NativeExitFailed(String),
    State(bool, bool),
    Complete(u64, bool),
    Failed(u64, String),
    Recover,
}

enum QueuedEvent {
    Publish(Event),
    NativeExit {
        generation: u64,
        native_generation: u64,
    },
}

#[derive(Default)]
struct Inbox {
    events: RefCell<VecDeque<QueuedEvent>>,
    screen_changed: Cell<bool>,
    refit_scheduled: Cell<bool>,
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
        if object.is_null() {
            return Self(object);
        }
        // SAFETY: All callers pass a live main-thread AppKit object, or nil.
        unsafe {
            let _: *mut Object = msg_send![object, retain];
        }
        Self(object)
    }
}
impl Drop for Retained {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
        // SAFETY: This !Send wrapper balances its retain on the AppKit thread.
        unsafe {
            let _: () = msg_send![self.0, release];
        }
    }
}

struct Saved {
    content: Bounds<f64>,
    display: Display,
    fullscreen_display: Display,
    style: usize,
    shadow: objc::runtime::BOOL,
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
    pub fn check_native_transition(&self) -> anyhow::Result<()> {
        ensure!(
            !self.0.inbox.native_transition.get(),
            "native fullscreen transition has not completed"
        );
        Ok(())
    }

    pub fn preflight(&self) -> anyhow::Result<()> {
        ensure!(!self.0.inbox.gate.closing(), "native window is closing");
        // SAFETY: Read-only main-thread inspection of the retained NSWindow.
        unsafe {
            let screen: *mut Object = msg_send![self.0.window.0, screen];
            display(screen)?;
        }
        Ok(())
    }

    /// Custom fullscreen covers the screen's camera housing; native Spaces
    /// already inset their content. Read on each refresh to follow screen changes.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "AppKit points fit GPUI logical pixels"
    )]
    pub fn safe_area(&self) -> gpui::Edges<gpui::Pixels> {
        if !self
            .0
            .saved
            .borrow()
            .as_ref()
            .is_some_and(|saved| saved.complete)
        {
            return gpui::Edges::default();
        }
        // SAFETY: Read-only main-thread getters on the retained NSWindow and its
        // NSScreen. NSEdgeInsets is four CGFloat values on supported 64-bit Macs.
        unsafe {
            let style: usize = msg_send![self.0.window.0, styleMask];
            if style & NATIVE != 0 {
                return gpui::Edges::default();
            }
            let screen: *mut Object = msg_send![self.0.window.0, screen];
            screen_safe_area(screen).map(|value| gpui::px(*value as f32))
        }
    }

    /// GPUI's macOS hover flag only reports activation, including when the
    /// cursor has left this fullscreen window for another display.
    pub fn pointer_on_display(&self) -> bool {
        // SAFETY: Main-thread, read-only AppKit getters on the retained window.
        unsafe {
            let screen: *mut Object = msg_send![self.0.window.0, screen];
            if screen.is_null() {
                return false;
            }
            let Some(event) = Class::get("NSEvent") else {
                return false;
            };
            let pointer: gpui::Point<f64> = msg_send![event, mouseLocation];
            let frame: Bounds<f64> = msg_send![screen, frame];
            frame.contains(&pointer)
        }
    }

    /// `AppKit` screen coordinates include the camera-housing region outside a
    /// native fullscreen content view. Both getters use logical screen points.
    pub fn pointer_in_top_edge(&self) -> bool {
        // SAFETY: Read-only AppKit queries run on the main thread against the
        // retained NSWindow. NSEvent mouseLocation is an NSPoint of two CGFloat.
        unsafe {
            let key: objc::runtime::BOOL =
                msg_send![self.0.window.0, isKeyWindow];
            if key != YES {
                return false;
            }
            let screen: *mut Object = msg_send![self.0.window.0, screen];
            if screen.is_null() {
                return false;
            }
            let Some(event) = Class::get("NSEvent") else {
                return false;
            };
            let pointer: gpui::Point<f64> = msg_send![event, mouseLocation];
            let frame: Bounds<f64> = msg_send![screen, frame];
            let depth = screen_safe_area(screen).top.max(2.0);
            pointer.x >= frame.origin.x
                && pointer.x < frame.right()
                && pointer.y >= frame.bottom() - depth
                && pointer.y <= frame.bottom()
        }
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
            let safe_area = screen_safe_area(screen);
            let screen = display(screen)?;
            let shadow: objc::runtime::BOOL = msg_send![window, hasShadow];
            let responder: *mut Object = msg_send![window, firstResponder];
            let options: usize = msg_send![application()?, presentationOptions];
            let simple = self
                .0
                .saved
                .borrow()
                .as_ref()
                .is_some_and(|saved| saved.complete);
            Ok(format!(
                "style={style}\nframe={}\ncontent={}\nscreen={}\nresponder={}\noptions={options}\nsimple={simple}\nshadow={}\nsafe_area={},{},{},{}",
                native_rect(frame),
                native_rect(content),
                native_rect(screen.frame),
                responder as usize,
                shadow == YES,
                safe_area.top,
                safe_area.right,
                safe_area.bottom,
                safe_area.left
            ))
        }
    }

    /// Inject an incomplete native lifecycle without starting an OS animation.
    pub fn probe_native_transition(
        &self,
        complete: bool,
    ) -> anyhow::Result<String> {
        ensure!(
            std::env::var_os("HUTERM_FULLSCREEN_SMOKE").is_some(),
            "native transition probe requires the fullscreen smoke"
        );
        // SAFETY: This adapter retains its main-thread observer. Inject at the
        // observer queue boundary so AppKit itself does not begin suppressing
        // keyboard input for a synthetic transition with no OS animation.
        unsafe {
            enqueue(
                &*self.0.observer.0,
                Some(if complete {
                    NativeEvent::DidExit
                } else {
                    NativeEvent::WillEnter
                }),
            );
        }
        Ok("posted".to_owned())
    }

    /// Create the stale fullscreen frame produced by a display resize, then
    /// exercise the production screen-parameters notification path.
    pub fn probe_display_refit(&self) -> anyhow::Result<String> {
        ensure!(
            std::env::var_os("HUTERM_FULLSCREEN_SMOKE").is_some(),
            "display probe requires the fullscreen smoke"
        );
        ensure!(
            self.0
                .saved
                .borrow()
                .as_ref()
                .is_some_and(|saved| saved.complete),
            "display probe requires settled non-native fullscreen"
        );
        // SAFETY: The smoke runs this on the foreground executor outside GPUI
        // borrows. It changes only its own window, not the host display mode.
        unsafe {
            let window = self.0.window.0;
            let screen: *mut Object = msg_send![window, screen];
            let mut frame = display(screen)?.frame;
            frame.size.width -= 64.0;
            frame.size.height -= 48.0;
            let _: () = msg_send![window, setFrame: frame display: YES];
            let center: *mut Object = msg_send![
                Class::get("NSNotificationCenter")
                    .context("NSNotificationCenter")?,
                defaultCenter
            ];
            let name: *mut Object = msg_send![Class::get("NSString").context("NSString")?, stringWithUTF8String: c"NSApplicationDidChangeScreenParametersNotification".as_ptr()];
            let _: () = msg_send![center, postNotificationName: name object: application()?];
            Ok(native_rect(frame))
        }
    }

    /// Exercise native-exit frame reconciliation with the CI regression's input.
    pub fn probe_native_exit(&self) -> anyhow::Result<String> {
        ensure!(
            std::env::var_os("HUTERM_FULLSCREEN_SMOKE").is_some(),
            "native exit probe requires the fullscreen smoke"
        );
        ensure!(
            self.0.saved.borrow().is_none(),
            "probe requires a windowed window"
        );
        // SAFETY: The smoke invokes this outside an App update. Supply an
        // offscreen native frame to the same constraint/setter used on exit.
        unsafe {
            let window = self.0.window.0;
            let style: usize = msg_send![window, styleMask];
            ensure!(style & NATIVE == 0, "probe requires a windowed window");
            let screen: *mut Object = msg_send![window, screen];
            let display = display(screen)?;
            let mut frame: Bounds<f64> = msg_send![window, frame];
            frame.origin.y = display.frame.origin.y + display.frame.size.height
                - frame.size.height / 2.0;
            let constrained = self.constrain_native_exit_frame(frame)?;
            ensure!(
                constrained != frame,
                "probe frame must need reconciliation"
            );
            Ok(native_rect(constrained))
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
                HasWindowHandle::window_handle(window)
                    .map_err(|error| {
                        anyhow::anyhow!("native window handle: {error}")
                    })?
                    .as_raw()
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

    /// Quake owns frame and style effects while associated. Native transition
    /// flags are updated directly by callbacks, but ordinary effects are not run.
    pub fn discard_quake_events(&self) {
        self.0.inbox.events.borrow_mut().clear();
        self.0.inbox.screen_changed.set(false);
    }

    pub fn drain(&self) -> Vec<Event> {
        if self.0.inbox.screen_changed.replace(false)
            && !self.0.inbox.refit_scheduled.replace(true)
        {
            let adapter = self.clone();
            let generation = self.0.inbox.gate.generation();
            let native_generation = self.0.inbox.gate.native_generation();
            self.0
                .executor
                .spawn(async move {
                    adapter.0.inbox.refit_scheduled.set(false);
                    if !adapter
                        .0
                        .inbox
                        .gate
                        .can_refit(generation, native_generation)
                    {
                        return;
                    }
                    if let Err(error) = adapter.reconcile_display() {
                        eprintln!("fullscreen display refit failed: {error:#}");
                        if adapter
                            .0
                            .inbox
                            .gate
                            .can_refit(generation, native_generation)
                        {
                            adapter.emit(Event::Recover);
                        }
                    }
                })
                .detach();
        }
        let queued: Vec<_> =
            self.0.inbox.events.borrow_mut().drain(..).collect();
        let mut events = Vec::new();
        for event in queued {
            match event {
                QueuedEvent::Publish(event) => events.push(event),
                QueuedEvent::NativeExit {
                    generation,
                    native_generation,
                } => {
                    let adapter = self.clone();
                    self.0
                        .executor
                        .spawn(async move {
                            if !adapter
                                .0
                                .inbox
                                .gate
                                .native_is_current(native_generation)
                            {
                                return;
                            }
                            // A timeout cancels mutation, but the actual exit must
                            // still be observed unless a newer native event replaces it.
                            let result = if adapter
                                .0
                                .inbox
                                .gate
                                .is_current(generation)
                            {
                                adapter.settle_native_exit()
                            } else {
                                Ok(())
                            };
                            if !adapter
                                .0
                                .inbox
                                .gate
                                .native_is_current(native_generation)
                            {
                                return;
                            }
                            adapter.emit(Event::Native(NativeEvent::DidExit));
                            if let Err(error) = result {
                                adapter.emit(Event::NativeExitFailed(
                                    error.to_string(),
                                ));
                            }
                        })
                        .detach();
                }
            }
        }
        events
    }

    // AppKit 14 can finish a native Space exit with an offscreen frame. Apply
    // its titled-window constraint before publishing DidExit, so a queued
    // non-native entry cannot save geometry that AppKit later refuses to restore.
    fn settle_native_exit(&self) -> anyhow::Result<()> {
        if self.0.saved.borrow().is_some() {
            // Existing non-native recovery owns its own frame and exit sequence.
            return Ok(());
        }
        let window = self.0.window.0;
        // SAFETY: A generation-checked foreground task owns the retained window.
        // Setters run after the notification and outside GPUI update borrows.
        unsafe {
            let style: usize = msg_send![window, styleMask];
            ensure!(
                style & NATIVE == 0 && !self.0.inbox.native_transition.get(),
                "native fullscreen interrupted frame reconciliation"
            );
            let frame: Bounds<f64> = msg_send![window, frame];
            self.constrain_native_exit_frame(frame)?;
        }
        Ok(())
    }

    fn constrain_native_exit_frame(
        &self,
        frame: Bounds<f64>,
    ) -> anyhow::Result<Bounds<f64>> {
        let window = self.0.window.0;
        // SAFETY: Called only on the foreground executor with a live retained
        // window, outside GPUI update borrows. AppKit chooses its own constraint.
        unsafe {
            let screen: *mut Object = msg_send![window, screen];
            ensure!(!screen.is_null(), "native exit display is unavailable");
            let constrained: Bounds<f64> =
                msg_send![window, constrainFrameRect: frame toScreen: screen];
            if constrained != frame {
                let _: () =
                    msg_send![window, setFrame: constrained display: YES];
                let actual: Bounds<f64> = msg_send![window, frame];
                ensure!(
                    actual == constrained,
                    "native exit frame did not reach constrained bounds"
                );
            }
            Ok(constrained)
        }
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
        self.0
            .inbox
            .events
            .borrow_mut()
            .push_back(QueuedEvent::Publish(event));
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
        // A timed-out native transition may still be animating. Reject before
        // acquiring a lease or changing style; rollback cannot run either.
        self.check_native_transition()?;
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
                fullscreen_display: display,
                style,
                shadow: msg_send![window, hasShadow],
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
            // AppKit's shadow includes a thin outline even without a titlebar.
            let _: () = msg_send![window, setHasShadow: NO];
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
            saved.complete = false;
            let _: () = msg_send![self.0.window.0, setStyleMask: saved.style];
            let _: () = msg_send![self.0.window.0, setHasShadow: saved.shadow];
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
            let shadow: objc::runtime::BOOL = msg_send![window, hasShadow];
            ensure!(shadow == state.shadow, "window shadow did not restore");
        }
        saved.take();
        self.emit(Event::State(false, false));
        Ok(())
    }

    fn reconcile_display(&self) -> anyhow::Result<()> {
        let mut saved = self.0.saved.borrow_mut();
        let Some(saved) = saved.as_mut() else {
            return Ok(());
        };
        if !saved.complete || self.0.inbox.native_transition.get() {
            return Ok(());
        }
        let window = self.0.window.0;
        // SAFETY: Deferred foreground turn, outside GPUI's window update.
        // Notifications only mutate Inbox. Refitting changes geometry alone;
        // the restoration anchor, responder, shadow and lease remain owned.
        unsafe {
            let style: usize = msg_send![window, styleMask];
            if style & NATIVE != 0 {
                return Ok(());
            }
            let screen: *mut Object = msg_send![window, screen];
            let frame: Bounds<f64> = msg_send![window, frame];
            match display_change(
                true,
                saved.fullscreen_display,
                display(screen).ok(),
                frame,
                &displays()?,
            ) {
                DisplayChange::Unchanged => {}
                DisplayChange::Recover => self.emit(Event::Recover),
                DisplayChange::Refit(target) => {
                    if frame != target.frame {
                        let _: () = msg_send![window, setFrame: target.frame display: YES];
                    }
                    let actual: Bounds<f64> = msg_send![window, frame];
                    if actual != target.frame
                        && displays()?.iter().any(|display| {
                            display.id == target.id
                                && display.frame != target.frame
                        })
                    {
                        self.0.inbox.screen_changed.set(true);
                        return Ok(());
                    }
                    ensure!(
                        actual == target.frame,
                        "window did not refit its display"
                    );
                    saved.fullscreen_display = target;
                }
            }
        }
        Ok(())
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

// AppKit NSEdgeInsets uses top/left/bottom/right, unlike GPUI's edge order.
#[repr(C)]
struct NativeInsets {
    top: f64,
    left: f64,
    bottom: f64,
    right: f64,
}

pub(crate) unsafe fn screen_safe_area(screen: *mut Object) -> gpui::Edges<f64> {
    if screen.is_null() {
        return gpui::Edges::default();
    }
    // SAFETY: Callers supply NSScreen on the main thread. The selector was added
    // in macOS 12; older systems have no display safe-area API.
    unsafe {
        let supported: objc::runtime::BOOL =
            msg_send![screen, respondsToSelector: sel!(safeAreaInsets)];
        if supported != YES {
            return gpui::Edges::default();
        }
        let insets: NativeInsets = msg_send![screen, safeAreaInsets];
        gpui::Edges {
            top: insets.top,
            right: insets.right,
            bottom: insets.bottom,
            left: insets.left,
        }
    }
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
                inbox.gate.native_event(inbox.native_transition.get());
                let queued = if matches!(event, NativeEvent::DidExit) {
                    QueuedEvent::NativeExit {
                        generation: inbox.gate.generation(),
                        native_generation: inbox.gate.native_generation(),
                    }
                } else {
                    QueuedEvent::Publish(Event::Native(event))
                };
                inbox.events.borrow_mut().push_back(queued);
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
    enqueue(observer, None);
}
extern "C" fn screen_parameters(observer: &Object, _: Sel, _: *mut Object) {
    enqueue(observer, None);
}

#[cfg(test)]
mod retained_tests {
    use super::Retained;

    #[test]
    fn nil_retained_object_can_be_retained_and_dropped() {
        // SAFETY: Nil is an explicitly supported empty retained handle.
        let retained = unsafe { Retained::retain(std::ptr::null_mut()) };
        drop(retained);
        drop(Retained(std::ptr::null_mut()));
    }
}

/// Quake and ordinary non-native fullscreen share one presentation lease pool.
/// The caller owns frame/style transitions; this owner changes only app options.
#[derive(Default)]
pub(crate) struct QuakeLease(RefCell<PresentationLease>);
impl QuakeLease {
    pub fn set(&self, held: bool) -> anyhow::Result<()> {
        // SAFETY: Called only by the main-thread quake adapter, outside GPUI
        // borrows. The shared lease reducer validates AppKit's option set.
        unsafe {
            let app = application()?;
            let options: usize = msg_send![app, presentationOptions];
            let next = LEASES
                .with(|leases| {
                    if held {
                        self.0
                            .borrow_mut()
                            .acquire(&mut leases.borrow_mut(), options)
                    } else {
                        self.0
                            .borrow_mut()
                            .release(&mut leases.borrow_mut(), options)
                    }
                })
                .map_err(anyhow::Error::msg)?;
            let _: () = msg_send![app, setPresentationOptions: next];
        }
        Ok(())
    }
    pub fn held(&self) -> bool {
        self.0.borrow().held()
    }
}
