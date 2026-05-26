//! MetalView and WindowDelegate — AppKit view and delegate callbacks.
//!
//! This module hosts only the `define_class!` invocations that register the
//! Obj-C classes and their selector tables. Bodies live in the per-concern
//! submodules:
//!
//! - `state` — instance variables, constructors, and view helpers.
//! - `render` — backing layer + drawRect:.
//! - `resize` — live-resize and frame-change handlers.
//! - `responder` — mouse, keyboard, modifier, and tracking-area handlers.
//!
//! The `define_class!` macro requires all selector declarations for a class
//! to share a single macro invocation, so the method table cannot itself be
//! split. Each declaration here is a thin delegation to the matching impl in
//! a submodule.

mod render;
mod resize;
mod responder;
mod state;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSTextInputClient, NSView, NSWindow, NSWindowDelegate};
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSNotification, NSObject, NSObjectProtocol,
    NSPoint, NSRange, NSRangePointer, NSRect, NSSize, NSUInteger,
};

use super::{dispatch_event, ffi_boundary, unregister_window};
use crate::Event;

pub(crate) use state::{MetalViewIvars, PendingKey, WindowDelegateIvars};

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
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method_id(makeBackingLayer))]
        fn make_backing_layer(&self) -> Retained<objc2_quartz_core::CALayer> {
            self.render_make_backing_layer()
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, rect: NSRect) {
            self.render_draw_rect(rect);
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: NSView::setFrameSize: takes NSSize; super signature matches.
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            self.resize_set_frame_size(size);
        }

        #[unsafe(method(viewWillStartLiveResize))]
        fn view_will_start_live_resize(&self) {
            // SAFETY: super signature is `- (void)viewWillStartLiveResize`.
            let _: () = unsafe { msg_send![super(self), viewWillStartLiveResize] };
            self.resize_view_will_start_live_resize();
        }

        #[unsafe(method(viewDidEndLiveResize))]
        fn view_did_end_live_resize(&self) {
            // SAFETY: super signature is `- (void)viewDidEndLiveResize`.
            let _: () = unsafe { msg_send![super(self), viewDidEndLiveResize] };
            self.resize_view_did_end_live_resize();
        }

        // Mouse selectors — bodies in `responder.rs`.

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) { self.responder_mouse_down(event); }
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) { self.responder_mouse_up(event); }
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) { self.responder_mouse_dragged(event); }
        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) { self.responder_right_mouse_down(event); }
        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) { self.responder_right_mouse_up(event); }
        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) { self.responder_right_mouse_dragged(event); }
        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) { self.responder_other_mouse_down(event); }
        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) { self.responder_other_mouse_up(event); }
        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) { self.responder_other_mouse_dragged(event); }
        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) { self.responder_mouse_moved(event); }

        /// Receive but drop — framework synthesizes Enter from hit-test diff.
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            // Intentionally empty; prevents responder chain walk.
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) { self.responder_mouse_exited(); }
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) { self.responder_scroll_wheel(event); }

        // Keyboard selectors — bodies in `responder.rs`.

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) { self.responder_key_down(event); }
        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) { self.responder_key_up(event); }
        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) { self.responder_flags_changed(event); }
        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) { self.responder_update_tracking_areas(); }
    }

    // SAFETY: All NSTextInputClient methods are implemented per the
    // protocol contract; bodies are wrapped in `ffi_boundary` so Rust
    // panics cannot cross the Obj-C ABI. String parameters arrive as
    // either NSString or NSAttributedString (per Apple docs); the helper
    // `view_ime::ns_input_to_string` handles both.
    unsafe impl NSTextInputClient for MetalView {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text_replacement_range(&self, string: &AnyObject, replacement_range: NSRange) {
            ffi_boundary(|| self.ime_handle_insert_text(string, replacement_range));
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command_by_selector(&self, _selector: Sel) {
            self.responder_do_command_by_selector();
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text_selected_range_replacement_range(
            &self,
            string: &AnyObject,
            selected_range: NSRange,
            replacement_range: NSRange,
        ) {
            ffi_boundary(|| {
                self.ime_handle_set_marked_text(string, selected_range, replacement_range)
            });
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) { ffi_boundary(|| self.ime_handle_unmark_text()); }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            let mut out = NSRange::new(0, 0);
            ffi_boundary(|| out = self.ime_handle_selected_range());
            out
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            let mut out = NSRange::new(0, 0);
            ffi_boundary(|| out = self.ime_handle_marked_range());
            out
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            let mut out = false;
            ffi_boundary(|| out = self.ime_handle_has_marked_text());
            out
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        fn attributed_substring_for_proposed_range_actual_range(
            &self,
            range: NSRange,
            _actual_range: NSRangePointer,
        ) -> Option<Retained<NSAttributedString>> {
            let mut out = None;
            ffi_boundary(|| out = self.ime_handle_attributed_substring(range));
            out
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes_for_marked_text(&self) -> Retained<NSArray<NSAttributedStringKey>> {
            // Cannot return early on panic; default to empty array.
            let mut out: Option<Retained<NSArray<NSAttributedStringKey>>> = None;
            ffi_boundary(|| out = Some(self.ime_handle_valid_attributes()));
            out.unwrap_or_default()
        }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        fn first_rect_for_character_range_actual_range(
            &self,
            range: NSRange,
            _actual_range: NSRangePointer,
        ) -> NSRect {
            let mut out = NSRect::ZERO;
            ffi_boundary(|| out = self.ime_handle_first_rect(range));
            out
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index_for_point(&self, point: NSPoint) -> NSUInteger {
            let mut out: NSUInteger = 0;
            ffi_boundary(|| out = self.ime_handle_character_index(point));
            out
        }
    }
);

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
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, notification: &NSNotification) {
            self.resize_window_did_resize(notification);
        }

        /// Returns `true` to allow the close to proceed; quit is triggered
        /// in `windowWillClose:`.
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSWindow) -> bool {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| dispatch_event(Event::WindowCloseRequested { window: id }));
            true
        }

        /// Called just before the window's Obj-C resources are released.
        /// Unregister FIRST so AppKit's late paint events cannot reach a
        /// stale delegate.
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            let id = self.ivars().window_id.get();
            ffi_boundary(|| {
                unregister_window(id);
                dispatch_event(Event::WindowDestroyed { window: id });
                // Quit policy lives in the user's `WindowCloseRequested`
                // handler; calling `terminate:` here would invoke `exit()`
                // and skip `Event::Exiting` entirely.
            });
        }

        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn window_did_change_backing_properties(&self, notification: &NSNotification) {
            self.resize_window_did_change_backing_properties(notification);
        }

        #[unsafe(method(windowDidChangeOcclusionState:))]
        fn window_did_change_occlusion_state(&self, notification: &NSNotification) {
            self.render_window_did_change_occlusion_state(notification);
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, notification: &NSNotification) {
            self.responder_window_did_become_key(notification);
        }
    }
);
