//! Test-only accessors on `AppState`, gated on the `test-hooks` feature.
//!
//! These wrappers expose `pub(crate)` setters and synthetic event dispatchers
//! so integration tests can drive the runtime without going through the
//! platform layer. None of these are reachable in a release build of a
//! downstream crate unless the `test-hooks` feature is explicitly enabled.

#![cfg(any(test, feature = "test-hooks"))]

use slate_platform::{Key, KeyCode, Modifiers, Window, WindowId};

use crate::event::{
    ImeCommitHandler, ImeLifecycleHandler, ImePreeditHandler, KeyHandler, TextInputHandler,
};
use crate::types::ElementId;

use super::state::AppState;
use super::types::{AppSignal, RecoveryState};

impl AppState {
    // -------------------------------------------------------------------------
    // Window setup helpers — public to integration tests under test-hooks.
    // -------------------------------------------------------------------------

    /// Register a window's redraw requester. Test-only public wrapper around
    /// the `pub(crate)` `register_redraw_requester` so integration tests in
    /// `tests/` can call it without a private-items import.
    pub fn register_redraw_requester_for_test(
        &self,
        window: WindowId,
        req: crate::executor::RedrawRequester,
    ) {
        self.register_redraw_requester(window, req);
    }

    // -------------------------------------------------------------------------
    // Window-id helper — returns first registered window id for single-window tests.
    // -------------------------------------------------------------------------

    /// Return the `WindowId` of the first registered window. Test-only.
    /// Panics if no window is registered (harness not yet set up).
    pub fn window_id_for_test(&self) -> WindowId {
        self.windows
            .borrow()
            .keys()
            .next()
            .copied()
            .expect("no windows registered — call make_state() which inserts a test window")
    }

    // -------------------------------------------------------------------------
    // Per-window renderer accessors
    // -------------------------------------------------------------------------

    /// Current renderer generation for a window. `None` if not yet initialized.
    pub fn renderer_generation_for(&self, window: WindowId) -> Option<u64> {
        self.windows
            .borrow()
            .get(&window)
            .and_then(|w| w.renderer.borrow().as_ref().map(|r| r.current_generation()))
    }

    /// Convenience: renderer generation for the first registered window.
    pub fn renderer_generation(&self) -> Option<u64> {
        self.renderer_generation_for(self.window_id_for_test())
    }

    /// True if the renderer reports the device as lost.
    pub fn renderer_is_device_lost_for(&self, window: WindowId) -> bool {
        self.windows
            .borrow()
            .get(&window)
            .and_then(|w| w.renderer.borrow().as_ref().map(|r| r.is_device_lost()))
            .unwrap_or(false)
    }

    /// Convenience: device-lost status for the first registered window.
    pub fn renderer_is_device_lost(&self) -> bool {
        self.renderer_is_device_lost_for(self.window_id_for_test())
    }

    /// Snapshot of the current recovery state for a window.
    pub fn current_recovery_state(&self) -> RecoveryState {
        let window = self.window_id_for_test();
        self.windows
            .borrow()
            .get(&window)
            .map(|w| w.recovery_state.borrow().clone())
            .unwrap_or(RecoveryState::NotLost)
    }

    /// Trigger a synthetic device-lost. Returns `false` if no renderer initialized.
    #[cfg(all(target_os = "windows", feature = "test-hooks"))]
    pub fn force_renderer_device_lost(&self, window: WindowId) -> bool {
        self.windows
            .borrow()
            .get(&window)
            .and_then(|w| {
                w.renderer.borrow().as_ref().map(|r| {
                    r.force_device_lost();
                    true
                })
            })
            .unwrap_or(false)
    }

    /// Synthetic `LuidMigration`-origin device loss.
    #[cfg(all(target_os = "windows", feature = "test-hooks"))]
    pub fn force_renderer_device_lost_luid_migration_for_test(&self) -> bool {
        let window = self.window_id_for_test();
        self.windows
            .borrow()
            .get(&window)
            .and_then(|w| {
                w.renderer.borrow().as_ref().map(|r| {
                    r.force_device_lost_luid_migration();
                    true
                })
            })
            .unwrap_or(false)
    }

    /// Consume the renderer's `wgpu_callback_fired` flag.
    #[cfg(feature = "test-hooks")]
    pub fn consume_wgpu_callback_fired_for_test(&self) -> bool {
        let window = self.window_id_for_test();
        self.windows
            .borrow()
            .get(&window)
            .and_then(|w| {
                w.renderer
                    .borrow()
                    .as_ref()
                    .map(|r| r.consume_wgpu_callback_fired())
            })
            .unwrap_or(false)
    }

    /// Fire the device-lost callback logic for testing.
    #[cfg(feature = "test-hooks")]
    pub fn fire_renderer_device_lost_callback(
        &self,
        reason: wgpu::DeviceLostReason,
        message: String,
    ) -> bool {
        let window = self.window_id_for_test();
        self.windows
            .borrow()
            .get(&window)
            .and_then(|w| {
                w.renderer
                    .borrow()
                    .as_ref()
                    .map(|r| r.fire_device_lost_callback_for_test(reason, message))
            })
            .unwrap_or(false)
    }

    // -------------------------------------------------------------------------
    // Misc accessors
    // -------------------------------------------------------------------------

    /// Read the sync-path quit signal.
    pub fn pending_quit(&self) -> bool {
        self.pending_quit.get()
    }

    /// Request a redraw on the first registered window.
    pub fn request_redraw(&self) {
        let window = self.window_id_for_test();
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.window.request_redraw();
        }
    }

    // -------------------------------------------------------------------------
    // Handler installation helpers
    // -------------------------------------------------------------------------

    /// Install App-level keyboard handlers. Test-only wrapper.
    pub fn install_key_handlers_for_test(
        &self,
        on_key_down: Vec<KeyHandler>,
        on_key_up: Vec<KeyHandler>,
        on_text_input: Vec<TextInputHandler>,
    ) {
        self.install_key_handlers(on_key_down, on_key_up, on_text_input);
    }

    // -------------------------------------------------------------------------
    // Per-window element registration helpers
    // -------------------------------------------------------------------------

    /// Register a per-element keyboard handler bundle. Test-only.
    pub fn install_element_key_handlers_for_test(
        &self,
        window: WindowId,
        id: ElementId,
        handlers: crate::event::KeyHandlers,
    ) {
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.key_handler_map.borrow_mut().insert(id, handlers);
        }
    }

    /// Register a focusable entry directly into the registry. Test-only.
    pub fn register_focusable_for_test(
        &self,
        window: WindowId,
        entry: crate::focus::FocusableEntry,
    ) {
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.focus_registry.borrow_mut().register(entry);
        }
    }

    /// Set focus to the given element. Test-only.
    pub fn set_focus_for_test(&self, window: WindowId, id: ElementId) {
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.focus_registry.borrow_mut().set_focus(id);
        }
    }

    /// Insert a parent_map edge. Test-only.
    pub fn set_parent_for_test(&self, window: WindowId, child: ElementId, parent: ElementId) {
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.parent_map.borrow_mut().insert(child, parent);
        }
    }

    /// Read the current focused element. Test-only.
    pub fn focused_for_test(&self, window: WindowId) -> Option<ElementId> {
        self.windows
            .borrow()
            .get(&window)
            .and_then(|w| w.focus_registry.borrow().focused())
    }

    /// Register a per-element mouse handler bundle. Test-only.
    pub fn install_element_mouse_handlers_for_test(
        &self,
        window: WindowId,
        id: ElementId,
        handlers: crate::event::MouseHandlers,
    ) {
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.mouse_handler_map.borrow_mut().insert(id, handlers);
        }
    }

    // -------------------------------------------------------------------------
    // Synthetic dispatch helpers
    // -------------------------------------------------------------------------

    /// Dispatch a synthetic `KeyDown`. Test-only.
    pub fn dispatch_key_down_for_test(
        &self,
        window: WindowId,
        code: KeyCode,
        key: Key,
        modifiers: Modifiers,
        is_repeat: bool,
    ) -> AppSignal {
        self.dispatch_key_down(window, code, key, modifiers, is_repeat)
    }

    /// Dispatch a synthetic `KeyUp`. Test-only.
    pub fn dispatch_key_up_for_test(
        &self,
        window: WindowId,
        code: KeyCode,
        key: Key,
        modifiers: Modifiers,
    ) -> AppSignal {
        self.dispatch_key_up(window, code, key, modifiers)
    }

    /// Dispatch a synthetic `TextInput`. Test-only.
    pub fn dispatch_text_input_for_test(&self, window: WindowId, text: String) -> AppSignal {
        self.dispatch_text_input(window, text)
    }

    /// Dispatch a synthetic `MouseDown`. Test-only.
    pub fn dispatch_mouse_down_for_test(
        &self,
        window: WindowId,
        position: (f32, f32),
        button: crate::event::MouseButton,
        modifiers: Modifiers,
    ) -> AppSignal {
        self.dispatch_mouse_down(window, position, button, modifiers)
    }

    /// Dispatch a synthetic `MouseMoved` then flush the coalesced move. Test-only.
    pub fn dispatch_mouse_move_for_test(
        &self,
        window: WindowId,
        position: (f32, f32),
    ) -> AppSignal {
        let _ = self.dispatch_mouse_moved(window, position, Modifiers::default());
        self.flush_coalesced_move(window);
        AppSignal::RequestRedraw { window }
    }

    /// Dispatch a synthetic `MouseMoved` returning the raw `AppSignal`. Test-only.
    pub fn dispatch_mouse_moved_raw_for_test(
        &self,
        window: WindowId,
        position: (f32, f32),
    ) -> AppSignal {
        self.dispatch_mouse_moved(window, position, Modifiers::default())
    }

    /// Dispatch a synthetic `MouseUp`. Test-only.
    pub fn dispatch_mouse_up_for_test(
        &self,
        window: WindowId,
        position: (f32, f32),
        button: crate::event::MouseButton,
        modifiers: Modifiers,
    ) -> AppSignal {
        self.dispatch_mouse_up(window, position, button, modifiers)
    }

    /// Dispatch a synthetic `CaptureLost`. Test-only.
    pub fn dispatch_capture_lost_for_test(&self, window: WindowId) -> AppSignal {
        self.dispatch_capture_lost(window)
    }

    /// Run the unmount-driven capture release. Test-only.
    pub fn release_capture_if_unmounted_for_test(&self, window: WindowId) {
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            let hit = win.hit_test_list.borrow();
            AppState::release_capture_if_unmounted(win, &hit);
        }
    }

    /// Read the current capture target for a window. Test-only.
    pub fn capture_target_for_test(&self, window: WindowId) -> Option<ElementId> {
        self.windows
            .borrow()
            .get(&window)
            .and_then(|w| *w.capture_target.borrow())
    }

    /// Read the explicit-capture flag for a window. Test-only.
    pub fn explicit_capture_for_test(&self, window: WindowId) -> bool {
        self.windows
            .borrow()
            .get(&window)
            .map(|w| *w.explicit_capture.borrow())
            .unwrap_or(false)
    }

    /// Clear the hit-test list for a window. Test-only.
    pub fn clear_hit_test_list_for_test(&self, window: WindowId) {
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.hit_test_list.borrow_mut().clear();
        }
    }

    /// Push a hit region directly into the window's hit-test list. Test-only.
    pub fn push_hit_region_for_test(&self, window: WindowId, region: crate::hit_test::HitRegion) {
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.hit_test_list.borrow_mut().push(region);
        }
    }

    // -------------------------------------------------------------------------
    // IME test-hook accessors
    // -------------------------------------------------------------------------

    /// Install App-level IME handlers. Test-only.
    pub fn install_ime_handlers_for_test(
        &self,
        on_ime_preedit: Vec<ImePreeditHandler>,
        on_ime_commit: Vec<ImeCommitHandler>,
        on_ime_enabled: Vec<ImeLifecycleHandler>,
        on_ime_disabled: Vec<ImeLifecycleHandler>,
    ) {
        self.install_ime_handlers(
            on_ime_preedit,
            on_ime_commit,
            on_ime_enabled,
            on_ime_disabled,
        );
    }

    /// Register a per-element IME handler bundle. Test-only.
    pub fn install_element_ime_handlers_for_test(
        &self,
        window: WindowId,
        id: ElementId,
        handlers: crate::event::ImeHandlers,
    ) {
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.ime_handler_map.borrow_mut().insert(id, handlers);
        }
    }

    /// Register an IME-capable element. Test-only.
    pub fn register_ime_state_for_test(
        &self,
        window: WindowId,
        id: ElementId,
    ) -> std::rc::Rc<std::cell::RefCell<crate::ime::ImeState>> {
        let guard = self.windows.borrow();
        guard
            .get(&window)
            .expect("window not found")
            .ime_registry
            .borrow_mut()
            .register(id)
    }

    /// Write an `ImeState` and republish the cache. Test-only.
    pub fn set_ime_state_for_test(
        &self,
        window: WindowId,
        id: ElementId,
        new_state: crate::ime::ImeState,
    ) {
        let rc = {
            let guard = self.windows.borrow();
            guard
                .get(&window)
                .expect("window not found")
                .ime_registry
                .borrow_mut()
                .register(id)
        };
        *rc.borrow_mut() = new_state;
        self.republish_ime_cache(window);
    }

    /// Dispatch a synthetic `ImeEnabled`. Test-only.
    pub fn dispatch_ime_enabled_for_test(&self, window: WindowId) -> AppSignal {
        self.dispatch_ime_enabled(window)
    }

    /// Dispatch a synthetic `ImeDisabled`. Test-only.
    pub fn dispatch_ime_disabled_for_test(&self, window: WindowId) -> AppSignal {
        self.dispatch_ime_disabled(window)
    }

    /// Dispatch a synthetic `ImePreedit`. Test-only.
    pub fn dispatch_ime_preedit_for_test(
        &self,
        window: WindowId,
        text: String,
        cursor_byte_offset: usize,
        selection: Option<std::ops::Range<usize>>,
    ) -> AppSignal {
        self.dispatch_ime_preedit(window, text, cursor_byte_offset, selection)
    }

    /// Dispatch a synthetic `ImeCommit`. Test-only.
    pub fn dispatch_ime_commit_for_test(&self, window: WindowId, text: String) -> AppSignal {
        self.dispatch_ime_commit(window, text)
    }

    /// Re-publish the IME cache snapshot. Test-only.
    pub fn republish_ime_cache_for_test(&self, window: WindowId) {
        self.republish_ime_cache(window);
    }

    /// Query the cached caret rect. Test-only.
    pub fn ime_caret_rect_query_for_test(
        &self,
        window: WindowId,
    ) -> Option<slate_platform::PhysicalRect> {
        use slate_platform::WindowImeDelegate;
        WindowImeDelegate::ime_caret_rect(self, window)
    }

    /// Query a text substring from the cached IME snapshot. Test-only.
    pub fn ime_text_query_for_test(
        &self,
        window: WindowId,
        range: std::ops::Range<usize>,
    ) -> Option<String> {
        use slate_platform::WindowImeDelegate;
        WindowImeDelegate::ime_text(self, window, range)
    }
}
