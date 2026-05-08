//! MacWindow — native macOS window handle.

use std::ptr::NonNull;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, DisplayHandle, HandleError, HasDisplayHandle,
    HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};

use crate::{Window, WindowId, WindowOptions};
use super::view::{MetalView, WindowDelegate};
use super::{next_window_id, post_redraw_event};

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
    pub(crate) fn new(opts: &WindowOptions, mtm: MainThreadMarker) -> Arc<Self> {
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
        // Post a synthetic event processed on the next run loop iteration.
        // Safe to call from within an event handler (no re-entrancy risk).
        post_redraw_event(self.id);
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
