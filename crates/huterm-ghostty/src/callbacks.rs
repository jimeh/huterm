//! Effect callbacks that libghostty-vt invokes during `vt_write`.
//!
//! terminal.h: callbacks run synchronously inside the write and must not
//! write to the same terminal. The only userdata is a pointer to a
//! [`State`] owned by `Terminal`, reached here through a shared reference
//! and interior mutability, never `&mut`. Host code receives plain values,
//! never the terminal. Each trampoline catches panics, answers with the safe
//! C default, and poisons the terminal.
#![expect(
    unsafe_code,
    reason = "C invokes these trampolines with raw pointers"
)]
#![deny(
    clippy::as_pointer_underscore,
    clippy::as_ptr_cast_mut,
    clippy::as_underscore,
    clippy::fn_to_numeric_cast_any,
    clippy::mem_forget,
    clippy::missing_safety_doc,
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    clippy::unnecessary_safety_comment,
    clippy::unnecessary_safety_doc
)]

use core::ffi::c_void;
use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;

use crate::ffi::{self, keys::terminal_data as data};
use crate::native::{NativeTerminal, TerminalRef, string_bytes};
use crate::types::{
    ClipboardContent, ClipboardLocation, ClipboardWrite, ClipboardWriteResult,
    ColorScheme, Effect, Rgb,
};

/// Host policy invoked from inside a terminal write.
///
/// Implementations receive plain values and must not block for long: the
/// write waits for them.
pub trait Host {
    /// Answers a clipboard write. The default refuses it.
    fn clipboard_write(
        &mut self,
        request: &ClipboardWrite<'_>,
    ) -> ClipboardWriteResult {
        let _ = request;
        ClipboardWriteResult::Unsupported
    }

    /// Answers `CSI ? 996 n` for the effective background, or stays silent
    /// with `None`, the default.
    fn color_scheme(&mut self, background: Rgb) -> Option<ColorScheme> {
        let _ = background;
        None
    }
}

/// Per-terminal callback state, at a stable heap address for the terminal's
/// lifetime.
pub(crate) struct State<H> {
    pub(crate) host: RefCell<H>,
    pub(crate) effects: RefCell<Vec<Effect>>,
    pub(crate) poisoned: Cell<bool>,
    /// Cell pixel size for size reports; zero means unknown.
    pub(crate) cell_size: Cell<(u32, u32)>,
    pub(crate) device_attributes: Option<ffi::GhosttyDeviceAttributes>,
    pub(crate) xtversion: Option<Box<[u8]>>,
}

impl<H> State<H> {
    fn push(&self, effect: Effect) {
        self.effects.borrow_mut().push(effect);
    }
}

/// Runs `body` against the state behind `userdata`, containing panics.
fn guard<H, R>(
    userdata: *mut c_void,
    fallback: R,
    body: impl FnOnce(&State<H>) -> R,
) -> R {
    // SAFETY: terminal.h passes the `GHOSTTY_TERMINAL_OPT_USERDATA` pointer
    // to every callback. `Terminal` installs a pointer to its `State<H>`,
    // frees the state only after the native terminal, and callbacks run
    // only inside calls made through that terminal.
    let Some(state) = (unsafe { userdata.cast::<State<H>>().as_ref() }) else {
        return fallback;
    };
    if state.poisoned.get() {
        return fallback;
    }
    catch_unwind(AssertUnwindSafe(|| body(state))).unwrap_or_else(|_| {
        state.poisoned.set(true);
        fallback
    })
}

/// # Safety
///
/// C calls this with the userdata pointer `install` configured and `len`
/// readable bytes at `data`.
unsafe extern "C" fn write_pty<H: Host>(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    guard::<H, ()>(userdata, (), |state| {
        let bytes = if data.is_null() || len == 0 {
            &[][..]
        } else {
            // SAFETY: terminal.h `GhosttyTerminalWritePtyFn`: `data` holds
            // `len` bytes valid for the duration of the call.
            unsafe { std::slice::from_raw_parts(data, len) }
        };
        state.push(Effect::PtyWrite(bytes.to_vec()));
    });
}

/// # Safety
///
/// C calls this with the userdata pointer `install` configured.
unsafe extern "C" fn bell<H: Host>(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
) {
    guard::<H, ()>(userdata, (), |state| state.push(Effect::Bell));
}

/// # Safety
///
/// C calls this with the userdata pointer `install` configured.
unsafe extern "C" fn title_changed<H: Host>(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
) {
    guard::<H, ()>(userdata, (), |state| state.push(Effect::TitleChanged));
}

/// # Safety
///
/// C calls this with a live terminal and the userdata pointer `install`
/// configured.
unsafe extern "C" fn pwd_changed<H: Host>(
    terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
) {
    guard::<H, ()>(userdata, (), |state| {
        // SAFETY: terminal.h `GhosttyTerminalPwdChangedFn` directs callers to
        // read `GHOSTTY_TERMINAL_DATA_PWD` inside the callback; the write
        // that invokes it does not mutate the terminal until it returns.
        let Some(terminal) = (unsafe { TerminalRef::from_callback(terminal) })
        else {
            return;
        };
        if let Ok(pwd) = terminal.string::<data::Pwd>() {
            state.push(Effect::PwdChanged(pwd.to_vec()));
        }
    });
}

/// # Safety
///
/// C calls this with the userdata pointer `install` configured.
unsafe extern "C" fn xtversion<H: Host>(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
) -> ffi::GhosttyString {
    let empty = ffi::GhosttyString {
        ptr: std::ptr::null(),
        len: 0,
    };
    guard::<H, _>(userdata, empty, |state| {
        // terminal.h: the returned bytes must stay valid until the callback
        // returns; they live in the state for the terminal's lifetime.
        state
            .xtversion
            .as_deref()
            .map_or(empty, |version| ffi::GhosttyString {
                ptr: version.as_ptr(),
                len: version.len(),
            })
    })
}

/// # Safety
///
/// C calls this with a live terminal, the configured userdata pointer, and
/// a writable `out_size`.
unsafe extern "C" fn size<H: Host>(
    terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    out_size: *mut ffi::GhosttySizeReportSize,
) -> bool {
    guard::<H, _>(userdata, false, |state| {
        let (cell_width, cell_height) = state.cell_size.get();
        if cell_width == 0 || cell_height == 0 {
            return false;
        }
        // SAFETY: reading the grid size is a bounded read-only query; the
        // write invoking this callback does not mutate the terminal until it
        // returns.
        let Some(terminal) = (unsafe { TerminalRef::from_callback(terminal) })
        else {
            return false;
        };
        let (Ok(columns), Ok(rows)) = (
            terminal.value::<data::Cols>(),
            terminal.value::<data::Rows>(),
        ) else {
            return false;
        };
        let report = ffi::GhosttySizeReportSize {
            rows,
            columns,
            cell_width,
            cell_height,
        };
        // SAFETY: terminal.h `GhosttyTerminalSizeFn`: `out_size` points to a
        // size struct the callback fills before returning true.
        unsafe { out_size.write(report) };
        true
    })
}

/// # Safety
///
/// C calls this with a live terminal, the configured userdata pointer, and
/// a writable `out_scheme`.
unsafe extern "C" fn color_scheme<H: Host>(
    terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    out_scheme: *mut ffi::GhosttyColorScheme,
) -> bool {
    guard::<H, _>(userdata, false, |state| {
        // SAFETY: reading the effective background is a bounded read-only
        // query during the write that invokes this callback.
        let Some(terminal) = (unsafe { TerminalRef::from_callback(terminal) })
        else {
            return false;
        };
        let Ok(background) = terminal.value::<data::ColorBackground>() else {
            return false;
        };
        let Some(scheme) =
            state.host.borrow_mut().color_scheme(background.into())
        else {
            return false;
        };
        let scheme = match scheme {
            ColorScheme::Light => ffi::GHOSTTY_COLOR_SCHEME_LIGHT,
            ColorScheme::Dark => ffi::GHOSTTY_COLOR_SCHEME_DARK,
        };
        // SAFETY: terminal.h `GhosttyTerminalColorSchemeFn`: `out_scheme` is
        // filled before returning true.
        unsafe { out_scheme.write(scheme) };
        true
    })
}

/// # Safety
///
/// C calls this with the configured userdata pointer and a writable
/// `out_attrs`.
unsafe extern "C" fn device_attributes<H: Host>(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    out_attrs: *mut ffi::GhosttyDeviceAttributes,
) -> bool {
    guard::<H, _>(userdata, false, |state| {
        let Some(attributes) = state.device_attributes else {
            return false;
        };
        // SAFETY: terminal.h `GhosttyTerminalDeviceAttributesFn`: `out_attrs`
        // is filled before returning true.
        unsafe { out_attrs.write(attributes) };
        true
    })
}

/// Borrows the request's contents for the callback.
///
/// # Safety
///
/// `request` must be the live request passed to the clipboard callback.
unsafe fn clipboard_request<'a>(
    request: &'a ffi::GhosttyClipboardWrite,
) -> ClipboardWrite<'a> {
    let raw: &'a [ffi::GhosttyClipboardContent] = if request.contents.is_null()
        || request.contents_len == 0
    {
        &[]
    } else {
        // SAFETY: terminal.h `GhosttyClipboardWrite`: `contents` holds
        // `contents_len` entries borrowed for the callback.
        unsafe {
            std::slice::from_raw_parts(request.contents, request.contents_len)
        }
    };
    let contents = raw
        .iter()
        .map(|content| ClipboardContent {
            // SAFETY: MIME and data strings are borrowed for the callback.
            mime: unsafe { string_bytes(content.mime) },
            // SAFETY: as above.
            data: unsafe { string_bytes(content.data) },
        })
        .collect();
    ClipboardWrite {
        location: ClipboardLocation::from_ffi(request.location),
        contents,
        // SAFETY: the program name is borrowed for the callback.
        name: unsafe { string_bytes(request.name) },
        granted: request.granted,
        can_remember: request.can_remember,
    }
}

/// # Safety
///
/// C calls this with the configured userdata pointer and a request that is
/// valid until the callback returns.
unsafe extern "C" fn clipboard_write<H: Host>(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    write: *const ffi::GhosttyClipboardWrite,
) {
    guard::<H, ()>(userdata, (), |state| {
        // SAFETY: terminal.h `GhosttyTerminalClipboardWriteFn`: `write` is a
        // borrowed request valid for the callback.
        let Some(request) = (unsafe { write.as_ref() }) else {
            return;
        };
        // The request is a sized struct; every field read here predates it.
        if request.size < size_of::<ffi::GhosttyClipboardWrite>() {
            return;
        }
        let Some(reply) = request.reply else {
            return;
        };
        // SAFETY: `request` is the live request passed to this callback.
        let view = unsafe { clipboard_request(request) };
        let result = state.host.borrow_mut().clipboard_write(&view);
        let mut answer: ffi::GhosttyClipboardWriteReply = ffi::sized();
        answer.result = result.to_ffi();
        // SAFETY: terminal.h `GhosttyClipboardWrite::reply` answers the
        // request during the callback; the reply is borrowed for the call.
        unsafe { reply(write, &raw const answer) };
    });
}

/// Owns the callback state at a stable heap address.
struct StateBox<H>(NonNull<State<H>>);

impl<H> Drop for StateBox<H> {
    fn drop(&mut self) {
        // SAFETY: the pointer came from `Box::leak` in `Bound::new`, and
        // `Bound` drops its native terminal, the only other holder, first.
        drop(unsafe { Box::from_raw(self.0.as_ptr()) });
    }
}

/// A native terminal bound to its callback state.
pub(crate) struct Bound<H> {
    // Declared before `state` so the terminal, which holds the state
    // pointer as userdata, is freed first.
    native: NativeTerminal,
    state: StateBox<H>,
}

impl<H> std::fmt::Debug for Bound<H> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Bound")
            .field("native", &self.native)
            .field("poisoned", &self.state().poisoned.get())
            .finish_non_exhaustive()
    }
}

impl<H: Host> Bound<H> {
    /// Moves `state` to the heap and installs every callback Huterm
    /// handles, with the state as userdata.
    pub(crate) fn new(
        native: NativeTerminal,
        state: State<H>,
    ) -> crate::Result<Self> {
        use ffi::keys::terminal_option as option;

        let has_device_attributes = state.device_attributes.is_some();
        let has_xtversion = state.xtversion.is_some();
        // Bind first so an installation error still drops the terminal
        // before the state.
        let mut bound = Self {
            native,
            state: StateBox(NonNull::from(Box::leak(Box::new(state)))),
        };
        let pointer = bound.state.0.as_ptr().cast();
        let native = &mut bound.native;
        // SAFETY: the state lives until `Bound` drops, after the terminal,
        // and every trampoline reads it only as a shared `State<H>`.
        unsafe {
            native.set_pointer::<option::Userdata>(pointer)?;
        }
        native.set_callback::<option::WritePty>(Some(write_pty::<H>))?;
        native.set_callback::<option::Bell>(Some(bell::<H>))?;
        native
            .set_callback::<option::TitleChanged>(Some(title_changed::<H>))?;
        native.set_callback::<option::PwdChanged>(Some(pwd_changed::<H>))?;
        native.set_callback::<option::Size>(Some(size::<H>))?;
        native.set_callback::<option::ColorScheme>(Some(color_scheme::<H>))?;
        native.set_callback::<option::ClipboardWrite>(Some(
            clipboard_write::<H>,
        ))?;
        if has_device_attributes {
            native.set_callback::<option::DeviceAttributes>(Some(
                device_attributes::<H>,
            ))?;
        }
        if has_xtversion {
            native.set_callback::<option::Xtversion>(Some(xtversion::<H>))?;
        }
        Ok(bound)
    }
}

impl<H> Bound<H> {
    pub(crate) fn state(&self) -> &State<H> {
        // SAFETY: the box is live for `self`'s lifetime and only ever
        // shared-borrowed.
        unsafe { self.state.0.as_ref() }
    }

    pub(crate) const fn native(&self) -> &NativeTerminal {
        &self.native
    }

    pub(crate) const fn native_mut(&mut self) -> &mut NativeTerminal {
        &mut self.native
    }
}

/// Library contracts that need callbacks the safe API never installs.
#[cfg(test)]
mod contract_tests {
    use super::*;
    use ffi::keys::terminal_option as option;

    #[derive(Debug, Default)]
    struct Replies(RefCell<Vec<Vec<u8>>>);

    /// # Safety
    ///
    /// `userdata` points to a live `Replies` and `data` holds `len` bytes.
    unsafe extern "C" fn record(
        _terminal: ffi::GhosttyTerminal,
        userdata: *mut c_void,
        data: *const u8,
        len: usize,
    ) {
        // SAFETY: the test installs a `Replies` that outlives the terminal.
        let replies = unsafe { &*userdata.cast::<Replies>() };
        // SAFETY: terminal.h: `data` holds `len` bytes for the call.
        let bytes = unsafe { std::slice::from_raw_parts(data, len) };
        replies.0.borrow_mut().push(bytes.to_vec());
    }

    /// # Safety
    ///
    /// Never dereferences its arguments.
    unsafe extern "C" fn decline_attributes(
        _terminal: ffi::GhosttyTerminal,
        _userdata: *mut c_void,
        _out: *mut ffi::GhosttyDeviceAttributes,
    ) -> bool {
        false
    }

    /// # Safety
    ///
    /// Never dereferences its arguments.
    unsafe extern "C" fn ignore_clipboard(
        _terminal: ffi::GhosttyTerminal,
        _userdata: *mut c_void,
        _write: *const ffi::GhosttyClipboardWrite,
    ) {
    }

    fn terminal(replies: &Replies) -> NativeTerminal {
        let mut terminal = NativeTerminal::new(None, 8, 3).unwrap();
        // SAFETY: `replies` outlives the terminal in every test, and
        // `record` is the only callback reading it.
        unsafe {
            terminal
                .set_pointer::<option::Userdata>(
                    std::ptr::from_ref(replies).cast_mut().cast(),
                )
                .unwrap();
        }
        terminal
            .set_callback::<option::WritePty>(Some(record))
            .unwrap();
        terminal
    }

    #[test]
    fn declined_device_attributes_still_reply_with_defaults() {
        let replies = Replies::default();
        let mut terminal = terminal(&replies);
        terminal
            .set_callback::<option::DeviceAttributes>(Some(decline_attributes))
            .unwrap();
        terminal.vt_write(b"\x1b[c\x1b[>c");
        drop(terminal);
        assert_eq!(
            replies.0.take(),
            [b"\x1b[?62;22c".to_vec(), b"\x1b[>1;0;0c".to_vec()]
        );
    }

    #[test]
    fn clipboard_write_without_a_reply_is_denied() {
        let replies = Replies::default();
        let mut terminal = terminal(&replies);
        terminal
            .set_callback::<option::ClipboardWrite>(Some(ignore_clipboard))
            .unwrap();
        terminal.vt_write(
            b"\x1b]5522;type=write\x1b\\\x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;YQ==\x1b\\\x1b]5522;type=wdata\x1b\\",
        );
        drop(terminal);
        assert_eq!(
            replies.0.take(),
            [b"\x1b]5522;type=write:status=EPERM\x1b\\".to_vec()]
        );
    }
}
