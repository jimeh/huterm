//! A counting allocator passed through allocator.h's vtable, for tests
//! that prove every native allocation is released.
#![expect(unsafe_code, reason = "the allocator vtable is a set of C callbacks")]
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
use std::alloc::Layout;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::ffi;

/// Allocation counters for one tracking allocator.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    live: AtomicUsize,
    live_bytes: AtomicUsize,
    total: AtomicUsize,
    /// Bit `n` is set once `alloc` receives an alignment argument of `n`.
    alignments: AtomicU32,
}

impl Counters {
    pub(crate) fn live(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    pub(crate) fn live_bytes(&self) -> usize {
        self.live_bytes.load(Ordering::SeqCst)
    }

    pub(crate) fn total(&self) -> usize {
        self.total.load(Ordering::SeqCst)
    }

    /// Distinct alignment arguments `alloc` received.
    pub(crate) fn alignment_arguments(&self) -> Vec<u8> {
        let seen = self.alignments.load(Ordering::SeqCst);
        (0..32).filter(|bit| seen & (1 << bit) != 0).collect()
    }
}

/// Leaks a tracking allocator so native objects can hold it for their whole
/// lifetime, as allocator.h requires.
pub(crate) fn tracking() -> (&'static Counters, &'static ffi::GhosttyAllocator)
{
    static VTABLE: ffi::GhosttyAllocatorVtable = ffi::GhosttyAllocatorVtable {
        alloc: Some(alloc),
        resize: Some(resize),
        remap: Some(remap),
        free: Some(free),
    };
    let counters: &'static Counters = Box::leak(Box::default());
    let allocator = Box::leak(Box::new(ffi::GhosttyAllocator {
        ctx: std::ptr::from_ref(counters).cast_mut().cast(),
        vtable: &raw const VTABLE,
    }));
    (counters, allocator)
}

fn counters<'a>(ctx: *mut c_void) -> &'a Counters {
    // SAFETY: `tracking` installs a leaked `Counters` as the context of
    // every allocator it creates.
    unsafe { &*ctx.cast::<Counters>() }
}

/// allocator.h describes `alignment` as a byte count, but the pinned
/// library passes Zig's `std.mem.Alignment`, the base-2 logarithm.
fn layout(len: usize, alignment: u8) -> Option<Layout> {
    let bytes = 1_usize.checked_shl(u32::from(alignment))?;
    Layout::from_size_align(len, bytes).ok()
}

/// # Safety
///
/// Called by libghostty-vt through the vtable with its context.
unsafe extern "C" fn alloc(
    ctx: *mut c_void,
    len: usize,
    alignment: u8,
    _ret_addr: usize,
) -> *mut c_void {
    if alignment < 32 {
        counters(ctx)
            .alignments
            .fetch_or(1 << alignment, Ordering::SeqCst);
    }
    let Some(layout) =
        layout(len, alignment).filter(|layout| layout.size() > 0)
    else {
        return std::ptr::null_mut();
    };
    // SAFETY: the layout has a nonzero size.
    let memory = unsafe { std::alloc::alloc(layout) };
    if !memory.is_null() {
        let counters = counters(ctx);
        counters.live.fetch_add(1, Ordering::SeqCst);
        counters.live_bytes.fetch_add(len, Ordering::SeqCst);
        counters.total.fetch_add(1, Ordering::SeqCst);
    }
    memory.cast()
}

/// Declines in-place resizing, which `std::alloc` cannot express; the
/// library then allocates, copies, and frees.
///
/// # Safety
///
/// Called by libghostty-vt through the vtable.
unsafe extern "C" fn resize(
    _ctx: *mut c_void,
    _memory: *mut c_void,
    memory_len: usize,
    _alignment: u8,
    new_len: usize,
    _ret_addr: usize,
) -> bool {
    new_len == memory_len
}

/// # Safety
///
/// Called by libghostty-vt through the vtable.
unsafe extern "C" fn remap(
    _ctx: *mut c_void,
    _memory: *mut c_void,
    _memory_len: usize,
    _alignment: u8,
    _new_len: usize,
    _ret_addr: usize,
) -> *mut c_void {
    std::ptr::null_mut()
}

/// # Safety
///
/// Called by libghostty-vt through the vtable with memory from `alloc`.
unsafe extern "C" fn free(
    ctx: *mut c_void,
    memory: *mut c_void,
    memory_len: usize,
    alignment: u8,
    _ret_addr: usize,
) {
    let Some(layout) = layout(memory_len, alignment) else {
        return;
    };
    if memory.is_null() || layout.size() == 0 {
        return;
    }
    // SAFETY: allocator.h: `memory_len` and `alignment` match the `alloc`
    // call that produced `memory`, which used the same layout.
    unsafe { std::alloc::dealloc(memory.cast(), layout) };
    let counters = counters(ctx);
    counters.live.fetch_sub(1, Ordering::SeqCst);
    counters.live_bytes.fetch_sub(memory_len, Ordering::SeqCst);
}
