//! Instance variables and constructors for [`MetalView`] and
//! [`WindowDelegate`]. Selector declarations themselves live in
//! [`super`]'s `define_class!` blocks; bodies live in the per-concern
//! submodules (`render`, `resize`, `responder`).

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadOnly, msg_send};
use objc2_app_kit::{NSEventModifierFlags, NSTrackingArea};
use objc2_foundation::{MainThreadMarker, NSRect};

use super::{MetalView, WindowDelegate};
use crate::{Key, KeyCode, Modifiers, WindowId};

// ---------------------------------------------------------------------------
// MetalView ivars
// ---------------------------------------------------------------------------

pub struct MetalViewIvars {
    pub(crate) window_id: Cell<WindowId>,
    /// Current tracking area for mouse move/enter/exit events.
    pub(super) tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
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
// WindowDelegate ivars
// ---------------------------------------------------------------------------

pub struct WindowDelegateIvars {
    pub(crate) window_id: Cell<WindowId>,
}

impl WindowDelegate {
    pub(crate) fn new(mtm: MainThreadMarker, window_id: WindowId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WindowDelegateIvars {
            window_id: Cell::new(window_id),
        });
        // SAFETY: `NSObject`'s `init` has no additional requirements.
        unsafe { msg_send![super(this), init] }
    }
}
