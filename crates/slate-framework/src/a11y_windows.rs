//! Windows AccessKit platform adapter — pushes Slate's a11y tree to Narrator.
//!
//! Slate builds an internal `AccessibilityNode` tree each frame (`context.rs`
//! prepaint walk), but nothing reaches the OS without a platform adapter. This
//! module owns a per-window `accesskit_windows::Adapter` that answers UI
//! Automation queries so Narrator (and other UIA clients) can read the same
//! tree VoiceOver reads on macOS.
//!
//! ## Why the non-subclassing `Adapter`
//!
//! `accesskit_windows` ships two adapters. `SubclassingAdapter` installs its own
//! `wnd_proc` but **panics if the window is already visible** — and Slate
//! creates `WS_VISIBLE` windows (required for `WM_PAINT`), so the macOS
//! lazy-on-first-frame pattern would panic. The non-subclassing `Adapter` works
//! on visible windows; in exchange the caller must feed it `WM_GETOBJECT` and
//! OS-focus changes. Slate owns its `wnd_proc`, so the platform layer routes
//! those messages through [`WindowA11yDelegate`] (which this adapter
//! implements) — the same synchronous-delegate shape Slate already uses for IME
//! queries.
//!
//! ## Lifetime / threading
//!
//! The adapter lives in an `Rc` on the per-window `WindowState` (main-thread
//! bound); the platform `Window` holds a `Weak<dyn WindowA11yDelegate>`,
//! mirroring the render/IME delegate ownership and avoiding a strong cycle.
//! Only the routed-action handler is `Send` (AccessKit requires it): it carries
//! just a `WindowId` and posts the action back through the thread-safe
//! `post_accessibility_action` seam. The `accesskit_windows` `windows`/
//! `windows-core` generation stays independent of `slate-platform`'s win deps
//! because only a raw `HWND` crosses the boundary — do not unify (mirrors the
//! macOS raw-NSView-pointer discipline).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, TreeUpdate};
use accesskit_windows::{Adapter, HWND, LPARAM, LRESULT, WPARAM};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slate_platform::{
    DefaultWindow, Window, WindowA11yDelegate, WindowId, post_accessibility_action,
};

use crate::a11y_accesskit::to_accesskit_tree_update;
use crate::a11y_action_routing::route_action_request;
use crate::types::{AccessibilityNode, ElementId};

/// Shared cache of the most recent full tree. The lazy
/// [`ActivationHandler::request_initial_tree`] (fired when a UIA client first
/// queries via `WM_GETOBJECT`) and the per-frame `update_if_active` both serve
/// from here, so a reader that connects mid-session still gets the current tree.
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
/// Holds only the owning [`WindowId`] so it stays `Send` (AccessKit requires the
/// Windows action handler be `Send` — UIA may invoke it off the main thread).
/// **Posts** the action (never dispatches inline): the post lands on a clean
/// pump turn via the thread-safe `post_accessibility_action` seam, dodging the
/// dispatch re-entrancy guard exactly as native-menu activation does.
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

/// Per-window Windows accessibility adapter.
pub struct WinA11yAdapter {
    adapter: RefCell<Adapter>,
    last_tree: SharedTree,
    /// One-shot log latch: true once the adapter has gone active (a UIA client
    /// connected). Avoids spamming the per-frame push.
    logged_active: Cell<bool>,
}

impl WinA11yAdapter {
    /// Build an adapter for `window`'s `HWND`, or `None` if the platform handle
    /// is not Win32 (e.g. headless/test windows). `window_id` is the routing
    /// target for AT actions posted back into Slate.
    ///
    /// The window's initial OS-focus state is assumed `true` (mirroring the
    /// macOS `is_key` default — a freshly created window is normally the active
    /// one); a later `WM_KILLFOCUS` corrects a backgrounded window. Returns an
    /// `Rc` so the platform can hold a `Weak` delegate to the same allocation.
    pub fn for_window(window: &DefaultWindow, window_id: WindowId) -> Option<Rc<Self>> {
        let raw = window.window_handle().ok()?.as_raw();
        let hwnd = match raw {
            RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as *mut _),
            _ => return None,
        };
        // Build before any UIA query: `Adapter::new` initialises UIA up-front to
        // avoid the nested-WM_GETOBJECT race documented on the crate.
        let adapter = Adapter::new(hwnd, true, RoutedActions { window: window_id });
        log::info!("a11y(windows): Adapter created on HWND {:?}", hwnd.0);
        Some(Rc::new(Self {
            adapter: RefCell::new(adapter),
            last_tree: Rc::new(RefCell::new(None)),
            logged_active: Cell::new(false),
        }))
    }

    /// Push the current a11y tree to the UIA adapter when a client is connected.
    /// `build` produces the full tree; it runs **only when a client is active**
    /// (`update_if_active` no-ops cheaply otherwise), so idle frames — the
    /// universal case — never pay the full-tree rebuild+clone cost. The cache is
    /// refreshed inside the closure so a reader connecting mid-session is served
    /// the current tree via `request_initial_tree`.
    pub fn update(&self, build: impl FnOnce() -> TreeUpdate) {
        let last_tree = self.last_tree.clone();
        let events = self.adapter.borrow_mut().update_if_active(|| {
            let tree = build();
            *last_tree.borrow_mut() = Some(tree.clone());
            tree
        });
        // Borrow dropped above; `raise` may synchronously re-enter UIA.
        if let Some(events) = events {
            events.raise();
            if !self.logged_active.replace(true) {
                log::info!("a11y(windows): adapter active — UIA client connected; tree pushed");
            }
        }
    }
}

impl WindowA11yDelegate for WinA11yAdapter {
    fn handle_wm_getobject(
        &self,
        _window_id: WindowId,
        wparam: usize,
        lparam: isize,
    ) -> Option<isize> {
        let mut activation = CachedTreeActivation(self.last_tree.clone());
        let mut adapter = self.adapter.borrow_mut();
        let result = adapter.handle_wm_getobject(WPARAM(wparam), LPARAM(lparam), &mut activation)?;
        // Drop the adapter borrow before `.into()`: converting to the LRESULT
        // calls UIA, which can nest a WM_GETOBJECT and re-enter this method.
        drop(adapter);
        let lresult: LRESULT = result.into();
        Some(lresult.0)
    }

    fn window_focus_changed(&self, _window_id: WindowId, focused: bool) {
        let events = self.adapter.borrow_mut().update_window_focus_state(focused);
        // Borrow dropped above before raising (UIA may re-enter).
        if let Some(events) = events {
            events.raise();
        }
    }
}

/// Lazily create the per-window adapter (on the first frame after the surface is
/// realized) and install it as the window's a11y delegate, then push the
/// current a11y tree when a UIA client is connected.
///
/// No-op until the renderer is ready or on non-Win32 windows. The tree is built
/// lazily (inside [`WinA11yAdapter::update`]) only when a screen reader is
/// active, so idle frames cost nothing.
pub(crate) fn push_tree_to_narrator(
    adapter_slot: &RefCell<Option<Rc<WinA11yAdapter>>>,
    window: &DefaultWindow,
    window_id: WindowId,
    renderer_ready: bool,
    roots: &[AccessibilityNode],
    focus: Option<ElementId>,
) {
    if !renderer_ready {
        return;
    }
    let adapter = {
        let mut slot = adapter_slot.borrow_mut();
        if slot.is_none() {
            match WinA11yAdapter::for_window(window, window_id) {
                Some(adapter) => {
                    let weak: Weak<dyn WindowA11yDelegate> = Rc::downgrade(&adapter) as _;
                    window.set_a11y_delegate(weak);
                    *slot = Some(adapter);
                }
                None => return, // not a Win32 window — nothing to drive
            }
        }
        slot.as_ref().expect("adapter present").clone()
    };
    // Slot borrow released; `update` may re-enter the delegate (which borrows
    // the adapter's own RefCell, not the slot) during a UIA raise.
    adapter.update(|| to_accesskit_tree_update(roots, focus));
}
