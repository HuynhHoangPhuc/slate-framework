//! Layer backing and redraw routing for [`MetalView`] and [`WindowDelegate`].
//!
//! The `#[unsafe(method(...))]` selector declarations live in the parent
//! [`super`] module's `define_class!` blocks; each selector body delegates
//! to one of the `render_*` methods defined below.

use objc2::DefinedClass;
use objc2::rc::Retained;
use objc2_foundation::{NSNotification, NSRect};
use objc2_quartz_core::{CALayer, CAMetalLayer};

use super::{MetalView, WindowDelegate};
use super::super::{dispatch_event, ffi_boundary, post_redraw_event, with_window_delegate};
use crate::Event;

impl MetalView {
    /// Provide a CAMetalLayer as the view's backing layer. Called once by
    /// AppKit when `wantsLayer` is true. Supplying the layer here (instead
    /// of via `setLayer:`) keeps the view layer-backed (not layer-hosting).
    pub(super) fn render_make_backing_layer(&self) -> Retained<CALayer> {
        let metal_layer = CAMetalLayer::new();
        // Synchronize Metal present with AppKit's CATransaction commit.
        // Without this, async present causes compositor to scale stale drawables.
        metal_layer.setPresentsWithTransaction(true);
        // Redraw when bounds change during live resize.
        metal_layer.setNeedsDisplayOnBoundsChange(true);
        // Never block waiting for drawable — prevents UI stalls during resize.
        metal_layer.setAllowsNextDrawableTimeout(false);
        Retained::into_super(metal_layer)
    }

    /// AppKit drawRect: hook. Translated to `Event::WindowRedrawRequested`.
    pub(super) fn render_draw_rect(&self, _rect: NSRect) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            with_window_delegate(id, |d| d.on_redraw(id));
            dispatch_event(Event::WindowRedrawRequested { window: id });
        });
    }
}

impl WindowDelegate {
    /// On macOS, the first frame cannot render until the window is visible
    /// to the compositor (wgpu checks occlusionState before nextDrawable).
    /// Fire a redraw when the window becomes visible.
    pub(super) fn render_window_did_change_occlusion_state(&self, _notification: &NSNotification) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            post_redraw_event(id);
        });
    }
}
