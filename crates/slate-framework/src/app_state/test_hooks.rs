//! Test-only accessors on `AppState`, gated on the `test-hooks` feature.
//!
//! These wrappers expose `pub(crate)` setters and synthetic event dispatchers
//! so integration tests can drive the runtime without going through the
//! platform layer. None of these are reachable in a release build of a
//! downstream crate unless the `test-hooks` feature is explicitly enabled.

#![cfg(any(test, feature = "test-hooks"))]

use slate_platform::{Key, KeyCode, Modifiers, Window};

use crate::event::{
    ImeCommitHandler, ImeLifecycleHandler, ImePreeditHandler, KeyHandler, TextInputHandler,
};
use crate::types::ElementId;
use crate::view::View;

use super::state::AppState;
use super::types::{AppSignal, RecoveryState};

impl<V: View> AppState<V> {
    /// Current renderer generation. `None` if renderer not yet initialized.
    pub fn renderer_generation(&self) -> Option<u64> {
        self.renderer
            .borrow()
            .as_ref()
            .map(|r| r.current_generation())
    }

    /// True if the renderer reports the device as lost.
    pub fn renderer_is_device_lost(&self) -> bool {
        self.renderer
            .borrow()
            .as_ref()
            .map(|r| r.is_device_lost())
            .unwrap_or(false)
    }

    /// Snapshot of the current recovery state.
    pub fn current_recovery_state(&self) -> RecoveryState {
        self.recovery_state.borrow().clone()
    }

    /// Trigger a synthetic device-lost via `ID3D12Device5::RemoveDevice()`.
    /// Returns `false` if no renderer is initialized.
    ///
    /// Only available with the `test-hooks` feature enabled on both slate-framework
    /// and slate-renderer (the feature propagates via Cargo.toml `test-hooks = ["slate-renderer/test-hooks"]`).
    #[cfg(all(target_os = "windows", feature = "test-hooks"))]
    pub fn force_renderer_device_lost(&self) -> bool {
        if let Some(r) = self.renderer.borrow().as_ref() {
            r.force_device_lost();
            true
        } else {
            false
        }
    }

    /// Synthetic `LuidMigration`-origin device loss: sets the renderer's
    /// `device_lost` atomic without touching `wgpu_callback_fired`, so the
    /// classifier reports `DeviceLossReason::LuidMigration`.
    ///
    /// Only available with the `test-hooks` feature.
    #[cfg(all(target_os = "windows", feature = "test-hooks"))]
    pub fn force_renderer_device_lost_luid_migration_for_test(&self) -> bool {
        if let Some(r) = self.renderer.borrow().as_ref() {
            r.force_device_lost_luid_migration();
            true
        } else {
            false
        }
    }

    /// Consume the renderer's `wgpu_callback_fired` flag, returning prior value.
    /// Mirrors `Renderer::consume_wgpu_callback_fired` for the cycle-leak
    /// regression test.
    #[cfg(feature = "test-hooks")]
    pub fn consume_wgpu_callback_fired_for_test(&self) -> bool {
        self.renderer
            .borrow()
            .as_ref()
            .map(|r| r.consume_wgpu_callback_fired())
            .unwrap_or(false)
    }

    /// Fire the device-lost callback logic for testing.
    ///
    /// Mirrors `Renderer::fire_device_lost_callback_for_test`: filters `Destroyed`
    /// (returns false), otherwise captures telemetry and sets atomic flag.
    /// Use to validate the Destroyed filter and state-machine engagement paths
    /// without triggering a real wgpu device-lost condition.
    ///
    /// Only available with the `test-hooks` feature.
    #[cfg(feature = "test-hooks")]
    pub fn fire_renderer_device_lost_callback(
        &self,
        reason: wgpu::DeviceLostReason,
        message: String,
    ) -> bool {
        if let Some(r) = self.renderer.borrow().as_ref() {
            r.fire_device_lost_callback_for_test(reason, message)
        } else {
            false
        }
    }

    /// Read the sync-path quit signal.
    pub fn pending_quit(&self) -> bool {
        self.pending_quit.get()
    }

    /// Request a redraw on the associated window.
    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Install App-level keyboard handlers. Test-only wrapper exposing the
    /// `pub(crate)` setter so integration tests can wire handlers post-construction.
    pub fn install_key_handlers_for_test(
        &self,
        on_key_down: Vec<KeyHandler>,
        on_key_up: Vec<KeyHandler>,
        on_text_input: Vec<TextInputHandler>,
    ) {
        self.install_key_handlers(on_key_down, on_key_up, on_text_input);
    }

    /// Dispatch a synthetic `KeyDown` to installed handlers. Test-only.
    pub fn dispatch_key_down_for_test(
        &self,
        code: KeyCode,
        key: Key,
        modifiers: Modifiers,
        is_repeat: bool,
    ) -> AppSignal {
        self.dispatch_key_down(code, key, modifiers, is_repeat)
    }

    /// Dispatch a synthetic `KeyUp` to installed handlers. Test-only.
    pub fn dispatch_key_up_for_test(
        &self,
        code: KeyCode,
        key: Key,
        modifiers: Modifiers,
    ) -> AppSignal {
        self.dispatch_key_up(code, key, modifiers)
    }

    /// Dispatch a synthetic `TextInput` to installed handlers. Test-only.
    pub fn dispatch_text_input_for_test(&self, text: String) -> AppSignal {
        self.dispatch_text_input(text)
    }

    /// Dispatch a synthetic `MouseDown` at `position` to installed handlers.
    /// Test-only. Use together with direct writes to `hit_test_list` to
    /// stage hit-test geometry for the synthetic click.
    pub fn dispatch_mouse_down_for_test(
        &self,
        position: (f32, f32),
        button: crate::event::MouseButton,
        modifiers: Modifiers,
    ) -> AppSignal {
        self.dispatch_mouse_down(position, button, modifiers)
    }

    /// Dispatch a synthetic `MouseMoved` then flush the coalesced move so the
    /// `on_mouse_move` handlers fire on the same call. Test-only.
    pub fn dispatch_mouse_move_for_test(&self, position: (f32, f32)) -> AppSignal {
        let _ = self.dispatch_mouse_moved(position, Modifiers::default());
        self.flush_coalesced_move();
        AppSignal::RequestRedraw
    }

    /// Dispatch a synthetic `MouseMoved` and return its **raw** `AppSignal`
    /// without the masking flush. Test-only — lets tests assert whether a bare
    /// move requests a redraw (drag-in-progress) versus stays silent (idle
    /// hover), which `dispatch_mouse_move_for_test` hides by hardcoding
    /// `RequestRedraw`.
    pub fn dispatch_mouse_moved_raw_for_test(&self, position: (f32, f32)) -> AppSignal {
        self.dispatch_mouse_moved(position, Modifiers::default())
    }

    /// Dispatch a synthetic `MouseUp` at `position`. Test-only.
    pub fn dispatch_mouse_up_for_test(
        &self,
        position: (f32, f32),
        button: crate::event::MouseButton,
        modifiers: Modifiers,
    ) -> AppSignal {
        self.dispatch_mouse_up(position, button, modifiers)
    }

    /// Dispatch a synthetic `CaptureLost`. Test-only.
    pub fn dispatch_capture_lost_for_test(&self) -> AppSignal {
        self.dispatch_capture_lost()
    }

    /// Register a per-element keyboard handler bundle. Test-only — production
    /// code wires this through `PrepaintCtx::register_key_handlers` from Div.
    pub fn install_element_key_handlers_for_test(
        &self,
        id: ElementId,
        handlers: crate::event::KeyHandlers,
    ) {
        self.key_handler_map.borrow_mut().insert(id, handlers);
    }

    /// Register a focusable entry directly into the registry. Test-only.
    pub fn register_focusable_for_test(&self, entry: crate::focus::FocusableEntry) {
        self.focus_registry.borrow_mut().register(entry);
    }

    /// Set focus to the given element. Test-only — bypasses the focusable check
    /// (mirrors `AppContext::set_focus` minus the redraw request).
    pub fn set_focus_for_test(&self, id: ElementId) {
        self.focus_registry.borrow_mut().set_focus(id);
    }

    /// Insert a parent_map edge for building the focused-chain. Test-only —
    /// production code wires this through `PrepaintCtx::register_handlers`.
    pub fn set_parent_for_test(&self, child: ElementId, parent: ElementId) {
        self.parent_map.borrow_mut().insert(child, parent);
    }

    /// Read the current focused element. Test-only.
    pub fn focused_for_test(&self) -> Option<ElementId> {
        self.focus_registry.borrow().focused()
    }

    // -------------------------------------------------------------------------
    // IME test-hook accessors
    // -------------------------------------------------------------------------

    /// Install App-level IME handlers. Test-only wrapper exposing the
    /// `pub(crate)` setter so integration tests can wire handlers
    /// post-construction.
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
        id: ElementId,
        handlers: crate::event::ImeHandlers,
    ) {
        self.ime_handler_map.borrow_mut().insert(id, handlers);
    }

    /// Register a per-element mouse handler bundle. Test-only — production
    /// code wires this through `PrepaintCtx::register_mouse_handlers` from
    /// TextField.
    pub fn install_element_mouse_handlers_for_test(
        &self,
        id: ElementId,
        handlers: crate::event::MouseHandlers,
    ) {
        self.mouse_handler_map.borrow_mut().insert(id, handlers);
    }

    /// Register an IME-capable element in the registry (get-or-insert).
    /// Returns the shared `Rc<RefCell<ImeState>>`. Test-only.
    pub fn register_ime_state_for_test(
        &self,
        id: ElementId,
    ) -> std::rc::Rc<std::cell::RefCell<crate::ime::ImeState>> {
        self.ime_registry.borrow_mut().register(id)
    }

    /// Write an `ImeState` directly into the registry for `id`, then
    /// republish the cache. Test-only — bypasses the platform dispatch path.
    pub fn set_ime_state_for_test(&self, id: ElementId, new_state: crate::ime::ImeState) {
        let rc = self.ime_registry.borrow_mut().register(id);
        *rc.borrow_mut() = new_state;
        self.republish_ime_cache();
    }

    /// Dispatch a synthetic `ImeEnabled` event. Test-only.
    pub fn dispatch_ime_enabled_for_test(&self, window: slate_platform::WindowId) -> AppSignal {
        self.dispatch_ime_enabled(window)
    }

    /// Dispatch a synthetic `ImeDisabled` event. Test-only.
    pub fn dispatch_ime_disabled_for_test(&self, window: slate_platform::WindowId) -> AppSignal {
        self.dispatch_ime_disabled(window)
    }

    /// Dispatch a synthetic `ImePreedit` event. Test-only.
    pub fn dispatch_ime_preedit_for_test(
        &self,
        window: slate_platform::WindowId,
        text: String,
        cursor_byte_offset: usize,
        selection: Option<std::ops::Range<usize>>,
    ) -> AppSignal {
        self.dispatch_ime_preedit(window, text, cursor_byte_offset, selection)
    }

    /// Dispatch a synthetic `ImeCommit` event. Test-only.
    pub fn dispatch_ime_commit_for_test(
        &self,
        window: slate_platform::WindowId,
        text: String,
    ) -> AppSignal {
        self.dispatch_ime_commit(window, text)
    }

    /// Return the `WindowId` of the window backing this `AppState`. Test-only.
    pub fn window_id_for_test(&self) -> slate_platform::WindowId {
        self.window.id()
    }

    /// Run the unmount-driven capture release against the current hit list.
    /// Test-only — exercises the same path the prepaint pass uses.
    pub fn release_capture_if_unmounted_for_test(&self) {
        let hit = self.hit_test_list.borrow();
        self.release_capture_if_unmounted(&hit);
    }

    /// Re-publish the IME cache snapshot. Test-only.
    pub fn republish_ime_cache_for_test(&self) {
        self.republish_ime_cache();
    }

    /// Query the cached caret rect via `WindowImeDelegate`. Test-only.
    pub fn ime_caret_rect_query_for_test(&self) -> Option<slate_platform::PhysicalRect> {
        use slate_platform::WindowImeDelegate;
        WindowImeDelegate::ime_caret_rect(self, self.window_id_for_test())
    }

    /// Query a text substring from the cached IME snapshot. Test-only.
    pub fn ime_text_query_for_test(&self, range: std::ops::Range<usize>) -> Option<String> {
        use slate_platform::WindowImeDelegate;
        WindowImeDelegate::ime_text(self, self.window_id_for_test(), range)
    }
}
