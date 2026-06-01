//! macOS AccessKit platform adapter — pushes Slate's a11y tree to VoiceOver.
//!
//! Slate builds an internal `AccessibilityNode` tree each frame (`context.rs`
//! prepaint walk), but nothing reaches the OS without a platform adapter. This
//! module owns a per-window `accesskit_macos::SubclassingAdapter`, which
//! dynamically subclasses the window's `NSView` so VoiceOver can query it — no
//! manual NSView subclassing, which suits Slate's non-winit AppKit backend.
//!
//! Front-loaded by the S0 DataGrid a11y spike; this is the seed of the P7
//! platform-accessibility work. macOS only — Windows/Narrator is a later pass.
//!
//! Verified against real VoiceOver (S0 spike): the tree reaches VoiceOver once
//! the adapter is active, but VoiceOver stays silent unless the view is also
//! marked focused via `update_view_focus_state(true)` — without it AccessKit
//! pushes the tree but reports no focused element, so nothing is announced.
//! Slate's content view (`MetalView`) is the window's first responder, so the
//! window forwards accessibility focus queries to it; no NSWindow-subclass
//! focus forwarder is needed.
//!
//! ## Lifetime / threading
//!
//! `SubclassingAdapter` holds retained `objc2` `Id`s, so it is `!Send`/`!Sync`
//! and lives on the per-window `WindowState` (already main-thread-bound). The
//! objc2 0.5 generation it links is independent of the workspace's objc2 0.6;
//! only a raw `*mut c_void` NSView pointer crosses the boundary, so the two
//! coexist safely.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, TreeUpdate};
use accesskit_macos::SubclassingAdapter;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slate_platform::DefaultWindow;

use crate::a11y_accesskit::to_accesskit_tree_update;
use crate::types::{AccessibilityNode, ElementId};

/// Shared cache of the most recent full tree. The lazy
/// [`ActivationHandler::request_initial_tree`] (fired when VoiceOver first
/// connects) and the per-frame `update_if_active` both serve from here, so a
/// reader that connects mid-session still gets the current tree.
type SharedTree = Rc<RefCell<Option<TreeUpdate>>>;

/// Serves the cached tree when the assistive client first activates.
struct CachedTreeActivation(SharedTree);

impl ActivationHandler for CachedTreeActivation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.0.borrow().clone()
    }
}

/// Spike-level action handler: AT-invoked actions (Click/Focus/…) are not yet
/// routed back into Slate's event system. Tree navigation + announcement is
/// what the S0 spike validates; action round-trip is P7 proper.
struct UnroutedActions;

impl ActionHandler for UnroutedActions {
    fn do_action(&mut self, _request: ActionRequest) {}
}

/// Per-window macOS accessibility adapter.
pub struct MacA11yAdapter {
    adapter: SubclassingAdapter,
    last_tree: SharedTree,
    /// One-shot log latch: true once we've observed the adapter go active
    /// (VoiceOver connected). Avoids spamming the per-frame push.
    logged_active: Cell<bool>,
}

impl MacA11yAdapter {
    /// Build an adapter for `window`'s `NSView`, or `None` if the platform
    /// handle is not AppKit (e.g. headless/test windows).
    pub fn for_window(window: &DefaultWindow) -> Option<Self> {
        let raw = window.window_handle().ok()?.as_raw();
        let ns_view = match raw {
            RawWindowHandle::AppKit(handle) => handle.ns_view.as_ptr(),
            _ => return None,
        };
        let last_tree: SharedTree = Rc::new(RefCell::new(None));
        // SAFETY: `ns_view` is the live content view of `window`, valid for the
        // window's lifetime. The adapter is dropped with `WindowState`, before
        // the window's NSView is released.
        let adapter = unsafe {
            SubclassingAdapter::new(
                ns_view,
                CachedTreeActivation(last_tree.clone()),
                UnroutedActions,
            )
        };
        log::info!("a11y(macos): SubclassingAdapter created on NSView {ns_view:p}");
        Some(Self {
            adapter,
            last_tree,
            logged_active: Cell::new(false),
        })
    }

    /// Push a full tree to VoiceOver: cache it for lazy activation, apply via
    /// `update_if_active`, mark the view focused (so focus-based announcements
    /// fire), and raise any queued accessibility events.
    pub fn update(&mut self, update: TreeUpdate) {
        *self.last_tree.borrow_mut() = Some(update.clone());
        let active = match self.adapter.update_if_active(|| update) {
            Some(events) => {
                events.raise();
                true
            }
            None => false,
        };
        if active {
            // AccessKit only reports a focused element when the view is marked
            // focused; the window is key here, so the content view is focused.
            if let Some(events) = self.adapter.update_view_focus_state(true) {
                events.raise();
            }
            if !self.logged_active.replace(true) {
                log::info!("a11y(macos): adapter active — assistive client connected; tree pushed");
            }
        }
    }
}

/// Lazily create the per-window adapter (on the first frame after the surface
/// is realized, so the NSView exists), then push the current a11y tree.
///
/// No-op until the renderer is ready or on non-AppKit windows. Building a full
/// tree every frame is intentional for the spike — simplest correct behavior.
pub(crate) fn push_tree_to_voiceover(
    adapter_slot: &RefCell<Option<MacA11yAdapter>>,
    window: &DefaultWindow,
    renderer_ready: bool,
    roots: &[AccessibilityNode],
    focus: Option<ElementId>,
) {
    if !renderer_ready {
        return;
    }
    let mut slot = adapter_slot.borrow_mut();
    if slot.is_none() {
        *slot = MacA11yAdapter::for_window(window);
        if slot.is_none() {
            return; // not an AppKit window — nothing to drive
        }
    }
    let update = to_accesskit_tree_update(roots, focus);
    slot.as_mut().expect("adapter present").update(update);
}
