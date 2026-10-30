//! The `AppKit` menu presenter: builds an `NSMenu` from a [`NativeMenu`]
//! and pops it up over the window's content view.
//!
//! The popup runs from a run-loop perform in the common modes, after GPUI's
//! update borrow has returned. A GPUI foreground task would be the wrong
//! place: it runs inside a main-queue block, and the run loop drains no
//! main-queue block while one is running, so every GPUI task and frame
//! would stall until the menu closed. From a perform, the menu's tracking
//! loop keeps draining the main queue.

// Objective-C runtime integration is confined to this module. The rest of the
// desktop keeps the workspace's unsafe-code prohibition.
#![allow(unsafe_code)]
// objc 0.2's message macros refer to an obsolete cargo-clippy feature.
#![allow(unexpected_cfgs)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::OnceLock;

use anyhow::{Context as _, ensure};
use gpui::Window;
use objc::declare::ClassDecl;
use objc::runtime::{BOOL, Class, NO, Object, Sel, YES};
use objc::{msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::menu_spec::{KeyEquivalent, NativeItem, NativeMenu};
use super::native_menus::{MenuAnchor, MenuPresenter, PresentedMenu};

const COMMAND_MASK: usize = 1 << 20;
const SHIFT_MASK: usize = 1 << 17;
const CONTROL_MASK: usize = 1 << 18;
const OPTION_MASK: usize = 1 << 19;

#[repr(C)]
#[derive(Clone, Copy)]
struct NativePoint {
    x: f64,
    y: f64,
}

// SAFETY: NativePoint has CGPoint's C layout of two doubles.
unsafe impl objc::Encode for NativePoint {
    fn encode() -> objc::Encoding {
        // SAFETY: The string is CGPoint's Objective-C type encoding.
        unsafe { objc::Encoding::from_str("{CGPoint=dd}") }
    }
}

/// A menu waiting for its popup or tracking. `menu` and `view` are retained.
struct Popup {
    menu: *mut Object,
    view: *mut Object,
    location: NativePoint,
    picked: async_channel::Sender<Option<usize>>,
    tracking: bool,
}

impl Popup {
    fn release(&self) {
        // SAFETY: Balances the retains taken when the popup was registered.
        unsafe {
            let _: () = msg_send![self.menu, release];
            let _: () = msg_send![self.view, release];
        }
    }
}

thread_local! {
    static POPUPS: RefCell<HashMap<u64, Popup>> = RefCell::new(HashMap::new());
    static NEXT_TOKEN: Cell<u64> = const { Cell::new(0) };
    /// The tag of the item picked in the menu now tracking.
    static PICKED: Cell<Option<isize>> = const { Cell::new(None) };
}

pub(super) struct AppKitPresenter;

impl MenuPresenter for AppKitPresenter {
    fn present(
        &self,
        menu: &NativeMenu,
        anchor: MenuAnchor,
        window: &Window,
    ) -> anyhow::Result<PresentedMenu> {
        ensure!(is_main_thread()?, "menus open on the main thread");
        let view = content_view(window)?;
        let target = target()?;
        // SAFETY: The view is GPUI's live content view, retained until the
        // popup ends. Every object below is created and used on the main
        // thread; the menu is retained by the registry until it ends.
        unsafe {
            let menu = build(menu, target)?;
            let _: *mut Object = msg_send![view, retain];
            let location = location(menu, view, anchor);
            let (picked, receiver) = async_channel::bounded(1);
            let token = NEXT_TOKEN.with(|next| {
                let token = next.get().wrapping_add(1);
                next.set(token);
                token
            });
            POPUPS.with(|popups| {
                popups.borrow_mut().insert(
                    token,
                    Popup {
                        menu,
                        view,
                        location,
                        picked,
                        tracking: false,
                    },
                );
            });
            let number: *mut Object = msg_send![
                Class::get("NSNumber").context("NSNumber")?,
                numberWithUnsignedLongLong: token
            ];
            let _: () = msg_send![
                target,
                performSelector: sel!(popUp:)
                withObject: number
                afterDelay: 0.0_f64
                inModes: common_modes()?
            ];
            Ok(PresentedMenu {
                picked: receiver,
                _close: Box::new(Close(token)),
            })
        }
    }
}

/// Whether an `AppKit` menu is tracking now, for smoke state.
pub(super) fn tracking() -> bool {
    POPUPS.with(|popups| popups.borrow().values().any(|popup| popup.tracking))
}

/// Closes its menu when dropped: one still waiting never opens, and one
/// tracking is cancelled.
struct Close(u64);

impl Drop for Close {
    fn drop(&mut self) {
        POPUPS.with(|popups| {
            let mut popups = popups.borrow_mut();
            match popups.get(&self.0) {
                Some(popup) if popup.tracking => {
                    // SAFETY: The registry retains the tracking menu. The
                    // cancel runs on a later run-loop pass, outside any
                    // GPUI borrow; the perform retains the menu until then.
                    unsafe {
                        if let Ok(modes) = common_modes() {
                            let _: () = msg_send![
                                popup.menu,
                                performSelector: sel!(cancelTracking)
                                withObject: std::ptr::null_mut::<Object>()
                                afterDelay: 0.0_f64
                                inModes: modes
                            ];
                        }
                    }
                }
                Some(_) => {
                    if let Some(popup) = popups.remove(&self.0) {
                        popup.release();
                    }
                }
                None => {}
            }
        });
    }
}

/// Runs the registered popup `token` names, from the run loop.
extern "C" fn pop_up(_: &Object, _: Sel, number: *mut Object) {
    if number.is_null() {
        return;
    }
    // SAFETY: performSelector passes the NSNumber registered with the popup.
    let token: u64 = unsafe { msg_send![number, unsignedLongLongValue] };
    let Some((menu, view, location)) = POPUPS.with(|popups| {
        let mut popups = popups.borrow_mut();
        let popup = popups.get_mut(&token)?;
        popup.tracking = true;
        Some((popup.menu, popup.view, popup.location))
    }) else {
        return;
    };
    PICKED.with(|picked| picked.set(None));
    // SAFETY: The registry retains the menu and view until this returns.
    // No registry borrow is held while the menu tracks: GPUI keeps running
    // and may close it.
    let _: BOOL = unsafe {
        msg_send![
            menu,
            popUpMenuPositioningItem: std::ptr::null_mut::<Object>()
            atLocation: location
            inView: view
        ]
    };
    let picked = PICKED
        .with(Cell::take)
        .and_then(|tag| usize::try_from(tag).ok());
    if let Some(popup) =
        POPUPS.with(|popups| popups.borrow_mut().remove(&token))
    {
        popup.release();
        let _ = popup.picked.try_send(picked);
    }
}

extern "C" fn pick(_: &Object, _: Sel, item: *mut Object) {
    if item.is_null() {
        return;
    }
    // SAFETY: AppKit passes the chosen NSMenuItem as the action's sender.
    let tag: isize = unsafe { msg_send![item, tag] };
    PICKED.with(|picked| picked.set(Some(tag)));
}

/// The one target every menu item and popup perform uses.
fn target() -> anyhow::Result<*mut Object> {
    static TARGET: OnceLock<Option<usize>> = OnceLock::new();
    TARGET
        .get_or_init(|| {
            let superclass = Class::get("NSObject")?;
            let mut class =
                ClassDecl::new("HutermNativeMenuTarget", superclass)?;
            // SAFETY: Each method's signature matches its selector: one
            // object argument and no return value.
            unsafe {
                class.add_method(
                    sel!(pick:),
                    pick as extern "C" fn(&Object, Sel, *mut Object),
                );
                class.add_method(
                    sel!(popUp:),
                    pop_up as extern "C" fn(&Object, Sel, *mut Object),
                );
            }
            let class = class.register();
            // SAFETY: The instance lives for the process, as the class does.
            let target: *mut Object = unsafe { msg_send![class, new] };
            (!target.is_null()).then_some(target as usize)
        })
        .map(|target| target as *mut Object)
        .context("native menu target is unavailable")
}

/// Builds a retained `NSMenu` from `menu`; each item's tag is its index.
///
/// # Safety
/// Main thread only; `target` must be the live menu target.
unsafe fn build(
    menu: &NativeMenu,
    target: *mut Object,
) -> anyhow::Result<*mut Object> {
    let menu_class = Class::get("NSMenu").context("NSMenu")?;
    let item_class = Class::get("NSMenuItem").context("NSMenuItem")?;
    let badge_class = Class::get("NSMenuItemBadge");
    // SAFETY: The caller guarantees the main thread. New objects are owned
    // here and handed to the menu, which retains its items.
    unsafe {
        let native: *mut Object = msg_send![menu_class, new];
        ensure!(!native.is_null(), "NSMenu is unavailable");
        let _: () = msg_send![native, setAutoenablesItems: NO];
        let appearance: *mut Object = msg_send![
            Class::get("NSAppearance").context("NSAppearance")?,
            appearanceNamed: ns_string(if menu.dark {
                "NSAppearanceNameDarkAqua"
            } else {
                "NSAppearanceNameAqua"
            })?
        ];
        if !appearance.is_null() {
            let _: () = msg_send![native, setAppearance: appearance];
        }
        for (tag, item) in menu.items.iter().enumerate() {
            let NativeItem::Item {
                title,
                key,
                enabled,
                badge,
                ..
            } = item
            else {
                let separator: *mut Object =
                    msg_send![item_class, separatorItem];
                let _: () = msg_send![native, addItem: separator];
                continue;
            };
            let (equivalent, mask) = key
                .as_ref()
                .map_or(("", 0), |key| (key.key.as_str(), mask(key)));
            let item: *mut Object = msg_send![item_class, alloc];
            let item: *mut Object = msg_send![
                item,
                initWithTitle: ns_string(title)?
                action: sel!(pick:)
                keyEquivalent: ns_string(equivalent)?
            ];
            ensure!(!item.is_null(), "NSMenuItem is unavailable");
            let _: () = msg_send![item, setTarget: target];
            let _: () = msg_send![item, setTag: tag.cast_signed()];
            let _: () =
                msg_send![item, setEnabled: if *enabled { YES } else { NO }];
            let _: () = msg_send![item, setKeyEquivalentModifierMask: mask];
            let _: () = msg_send![item, setAllowsAutomaticKeyEquivalentLocalization: NO];
            if let (Some(count), Some(class)) = (badge, badge_class) {
                let badge: *mut Object = msg_send![class, alloc];
                let badge: *mut Object = msg_send![badge, initWithCount: isize::try_from(*count).unwrap_or(isize::MAX)];
                if !badge.is_null() {
                    let _: () = msg_send![item, setBadge: badge];
                    let _: () = msg_send![badge, release];
                }
            }
            let _: () = msg_send![native, addItem: item];
            let _: () = msg_send![item, release];
        }
        Ok(native)
    }
}

fn mask(key: &KeyEquivalent) -> usize {
    [
        (key.command, COMMAND_MASK),
        (key.control, CONTROL_MASK),
        (key.option, OPTION_MASK),
        (key.shift, SHIFT_MASK),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .fold(0, |mask, (_, flag)| mask | flag)
}

/// Where the menu's top-left corner goes in `view`'s coordinates. GPUI's
/// window coordinates run down from the top; the content view's run up
/// unless it is flipped.
///
/// # Safety
/// Main thread only, with a live menu and view.
unsafe fn location(
    menu: *mut Object,
    view: *mut Object,
    anchor: MenuAnchor,
) -> NativePoint {
    // SAFETY: The caller guarantees live objects on the main thread.
    let (bounds, flipped, menu_size): ([f64; 4], BOOL, [f64; 2]) = unsafe {
        (
            msg_send![view, bounds],
            msg_send![view, isFlipped],
            msg_send![menu, size],
        )
    };
    let [_, _, width, height] = bounds;
    let (x, y) = match anchor {
        MenuAnchor::Pointer(point) => {
            (f64::from(f32::from(point.x)), f64::from(f32::from(point.y)))
        }
        MenuAnchor::Below(control) => {
            let left = f64::from(f32::from(control.left()));
            let right = f64::from(f32::from(control.right()));
            let x = if f64::midpoint(left, right) > width / 2.0 {
                (right - menu_size[0]).max(0.0)
            } else {
                left
            };
            (x, f64::from(f32::from(control.bottom())))
        }
    };
    NativePoint {
        x,
        y: if flipped == YES { y } else { height - y },
    }
}

fn content_view(window: &Window) -> anyhow::Result<*mut Object> {
    let RawWindowHandle::AppKit(handle) =
        HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("native window handle: {error}"))?
            .as_raw()
    else {
        anyhow::bail!("native menus require an AppKit window");
    };
    Ok(handle.ns_view.as_ptr().cast::<Object>())
}

fn common_modes() -> anyhow::Result<*mut Object> {
    // SAFETY: arrayWithObject returns an autoreleased array of the mode name;
    // NSRunLoopCommonModes is the string kCFRunLoopCommonModes.
    unsafe {
        Ok(msg_send![
            Class::get("NSArray").context("NSArray")?,
            arrayWithObject: ns_string("kCFRunLoopCommonModes")?
        ])
    }
}

fn ns_string(text: &str) -> anyhow::Result<*mut Object> {
    let text = CString::new(text).context("menu text contains NUL")?;
    // SAFETY: stringWithUTF8String copies the NUL-terminated bytes into an
    // autoreleased NSString.
    let string: *mut Object = unsafe {
        msg_send![
            Class::get("NSString").context("NSString")?,
            stringWithUTF8String: text.as_ptr()
        ]
    };
    ensure!(!string.is_null(), "menu text is not UTF-8");
    Ok(string)
}

fn is_main_thread() -> anyhow::Result<bool> {
    let class = Class::get("NSThread").context("Foundation is unavailable")?;
    // SAFETY: isMainThread is a parameterless class method returning BOOL.
    let main: BOOL = unsafe { msg_send![class, isMainThread] };
    Ok(main == YES)
}

/// Builds the window menu `AppKit` would show and checks what its items
/// carry: titles, key equivalents, enablement, separators, the notices
/// badge, and the dark appearance. The menus smoke runs this.
pub(in crate::desktop) fn check_window_menu(
    keymap: &crate::keymap::InstalledKeymap,
) -> anyhow::Result<()> {
    use super::window_menu::{Restorable, WindowMenuInput, window_menu};
    use crate::keymap::Platform;

    let input = WindowMenuInput {
        platform: Platform::MacOs,
        updater: false,
        selection: false,
        notices: 2,
        restorable: Restorable {
            undo: false,
            redo: true,
        },
    };
    let contexts = ["Workspace", "Terminal"]
        .into_iter()
        .map(gpui::KeyContext::parse)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let spec = window_menu(&input, keymap, &contexts);
    let native = spec.to_native(true);
    ensure!(is_main_thread()?, "menus are built on the main thread");
    // SAFETY: The menu is built, read, and released on the main thread;
    // every item read is bounded by its count.
    unsafe {
        let menu = build(&native, target()?)?;
        let result = check_built(menu, &native);
        let _: () = msg_send![menu, release];
        result
    }
}

/// # Safety
/// Main thread only, with `menu` built from `native`.
unsafe fn check_built(
    menu: *mut Object,
    native: &NativeMenu,
) -> anyhow::Result<()> {
    // SAFETY: The caller guarantees a live menu on the main thread.
    unsafe {
        let count: usize = msg_send![menu, numberOfItems];
        ensure!(
            count == native.items.len(),
            "menu has {count} items, expected {}",
            native.items.len()
        );
        let appearance: *mut Object = msg_send![menu, appearance];
        ensure!(!appearance.is_null(), "the menu has no appearance");
        let name: *mut Object = msg_send![appearance, name];
        ensure!(
            string(name)? == "NSAppearanceNameDarkAqua",
            "the menu is not dark"
        );
        let mut items = Vec::new();
        for index in 0..count {
            let item: *mut Object = msg_send![menu, itemAtIndex: index];
            ensure!(!item.is_null(), "AppKit returned a null menu item");
            let separator: BOOL = msg_send![item, isSeparatorItem];
            if separator == YES {
                items.push("-".to_owned());
                continue;
            }
            let title: *mut Object = msg_send![item, title];
            let key: *mut Object = msg_send![item, keyEquivalent];
            let mask: usize = msg_send![item, keyEquivalentModifierMask];
            let enabled: BOOL = msg_send![item, isEnabled];
            let tag: isize = msg_send![item, tag];
            let badge: *mut Object = msg_send![item, badge];
            let badge = if badge.is_null() {
                String::new()
            } else {
                let count: isize = msg_send![badge, itemCount];
                format!("#{count}")
            };
            ensure!(tag == index.cast_signed(), "item {index} has tag {tag}");
            let key = string(key)?;
            let shortcut = if key.is_empty() {
                String::new()
            } else {
                let modifiers: String = [
                    (CONTROL_MASK, '⌃'),
                    (OPTION_MASK, '⌥'),
                    (SHIFT_MASK, '⇧'),
                    (COMMAND_MASK, '⌘'),
                ]
                .into_iter()
                .filter(|(flag, _)| mask & flag != 0)
                .map(|(_, symbol)| symbol)
                .collect();
                format!("@{modifiers}{key}")
            };
            items.push(format!(
                "{}{}{shortcut}{badge}",
                string(title)?,
                if enabled == YES { "" } else { "!" },
            ));
        }
        check_items(&items, native)
    }
}

/// Checks the window menu's items as `check_built` read them.
fn check_items(items: &[String], native: &NativeMenu) -> anyhow::Result<()> {
    let position = |title: &str| {
        items
            .iter()
            .position(|item| item.starts_with(title))
            .with_context(|| format!("no {title} item in {items:?}"))
    };
    let redo = position("Redo")?;
    ensure!(
        items.get(redo..redo + 3)
            == Some(
                &[
                    "Redo@⇧⌘z".to_owned(),
                    "Copy!@⌘c".to_owned(),
                    "Paste@⌘v".to_owned(),
                ][..]
            ),
        "Edit flattens after Redo: {items:?}"
    );
    for expected in [
        "Undo!@⌘z",
        "Open Settings@⌘,",
        // The smoke binds cmd-enter, which AppKit spells as Return.
        "Toggle Fullscreen@⌘\r",
        "Show Notices#2",
    ] {
        let title = expected.split(['!', '@', '#']).next().unwrap_or_default();
        ensure!(
            items.get(position(title)?).map(String::as_str) == Some(expected),
            "expected {expected:?} in {items:?}"
        );
    }
    ensure!(
        items.iter().filter(|item| *item == "-").count()
            == native
                .items
                .iter()
                .filter(|item| matches!(item, NativeItem::Separator))
                .count(),
        "separators: {items:?}"
    );
    Ok(())
}

/// # Safety
/// `value` must be a live `NSString`.
unsafe fn string(value: *mut Object) -> anyhow::Result<String> {
    ensure!(!value.is_null(), "AppKit returned a null string");
    // SAFETY: The caller supplies a live NSString; UTF8String is copied
    // before returning.
    let bytes: *const std::ffi::c_char =
        unsafe { msg_send![value, UTF8String] };
    ensure!(!bytes.is_null(), "AppKit returned a null UTF-8 string");
    Ok(unsafe { std::ffi::CStr::from_ptr(bytes) }
        .to_str()?
        .to_owned())
}
