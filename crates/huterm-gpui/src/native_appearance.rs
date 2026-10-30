//! The native window frame follows the theme: each window's `NSWindow`
//! appearance is Aqua under a light theme and Dark Aqua under a dark one,
//! so its border, title bar, and traffic lights match the theme rather
//! than the system's light or dark mode. Linux frames come from the window
//! manager or Huterm's own drawing, so there this does nothing.
//!
//! Huterm never picks a theme from an appearance. If it ever does, it must
//! read the application's appearance (`NSApp.effectiveAppearance`, GPUI's
//! `App::window_appearance`), never a window's, which this module sets.
#![cfg_attr(target_os = "macos", allow(unsafe_code, unexpected_cfgs))]

use gpui::{Context, Window};

/// The appearance one window last asked for, so the setter runs only when
/// the theme turns from light to dark or back.
#[derive(Default)]
pub(crate) struct FrameAppearance {
    applied: Option<bool>,
}

impl FrameAppearance {
    /// Gives `window`'s frame the `dark` or light appearance unless it
    /// already has it. The setter runs on a foreground turn after this
    /// update: `AppKit` relays the change to GPUI's window state at once.
    pub(crate) fn sync<V: 'static>(
        &mut self,
        dark: bool,
        window: &Window,
        cx: &mut Context<'_, V>,
    ) {
        if self.applied == Some(dark) {
            return;
        }
        self.applied = Some(dark);
        #[cfg(target_os = "macos")]
        match macos::NativeWindow::of(window) {
            Ok(native) => cx
                .spawn(async move |_, _| {
                    if let Err(error) = native.set_dark(dark) {
                        eprintln!("Window appearance: {error}");
                    }
                })
                .detach(),
            Err(error) => eprintln!("Window appearance: {error}"),
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (window, cx);
    }
}

/// The name of `window`'s own `NSWindow` appearance, such as
/// `NSAppearanceNameAqua`, or `none` while it follows the system. Smokes
/// report it to check that the frame follows the theme.
#[cfg(target_os = "macos")]
pub(crate) fn frame_appearance_name(window: &Window) -> anyhow::Result<String> {
    macos::NativeWindow::of(window)?.appearance_name()
}

#[cfg(target_os = "macos")]
mod macos {
    use anyhow::{Context as _, ensure};
    use gpui::Window;
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// A retained `NSWindow`, released on drop.
    pub(super) struct NativeWindow(*mut Object);

    impl NativeWindow {
        pub(super) fn of(window: &Window) -> anyhow::Result<Self> {
            let RawWindowHandle::AppKit(handle) =
                HasWindowHandle::window_handle(window)
                    .map_err(|error| {
                        anyhow::anyhow!("native window handle: {error}")
                    })?
                    .as_raw()
            else {
                anyhow::bail!("the frame appearance requires an AppKit window");
            };
            let view = handle.ns_view.as_ptr().cast::<Object>();
            // SAFETY: The borrowed handle names GPUI's live NSView on the
            // main thread; the retain keeps its window alive for the
            // deferred setter.
            unsafe {
                let native: *mut Object = msg_send![view, window];
                ensure!(!native.is_null(), "GPUI view has no window");
                let _: *mut Object = msg_send![native, retain];
                Ok(Self(native))
            }
        }

        pub(super) fn set_dark(&self, dark: bool) -> anyhow::Result<()> {
            let name = if dark {
                c"NSAppearanceNameDarkAqua"
            } else {
                c"NSAppearanceNameAqua"
            };
            // SAFETY: A retained main-thread NSWindow and AppKit's named
            // system appearances, outside any GPUI borrow.
            unsafe {
                let name: *mut Object = msg_send![
                    Class::get("NSString").context("NSString")?,
                    stringWithUTF8String: name.as_ptr()
                ];
                let appearance: *mut Object = msg_send![
                    Class::get("NSAppearance").context("NSAppearance")?,
                    appearanceNamed: name
                ];
                ensure!(!appearance.is_null(), "no system appearance");
                let _: () = msg_send![self.0, setAppearance: appearance];
            }
            Ok(())
        }
    }

    impl NativeWindow {
        pub(super) fn appearance_name(&self) -> anyhow::Result<String> {
            // SAFETY: A retained main-thread NSWindow; its appearance and
            // the appearance's name are AppKit-owned for this read.
            unsafe {
                let appearance: *mut Object = msg_send![self.0, appearance];
                if appearance.is_null() {
                    return Ok("none".to_owned());
                }
                let name: *mut Object = msg_send![appearance, name];
                ensure!(!name.is_null(), "the appearance has no name");
                let text: *const std::ffi::c_char = msg_send![name, UTF8String];
                ensure!(!text.is_null(), "the appearance name is not UTF-8");
                Ok(std::ffi::CStr::from_ptr(text)
                    .to_string_lossy()
                    .into_owned())
            }
        }
    }

    impl Drop for NativeWindow {
        fn drop(&mut self) {
            // SAFETY: Balances the retain in `of` on the main thread; the
            // handle is never nil.
            unsafe {
                let _: () = msg_send![self.0, release];
            }
        }
    }
}
