//! Surface lifecycle: initial renderer/text-system bring-up, sync resize,
//! and the device-lost / device-restored platform event arms.

use std::rc::Rc;

use slate_platform::{PhysicalSize, Platform, Window, WindowId};
use slate_renderer::{Renderer, RendererObserver};

use crate::app::AppContext;
use crate::erased_view::ErasedView;
use crate::text_system::TextSystem;

use super::super::state::AppState;
use super::super::types::{AppSignal, RecoveryState};
use super::super::window_state::WindowState;

impl AppState {
    /// Initialize renderer + text_system + view for a specific window.
    ///
    /// Called from `Event::Resumed`. Re-entry guarded: if renderer is already
    /// `Some`, returns `Ok` without re-allocating (e.g. screen-unlock).
    pub fn init_surfaces<P: Platform>(
        &self,
        window_id: WindowId,
        view_factory: &mut impl FnMut(&AppContext) -> Box<dyn ErasedView>,
        cx: &AppContext,
        platform: &P,
    ) -> Result<(), String> {
        // Re-entry guard — do NOT reset recovery_state here.
        {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id)
                && win.renderer.borrow().is_some()
            {
                return Ok(());
            }
        }

        // Build renderer.
        let platform_window = {
            let guard = self.windows.borrow();
            guard.get(&window_id).map(|w| w.window.clone())
        };
        let Some(platform_window) = platform_window else {
            log::error!("init_surfaces: unknown window {:?}", window_id);
            return Err(format!("init_surfaces: unknown window {:?}", window_id));
        };

        let renderer = match pollster::block_on(Renderer::new(platform_window.clone())) {
            Ok(r) => r,
            Err(e) => {
                log::error!("renderer init failed: {e}");
                platform.quit();
                return Err(format!("renderer init failed: {e}"));
            }
        };

        // Build text_system.
        let text_system = match TextSystem::new() {
            Ok(ts) => ts,
            Err(e) => {
                log::error!("text system init failed: {e}");
                platform.quit();
                return Err(format!("text system init failed: {e}"));
            }
        };

        log::info!("renderer and text system ready for window {:?}", window_id);

        // Register cache invalidation observer on the new renderer. Only the
        // process-wide `text_shaping_cache` is shared and needs observer-driven
        // invalidation; `glyph_cache` and `image_cache` are per-window in
        // `WindowState` and are cleared inline during recovery.
        renderer
            .register_observer(Rc::downgrade(&self.text_shaping_cache_observer)
                as std::rc::Weak<dyn RendererObserver>);

        let renderer_gen = renderer.current_generation();

        // Store components into the WindowState.
        {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id) {
                *win.renderer.borrow_mut() = Some(renderer);
                *self.text_system.borrow_mut() = Some(text_system);
                *win.view.borrow_mut() = Some(view_factory(cx));
                win.renderer_generation.set(renderer_gen);
                *win.recovery_state.borrow_mut() = RecoveryState::NotLost;
                win.skip_draws.set(false);
                win.rendering.set(false);
                win.window.request_redraw();
            }
        }

        Ok(())
    }

    /// Run synchronous resize for a specific window.
    ///
    /// Idempotent: skips when the requested size matches the last configured
    /// size. Caller is responsible for triggering a redraw afterwards.
    pub(crate) fn run_resize_sync_for(win: &WindowState, size: PhysicalSize) {
        if win.last_resize_size.get() == Some(size) {
            return;
        }
        if let Some(r) = win.renderer.borrow_mut().as_mut() {
            r.resize(size.as_tuple(), win.window.logical_size());
        }
        win.last_resize_size.set(Some(size));
    }

    /// `Event::WindowResized` arm — resize the renderer and set last size.
    pub fn handle_window_resized(&self, window_id: WindowId, physical_size: (u32, u32)) {
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window_id)
            && let Some(r) = win.renderer.borrow_mut().as_mut()
        {
            r.resize(physical_size, win.window.logical_size());
        }
    }

    /// `Event::DeviceLost` arm.
    pub(crate) fn dispatch_device_lost(&self, _window: WindowId, fatal: bool) -> AppSignal {
        if fatal {
            log::error!("GPU device lost (fatal) — recovery failed after max attempts");
            AppSignal::RequestQuit
        } else {
            log::warn!("GPU device lost — recovery will be attempted");
            AppSignal::None
        }
    }

    /// `Event::DeviceRestored` arm.
    pub(crate) fn dispatch_device_restored(&self, window: WindowId) -> AppSignal {
        log::info!(
            "GPU device restored — rendering resumed for window {:?}",
            window
        );
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            *win.recovery_state.borrow_mut() = RecoveryState::NotLost;
        }
        AppSignal::RequestRedraw { window }
    }
}
