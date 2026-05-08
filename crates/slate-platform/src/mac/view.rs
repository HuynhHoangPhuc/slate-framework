//! MetalView and WindowDelegate — AppKit view and delegate callbacks.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSView, NSWindow, NSWindowDelegate};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSRect,
};
use objc2_quartz_core::CAMetalLayer;

use crate::WindowId;
use super::{dispatch_event, ffi_boundary, post_redraw_event};
use crate::Event;

// ---------------------------------------------------------------------------
// MetalView — custom NSView backed by CAMetalLayer
// ---------------------------------------------------------------------------

pub struct MetalViewIvars {
    pub(crate) window_id: Cell<WindowId>,
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

        /// Provide a CAMetalLayer as the view's backing layer. Called once by
        /// AppKit when `wantsLayer` is true. Supplying the layer here (instead
        /// of via `setLayer:`) keeps the view layer-backed (not layer-hosting).
        #[unsafe(method_id(makeBackingLayer))]
        fn make_backing_layer(&self) -> Retained<objc2_quartz_core::CALayer> {
            let metal_layer = CAMetalLayer::new();
            // Sync Metal present with AppKit compositor — eliminates resize tearing.
            metal_layer.setPresentsWithTransaction(true);
            // Redraw when bounds change during live resize.
            metal_layer.setNeedsDisplayOnBoundsChange(true);
            // Never block waiting for drawable — prevents UI stalls during resize.
            metal_layer.setAllowsNextDrawableTimeout(false);
            Retained::into_super(metal_layer)
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
    pub(crate) fn new(mtm: MainThreadMarker, window_id: WindowId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MetalViewIvars {
            window_id: Cell::new(window_id),
        });
        // SAFETY: `NSView`'s designated initializer `initWithFrame:` takes an
        // `NSRect` and returns `Retained<NSView>`. The super-call signature matches.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };

        // Trigger layer-backed mode. AppKit calls our `makeBackingLayer`
        // override to create the CAMetalLayer.
        view.setWantsLayer(true);

        view
    }

    /// Returns the raw Obj-C pointer to this view for `HasWindowHandle`.
    pub(crate) fn as_raw_ptr(&self) -> *mut AnyObject {
        // SAFETY: The pointer is valid for the lifetime of `&self`.
        (self as *const Self as *mut Self).cast::<AnyObject>()
    }
}

// ---------------------------------------------------------------------------
// WindowDelegate — translates NSWindowDelegate callbacks into Slate Events
// ---------------------------------------------------------------------------

pub struct WindowDelegateIvars {
    pub(crate) window_id: Cell<WindowId>,
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
                if let Some(win) = notification
                    .object()
                    .and_then(|obj| obj.downcast::<NSWindow>().ok())
                {
                    let frame = win.contentView().map(|v| v.frame()).unwrap_or(NSRect::ZERO);
                    let scale = win.backingScaleFactor();
                    let w = (frame.size.width * scale).round() as u32;
                    let h = (frame.size.height * scale).round() as u32;
                    dispatch_event(Event::WindowResized {
                        window: id,
                        size: (w, h),
                    });
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

        /// Called when the window's occlusion state changes. On macOS, the
        /// first frame cannot render until the window is visible to the
        /// compositor (wgpu checks occlusionState before nextDrawable).
        /// Fire a redraw when the window becomes visible.
        #[unsafe(method(windowDidChangeOcclusionState:))]
        fn window_did_change_occlusion_state(&self, _notification: &NSNotification) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                post_redraw_event(id);
            });
        }
    }
);

impl WindowDelegate {
    pub(crate) fn new(mtm: MainThreadMarker, window_id: WindowId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WindowDelegateIvars {
            window_id: Cell::new(window_id),
        });
        // SAFETY: `NSObject`'s `init` has no additional requirements.
        unsafe { msg_send![super(this), init] }
    }
}
