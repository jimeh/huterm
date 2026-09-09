//! Only this helper constructs Objective-C events; `AppKit` dispatches them later.
#![allow(unsafe_code, unexpected_cfgs)]

use anyhow::{Context as _, ensure};
use objc::runtime::{Class, NO, Object};
use objc::{msg_send, sel, sel_impl};
use std::ffi::CString;

pub(super) fn post(command: &str) -> anyhow::Result<()> {
    let fields: Vec<_> = command.split('\t').collect();
    if fields[0] == "mouse" && matches!(fields.len(), 4 | 5) {
        return post_mouse(&fields);
    }
    ensure!(
        fields.len() == 4,
        "expected keycode, flags, characters, plain characters"
    );
    let key: u16 = fields[0].parse()?;
    let flags: usize = fields[1].parse()?;
    let characters = CString::new(fields[2])?;
    let plain = CString::new(fields[3])?;
    // SAFETY: Called on the AppKit main thread, outside GPUI's App borrow.
    // AppKit owns the window and retains the autoreleased event when queued.
    // Posting through the normal event loop sets NSApplication.currentEvent,
    // unlike directly invoking NSView.keyDown: or NSWindow.sendEvent:.
    unsafe {
        let app: *mut Object = msg_send![
            Class::get("NSApplication").context("NSApplication")?,
            sharedApplication
        ];
        let window: *mut Object = msg_send![app, keyWindow];
        ensure!(!window.is_null(), "no key window");
        let number: isize = msg_send![window, windowNumber];
        let string = Class::get("NSString").context("NSString")?;
        let characters: *mut Object =
            msg_send![string, stringWithUTF8String: characters.as_ptr()];
        let plain: *mut Object =
            msg_send![string, stringWithUTF8String: plain.as_ptr()];
        let event: *mut Object = msg_send![Class::get("NSEvent").context("NSEvent")?,
            keyEventWithType: if key == 56 { 12_usize } else { 10_usize }
            location: gpui::point(0.0_f64, 0.0_f64)
            modifierFlags: flags
            timestamp: 0.0_f64
            windowNumber: number
            context: std::ptr::null_mut::<Object>()
            characters: characters
            charactersIgnoringModifiers: plain
            isARepeat: NO
            keyCode: key];
        ensure!(!event.is_null(), "NSEvent construction failed");
        let _: () = msg_send![app, postEvent: event atStart: NO];
    }
    Ok(())
}

fn post_mouse(fields: &[&str]) -> anyhow::Result<()> {
    let kind: usize = fields[1].parse()?;
    let x: f64 = fields[2].parse()?;
    let y: f64 = fields[3].parse()?;
    let flags: usize = fields.get(4).map_or(Ok(0), |value| value.parse())?;
    // SAFETY: Main-thread AppKit objects; the queue retains the event.
    unsafe {
        let app: *mut Object = msg_send![
            Class::get("NSApplication").context("NSApplication")?,
            sharedApplication
        ];
        let window: *mut Object = msg_send![app, keyWindow];
        ensure!(!window.is_null(), "no key window for mouse event");
        let number: isize = msg_send![window, windowNumber];
        let view: *mut Object = msg_send![window, contentView];
        let bounds: gpui::Bounds<f64> = msg_send![view, bounds];
        let event: *mut Object = msg_send![Class::get("NSEvent").context("NSEvent")?,
            mouseEventWithType: kind
            location: gpui::point(if x < 1.0 { x * bounds.size.width } else { x }, bounds.size.height - y)
            modifierFlags: flags timestamp: 0.0_f64 windowNumber: number
            context: std::ptr::null_mut::<Object>() eventNumber: 0_isize
            clickCount: 1_isize pressure: 1.0_f32];
        ensure!(!event.is_null(), "mouse NSEvent construction failed");
        let _: () = msg_send![app, postEvent: event atStart: NO];
    }
    Ok(())
}

pub(super) fn validate_layout() -> anyhow::Result<()> {
    use std::ffi::{CStr, c_void};
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn TISCopyCurrentKeyboardInputSource() -> *const c_void;
        fn TISGetInputSourceProperty(
            source: *const c_void,
            key: *const c_void,
        ) -> *const c_void;
        static kTISPropertyInputSourceID: *const c_void;
    }
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
    }
    // SAFETY: Carbon returns a retained source. Copy its borrowed string before
    // releasing it. This reads the selected layout without changing it.
    let identifier = unsafe {
        let source = TISCopyCurrentKeyboardInputSource();
        ensure!(!source.is_null(), "no current keyboard input source");
        let name = TISGetInputSourceProperty(source, kTISPropertyInputSourceID)
            .cast::<Object>();
        ensure!(!name.is_null(), "input source has no identifier");
        let bytes: *const std::ffi::c_char = msg_send![name, UTF8String];
        ensure!(!bytes.is_null(), "input source identifier is not UTF-8");
        let name = CStr::from_ptr(bytes).to_string_lossy().into_owned();
        CFRelease(source);
        name
    };
    ensure!(
        [
            "com.apple.keylayout.US",
            "com.apple.keylayout.ABC",
            "com.apple.keylayout.British"
        ]
        .contains(&identifier.as_str()),
        "native input smoke requires US, ABC, or British layout; selected {identifier}; select a supported layout before running"
    );
    println!("NATIVE_INPUT_SMOKE layout={identifier}");
    Ok(())
}

/// Preserve every pasteboard item's declared data types across smoke failures.
pub(super) fn clipboard_file(mode: &str, file: &str) -> anyhow::Result<()> {
    use objc::runtime::YES;
    let file = CString::new(file)?;
    // SAFETY: This short-lived main-thread helper owns no GPUI objects. All
    // collection members remain retained by their containers until process exit.
    unsafe {
        let pool: *mut Object = msg_send![
            Class::get("NSAutoreleasePool").context("NSAutoreleasePool")?,
            new
        ];
        let name: *mut Object = msg_send![Class::get("NSString").context("NSString")?, stringWithUTF8String: file.as_ptr()];
        let pasteboard: *mut Object = msg_send![
            Class::get("NSPasteboard").context("NSPasteboard")?,
            generalPasteboard
        ];
        let array = Class::get("NSMutableArray").context("NSMutableArray")?;
        let serialization = Class::get("NSPropertyListSerialization")
            .context("NSPropertyListSerialization")?;
        if mode == "clipboard-save" {
            let saved: *mut Object = msg_send![array, array];
            let items: *mut Object = msg_send![pasteboard, pasteboardItems];
            let count: usize = msg_send![items, count];
            for index in 0..count {
                let item: *mut Object = msg_send![items, objectAtIndex: index];
                let types: *mut Object = msg_send![item, types];
                let entry: *mut Object = msg_send![
                    Class::get("NSMutableDictionary")
                        .context("NSMutableDictionary")?,
                    dictionary
                ];
                let count: usize = msg_send![types, count];
                for index in 0..count {
                    let kind: *mut Object =
                        msg_send![types, objectAtIndex: index];
                    let data: *mut Object = msg_send![item, dataForType: kind];
                    ensure!(
                        !data.is_null(),
                        "cannot materialize clipboard type for preservation"
                    );
                    let _: () = msg_send![entry, setObject: data forKey: kind];
                }
                let _: () = msg_send![saved, addObject: entry];
            }
            let data: *mut Object = msg_send![serialization, dataWithPropertyList: saved format: 200_usize options: 0_usize error: std::ptr::null_mut::<*mut Object>()];
            ensure!(!data.is_null(), "cannot serialize clipboard snapshot");
            let written: objc::runtime::BOOL =
                msg_send![data, writeToFile: name atomically: YES];
            ensure!(written == YES, "cannot write clipboard snapshot");
        } else {
            ensure!(
                mode == "clipboard-restore",
                "unknown clipboard helper mode"
            );
            let data: *mut Object = msg_send![Class::get("NSData").context("NSData")?, dataWithContentsOfFile: name];
            ensure!(!data.is_null(), "cannot read clipboard snapshot");
            let saved: *mut Object = msg_send![serialization, propertyListWithData: data options: 0_usize format: std::ptr::null_mut::<usize>() error: std::ptr::null_mut::<*mut Object>()];
            ensure!(!saved.is_null(), "cannot decode clipboard snapshot");
            let items: *mut Object = msg_send![array, array];
            let count: usize = msg_send![saved, count];
            for index in 0..count {
                let entry: *mut Object = msg_send![saved, objectAtIndex: index];
                let types: *mut Object = msg_send![entry, allKeys];
                let item: *mut Object = msg_send![
                    Class::get("NSPasteboardItem")
                        .context("NSPasteboardItem")?,
                    new
                ];
                let count: usize = msg_send![types, count];
                for index in 0..count {
                    let kind: *mut Object =
                        msg_send![types, objectAtIndex: index];
                    let data: *mut Object =
                        msg_send![entry, objectForKey: kind];
                    let accepted: objc::runtime::BOOL =
                        msg_send![item, setData: data forType: kind];
                    ensure!(accepted == YES, "cannot restore clipboard type");
                }
                let _: () = msg_send![items, addObject: item];
                let _: () = msg_send![item, release];
            }
            let _: isize = msg_send![pasteboard, clearContents];
            if count != 0 {
                let restored: objc::runtime::BOOL =
                    msg_send![pasteboard, writeObjects: items];
                ensure!(restored == YES, "cannot restore clipboard items");
            }
        }
        let _: () = msg_send![pool, drain];
    }
    Ok(())
}
