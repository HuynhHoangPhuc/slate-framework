//! Frame-change and live-resize hooks for [`MetalView`] and [`WindowDelegate`].
//!
//! The `#[unsafe(method(...))]` selector declarations live in the parent
//! [`super`] module's `define_class!` blocks. Selectors that need to invoke
//! `super` (setFrameSize:, viewWillStartLiveResize, viewDidEndLiveResize) keep
//! the super-send inline in the parent and delegate the post-super work to the
//! `resize_*` methods below. Selectors with no super-send (windowDidResize:,
//! windowDidChangeBackingProperties:) move their entire body here.

use objc2::DefinedClass;
use objc2_app_kit::NSWindow;
use objc2_foundation::{NSNotification, NSRect, NSSize};

use super::{MetalView, WindowDelegate};
use super::super::{
    dispatch_event, ffi_boundary, post_redraw_event, with_window_delegate,
};
use crate::{Event, PhysicalSize};

impl MetalView {
    /// Live-resize tick. AppKit has already updated the view bounds via super;
    /// while still inside the open implicit CATransaction, dispatch a sync
    /// resize so the renderer's present_sync flushes the transaction with the
    /// new framebuffer in place. Without this the bounds commit one selector
    /// before the new pixels land, producing the right/bottom edge flash.
    ///
    /// Gated on `live_resize` so steady-state programmatic frame changes flow
    /// through `windowDidResize:` instead.
    pub(super) fn resize_set_frame_size(&self, size: NSSize) {
        ffi_boundary(|| {
            if !self.ivars().live_resize.get() {
                return;
            }
            let scale = self.window().map(|w| w.backingScaleFactor()).unwrap_or(1.0);
            let pw = (size.width * scale).round() as u32;
            let ph = (size.height * scale).round() as u32;
            if pw == 0 || ph == 0 {
                return;
            }
            let id = self.ivars().window_id.get();
            with_window_delegate(id, |d| {
                d.on_resize_sync(id, PhysicalSize::new(pw, ph))
            });
        });
    }

    /// Opens the live-resize gate so `setFrameSize:` dispatches during a drag.
    pub(super) fn resize_view_will_start_live_resize(&self) {
        ffi_boundary(|| {
            self.ivars().live_resize.set(true);
        });
    }

    /// Closes the live-resize gate.
    pub(super) fn resize_view_did_end_live_resize(&self) {
        ffi_boundary(|| {
            self.ivars().live_resize.set(false);
        });
    }
}

impl WindowDelegate {
    /// AppKit windowDidResize: hook. Sync the delegate first so the framebuffer
    /// is ready before observers see the `WindowResized` event, then schedule a
    /// redraw.
    pub(super) fn resize_window_did_resize(&self, notification: &NSNotification) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            if let Some(win) = notification
                .object()
                .and_then(|obj| obj.downcast::<NSWindow>().ok())
            {
                let frame = win.contentView().map(|v| v.frame()).unwrap_or(NSRect::ZERO);
                let scale = win.backingScaleFactor();
                let lw = frame.size.width.round() as u32;
                let lh = frame.size.height.round() as u32;
                let pw = (frame.size.width * scale).round() as u32;
                let ph = (frame.size.height * scale).round() as u32;
                with_window_delegate(id, |d| d.on_resize_sync(id, PhysicalSize::new(pw, ph)));
                dispatch_event(Event::WindowResized {
                    window: id,
                    logical_size: (lw, lh),
                    physical_size: (pw, ph),
                    scale_factor: scale,
                });
                post_redraw_event(id);
            }
        });
    }

    /// AppKit windowDidChangeBackingProperties: hook (e.g. window dragged
    /// between displays of different DPI). Update `CAMetalLayer.contentsScale`
    /// BEFORE the sync delegate so subsequent renders match the new physical
    /// pixel density.
    pub(super) fn resize_window_did_change_backing_properties(
        &self,
        notification: &NSNotification,
    ) {
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
                let frame = view.frame();
                let lw = frame.size.width.round() as u32;
                let lh = frame.size.height.round() as u32;
                let pw = (frame.size.width * scale).round() as u32;
                let ph = (frame.size.height * scale).round() as u32;
                with_window_delegate(id, |d| d.on_resize_sync(id, PhysicalSize::new(pw, ph)));
                dispatch_event(Event::WindowResized {
                    window: id,
                    logical_size: (lw, lh),
                    physical_size: (pw, ph),
                    scale_factor: scale,
                });
                post_redraw_event(id);
            }
        });
    }
}
