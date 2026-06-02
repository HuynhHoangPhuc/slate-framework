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
use slate_platform::{DefaultWindow, WindowId, post_accessibility_action};

use crate::a11y_accesskit::to_accesskit_tree_update;
use crate::a11y_action_routing::route_action_request;
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

/// Routes AT-invoked actions (Click/Focus) back into Slate's event system.
///
/// Holds the owning [`WindowId`] so the posted event credits the right window.
/// **Posts** the action (never dispatches inline): VoiceOver can invoke an
/// action while Slate is mid-stack (inside a render/handler borrow), so an
/// inline focus/activation would hit the dispatch re-entrancy guard and be
/// dropped. Posting lets it land on a clean run-loop iteration — the same
/// discipline native menu activation uses (`post_menu_event`).
struct RoutedActions {
    window: WindowId,
}

impl ActionHandler for RoutedActions {
    fn do_action(&mut self, request: ActionRequest) {
        if let Some((node, action)) = route_action_request(&request) {
            post_accessibility_action(self.window, node, action);
        }
    }
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
    /// handle is not AppKit (e.g. headless/test windows). `window_id` is the
    /// routing target for AT actions posted back into Slate.
    ///
    /// Note: `accesskit_macos 0.26.1`'s `SubclassingAdapter` is a stateless
    /// forwarder — it takes only an activation + action handler and exposes no
    /// `DeactivationHandler` hook (the crate has no deactivation concept at
    /// all). Client disconnect is handled implicitly: AccessKit re-invokes
    /// `request_initial_tree` (served from the cached tree) on the next
    /// activation, so no explicit active-latch reset is needed.
    pub fn for_window(window: &DefaultWindow, window_id: WindowId) -> Option<Self> {
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
                RoutedActions { window: window_id },
            )
        };
        log::info!("a11y(macos): SubclassingAdapter created on NSView {ns_view:p}");
        Some(Self {
            adapter,
            last_tree,
            logged_active: Cell::new(false),
        })
    }

    /// Push the current a11y tree to VoiceOver when an assistive client is
    /// connected. `view_focused` reflects the real window key state (drives
    /// `update_view_focus_state`, without which AccessKit reports no focused
    /// element and nothing is announced). `build` produces the full tree.
    ///
    /// The build closure runs **only when a client is active**:
    /// `update_if_active` no-ops cheaply when no screen reader is connected, so
    /// idle runs (the universal case) never pay the full-tree rebuild+clone
    /// cost — fixing the spike's per-frame waste without a separate dirty flag.
    /// The cache is refreshed inside the closure so a reader connecting mid-
    /// session is served the current tree via `request_initial_tree`.
    pub fn update(&mut self, view_focused: bool, build: impl FnOnce() -> TreeUpdate) {
        let last_tree = self.last_tree.clone();
        let active = match self.adapter.update_if_active(|| {
            let tree = build();
            *last_tree.borrow_mut() = Some(tree.clone());
            tree
        }) {
            Some(events) => {
                events.raise();
                true
            }
            None => false,
        };
        if active {
            // AccessKit only reports a focused element when the view is marked
            // focused; gate on real window key state instead of a hardcoded
            // `true` so a background window does not claim focus.
            if let Some(events) = self.adapter.update_view_focus_state(view_focused) {
                events.raise();
            }
            if !self.logged_active.replace(true) {
                log::info!("a11y(macos): adapter active — assistive client connected; tree pushed");
            }
        }
    }
}

/// Lazily create the per-window adapter (on the first frame after the surface
/// is realized, so the NSView exists), then push the current a11y tree when an
/// assistive client is connected.
///
/// No-op until the renderer is ready or on non-AppKit windows. The tree is
/// built lazily (inside `MacA11yAdapter::update`) only when a screen reader is
/// active, so idle frames cost nothing. `view_focused` is the window's real key
/// state, driving AccessKit's focus marker.
pub(crate) fn push_tree_to_voiceover(
    adapter_slot: &RefCell<Option<MacA11yAdapter>>,
    window: &DefaultWindow,
    window_id: WindowId,
    renderer_ready: bool,
    view_focused: bool,
    roots: &[AccessibilityNode],
    focus: Option<ElementId>,
) {
    if !renderer_ready {
        return;
    }
    let mut slot = adapter_slot.borrow_mut();
    if slot.is_none() {
        *slot = MacA11yAdapter::for_window(window, window_id);
        if slot.is_none() {
            return; // not an AppKit window — nothing to drive
        }
    }
    slot.as_mut()
        .expect("adapter present")
        .update(view_focused, || to_accesskit_tree_update(roots, focus));
}
