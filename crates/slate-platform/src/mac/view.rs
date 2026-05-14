//! MetalView and WindowDelegate — AppKit view and delegate callbacks.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSEvent, NSEventModifierFlags, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow,
    NSWindowDelegate,
};
use objc2_foundation::{MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSRect};
use objc2_quartz_core::CAMetalLayer;

use super::{
    dispatch_event, ffi_boundary, post_redraw_event, unregister_window, with_window_delegate,
};
use crate::{Event, Modifiers, MouseButton, PhysicalSize, WindowId};

// ---------------------------------------------------------------------------
// Mouse event decode helpers
// ---------------------------------------------------------------------------

/// Decode position from NSEvent, flipping Y to top-left origin.
/// Returns logical position in view coordinates (no scale division).
fn decode_position(view: &MetalView, event: &NSEvent) -> (f32, f32) {
    let loc_in_window = event.locationInWindow();
    let bounds = view.bounds();
    let bounds_height = bounds.size.height as f32;

    let view_pt = view.convertPoint_fromView(loc_in_window, None);
    // Post-Phase-3: coords are already logical from NSView; just flip Y.
    let x = view_pt.x as f32;
    let y = bounds_height - view_pt.y as f32;
    (x, y)
}

/// Pure decode_position logic for unit testing.
/// Takes (x, y) in window coords (Y-up), bounds_height, and scale.
/// Post-Phase-3: scale is ignored (coords are already logical from NSView).
#[cfg(test)]
pub(crate) fn decode_position_pure(
    loc_in_window: (f32, f32),
    bounds_height: f32,
    _scale: f32,
) -> (f32, f32) {
    let x = loc_in_window.0;
    let y = bounds_height - loc_in_window.1;
    (x, y)
}

/// Decode modifier flags from NSEvent.
fn decode_modifiers(flags: NSEventModifierFlags) -> Modifiers {
    Modifiers {
        shift: flags.contains(NSEventModifierFlags::Shift),
        ctrl: flags.contains(NSEventModifierFlags::Control),
        alt: flags.contains(NSEventModifierFlags::Option),
        meta: flags.contains(NSEventModifierFlags::Command),
    }
}

/// Decode button number to MouseButton. Returns None for unsupported buttons.
fn decode_button(button_number: isize) -> Option<MouseButton> {
    match button_number {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Right),
        2 => Some(MouseButton::Middle),
        3..=7 => Some(MouseButton::Other((button_number - 3) as u8)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// MetalView — custom NSView backed by CAMetalLayer
// ---------------------------------------------------------------------------

pub struct MetalViewIvars {
    pub(crate) window_id: Cell<WindowId>,
    /// Current tracking area for mouse move/enter/exit events.
    tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
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
            // Synchronize Metal present with AppKit's CATransaction commit.
            // Without this, async present causes compositor to scale stale drawables.
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
                with_window_delegate(id, |d| d.on_redraw(id));
                dispatch_event(Event::WindowRedrawRequested { window: id });
            });
        }

        // -----------------------------------------------------------------------
        // Mouse event selectors (Phase 5a)
        // -----------------------------------------------------------------------

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                let position = decode_position(self, event);
                let modifiers = decode_modifiers(event.modifierFlags());
                dispatch_event(Event::MouseDown {
                    window: id,
                    position,
                    button: MouseButton::Left,
                    modifiers,
                });
            });
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                let position = decode_position(self, event);
                let modifiers = decode_modifiers(event.modifierFlags());
                dispatch_event(Event::MouseUp {
                    window: id,
                    position,
                    button: MouseButton::Left,
                    modifiers,
                });
            });
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                let position = decode_position(self, event);
                let modifiers = decode_modifiers(event.modifierFlags());
                dispatch_event(Event::MouseMoved {
                    window: id,
                    position,
                    modifiers,
                });
            });
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                let position = decode_position(self, event);
                let modifiers = decode_modifiers(event.modifierFlags());
                dispatch_event(Event::MouseDown {
                    window: id,
                    position,
                    button: MouseButton::Right,
                    modifiers,
                });
            });
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                let position = decode_position(self, event);
                let modifiers = decode_modifiers(event.modifierFlags());
                dispatch_event(Event::MouseUp {
                    window: id,
                    position,
                    button: MouseButton::Right,
                    modifiers,
                });
            });
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                let position = decode_position(self, event);
                let modifiers = decode_modifiers(event.modifierFlags());
                dispatch_event(Event::MouseMoved {
                    window: id,
                    position,
                    modifiers,
                });
            });
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                let button_num = event.buttonNumber();
                let Some(button) = decode_button(button_num) else {
                    return;
                };
                let position = decode_position(self, event);
                let modifiers = decode_modifiers(event.modifierFlags());
                dispatch_event(Event::MouseDown {
                    window: id,
                    position,
                    button,
                    modifiers,
                });
            });
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                let button_num = event.buttonNumber();
                let Some(button) = decode_button(button_num) else {
                    return;
                };
                let position = decode_position(self, event);
                let modifiers = decode_modifiers(event.modifierFlags());
                dispatch_event(Event::MouseUp {
                    window: id,
                    position,
                    button,
                    modifiers,
                });
            });
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                let position = decode_position(self, event);
                let modifiers = decode_modifiers(event.modifierFlags());
                dispatch_event(Event::MouseMoved {
                    window: id,
                    position,
                    modifiers,
                });
            });
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                let position = decode_position(self, event);
                let modifiers = decode_modifiers(event.modifierFlags());
                dispatch_event(Event::MouseMoved {
                    window: id,
                    position,
                    modifiers,
                });
            });
        }

        /// Receive but drop — framework synthesizes Enter from hit-test diff.
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            // Intentionally empty; prevents responder chain walk.
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                dispatch_event(Event::MouseExited { window: id });
            });
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                let position = decode_position(self, event);
                let modifiers = decode_modifiers(event.modifierFlags());
                let precise = event.hasPreciseScrollingDeltas();
                let (delta_x, delta_y) = if precise {
                    (
                        event.scrollingDeltaX() as f32,
                        event.scrollingDeltaY() as f32,
                    )
                } else {
                    (event.deltaX() as f32, event.deltaY() as f32)
                };
                dispatch_event(Event::MouseScrolled {
                    window: id,
                    position,
                    delta_x,
                    delta_y,
                    precise,
                    modifiers,
                });
            });
        }

        /// Called by AppKit when view bounds change. Reinstall tracking area.
        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            // Call super first
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };

            // Remove old tracking area if present
            {
                let mut ta = self.ivars().tracking_area.borrow_mut();
                if let Some(old) = ta.take() {
                    self.removeTrackingArea(&old);
                }
            }

            // Install new tracking area covering current bounds
            let bounds = self.bounds();
            let options = NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::InVisibleRect;

            let tracking_area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    bounds,
                    options,
                    Some(self),
                    None,
                )
            };
            self.addTrackingArea(&tracking_area);
            *self.ivars().tracking_area.borrow_mut() = Some(tracking_area);
        }
    }
);

impl MetalView {
    pub(crate) fn new(mtm: MainThreadMarker, window_id: WindowId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MetalViewIvars {
            window_id: Cell::new(window_id),
            tracking_area: RefCell::new(None),
        });
        // SAFETY: `NSView`'s designated initializer `initWithFrame:` takes an
        // `NSRect` and returns `Retained<NSView>`. The super-call signature matches.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };

        // Trigger layer-backed mode. AppKit calls our `makeBackingLayer`
        // override to create the CAMetalLayer.
        view.setWantsLayer(true);

        view
    }

    /// Install initial tracking area for mouse move/enter/exit events.
    ///
    /// Call this after the view is added to a window (so bounds are valid).
    /// Without this, hover events may not fire until the first layout change.
    pub(crate) fn install_tracking_area(&self) {
        // Trigger updateTrackingAreas to install initial tracking area.
        // SAFETY: updateTrackingAreas is a standard NSView method.
        let _: () = unsafe { msg_send![self, updateTrackingAreas] };
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
                    let lw = frame.size.width.round() as u32;
                    let lh = frame.size.height.round() as u32;
                    let pw = (frame.size.width * scale).round() as u32;
                    let ph = (frame.size.height * scale).round() as u32;
                    // Sync delegate first so framebuffer is ready before observers see resize.
                    with_window_delegate(id, |d| d.on_resize_sync(id, PhysicalSize::new(pw, ph)));
                    dispatch_event(Event::WindowResized {
                        window: id,
                        logical_size: (lw, lh),
                        physical_size: (pw, ph),
                        scale_factor: scale,
                    });
                    // Ensure redraw is scheduled after resize.
                    post_redraw_event(id);
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
                // Unregister FIRST — prevents stale delegate dispatch between
                // close and dealloc when AppKit sends late paint events.
                unregister_window(id);
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
                    // Update contentsScale FIRST — before invoking sync delegate.
                    if let Some(layer) = view.layer() {
                        layer.setContentsScale(scale);
                    }
                    // Drawable size in physical pixels follows the new scale.
                    let frame = view.frame();
                    let lw = frame.size.width.round() as u32;
                    let lh = frame.size.height.round() as u32;
                    let pw = (frame.size.width * scale).round() as u32;
                    let ph = (frame.size.height * scale).round() as u32;
                    // Sync delegate second — contentsScale is now correct.
                    with_window_delegate(id, |d| d.on_resize_sync(id, PhysicalSize::new(pw, ph)));
                    dispatch_event(Event::WindowResized {
                        window: id,
                        logical_size: (lw, lh),
                        physical_size: (pw, ph),
                        scale_factor: scale,
                    });
                    // Ensure redraw is scheduled after scale change.
                    post_redraw_event(id);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_position_does_not_divide_by_scale() {
        // bounds 400 high, click at logical (300, 100) from window-origin (Y-up)
        // Pre-fix: returned (300/2, (400-100)/2) = (150, 150) — bug
        // Post-fix: returns (300, 300) — Y-flip only, no scale division
        assert_eq!(
            decode_position_pure((300.0, 100.0), 400.0, 2.0),
            (300.0, 300.0)
        );
    }

    #[test]
    fn decode_position_flips_y_correctly() {
        // At scale=1.0 (no division anyway), verify Y flip.
        // Click at (50, 50) in Y-up coords with bounds_height=100
        // Expected: (50, 100-50) = (50, 50)
        assert_eq!(decode_position_pure((50.0, 50.0), 100.0, 1.0), (50.0, 50.0));
        // Click at (50, 0) in Y-up coords (bottom of view)
        // Expected: (50, 100-0) = (50, 100)
        assert_eq!(decode_position_pure((50.0, 0.0), 100.0, 1.0), (50.0, 100.0));
    }
}
