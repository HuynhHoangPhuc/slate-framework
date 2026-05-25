//! Surface lifecycle: initial renderer/text-system bring-up, sync resize,
//! and the device-lost / device-restored platform event arms.

use std::rc::Rc;

use slate_platform::{PhysicalSize, Platform, Window, WindowId};
use slate_renderer::{Renderer, RendererObserver};

use crate::app::AppContext;
use crate::text_system::TextSystem;
use crate::view::View;

use super::super::state::AppState;
use super::super::types::{AppSignal, RecoveryState};

impl<V: View> AppState<V> {
    /// Initialize renderer + text_system + view. Called from Event::Resumed.
    /// Re-entry guarded: if renderer is already Some, returns Ok without re-allocating.
    pub fn init_surfaces<P: Platform>(
        &self,
        view_factory: &mut impl FnMut(&AppContext) -> V,
        cx: &AppContext,
        platform: &P,
    ) -> Result<(), String> {
        // Re-entry guard: if already initialized (e.g. screen unlock fires Resumed again),
        // skip re-initialization. DO NOT reset recovery_state here — that would wipe
        // an active recovery counter. (Red-team RT-1.6)
        if self.renderer.borrow().is_some() {
            return Ok(());
        }

        // FIRST INIT path:
        // 1. Build renderer
        let renderer = match pollster::block_on(Renderer::new(self.window.clone())) {
            Ok(r) => r,
            Err(e) => {
                log::error!("renderer init failed: {e}");
                platform.quit();
                return Err(format!("renderer init failed: {e}"));
            }
        };

        // 2. Build text_system
        let text_system = match TextSystem::new() {
            Ok(ts) => ts,
            Err(e) => {
                log::error!("text system init failed: {e}");
                platform.quit();
                return Err(format!("text system init failed: {e}"));
            }
        };

        log::info!("renderer and text system ready");

        // 3. Register cache invalidation observers
        renderer.register_observer(
            Rc::downgrade(&self.text_system_observer) as std::rc::Weak<dyn RendererObserver>
        );
        renderer
            .register_observer(Rc::downgrade(&self.text_shaping_cache_observer)
                as std::rc::Weak<dyn RendererObserver>);
        renderer.register_observer(
            Rc::downgrade(&self.image_system_observer) as std::rc::Weak<dyn RendererObserver>
        );

        // 4. Store components + update generation signal
        let renderer_gen = renderer.current_generation();
        *self.renderer.borrow_mut() = Some(renderer);
        *self.text_system.borrow_mut() = Some(text_system);
        *self.view.borrow_mut() = Some(view_factory(cx));
        self.renderer_generation.set(renderer_gen);

        // 5. Reset state (only on first init)
        *self.recovery_state.borrow_mut() = RecoveryState::NotLost;
        self.skip_draws.set(false);
        self.rendering.set(false);
        self.pending_quit.set(false);

        // 5. Request initial redraw
        self.window.request_redraw();

        Ok(())
    }

    /// Run synchronous resize: resize the renderer.
    /// Caller is responsible for triggering redraw (sync path calls dispatch_redraw after).
    ///
    /// Idempotent: skips work when the requested size matches the last
    /// size we already configured. AppKit can fire setFrameSize: with the
    /// same PhysicalSize twice per drag tick (logical→backing rounding),
    /// and re-running configure on the wgpu surface for the same dimensions
    /// would be wasted GPU work mid-drag.
    pub(crate) fn run_resize_sync(&self, size: PhysicalSize) {
        if self.last_resize_size.get() == Some(size) {
            return;
        }
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.resize(size.as_tuple(), self.window.logical_size());
        }
        self.last_resize_size.set(Some(size));
    }

    /// Event::WindowResized arm — currently a no-op.
    /// Platform now drives WindowRedrawRequested post-resize.
    pub fn handle_window_resized(&self, physical_size: (u32, u32)) {
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.resize(physical_size, self.window.logical_size());
        }
    }

    /// Handle device-lost event from platform.
    pub(crate) fn dispatch_device_lost(&self, _window: WindowId, fatal: bool) -> AppSignal {
        if fatal {
            log::error!("GPU device lost (fatal) - recovery failed after max attempts");
            AppSignal::RequestQuit
        } else {
            log::warn!("GPU device lost - recovery will be attempted");
            AppSignal::None
        }
    }

    /// Handle device-restored event from platform.
    pub(crate) fn dispatch_device_restored(&self, window: WindowId) -> AppSignal {
        log::info!("GPU device restored - rendering resumed");
        *self.recovery_state.borrow_mut() = RecoveryState::NotLost;
        AppSignal::RequestRedraw { window }
    }
}
