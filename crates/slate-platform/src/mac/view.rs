//! MetalView and WindowDelegate — AppKit view and delegate callbacks.

mod render;
mod responder;

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSEvent, NSEventModifierFlags, NSTextInputClient, NSTrackingArea, NSView, NSWindow,
    NSWindowDelegate,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSAttributedStringKey, NSNotification, NSObject,
    NSObjectProtocol, NSPoint, NSRange, NSRangePointer, NSRect, NSSize, NSUInteger,
};
use super::{
    dispatch_event, ffi_boundary, post_redraw_event, unregister_window, with_window_delegate,
};
use crate::{Event, Key, KeyCode, Modifiers, PhysicalSize, WindowId};

// ---------------------------------------------------------------------------
// MetalView — custom NSView backed by CAMetalLayer
// ---------------------------------------------------------------------------

pub struct MetalViewIvars {
    pub(crate) window_id: Cell<WindowId>,
    /// Current tracking area for mouse move/enter/exit events.
    tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
    /// True between viewWillStartLiveResize and viewDidEndLiveResize. Gates
    /// the setFrameSize: sync-dispatch path: programmatic frame changes still
    /// flow through windowDidResize: rather than the sync-present path.
    pub(crate) live_resize: Cell<bool>,
    /// Previous modifier flags for `flagsChanged:` diff.
    pub(crate) prev_modifier_flags: Cell<NSEventModifierFlags>,
    /// True between the first `setMarkedText:` of a composition session and
    /// the matching `insertText:` / `unmarkText`. Drives the IME state
    /// machine so `insertText:` can tell IME commits apart from non-IME
    /// typing (AppKit clears marked text BEFORE `insertText:` on commit,
    /// so `hasMarkedText` is unreliable at that point).
    pub(crate) was_composing: Cell<bool>,
    /// IME-first key routing while composing.
    ///
    /// When `keyDown:` fires *during* a composition, we must NOT pre-dispatch
    /// `Event::KeyDown` (it would let the framework act on Tab/etc. before
    /// the IME got a chance to use the key — e.g. Pinyin uses Tab to cycle
    /// tone marks, but the framework's focus logic would also fire). We
    /// instead stash the decoded key here and run `interpretKeyEvents:`
    /// first. If the IME refuses the key (AppKit calls `doCommandBySelector:`),
    /// that callback dispatches the stashed `KeyDown` so navigation /
    /// Escape / IMEs-that-pass-Tab-through still reach the framework.
    ///
    /// Holds `RefCell` (not `Cell`) because `Key::Character(String)` isn't `Copy`.
    pub(crate) pending_keydown: RefCell<Option<PendingKey>>,
}

/// Decoded key info stashed by `keyDown:` while composing, dispatched
/// by `doCommandBySelector:` if the IME refused the key. See
/// [`MetalViewIvars::pending_keydown`].
pub(crate) struct PendingKey {
    pub(crate) code: KeyCode,
    pub(crate) key: Key,
    pub(crate) modifiers: Modifiers,
    pub(crate) is_repeat: bool,
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
        /// AppKit when `wantsLayer` is true. Body in `render.rs`.
        #[unsafe(method_id(makeBackingLayer))]
        fn make_backing_layer(&self) -> Retained<objc2_quartz_core::CALayer> {
            self.render_make_backing_layer()
        }

        /// Called by AppKit when the view's contents need refreshing.
        /// Body in `render.rs`.
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _rect: NSRect) {
            self.render_draw_rect(_rect);
        }

        /// Live-resize hook: AppKit calls setFrameSize: once per drag tick
        /// inside an open implicit CATransaction. We let super update the
        /// view bounds (the transaction stays open), then — while still
        /// inside the selector — dispatch a sync resize + render. The
        /// renderer's present_sync calls CATransaction::flush() to commit
        /// the open transaction with the new framebuffer in place. Without
        /// this the new bounds commit one selector before the new pixels
        /// land, producing the right/bottom edge flash.
        ///
        /// Gated on live_resize so steady-state size-from-code paths
        /// (programmatic frame changes) flow through windowDidResize:
        /// instead and aren't pulled onto the sync-present path unnecessarily.
        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: NSView::setFrameSize: takes NSSize; super signature matches.
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
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

        /// AppKit signals start of live resize. Sets the gate so the
        /// setFrameSize: override above only dispatches during a drag.
        #[unsafe(method(viewWillStartLiveResize))]
        fn view_will_start_live_resize(&self) {
            // SAFETY: super signature is `- (void)viewWillStartLiveResize`.
            let _: () = unsafe { msg_send![super(self), viewWillStartLiveResize] };
            ffi_boundary(|| {
                self.ivars().live_resize.set(true);
            });
        }

        /// AppKit signals end of live resize. Clears the gate.
        #[unsafe(method(viewDidEndLiveResize))]
        fn view_did_end_live_resize(&self) {
            // SAFETY: super signature is `- (void)viewDidEndLiveResize`.
            let _: () = unsafe { msg_send![super(self), viewDidEndLiveResize] };
            ffi_boundary(|| {
                self.ivars().live_resize.set(false);
            });
        }

        // -----------------------------------------------------------------------
        // Mouse event selectors (bodies in `responder.rs`)
        // -----------------------------------------------------------------------

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.responder_mouse_down(event);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.responder_mouse_up(event);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.responder_mouse_dragged(event);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            self.responder_right_mouse_down(event);
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            self.responder_right_mouse_up(event);
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            self.responder_right_mouse_dragged(event);
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            self.responder_other_mouse_down(event);
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            self.responder_other_mouse_up(event);
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            self.responder_other_mouse_dragged(event);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.responder_mouse_moved(event);
        }

        /// Receive but drop — framework synthesizes Enter from hit-test diff.
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            // Intentionally empty; prevents responder chain walk.
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.responder_mouse_exited();
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            self.responder_scroll_wheel(event);
        }

        // -----------------------------------------------------------------------
        // Keyboard event selectors (bodies in `responder.rs`)
        // -----------------------------------------------------------------------

        /// `keyDown:` — physical key press. See `responder::MetalView::responder_key_down`
        /// for the IME-first routing rationale.
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.responder_key_down(event);
        }

        /// `keyUp:` — physical key release.
        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            self.responder_key_up(event);
        }

        /// `flagsChanged:` — modifier-only press/release. AppKit does not
        /// produce keyDown:/keyUp: for bare Shift/Ctrl/Alt/Meta; we diff the
        /// previous flag bitfield against the new one and synthesize the
        /// corresponding events.
        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            self.responder_flags_changed(event);
        }

        /// Called by AppKit when view bounds change. Reinstall tracking area.
        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            self.responder_update_tracking_areas();
        }
    }

    // SAFETY: All NSTextInputClient methods are implemented per the
    // protocol contract; bodies are wrapped in `ffi_boundary` so Rust
    // panics cannot cross the Obj-C ABI. String parameters arrive as
    // either NSString or NSAttributedString (per Apple docs); the helper
    // [`view_ime::ns_input_to_string`] handles both.
    unsafe impl NSTextInputClient for MetalView {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text_replacement_range(
            &self,
            string: &AnyObject,
            replacement_range: NSRange,
        ) {
            ffi_boundary(|| self.ime_handle_insert_text(string, replacement_range));
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command_by_selector(&self, _selector: Sel) {
            // AppKit calls this when the IME refused the keystroke.
            // - Composing path (`keyDown:` deferred the `KeyDown` dispatch):
            //   emit the stashed `Event::KeyDown` now so navigation /
            //   Escape / IMEs-that-pass-Tab-through still reach the
            //   framework. This is also what makes the "Tab commits then
            //   focuses" flow work for IMEs that commit-on-Tab — by the
            //   time AppKit reaches `doCommandBySelector:(insertTab:)`,
            //   the prior `insertText:` already cleared `was_composing`
            //   and emitted `ImeCommit`, so the synthesized `KeyDown{Tab}`
            //   moves focus *after* the commit on a single press.
            // - Non-composing path: `keyDown:` already dispatched `KeyDown`,
            //   so `pending_keydown` is `None` and this is a no-op.
            let id = self.ivars().window_id.get();
            if let Some(pk) = self.ivars().pending_keydown.borrow_mut().take() {
                dispatch_event(Event::KeyDown {
                    window: id,
                    code: pk.code,
                    key: pk.key,
                    modifiers: pk.modifiers,
                    is_repeat: pk.is_repeat,
                });
            }
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
        fn unmark_text(&self) {
            ffi_boundary(|| self.ime_handle_unmark_text());
        }

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

impl MetalView {
    pub(crate) fn new(mtm: MainThreadMarker, window_id: WindowId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MetalViewIvars {
            window_id: Cell::new(window_id),
            tracking_area: RefCell::new(None),
            live_resize: Cell::new(false),
            prev_modifier_flags: Cell::new(NSEventModifierFlags::empty()),
            was_composing: Cell::new(false),
            pending_keydown: RefCell::new(None),
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

        /// Called when the window's occlusion state changes. Body in
        /// `render.rs`.
        #[unsafe(method(windowDidChangeOcclusionState:))]
        fn window_did_change_occlusion_state(&self, notification: &NSNotification) {
            self.render_window_did_change_occlusion_state(notification);
        }

        /// Called when the window becomes key. Snapshot the current modifier
        /// flags onto the content view so `flagsChanged:` diffs are aligned
        /// with the actual hardware state — absorbs cross-app modifier drift
        /// (e.g. Shift pressed in another app during Alt-Tab). Body in
        /// `responder.rs`.
        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, notification: &NSNotification) {
            self.responder_window_did_become_key(notification);
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

