//! `AppKit` dragging destination callbacks, backed by a private native pasteboard.
#![allow(unsafe_code, unexpected_cfgs)]
use anyhow::{Context as _, ensure};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use std::ffi::CString;
use std::sync::OnceLock;

#[repr(C)]
struct NativePoint {
    x: f64,
    y: f64,
}
unsafe impl objc::Encode for NativePoint {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{CGPoint=dd}") }
    }
}

extern "C" fn pasteboard(this: &Object, _: Sel) -> *mut Object {
    unsafe { *this.get_ivar("pasteboard") }
}
extern "C" fn location(this: &Object, _: Sel) -> NativePoint {
    unsafe {
        NativePoint {
            x: *this.get_ivar("x"),
            y: *this.get_ivar("y"),
        }
    }
}

pub(super) fn drop_event(command: &str) -> anyhow::Result<()> {
    let fields: Vec<_> = command.split('\t').collect();
    ensure!(
        fields.len() == 4,
        "drop requires phase, x, y, and path-list file"
    );
    let x: f64 = fields[1].parse()?;
    let y: f64 = fields[2].parse()?;
    let paths = std::fs::read_to_string(fields[3])?;
    // SAFETY: The probe runs on AppKit's main thread outside any GPUI borrow.
    // The native callback synchronously copies our pasteboard paths into GPUI.
    unsafe {
        static INFO: OnceLock<&Class> = OnceLock::new();
        let class = INFO.get_or_init(|| {
            let mut class = ClassDecl::new(
                "HutermIntegrationDraggingInfo",
                Class::get("NSObject").expect("NSObject"),
            )
            .expect("drag info class");
            class.add_ivar::<*mut Object>("pasteboard");
            class.add_ivar::<f64>("x");
            class.add_ivar::<f64>("y");
            class.add_method(
                sel!(draggingPasteboard),
                pasteboard as extern "C" fn(&Object, Sel) -> *mut Object,
            );
            class.add_method(
                sel!(draggingLocation),
                location as extern "C" fn(&Object, Sel) -> NativePoint,
            );
            class.register()
        });
        let app: *mut Object = msg_send![
            Class::get("NSApplication").context("NSApplication")?,
            sharedApplication
        ];
        let window: *mut Object = msg_send![app, keyWindow];
        ensure!(!window.is_null(), "no native drop target");
        let view: *mut Object = msg_send![window, contentView];
        let bounds: gpui::Bounds<f64> = msg_send![view, bounds];
        let board: *mut Object = msg_send![
            Class::get("NSPasteboard").context("NSPasteboard")?,
            pasteboardWithUniqueName
        ];
        let array: *mut Object = msg_send![
            Class::get("NSMutableArray").context("NSMutableArray")?,
            array
        ];
        let string = Class::get("NSString").context("NSString")?;
        for path in paths.lines() {
            let value = CString::new(path)?;
            let value: *mut Object =
                msg_send![string, stringWithUTF8String: value.as_ptr()];
            let _: () = msg_send![array, addObject: value];
        }
        let kind = CString::new("NSFilenamesPboardType")?;
        let kind: *mut Object =
            msg_send![string, stringWithUTF8String: kind.as_ptr()];
        let types: *mut Object = msg_send![Class::get("NSArray").context("NSArray")?, arrayWithObject: kind];
        let _: isize = msg_send![board, declareTypes: types owner: std::ptr::null_mut::<Object>()];
        let _: bool = msg_send![board, setPropertyList: array forType: kind];
        let info: *mut Object = msg_send![*class, new];
        (*info).set_ivar("pasteboard", board);
        (*info).set_ivar("x", x);
        (*info).set_ivar("y", bounds.size.height - y);
        match fields[0] {
            "enter" => {
                let _: usize = msg_send![window, draggingEntered: info];
            }
            "move" => {
                let _: usize = msg_send![window, draggingUpdated: info];
            }
            "drop" => {
                let _: bool = msg_send![window, performDragOperation: info];
            }
            "exit" => {
                let _: () = msg_send![window, draggingExited: info];
            }
            phase => anyhow::bail!("unknown drag phase {phase}"),
        }
        let _: () = msg_send![info, release];
        let _: () = msg_send![board, releaseGlobally];
    }
    Ok(())
}

pub(super) fn close_window() -> anyhow::Result<()> {
    // SAFETY: Called on AppKit's main thread outside the GPUI App borrow.
    unsafe {
        let app: *mut Object = msg_send![
            Class::get("NSApplication").context("NSApplication")?,
            sharedApplication
        ];
        let window: *mut Object = msg_send![app, keyWindow];
        ensure!(!window.is_null(), "no native close target");
        let _: () =
            msg_send![window, performClose: std::ptr::null_mut::<Object>()];
    }
    Ok(())
}
