//! Counting allocator wrapper for profiling builds.
//!
//! [`CountingAllocator`] wraps [`std::alloc::System`] and increments atomic
//! counters on each `alloc` / `dealloc`. Install it as a `#[global_allocator]`
//! in a *bench binary* (not in any shipped library), e.g.
//!
//! ```ignore
//! use slate_framework::profiling::CountingAllocator;
//!
//! #[global_allocator]
//! static ALLOC: CountingAllocator = CountingAllocator;
//! ```
//!
//! # Per-binary scope
//!
//! `#[global_allocator]` is a per-binary attribute. The counter is intended
//! for bench binaries and integration tests; do NOT install it in shipped
//! crates, since the wrapper is unconditional (the feature gate keeps the
//! *re-export* out of default builds, not the wrapper itself).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static DEALLOC_COUNT: AtomicU64 = AtomicU64::new(0);

/// System-allocator wrapper that increments atomic counters per request.
///
/// Use [`alloc_count`] and [`dealloc_count`] to read the current values.
/// Reset both with [`reset_counters`].
pub struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarding to the system allocator with the caller's layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarding `ptr` + `layout` originally produced by `alloc`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarding to the system allocator with the caller's layout.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // realloc may either resize in place or allocate a new block + free
        // the old one. Count it as one allocation event regardless; counter
        // is for tracking heap activity volume, not exact malloc invocations.
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarding caller-provided `ptr`, `layout`, and `new_size`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Returns the cumulative number of `alloc` / `alloc_zeroed` / `realloc`
/// invocations served since the last [`reset_counters`] call.
pub fn alloc_count() -> u64 {
    ALLOC_COUNT.load(Ordering::Relaxed)
}

/// Returns the cumulative number of `dealloc` invocations served since the
/// last [`reset_counters`] call.
pub fn dealloc_count() -> u64 {
    DEALLOC_COUNT.load(Ordering::Relaxed)
}

/// Resets both allocator counters to zero.
pub fn reset_counters() {
    ALLOC_COUNT.store(0, Ordering::Relaxed);
    DEALLOC_COUNT.store(0, Ordering::Relaxed);
}
