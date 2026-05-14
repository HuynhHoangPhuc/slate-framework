//! CVDisplayLink wrapper for vsync-synchronized rendering.
//!
//! CVDisplayLink fires a callback at each display refresh (typically 60Hz or 120Hz).
//! This ensures rendering continues even during live resize when the main run loop
//! is in NSEventTrackingRunLoopMode.
//!
//! Key insight: NSEvents posted via `postEvent:atStart:` target NSDefaultRunLoopMode
//! and are ignored during tracking mode. We use `dispatch_async` to the main queue
//! instead, which IS processed during tracking mode.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use dispatch::Queue;

use super::{dispatch_event, with_window_delegate};
use crate::{Event, WindowId};

// Core Video FFI bindings
#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVDisplayLinkCreateWithActiveCGDisplays(display_link_out: *mut CVDisplayLinkRef) -> i32;
    fn CVDisplayLinkSetOutputCallback(
        display_link: CVDisplayLinkRef,
        callback: CVDisplayLinkOutputCallback,
        user_info: *mut c_void,
    ) -> i32;
    fn CVDisplayLinkStart(display_link: CVDisplayLinkRef) -> i32;
    fn CVDisplayLinkStop(display_link: CVDisplayLinkRef) -> i32;
    fn CVDisplayLinkRelease(display_link: CVDisplayLinkRef);
}

type CVDisplayLinkRef = *mut c_void;
type CVDisplayLinkOutputCallback = extern "C" fn(
    display_link: CVDisplayLinkRef,
    in_now: *const CVTimeStamp,
    in_output_time: *const CVTimeStamp,
    flags_in: u64,
    flags_out: *mut u64,
    user_info: *mut c_void,
) -> i32;

#[repr(C)]
struct CVTimeStamp {
    version: u32,
    video_time_scale: i32,
    video_time: i64,
    host_time: u64,
    rate_scalar: f64,
    video_refresh_period: i64,
    smpte_time: [u8; 24], // SMPTETime struct, not used
    flags: u64,
    reserved: u64,
}

const K_CV_RETURN_SUCCESS: i32 = 0;

/// Display link state, shared between main thread and callback thread.
struct DisplayLinkState {
    /// The window to trigger redraws for.
    window_id: WindowId,
    /// Whether the display link is running.
    running: AtomicBool,
}

/// CVDisplayLink wrapper for a single window.
pub struct DisplayLink {
    link: CVDisplayLinkRef,
    state: Box<DisplayLinkState>,
}

// SAFETY: DisplayLink is only accessed from main thread. The CVDisplayLink callback
// only reads from AtomicBool and posts thread-safe events.
unsafe impl Send for DisplayLink {}

impl DisplayLink {
    /// Create a new display link for the given window.
    ///
    /// # Safety
    /// Must be called from the main thread.
    pub fn new(window_id: WindowId) -> Option<Self> {
        let mut link: CVDisplayLinkRef = ptr::null_mut();
        let result = unsafe { CVDisplayLinkCreateWithActiveCGDisplays(&mut link) };
        if result != K_CV_RETURN_SUCCESS || link.is_null() {
            log::warn!("CVDisplayLinkCreateWithActiveCGDisplays failed: {}", result);
            return None;
        }

        let state = Box::new(DisplayLinkState {
            window_id,
            running: AtomicBool::new(false),
        });

        let result = unsafe {
            CVDisplayLinkSetOutputCallback(
                link,
                display_link_callback,
                &*state as *const DisplayLinkState as *mut c_void,
            )
        };
        if result != K_CV_RETURN_SUCCESS {
            log::warn!("CVDisplayLinkSetOutputCallback failed: {}", result);
            unsafe { CVDisplayLinkRelease(link) };
            return None;
        }

        Some(DisplayLink { link, state })
    }

    /// Start the display link. Callback will fire at each vsync.
    pub fn start(&self) {
        if self.state.running.load(Ordering::Relaxed) {
            return;
        }
        let result = unsafe { CVDisplayLinkStart(self.link) };
        if result == K_CV_RETURN_SUCCESS {
            self.state.running.store(true, Ordering::Relaxed);
        } else {
            log::warn!("CVDisplayLinkStart failed: {}", result);
        }
    }

    /// Stop the display link.
    pub fn stop(&self) {
        if !self.state.running.load(Ordering::Relaxed) {
            return;
        }
        let result = unsafe { CVDisplayLinkStop(self.link) };
        if result == K_CV_RETURN_SUCCESS {
            self.state.running.store(false, Ordering::Relaxed);
        }
    }
}

impl Drop for DisplayLink {
    fn drop(&mut self) {
        self.stop();
        unsafe { CVDisplayLinkRelease(self.link) };
    }
}

/// CVDisplayLink callback. Runs on a high-priority background thread at each vsync.
extern "C" fn display_link_callback(
    _display_link: CVDisplayLinkRef,
    _in_now: *const CVTimeStamp,
    _in_output_time: *const CVTimeStamp,
    _flags_in: u64,
    _flags_out: *mut u64,
    user_info: *mut c_void,
) -> i32 {
    // SAFETY: user_info is a valid pointer to DisplayLinkState that outlives
    // the display link (Box lives as long as DisplayLink struct).
    let state = unsafe { &*(user_info as *const DisplayLinkState) };

    if !state.running.load(Ordering::Relaxed) {
        return K_CV_RETURN_SUCCESS;
    }

    // Dispatch to main queue. Unlike NSApplication.postEvent: which targets
    // NSDefaultRunLoopMode (ignored during tracking mode), dispatch_async to
    // the main queue IS processed during tracking mode.
    // NOTE: with_window_delegate must be called on main thread (thread-local registry).
    let window_id = state.window_id;
    Queue::main().exec_async(move || {
        with_window_delegate(window_id, |d| d.on_redraw(window_id));
        dispatch_event(Event::WindowRedrawRequested { window: window_id });
    });

    K_CV_RETURN_SUCCESS
}
