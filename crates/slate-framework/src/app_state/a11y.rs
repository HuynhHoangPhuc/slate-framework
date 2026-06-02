//! Screen-reader action routing + window key-state tracking on `AppState`.
//!
//! Platform a11y adapters post [`Event::AccessibilityAction`] (never dispatch
//! inline — see [`a11y_macos::RoutedActions`](crate::a11y_macos)); the framework
//! maps the node id back to its [`ElementId`] and synthesises the action against
//! the same focus / keyboard dispatch the OS event path uses. `Focus` → set
//! focus; `Activate` → focus + a synthetic Space press, which every interactive
//! widget already treats as activation (Button/Checkbox/Switch on Enter/Space),
//! so there is no per-widget click logic to duplicate.
//!
//! [`Event::AccessibilityAction`]: slate_platform::Event::AccessibilityAction

use slate_platform::{A11yAction, Key, KeyCode, Modifiers, NamedKey, WindowId};

use crate::types::ElementId;

use super::state::AppState;
use super::types::AppSignal;

impl AppState {
    /// Route a screen-reader action to the targeted element. `node` is the
    /// element's [`ElementId`] numeric value (set when the a11y tree was
    /// lowered to AccessKit).
    ///
    /// Runs on a clean run-loop iteration (the action was *posted*, not
    /// inlined), so the focus / keyboard dispatch it triggers never aliases a
    /// live render or handler borrow.
    pub(crate) fn dispatch_a11y_action(
        &self,
        window: WindowId,
        node: u64,
        action: A11yAction,
    ) -> AppSignal {
        let id = ElementId::from_raw(node);
        match action {
            A11yAction::Focus => {
                if self.focus_element(window, id) {
                    AppSignal::RequestRedraw { window }
                } else {
                    AppSignal::None
                }
            }
            A11yAction::Activate => {
                // Focus first so the synthetic key lands on the target, then
                // press Space — reuses each widget's existing keyboard
                // activation rather than duplicating per-widget click logic.
                self.focus_element(window, id);
                self.dispatch_key_down(
                    window,
                    KeyCode::Space,
                    Key::Named(NamedKey::Space),
                    Modifiers::default(),
                    false,
                );
                self.dispatch_key_up(
                    window,
                    KeyCode::Space,
                    Key::Named(NamedKey::Space),
                    Modifiers::default(),
                );
                AppSignal::RequestRedraw { window }
            }
            // `A11yAction` is `#[non_exhaustive]`; a future kind Slate cannot yet
            // synthesise is a safe no-op until a handler is added.
            _ => AppSignal::None,
        }
    }

    /// Set focus to `id` on `window`'s registry without an `EventCtx`. Returns
    /// whether focus actually moved (`false` if `id` is not a registered
    /// focusable — which also guards against a stale/dangling AT target).
    /// Drops the outer `windows` borrow before touching the per-window registry.
    fn focus_element(&self, window: WindowId, id: ElementId) -> bool {
        let registry = {
            let guard = self.windows.borrow();
            guard.get(&window).map(|w| w.focus_registry.clone())
        };
        match registry {
            Some(reg) => reg.borrow_mut().set_focus(id),
            None => false,
        }
    }
}

#[cfg(target_os = "macos")]
impl AppState {
    /// Record `window` as the key window for a11y focus-state: mark it key and
    /// every other window non-key (only one window is key at a time). Drives the
    /// macOS adapter's `update_view_focus_state` so a background window's adapter
    /// does not claim VoiceOver focus.
    pub(crate) fn update_a11y_key_window(&self, window: WindowId) {
        let guard = self.windows.borrow();
        for (id, win) in guard.iter() {
            win.is_key.set(*id == window);
        }
    }
}
