//! macOS platform backend using `objc2` + AppKit.
//!
//! # Thread safety
//! All types in this module are **main-thread-only**. `MacWindow` is
//! deliberately not `Send` or `Sync`; AppKit `NSWindow`/`NSView` may only be
//! touched on the thread that created them.
//!
//! # Handler lifetime
//! `MacPlatform::run` accepts a `FnMut(Event)` of any lifetime. Internally
//! it erases the lifetime to `'static` for storage in a `thread_local!`
//! `RefCell`. This is sound because `run` **blocks** for the entire duration
//! of the handler's borrow: the `'static`-cast pointer is cleared before
//! `run` returns, so the raw pointer can never outlive the closure.
//! See the SAFETY comment in `run` for the full argument.

use std::cell::Cell;
use std::panic::AssertUnwindSafe;
use std::ptr::NonNull;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType,
    NSEvent, NSEventModifierFlags, NSEventType, NSView, NSWindow, NSWindowDelegate,
    NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString,
};
use objc2_quartz_core::CAMetalLayer;
use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, DisplayHandle, HandleError, HasDisplayHandle,
    HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};

use crate::{Event, Platform, Window, WindowId, WindowOptions};

// ---------------------------------------------------------------------------
// Thread-local event handler storage
// ---------------------------------------------------------------------------

// The handler is stored as `'static` via a lifetime-erasing transmute inside
// `MacPlatform::run`. See the SAFETY block there for the soundness argument.
type EventHandler = std::cell::RefCell<Option<Box<dyn FnMut(Event) + 'static>>>;

thread_local! {
    static HANDLER: EventHandler = const { std::cell::RefCell::new(None) };
}

/// Dispatch an `Event` through the thread-local handler. No-op if none installed.
fn dispatch_event(event: Event) {
    HANDLER.with(|h| {
        if let Some(handler) = h.borrow_mut().as_mut() {
            handler(event);
        }
    });
}

/// Abort-safe wrapper for every Rust body reachable from AppKit dispatch.
///
/// Unwinding across an `extern "C"` boundary is undefined behavior on stable
/// Rust. Every Obj-C delegate method / view callback must call this wrapper
/// so that a Rust panic aborts the process loudly instead of silently
/// corrupting AppKit state.
///
/// We use `AssertUnwindSafe` because the closures capture `&self` whose ivars
/// contain `Cell<T>` (not `UnwindSafe`). This is acceptable: we abort on any
/// panic, so we never observe the inconsistent state that would follow from a
/// non-unwind-safe unwinding path.
fn ffi_boundary(f: impl FnOnce()) {
    if std::panic::catch_unwind(AssertUnwindSafe(f)).is_err() {
        log::error!("slate: Rust panic crossed AppKit FFI boundary — aborting");
        std::process::abort();
    }
}

// ---------------------------------------------------------------------------
// Monotonic WindowId counter (main-thread-only → Cell is fine)
// ---------------------------------------------------------------------------

thread_local! {
    static NEXT_WINDOW_ID: Cell<u64> = const { Cell::new(1) };
}

fn next_window_id() -> WindowId {
    NEXT_WINDOW_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        WindowId(id)
    })
}

// ---------------------------------------------------------------------------
// MetalView — custom NSView backed by CAMetalLayer
// ---------------------------------------------------------------------------

pub struct MetalViewIvars {
    window_id: Cell<WindowId>,
}

define_class!(
    // SAFETY:
    // - `NSView` has no documented subclassing restrictions beyond main-thread use.
    // - `MetalView` does not implement `Drop`; Obj-C ARC handles deallocation.
    #[unsafe(super(NSView, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = MetalViewIvars]
    pub struct MetalView;

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for MetalView {}

    impl MetalView {
        /// AppKit asks this to decide if the view accepts key events.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        /// Called by AppKit when the view's contents need refreshing.
        /// Translated to `Event::WindowRedrawRequested`.
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _rect: NSRect) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                dispatch_event(Event::WindowRedrawRequested { window: id });
            });
        }
    }
);

impl MetalView {
    fn new(mtm: MainThreadMarker, window_id: WindowId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MetalViewIvars {
            window_id: Cell::new(window_id),
        });
        // SAFETY: `NSView`'s designated initializer `initWithFrame:` takes an
        // `NSRect` and returns `Retained<NSView>`. The super-call signature matches.
        let view: Retained<Self> =
            unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };

        // Make the view layer-backed for Metal rendering.
        view.setWantsLayer(true);

        // Create a CAMetalLayer and assign it as the view's backing layer.
        // `CAMetalLayer::new()` is a safe factory. Casting the retained value
        // to `&CALayer` via `AsRef` is valid because CAMetalLayer inherits
        // from CALayer in the Obj-C class hierarchy.
        let metal_layer = CAMetalLayer::new();
        let ca_layer: &objc2_quartz_core::CALayer = metal_layer.as_ref();
        view.setLayer(Some(ca_layer));

        view
    }

    /// Returns the raw Obj-C pointer to this view for `HasWindowHandle`.
    fn as_raw_ptr(&self) -> *mut AnyObject {
        // SAFETY: The pointer is valid for the lifetime of `&self`.
        (self as *const Self as *mut Self).cast::<AnyObject>()
    }
}

// ---------------------------------------------------------------------------
// WindowDelegate — translates NSWindowDelegate callbacks into Slate Events
// ---------------------------------------------------------------------------

pub struct WindowDelegateIvars {
    window_id: Cell<WindowId>,
}

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing restrictions.
    // - `WindowDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = WindowDelegateIvars]
    pub struct WindowDelegate;

    // SAFETY: no requirements on NSObjectProtocol.
    unsafe impl NSObjectProtocol for WindowDelegate {}

    // SAFETY: NSWindowDelegate has no additional safety requirements.
    unsafe impl NSWindowDelegate for WindowDelegate {
        /// Called after the window has been resized.
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, notification: &NSNotification) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                // Retrieve the physical size from the notification's NSWindow object.
                if let Some(win) = notification.object()
                    .and_then(|obj| obj.downcast::<NSWindow>().ok())
                {
                    let frame = win
                        .contentView()
                        .map(|v| v.frame())
                        .unwrap_or(NSRect::ZERO);
                    let scale = win.backingScaleFactor();
                    let w = (frame.size.width * scale).round() as u32;
                    let h = (frame.size.height * scale).round() as u32;
                    dispatch_event(Event::WindowResized { window: id, size: (w, h) });
                }
            });
        }

        /// Called when the user (or code) requests the window to close.
        /// Returns `true` to allow the close to proceed.
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSWindow) -> bool {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                dispatch_event(Event::WindowCloseRequested { window: id });
            });
            // Allow AppKit to proceed with closing; quit is triggered in windowWillClose.
            true
        }

        /// Called just before the window's Obj-C resources are released.
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                dispatch_event(Event::WindowDestroyed { window: id });
                // Quit policy lives in the user's `WindowCloseRequested`
                // handler; calling `terminate:` here would invoke `exit()`
                // and skip `Event::Exiting` entirely.
            });
        }

        /// Called when the window's backing scale factor changes (e.g. user
        /// drags the window between displays of different DPI). Re-syncs the
        /// `CAMetalLayer.contentsScale` so subsequent renders match the new
        /// physical pixel density.
        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn window_did_change_backing_properties(&self, notification: &NSNotification) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                let Some(win) = notification
                    .object()
                    .and_then(|obj| obj.downcast::<NSWindow>().ok())
                else {
                    return;
                };
                let scale = win.backingScaleFactor();
                if let Some(view) = win.contentView() {
                    if let Some(layer) = view.layer() {
                        layer.setContentsScale(scale);
                    }
                    // Drawable size in physical pixels follows the new scale.
                    let frame = view.frame();
                    let w = (frame.size.width * scale).round() as u32;
                    let h = (frame.size.height * scale).round() as u32;
                    dispatch_event(Event::WindowResized {
                        window: id,
                        size: (w, h),
                    });
                }
            });
        }
    }
);

impl WindowDelegate {
    fn new(mtm: MainThreadMarker, window_id: WindowId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WindowDelegateIvars {
            window_id: Cell::new(window_id),
        });
        // SAFETY: `NSObject`'s `init` has no additional requirements.
        unsafe { msg_send![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// AppDelegate — application-level callbacks
// ---------------------------------------------------------------------------

define_class!(
    // SAFETY: NSObject has no subclassing restrictions; AppDelegate carries no
    // Rust state beyond the empty ivars tuple.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    pub struct AppDelegate;

    // SAFETY: no requirements.
    unsafe impl NSObjectProtocol for AppDelegate {}

    // SAFETY: NSApplicationDelegate has no additional safety requirements.
    unsafe impl NSApplicationDelegate for AppDelegate {
        /// Sent after the run loop starts and the app is ready.
        /// This is the canonical point to emit `Event::Resumed`.
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn application_did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = self.mtm();
            ffi_boundary(|| {
                // Bring the app to front when launched from `cargo run` (unbundled).
                #[allow(deprecated)]
                NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
                dispatch_event(Event::Resumed);
            });
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: `NSObject`'s `init` has no additional requirements.
        unsafe { msg_send![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// MacWindow — public window handle
// ---------------------------------------------------------------------------

/// A native macOS window.
///
/// Not `Send` or `Sync`; must be used exclusively on the main OS thread.
pub struct MacWindow {
    id: WindowId,
    ns_window: Retained<NSWindow>,
    view: Retained<MetalView>,
    /// Keeps the delegate alive for the window's full lifetime.
    _delegate: Retained<WindowDelegate>,
}

impl MacWindow {
    fn new(opts: &WindowOptions, mtm: MainThreadMarker) -> Arc<Self> {
        let id = next_window_id();

        let mut mask = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
        if opts.resizable {
            mask |= NSWindowStyleMask::Resizable | NSWindowStyleMask::Miniaturizable;
        }

        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(opts.size.0 as f64, opts.size.1 as f64),
        );

        // SAFETY: `initWithContentRect_styleMask_backing_defer` is the designated
        // initializer for `NSWindow`; all arguments are valid Obj-C values.
        // We immediately call `setReleasedWhenClosed(false)` because windows
        // created outside a window controller are auto-released on close by
        // default, which would cause a double-free against our `Retained` handle.
        let ns_window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                mask,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: Must be called before the window is ever closed.
        unsafe { ns_window.setReleasedWhenClosed(false) };

        ns_window.setTitle(&NSString::from_str(&opts.title));

        if let Some((min_w, min_h)) = opts.min_size {
            ns_window.setContentMinSize(NSSize::new(min_w as f64, min_h as f64));
        }

        // Create the Metal-backed content view and install it.
        let view = MetalView::new(mtm, id);
        // Coerce MetalView to &NSView for setContentView; MetalView IS-A NSView.
        let ns_view_ref: &NSView = &view;
        ns_window.setContentView(Some(ns_view_ref));

        // Sync the CAMetalLayer's contentsScale to the window's backing factor.
        // Must happen after attaching the view (so `backingScaleFactor` is valid).
        let scale = ns_window.backingScaleFactor();
        if let Some(layer) = view.layer() {
            layer.setContentsScale(scale);
        }

        // Attach the window delegate.
        let delegate = WindowDelegate::new(mtm, id);
        ns_window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        ns_window.center();
        ns_window.makeKeyAndOrderFront(None);

        // The `Platform` trait mandates `Arc<Self::Window>`; `MacWindow` is
        // intentionally not Send+Sync (main-thread-only), so suppress the
        // arc_with_non_send_sync lint rather than changing the trait contract.
        #[allow(clippy::arc_with_non_send_sync)]
        Arc::new(MacWindow {
            id,
            ns_window,
            view,
            _delegate: delegate,
        })
    }
}

impl Window for MacWindow {
    fn size(&self) -> (u32, u32) {
        // Physical pixels = logical content size × backing scale. Round to the
        // nearest integer so a 1.5× display reports 600 px for a 400 pt frame
        // instead of truncating to 599.
        let frame = self.view.frame();
        let scale = self.ns_window.backingScaleFactor();
        (
            (frame.size.width * scale).round() as u32,
            (frame.size.height * scale).round() as u32,
        )
    }

    fn scale_factor(&self) -> f64 {
        self.ns_window.backingScaleFactor()
    }

    fn request_redraw(&self) {
        self.view.setNeedsDisplay(true);
    }

    fn set_title(&self, title: &str) {
        self.ns_window.setTitle(&NSString::from_str(title));
    }

    fn close(&self) {
        // `performClose:` mimics the user clicking the close button, firing
        // `windowShouldClose:` and then `windowWillClose:` via the delegate.
        self.ns_window.performClose(None);
    }

    fn id(&self) -> WindowId {
        self.id
    }
}

impl HasWindowHandle for MacWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw_ptr = self.view.as_raw_ptr();
        // SAFETY: `raw_ptr` is a valid NSView pointer that lives at least as
        // long as `&self` (the Arc keeps `view` alive). NonNull is safe
        // because `self.view` is a non-null `Retained`.
        let nn = unsafe { NonNull::new_unchecked(raw_ptr.cast()) };
        let handle = AppKitWindowHandle::new(nn);
        // SAFETY: the handle is valid for the duration of the `&self` borrow.
        unsafe { Ok(WindowHandle::borrow_raw(RawWindowHandle::AppKit(handle))) }
    }
}

impl HasDisplayHandle for MacWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // AppKitDisplayHandle carries no pointer; it is always valid.
        let handle = AppKitDisplayHandle::new();
        // SAFETY: AppKit display handle is always valid on macOS.
        unsafe { Ok(DisplayHandle::borrow_raw(RawDisplayHandle::AppKit(handle))) }
    }
}

// ---------------------------------------------------------------------------
// MacPlatform — public platform handle
// ---------------------------------------------------------------------------

/// The macOS platform driver.
///
/// Must be created and used exclusively on the main OS thread.
pub struct MacPlatform {
    _mtm: MainThreadMarker,
    app: Retained<NSApplication>,
    /// Guards against re-entrant calls to `run`.
    running: Cell<bool>,
}

impl Platform for MacPlatform {
    type Window = MacWindow;

    fn new() -> Self {
        // Panic immediately if not on the main thread — AppKit mandates it.
        let mtm = MainThreadMarker::new()
            .expect("MacPlatform::new must be called from the main OS thread");

        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

        MacPlatform {
            _mtm: mtm,
            app,
            running: Cell::new(false),
        }
    }

    fn create_window(&self, opts: WindowOptions) -> Arc<MacWindow> {
        MacWindow::new(&opts, self._mtm)
    }

    fn run<F>(&self, mut handler: F)
    where
        F: FnMut(Event),
    {
        assert!(
            !self.running.get(),
            "MacPlatform::run called re-entrantly"
        );
        self.running.set(true);

        // Erase the handler's lifetime to 'static for HANDLER thread-local storage.
        //
        // SAFETY:
        //   (1) `NSApplication::run` blocks this call stack until `terminate:`
        //       or `stop:` is called — i.e. until the run loop exits.
        //   (2) All AppKit callbacks that access HANDLER execute on this same
        //       main thread (single-threaded UI); there is no concurrent access.
        //   (3) We unconditionally clear HANDLER before this function returns
        //       (in the `finally` section below), so the raw pointer embedded
        //       in the Box is never dereferenced after the closure's borrow ends.
        //   (4) Therefore the transmute is equivalent to lending a &'run_scope
        //       reference to a scope that is strictly nested within run_scope.
        let handler_static: Box<dyn FnMut(Event) + 'static> = {
            let erased: Box<dyn FnMut(Event) + '_> = Box::new(&mut handler);
            // SAFETY: see argument above.
            unsafe { std::mem::transmute(erased) }
        };

        HANDLER.with(|h| {
            *h.borrow_mut() = Some(handler_static);
        });

        // Install the app delegate (sends Resumed in applicationDidFinishLaunching).
        let delegate = AppDelegate::new(self._mtm);
        self.app
            .setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        // Enter the native run loop. Blocks until terminate:/stop: is called.
        self.app.run();

        // --- run loop has exited; emit Exiting while handler is still live ---
        dispatch_event(Event::Exiting);

        // CRITICAL: clear the handler to invalidate the 'static-erased pointer
        // BEFORE this function returns and the borrow of `handler` ends.
        HANDLER.with(|h| {
            *h.borrow_mut() = None;
        });

        self.running.set(false);
    }

    fn quit(&self) {
        // Use `stop:` rather than `terminate:` so `app.run()` returns control
        // to our caller — `terminate:` invokes `exit()` and skips the
        // `Event::Exiting` dispatch in `run()`.
        //
        // `stop:` only takes effect on the next event-loop iteration; if no
        // events are pending the loop blocks indefinitely. Post a synthetic
        // ApplicationDefined event to guarantee the loop wakes immediately.
        self.app.stop(None);
        let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::new(0.0, 0.0),
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            0,
            0,
            0,
        );
        if let Some(event) = event {
            self.app.postEvent_atStart(&event, true);
        }
    }
}
