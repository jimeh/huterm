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
        // Decide under the registry borrow, act after it: releasing the
        // view or menu must not run inside it.
        let closing = POPUPS.with(|popups| {
            let mut popups = popups.borrow_mut();
            match popups.get(&self.0) {
                Some(popup) if popup.tracking => {
                    Some(Closing::Cancel(popup.menu))
                }
                Some(_) => popups.remove(&self.0).map(Closing::Drop),
                None => None,
            }
        });
        match closing {
            // SAFETY: The registry retains the tracking menu until its
            // popup returns. The cancel runs on a later run-loop pass,
            // outside any GPUI borrow, and the perform retains the menu.
            Some(Closing::Cancel(menu)) => unsafe {
                if let Ok(modes) = common_modes() {
                    let _: () = msg_send![
                        menu,
                        performSelector: sel!(cancelTracking)
                        withObject: std::ptr::null_mut::<Object>()
                        afterDelay: 0.0_f64
                        inModes: modes
                    ];
                }
            },
            Some(Closing::Drop(popup)) => popup.release(),
            None => {}
        }
    }
}

enum Closing {
    Cancel(*mut Object),
    /// A popup that never opened; its perform finds nothing to run.
    Drop(Popup),
}

/// Runs the registered popup `token` names, from the run loop.
extern "C" fn pop_up(this: &Object, _: Sel, number: *mut Object) {
    if number.is_null() {
        return;
    }
    // SAFETY: performSelector passes the NSNumber registered with the popup.
    let token: u64 = unsafe { msg_send![number, unsignedLongLongValue] };
    // Another window's menu can still be tracking, or ending after its
    // cancel. A popup inside its loop would share `PICKED`, so wait for it.
    let busy = POPUPS.with(|popups| {
        popups
            .borrow()
            .iter()
            .any(|(other, popup)| *other != token && popup.tracking)
    });
    if busy {
        // SAFETY: Requeues this perform on the same target and token.
        unsafe {
            if let Ok(modes) = common_modes() {
                let _: () = msg_send![
                    this,
                    performSelector: sel!(popUp:)
                    withObject: number
                    afterDelay: 0.05_f64
                    inModes: modes
                ];
            }
        }
        return;
    }
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

/// Builds a retained `NSMenu` from `menu`. Tags number every entry
/// depth-first, as [`NativeMenu::id_at`] reads them.
///
/// # Safety
/// Main thread only; `target` must be the live menu target.
unsafe fn build(
    menu: &NativeMenu,
    target: *mut Object,
) -> anyhow::Result<*mut Object> {
    let mut tag = 0;
    // SAFETY: The caller guarantees the main thread and the target.
    unsafe { build_menu(&menu.items, menu.dark, target, &mut tag) }
}

/// A retained `NSMenu` of `items`, which never enables items itself and
/// takes the dark or light appearance. `tag` is the next entry's tag.
///
/// # Safety
/// Main thread only; `target` must be the live menu target.
unsafe fn build_menu(
    items: &[NativeItem],
    dark: bool,
    target: *mut Object,
    tag: &mut isize,
) -> anyhow::Result<*mut Object> {
    let item_class = Class::get("NSMenuItem").context("NSMenuItem")?;
    let badge_class = Class::get("NSMenuItemBadge");
    // SAFETY: The caller guarantees the main thread. New objects are owned
    // here and handed to the menu, which retains its items and submenus.
    unsafe {
        let native = new_menu(dark)?;
        for entry in items {
            let entry_tag = *tag;
            *tag += 1;
            match entry {
                NativeItem::Separator => {
                    let separator: *mut Object =
                        msg_send![item_class, separatorItem];
                    let _: () = msg_send![native, addItem: separator];
                }
                NativeItem::Submenu {
                    title,
                    enabled,
                    items,
                } => {
                    // No action of ours: `setSubmenu:` gives the item AppKit's
                    // `submenuAction:`, which opens the submenu.
                    let item: *mut Object = msg_send![item_class, alloc];
                    let item: *mut Object = msg_send![
                        item,
                        initWithTitle: ns_string(title)?
                        action: Sel::from_ptr(std::ptr::null())
                        keyEquivalent: ns_string("")?
                    ];
                    ensure!(!item.is_null(), "NSMenuItem is unavailable");
                    let _: () = msg_send![item, setTag: entry_tag];
                    let _: () = msg_send![
                        item,
                        setEnabled: if *enabled { YES } else { NO }
                    ];
                    let submenu = build_menu(items, dark, target, tag);
                    let submenu = match submenu {
                        Ok(submenu) => submenu,
                        Err(error) => {
                            let _: () = msg_send![item, release];
                            let _: () = msg_send![native, release];
                            return Err(error);
                        }
                    };
                    let _: () = msg_send![item, setSubmenu: submenu];
                    let _: () = msg_send![submenu, release];
                    let _: () = msg_send![native, addItem: item];
                    let _: () = msg_send![item, release];
                }
                NativeItem::Item {
                    title,
                    key,
                    enabled,
                    badge,
                    ..
                } => {
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
                    let _: () = msg_send![item, setTag: entry_tag];
                    let _: () = msg_send![
                        item,
                        setEnabled: if *enabled { YES } else { NO }
                    ];
                    let _: () =
                        msg_send![item, setKeyEquivalentModifierMask: mask];
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
            }
        }
        Ok(native)
    }
}

/// A retained, empty `NSMenu` that never enables items itself, in the dark
/// or light appearance.
///
/// # Safety
/// Main thread only.
unsafe fn new_menu(dark: bool) -> anyhow::Result<*mut Object> {
    let menu_class = Class::get("NSMenu").context("NSMenu")?;
    // SAFETY: The caller guarantees the main thread; the menu is returned
    // retained to the caller.
    unsafe {
        let native: *mut Object = msg_send![menu_class, new];
        ensure!(!native.is_null(), "NSMenu is unavailable");
        let _: () = msg_send![native, setAutoenablesItems: NO];
        let appearance: *mut Object = msg_send![
            Class::get("NSAppearance").context("NSAppearance")?,
            appearanceNamed: ns_string(if dark {
                "NSAppearanceNameDarkAqua"
            } else {
                "NSAppearanceNameAqua"
            })?
        ];
        if !appearance.is_null() {
            let _: () = msg_send![native, setAppearance: appearance];
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

/// Where the menu's top-left corner goes in `view`'s coordinates.
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
    let (x, y) = origin(anchor, width, menu_size[0]);
    NativePoint {
        x,
        y: if flipped == YES { y } else { height - y },
    }
}

/// The menu's top-left corner in window coordinates, which run down from
/// the top: at the pointer, or below a control. Below a control in the
/// view's right half, the menu's right edge lines up with the control's.
fn origin(anchor: MenuAnchor, view_width: f64, menu_width: f64) -> (f64, f64) {
    let points = |value: gpui::Pixels| f64::from(f32::from(value));
    match anchor {
        MenuAnchor::Pointer(point) => (points(point.x), points(point.y)),
        MenuAnchor::Below(control) => {
            let (left, right) =
                (points(control.left()), points(control.right()));
            let x = if f64::midpoint(left, right) > view_width / 2.0 {
                (right - menu_width).max(0.0)
            } else {
                left
            };
            (x, points(control.bottom()))
        }
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
/// badge, and the dark and light appearances. The menus smoke runs this.
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
    ensure!(is_main_thread()?, "menus are built on the main thread");
    for dark in [true, false] {
        let native = spec.to_native(dark);
        let items = built_items(&native)?;
        check_items(&items, &native)?;
    }
    Ok(())
}

/// Builds the terminal menu `AppKit` would show for a split tab of three
/// panes with one user layout, and checks its Arrange Panes submenu: the
/// layouts' titles in order, the user layout that cannot hold three panes
/// disabled, depth-first tags, and the ids picks resolve to. The menus
/// smoke runs this.
pub(in crate::desktop) fn check_terminal_menu(
    keymap: &crate::keymap::InstalledKeymap,
) -> anyhow::Result<()> {
    use super::terminal_menu::{
        DirectoryState, TerminalMenuInput, terminal_menu,
    };
    use crate::keymap::Platform;

    let input = TerminalMenuInput {
        platform: Platform::MacOs,
        link: false,
        selection: false,
        exited: false,
        scrolled_back: false,
        panes: 3,
        input_sync: false,
        directory: DirectoryState::Local,
    };
    let panes = huterm_config::PanesConfig {
        layouts: [("pair".to_owned(), "columns(pane, pane)".to_owned())].into(),
        ..huterm_config::PanesConfig::default()
    };
    let contexts = ["Workspace", "Terminal"]
        .into_iter()
        .map(gpui::KeyContext::parse)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let spec = terminal_menu(input, &panes.layouts(), keymap, &contexts);
    ensure!(is_main_thread()?, "menus are built on the main thread");
    let native = spec.to_native(true);
    let items = built_items(&native)?;
    let start = items
        .iter()
        .position(|item| item == "Arrange Panes>")
        .with_context(|| format!("no Arrange Panes submenu in {items:?}"))?;
    let expected = [
        ("Equal Columns", "arrange_panes:columns"),
        ("Equal Rows", "arrange_panes:rows"),
        ("Main Left", "arrange_panes:main_left"),
        ("Main Top", "arrange_panes:main_top"),
        ("Main Centered", "arrange_panes:main_centered"),
        ("Tiled", "arrange_panes:tiled"),
        ("pair!", ""),
    ];
    ensure!(
        items.get(start + 1..start + 1 + expected.len())
            == Some(
                &expected
                    .iter()
                    .map(|(title, _)| (*title).to_owned())
                    .collect::<Vec<_>>()[..]
            ),
        "the Arrange Panes submenu lists every layout: {items:?}"
    );
    for (offset, (title, id)) in expected.iter().enumerate() {
        let tag = start + 1 + offset;
        let picked = native.id_at(tag);
        ensure!(
            picked.as_deref() == Some(*id).filter(|id| !id.is_empty()),
            "{title} at tag {tag} picks {picked:?}"
        );
    }
    ensure!(
        native.id_at(start).is_none(),
        "the submenu's own item picks nothing"
    );
    Ok(())
}

/// Builds `native` as `AppKit` would, checks it against `native` through
/// [`check_built`], and returns its entries depth-first.
fn built_items(native: &NativeMenu) -> anyhow::Result<Vec<String>> {
    // SAFETY: The menu is built, read, and released on the main thread;
    // every item read is bounded by its count.
    unsafe {
        let menu = build(native, target()?)?;
        let result = check_built(menu, native);
        let _: () = msg_send![menu, release];
        result
    }
}

/// Checks a menu built from `native`, its submenus included, and returns
/// each entry depth-first: `-` for a separator, a submenu's title and `>`
/// before its own entries, and an item's title, `!` when disabled, its
/// shortcut after `@`, and its badge after `#`.
///
/// # Safety
/// Main thread only, with `menu` built from `native`.
unsafe fn check_built(
    menu: *mut Object,
    native: &NativeMenu,
) -> anyhow::Result<Vec<String>> {
    let mut entries = Vec::new();
    let mut tag = 0;
    // SAFETY: The caller guarantees a live menu on the main thread.
    unsafe {
        read_menu(menu, &native.items, native.dark, &mut tag, &mut entries)?;
    }
    Ok(entries)
}

/// # Safety
/// Main thread only, with `menu` built from `items`.
unsafe fn read_menu(
    menu: *mut Object,
    items: &[NativeItem],
    dark: bool,
    tag: &mut isize,
    entries: &mut Vec<String>,
) -> anyhow::Result<()> {
    // SAFETY: The caller guarantees a live menu on the main thread.
    unsafe {
        let count: usize = msg_send![menu, numberOfItems];
        ensure!(
            count == items.len(),
            "menu has {count} items, expected {}",
            items.len()
        );
        let autoenables: BOOL = msg_send![menu, autoenablesItems];
        ensure!(autoenables == NO, "the menu enables its own items");
        let appearance: *mut Object = msg_send![menu, appearance];
        ensure!(!appearance.is_null(), "the menu has no appearance");
        let name: *mut Object = msg_send![appearance, name];
        let expected = if dark {
            "NSAppearanceNameDarkAqua"
        } else {
            "NSAppearanceNameAqua"
        };
        let name = string(name)?;
        ensure!(name == expected, "the menu's appearance is {name}");
        for (index, entry) in items.iter().enumerate() {
            let entry_tag = *tag;
            *tag += 1;
            let item: *mut Object = msg_send![menu, itemAtIndex: index];
            ensure!(!item.is_null(), "AppKit returned a null menu item");
            let separator: BOOL = msg_send![item, isSeparatorItem];
            if separator == YES {
                entries.push("-".to_owned());
                continue;
            }
            let title: *mut Object = msg_send![item, title];
            let enabled: BOOL = msg_send![item, isEnabled];
            let item_tag: isize = msg_send![item, tag];
            ensure!(
                item_tag == entry_tag,
                "entry {entry_tag} has tag {item_tag}"
            );
            let disabled = if enabled == YES { "" } else { "!" };
            if let NativeItem::Submenu { items, .. } = entry {
                let submenu: *mut Object = msg_send![item, submenu];
                ensure!(!submenu.is_null(), "a submenu item has no submenu");
                let action: Sel = msg_send![item, action];
                // AppKit gives a submenu's item its own `submenuAction:`;
                // it must never report a pick.
                ensure!(action != sel!(pick:), "a submenu item picks");
                entries.push(format!("{}{disabled}>", string(title)?));
                read_menu(submenu, items, dark, tag, entries)?;
                continue;
            }
            let key: *mut Object = msg_send![item, keyEquivalent];
            let mask: usize = msg_send![item, keyEquivalentModifierMask];
            let badge: *mut Object = msg_send![item, badge];
            let badge = if badge.is_null() {
                String::new()
            } else {
                let count: isize = msg_send![badge, itemCount];
                format!("#{count}")
            };
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
            entries
                .push(format!("{}{disabled}{shortcut}{badge}", string(title)?));
        }
        Ok(())
    }
}

/// Checks the window menu's entries as `check_built` read them.
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
                .entries()
                .into_iter()
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, point, px, size};

    #[test]
    fn menus_open_at_the_pointer_or_below_their_control() {
        let control = |x: f32| {
            MenuAnchor::Below(Bounds::new(
                point(px(x), px(4.0)),
                size(px(26.0), px(26.0)),
            ))
        };
        assert_eq!(
            origin(
                MenuAnchor::Pointer(point(px(120.0), px(80.0))),
                800.0,
                230.0
            ),
            (120.0, 80.0)
        );
        assert_eq!(origin(control(20.0), 800.0, 230.0), (20.0, 30.0));
        assert_eq!(
            origin(control(770.0), 800.0, 230.0),
            (566.0, 30.0),
            "a control on the right lines the menu up with its right edge"
        );
        assert_eq!(
            origin(control(500.0), 520.0, 600.0),
            (0.0, 30.0),
            "a menu wider than the room keeps its left edge in the view"
        );
    }
}
