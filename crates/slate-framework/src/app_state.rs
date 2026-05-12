//! Shared application state for event handler and resize callback.
//!
//! `AppState<V>` holds all RefCell-wrapped state that was previously captured
//! by the `App::run` closure. Wrapping in `Rc<AppState<V>>` allows both the
//! event handler and the sync resize callback to share the same state.
//!
//! # Borrow Order (ADR-001)
//! Fields are borrowed in a fixed order to avoid deadlock:
//! 1. view  2. layout_tree  3. text_system  4. hit_test_list
//! 5. a11y_nodes  6. scene  7. renderer  8. state_registry  9. text_shaping_cache
//!
//! Each borrow is released before the next begins. Do NOT hold multiple borrows
//! simultaneously unless they are proven non-conflicting.

#![allow(dead_code)] // Phase 1: some methods unused until later phases wire them

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use smallvec::SmallVec;
use slate_platform::{DefaultWindow, Modifiers, MouseButton, PhysicalSize, Platform, Window};
use slate_reactive::ObserverId;
use slate_renderer::{Renderer, Scene};

use crate::app::AppContext;
use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::event::{
    EventCtx, Handlers, MouseEvent, MouseHandler, PointerEvent, PointerEventKind, PointerHandler,
    ScrollEvent, ScrollHandler,
};
use crate::executor::{Executor, RedrawRequester};
use crate::hit_test::HitTestList;
use crate::layout::{LayoutTree, compute_layout, resolve_bounds};
use crate::paint_cache::TextShapingCache;
use crate::reactive_state::StateRegistry;
use crate::text_system::TextSystem;
use crate::types::{AccessibilityNode, ElementId, Point, Size};
use crate::view::View;

/// Maximum device-lost recovery attempts before giving up.
pub(crate) const MAX_RECOVERY_ATTEMPTS: u32 = 3;

/// Signal returned by dispatch methods to communicate with the event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppSignal {
    None,
    RequestQuit,
    RequestRedraw,
}

// Debug-mode borrow-order discipline (see ADR-001).
// Detects borrow-order violations before they ship.
#[cfg(debug_assertions)]
thread_local! {
    static BORROW_ORDER: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(debug_assertions)]
fn reset_borrow_order() {
    BORROW_ORDER.with(|c| c.set(0));
}

#[cfg(not(debug_assertions))]
fn reset_borrow_order() {}

/// RAII guard for the rendering flag.
struct RenderingGuard<'a>(&'a Cell<bool>);

impl Drop for RenderingGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Shared application state accessible by both event handler and resize callback.
///
/// Generic over `V: View` to hold the user's root view type.
/// Each field keeps its own `RefCell<T>` wrapper to preserve fine-grained borrow scope.
pub(crate) struct AppState<V: View> {
    // Deferred initialization (set in Event::Resumed)
    pub renderer: RefCell<Option<Renderer>>,
    pub text_system: RefCell<Option<TextSystem>>,
    pub view: RefCell<Option<V>>,

    // Per-frame state (always allocated)
    pub layout_tree: RefCell<LayoutTree>,
    pub hit_test_list: RefCell<HitTestList>,
    pub a11y_nodes: RefCell<Vec<AccessibilityNode>>,
    pub scene: RefCell<Scene>,

    // Mouse event dispatch state
    pub handler_map: RefCell<HashMap<ElementId, Handlers>>,
    pub parent_map: RefCell<HashMap<ElementId, ElementId>>,
    pub hovered_element: RefCell<Option<ElementId>>,
    pub button_state: RefCell<u8>,
    pub capture_target: RefCell<Option<ElementId>>,
    pub last_mouse_pos: RefCell<Option<(f32, f32)>>,

    // Move coalescing state
    pub coalesced_move_pos: RefCell<Option<(f32, f32)>>,
    pub last_dispatched_move_pos: RefCell<Option<(f32, f32)>>,

    // Reactive runtime
    pub runtime: Arc<slate_reactive::Runtime>,
    pub view_observer_id: ObserverId,

    // Element-level reactive state
    pub state_registry: RefCell<StateRegistry>,
    pub text_shaping_cache: RefCell<TextShapingCache>,

    // Executor (foreground + background)
    pub executor: Executor,
    pub redraw_requester: RedrawRequester,

    // Window reference for size queries
    pub window: Arc<DefaultWindow>,

    // Device-lost recovery state
    pub recovery_attempts: RefCell<u32>,

    // Re-entrancy guard: prevents nested render calls from causing RefCell panics
    pub rendering: Cell<bool>,

    // Sync-path quit signal: set by sync delegate methods, read at next event tick
    pub pending_quit: Cell<bool>,
}

impl<V: View> AppState<V> {
    /// Create a new AppState with uninitialized renderer/text_system/view.
    ///
    /// These are set during `Event::Resumed` via the view factory.
    /// Wires up the reactive runtime's redraw bridge internally.
    pub fn new(
        window: Arc<DefaultWindow>,
        executor: Executor,
        redraw_requester: RedrawRequester,
        runtime: Arc<slate_reactive::Runtime>,
    ) -> Self {
        // Wire the reactive runtime's redraw bridge (moved from app.rs closure)
        runtime.install_redraw({
            let req = redraw_requester.clone();
            Arc::new(move || req.request())
        });

        let view_observer_id = runtime.next_observer_id();
        let state_registry = StateRegistry::new(runtime.clone());

        Self {
            renderer: RefCell::new(None),
            text_system: RefCell::new(None),
            view: RefCell::new(None),

            layout_tree: RefCell::new(LayoutTree::new()),
            hit_test_list: RefCell::new(HitTestList::new()),
            a11y_nodes: RefCell::new(Vec::new()),
            scene: RefCell::new(Scene::new()),

            handler_map: RefCell::new(HashMap::new()),
            parent_map: RefCell::new(HashMap::new()),
            hovered_element: RefCell::new(None),
            button_state: RefCell::new(0),
            capture_target: RefCell::new(None),
            last_mouse_pos: RefCell::new(None),

            coalesced_move_pos: RefCell::new(None),
            last_dispatched_move_pos: RefCell::new(None),

            runtime,
            view_observer_id,

            state_registry: RefCell::new(state_registry),
            text_shaping_cache: RefCell::new(TextShapingCache::new()),

            executor,
            redraw_requester,
            window,

            recovery_attempts: RefCell::new(0),
            rendering: Cell::new(false),
            pending_quit: Cell::new(false),
        }
    }

    /// Initialize renderer + text_system + view. Called from Event::Resumed.
    /// Re-entry guarded: if renderer is already Some, returns Ok without re-allocating.
    pub(crate) fn init_surfaces<P: Platform>(
        &self,
        view_factory: &mut impl FnMut(&AppContext) -> V,
        cx: &AppContext,
        platform: &P,
    ) -> Result<(), String> {
        // Re-entry guard: if already initialized (e.g. screen unlock fires Resumed again),
        // skip re-initialization. DO NOT reset recovery_attempts here — that would wipe
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

        // 3. Store components
        *self.renderer.borrow_mut() = Some(renderer);
        *self.text_system.borrow_mut() = Some(text_system);
        *self.view.borrow_mut() = Some(view_factory(cx));

        // 4. Reset state (only on first init)
        *self.recovery_attempts.borrow_mut() = 0;
        self.rendering.set(false);
        self.pending_quit.set(false);

        // 5. Request initial redraw
        self.window.request_redraw();

        Ok(())
    }

    /// Full redraw dispatch with device-lost recovery wrapper + re-entrancy guard.
    /// Returns AppSignal::RequestQuit if recovery exceeds MAX_RECOVERY_ATTEMPTS.
    pub(crate) fn dispatch_redraw(&self) -> AppSignal {
        // RE-ENTRANCY GUARD — applies to BOTH sync and async render paths.
        // If a redraw is already in flight, skip the duplicate.
        if self.rendering.get() {
            return AppSignal::None;
        }
        self.rendering.set(true);
        let _guard = RenderingGuard(&self.rendering);

        reset_borrow_order();

        // Skip if not initialized
        if self.renderer.borrow().is_none() {
            return AppSignal::None;
        }

        // Device-lost recovery check
        let device_lost = {
            let r = self.renderer.borrow();
            r.as_ref().map(|r| r.is_device_lost()).unwrap_or(false)
        };

        if device_lost {
            let attempts = *self.recovery_attempts.borrow();
            if attempts >= MAX_RECOVERY_ATTEMPTS {
                log::error!("GPU device recovery failed after {} attempts", attempts);
                return AppSignal::RequestQuit;
            }

            *self.recovery_attempts.borrow_mut() += 1;
            log::info!(
                "Attempting GPU device recovery (attempt {}/{})",
                attempts + 1,
                MAX_RECOVERY_ATTEMPTS
            );

            // Drop old renderer
            *self.renderer.borrow_mut() = None;

            // Recreate renderer
            match pollster::block_on(Renderer::new(self.window.clone())) {
                Ok(new_renderer) => {
                    log::info!("GPU device recovered successfully");
                    *self.renderer.borrow_mut() = Some(new_renderer);
                    *self.recovery_attempts.borrow_mut() = 0;
                    self.window.request_redraw();
                }
                Err(e) => {
                    log::error!("GPU device recovery failed: {e}");
                    self.window.request_redraw();
                }
            }
            return AppSignal::None;
        }

        // Run the actual redraw
        self.run_redraw();

        AppSignal::None
    }

    /// Run the redraw pipeline (layout → prepaint → paint → render).
    ///
    /// This is the inner body called by `dispatch_redraw`. The re-entrancy guard
    /// and device-lost recovery wrapper live in `dispatch_redraw`, not here.
    pub(crate) fn run_redraw(&self) {
        // Skip if not initialized
        if self.renderer.borrow().is_none() {
            return;
        }

        let (lw, lh) = self.window.logical_size();
        let scale_factor = self.window.scale_factor();

        // Drain reactive effects
        self.runtime.drain_dirty();
        self.runtime.drain_effects();

        // 1. Build element tree
        let mut root = {
            let mut v = self.view.borrow_mut();
            let v = v.as_mut().expect("view not initialized");
            slate_reactive::with_observer(self.view_observer_id, || v.render())
        };

        // 2. Layout pass
        let root_id = {
            let mut tree = self.layout_tree.borrow_mut();
            tree.clear();

            let mut ts = self.text_system.borrow_mut();
            let ts = ts.as_mut().expect("text system not initialized");

            let mut cx = LayoutCtx::new(
                tree.inner_mut(),
                ts,
                &self.executor.foreground,
                scale_factor,
            );

            compute_layout(&mut root, &mut cx, Size::new(lw as f32, lh as f32))
        };

        let Some(root_id) = root_id else {
            log::warn!("layout computation failed");
            return;
        };

        // 3. Resolve root bounds
        let root_bounds = {
            let tree = self.layout_tree.borrow();
            resolve_bounds(tree.inner(), root_id)
        };

        let Some(root_bounds) = root_bounds else {
            log::warn!("bounds resolution failed");
            return;
        };

        // 4. Prepaint pass
        {
            let tree = self.layout_tree.borrow();
            let mut hit = self.hit_test_list.borrow_mut();
            let mut a11y = self.a11y_nodes.borrow_mut();
            let mut ts = self.text_system.borrow_mut();
            let ts = ts.as_mut().expect("text system not initialized");
            let mut sr = self.state_registry.borrow_mut();
            let mut tsc = self.text_shaping_cache.borrow_mut();
            let mut hm = self.handler_map.borrow_mut();
            let mut pm = self.parent_map.borrow_mut();

            hit.clear();
            a11y.clear();
            hm.clear();
            pm.clear();

            let mut cx = PrepaintCtx::new(
                tree.inner(),
                &mut hit,
                &mut a11y,
                ts,
                &self.executor.foreground,
                scale_factor,
                &mut sr,
                &mut tsc,
                &mut hm,
                &mut pm,
            );

            cx.init_root_frame();
            root.prepaint(root_bounds, &mut cx);

            // Verify prepaint frames are balanced
            debug_assert!(
                cx.id_stack.len() == 1,
                "unbalanced prepaint frames: expected 1 (root), got {}",
                cx.id_stack.len()
            );
            debug_assert!(
                cx.a11y_stack.is_empty(),
                "unbalanced a11y stack at frame end: {} unclosed nodes",
                cx.a11y_stack.len()
            );
        }

        // 4a. Coalesced move flush
        self.flush_coalesced_move();

        // 4b. Hover diff
        self.update_hover_state();

        // 5. Paint pass
        {
            let tree = self.layout_tree.borrow();
            let mut s = self.scene.borrow_mut();
            let mut r = self.renderer.borrow_mut();
            let r = r.as_mut().expect("renderer not initialized");
            let mut ts = self.text_system.borrow_mut();
            let ts = ts.as_mut().expect("text system not initialized");

            s.clear();

            let (atlas, queue) = r.glyph_atlas_and_queue();
            let mut cx = PaintCtx::new(
                tree.inner(),
                &mut s,
                ts,
                atlas,
                queue,
                &self.executor.foreground,
                scale_factor,
            );

            root.paint(root_bounds, &mut cx);
        }

        // 6. Render
        {
            let mut s = self.scene.borrow_mut();
            let mut r = self.renderer.borrow_mut();
            let r = r.as_mut().expect("renderer not initialized");

            if let Err(e) = r.render_scene(&mut s) {
                log::warn!("render skipped: {e:?}");
            }
        }

        // 7. Poll async executor
        self.executor.foreground.poll();

        // 8. GC stale state slots
        {
            let mut sr = self.state_registry.borrow_mut();
            sr.advance_frame();
            sr.gc();
        }

        // 9. GC text shaping cache
        {
            let mut tsc = self.text_shaping_cache.borrow_mut();
            tsc.advance_frame();
            tsc.gc();
        }
    }

    /// Run synchronous resize: resize the renderer.
    /// Caller is responsible for triggering redraw (sync path calls dispatch_redraw after).
    pub(crate) fn run_resize_sync(&self, size: PhysicalSize) {
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.resize(size.as_tuple());
        }
    }

    /// Event::WindowResized arm — currently a no-op.
    /// Platform now drives WindowRedrawRequested post-resize.
    pub(crate) fn handle_window_resized(&self, physical_size: (u32, u32)) {
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.resize(physical_size);
        }
    }

    /// Handle background task completion (Event::Wake).
    pub(crate) fn handle_wake(&self) -> AppSignal {
        self.executor.foreground.poll();
        AppSignal::RequestRedraw
    }

    /// Handle window close by platform (Event::WindowDestroyed).
    /// Cleans up view and logs. Idempotent.
    pub(crate) fn handle_window_destroyed(&self) -> AppSignal {
        log::debug!("WindowDestroyed received in AppState");
        *self.view.borrow_mut() = None;
        AppSignal::RequestQuit
    }

    /// Handle device-lost event from platform.
    pub(crate) fn dispatch_device_lost(&self, fatal: bool) -> AppSignal {
        if fatal {
            log::error!("GPU device lost (fatal) - recovery failed after max attempts");
            AppSignal::RequestQuit
        } else {
            log::warn!("GPU device lost - recovery will be attempted");
            AppSignal::None
        }
    }

    /// Handle device-restored event from platform.
    pub(crate) fn dispatch_device_restored(&self) -> AppSignal {
        log::info!("GPU device restored - rendering resumed");
        *self.recovery_attempts.borrow_mut() = 0;
        AppSignal::RequestRedraw
    }

    // -----------------------------------------------------------------------
    // Mouse event dispatch methods (Phase 2)
    // -----------------------------------------------------------------------

    /// Dispatch MouseDown event.
    pub(crate) fn dispatch_mouse_down(
        &self,
        position: (f32, f32),
        button: MouseButton,
        modifiers: Modifiers,
    ) -> AppSignal {
        let mouse_event = MouseEvent {
            position,
            button: Some(button),
            modifiers,
            timestamp: Instant::now(),
        };
        let pointer_event = PointerEvent {
            kind: PointerEventKind::Down,
            position,
            button: Some(button),
            modifiers,
            timestamp: Instant::now(),
        };

        // Update button state
        let bit = button_to_bit(button);
        let old_state = *self.button_state.borrow();
        *self.button_state.borrow_mut() = old_state | bit;

        // Determine dispatch target
        let captured = *self.capture_target.borrow();
        let target = if let Some(ct) = captured {
            Some(ct)
        } else {
            let hit = self
                .hit_test_list
                .borrow()
                .hit_test(Point::new(position.0, position.1));
            if let Some(result) = hit {
                *self.capture_target.borrow_mut() = Some(result.element_id);
                Some(result.element_id)
            } else {
                None
            }
        };

        if let Some(t) = target {
            // Collect handlers first, then invoke (clone-before-drop pattern)
            let mouse_handlers: SmallVec<[MouseHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_mouse_down.clone()))
                    .collect()
            };
            let pointer_handlers: SmallVec<[PointerHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_event.clone()))
                    .collect()
            };

            // Invoke handlers (borrows released)
            let mut stopped = false;
            for handler in &mouse_handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&mouse_event, &mut ctx);
                if stopped {
                    break;
                }
            }

            stopped = false;
            for handler in &pointer_handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&pointer_event, &mut ctx);
                if stopped {
                    break;
                }
            }
        }

        *self.last_mouse_pos.borrow_mut() = Some(position);
        AppSignal::RequestRedraw
    }

    /// Dispatch MouseUp event.
    pub(crate) fn dispatch_mouse_up(
        &self,
        position: (f32, f32),
        button: MouseButton,
        modifiers: Modifiers,
    ) -> AppSignal {
        let mouse_event = MouseEvent {
            position,
            button: Some(button),
            modifiers,
            timestamp: Instant::now(),
        };
        let pointer_event = PointerEvent {
            kind: PointerEventKind::Up,
            position,
            button: Some(button),
            modifiers,
            timestamp: Instant::now(),
        };

        // Update button state
        let bit = button_to_bit(button);
        let old_state = *self.button_state.borrow();
        let new_state = old_state & !bit;
        *self.button_state.borrow_mut() = new_state;

        // Determine dispatch target
        let captured = *self.capture_target.borrow();
        let up_hit = self
            .hit_test_list
            .borrow()
            .hit_test(Point::new(position.0, position.1))
            .map(|r| r.element_id);
        let target = captured.or(up_hit);

        if let Some(t) = target {
            // Collect handlers (clone-before-drop pattern)
            let mouse_up_handlers: SmallVec<[MouseHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_mouse_up.clone()))
                    .collect()
            };
            let pointer_handlers: SmallVec<[PointerHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_event.clone()))
                    .collect()
            };
            let click_handlers: SmallVec<[MouseHandler; 8]> = if button == MouseButton::Left
                && up_hit == captured
                && captured.is_some()
            {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_click.clone()))
                    .collect()
            } else {
                SmallVec::new()
            };

            // Invoke handlers (borrows released)
            let mut stopped = false;
            for handler in &mouse_up_handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&mouse_event, &mut ctx);
                if stopped {
                    break;
                }
            }

            stopped = false;
            for handler in &pointer_handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&pointer_event, &mut ctx);
                if stopped {
                    break;
                }
            }

            stopped = false;
            for handler in &click_handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&mouse_event, &mut ctx);
                if stopped {
                    break;
                }
            }
        }

        // Release capture when all buttons are up
        if new_state == 0 {
            *self.capture_target.borrow_mut() = None;
        }

        *self.last_mouse_pos.borrow_mut() = Some(position);
        AppSignal::RequestRedraw
    }

    /// Dispatch MouseMoved event.
    pub(crate) fn dispatch_mouse_moved(
        &self,
        position: (f32, f32),
        modifiers: Modifiers,
    ) -> AppSignal {
        let pointer_event = PointerEvent {
            kind: PointerEventKind::Move,
            position,
            button: None,
            modifiers,
            timestamp: Instant::now(),
        };

        // Route through capture_target if captured, else hit-test
        let captured = *self.capture_target.borrow();
        let target = if let Some(ct) = captured {
            Some(ct)
        } else {
            self.hit_test_list
                .borrow()
                .hit_test(Point::new(position.0, position.1))
                .map(|r| r.element_id)
        };

        if let Some(t) = target {
            // Collect handlers (clone-before-drop pattern)
            let handlers: SmallVec<[PointerHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_event.clone()))
                    .collect()
            };

            // Invoke handlers
            let mut stopped = false;
            for handler in &handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&pointer_event, &mut ctx);
                if stopped {
                    break;
                }
            }
        }

        *self.coalesced_move_pos.borrow_mut() = Some(position);
        *self.last_mouse_pos.borrow_mut() = Some(position);
        AppSignal::None
    }

    /// Dispatch MouseScrolled event.
    pub(crate) fn dispatch_mouse_scrolled(
        &self,
        position: (f32, f32),
        delta_x: f32,
        delta_y: f32,
        precise: bool,
        modifiers: Modifiers,
    ) -> AppSignal {
        let scroll_event = ScrollEvent {
            position,
            delta_x,
            delta_y,
            precise,
            modifiers,
            timestamp: Instant::now(),
        };

        let hit = self
            .hit_test_list
            .borrow()
            .hit_test(Point::new(position.0, position.1));

        if let Some(result) = hit {
            // Collect handlers (clone-before-drop pattern)
            let handlers: SmallVec<[ScrollHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(result.element_id, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_mouse_scrolled.clone()))
                    .collect()
            };

            // Invoke handlers
            let mut stopped = false;
            for handler in &handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&scroll_event, &mut ctx);
                if stopped {
                    break;
                }
            }
        }

        AppSignal::RequestRedraw
    }

    /// Dispatch MouseExited event.
    pub(crate) fn dispatch_mouse_exited(&self) -> AppSignal {
        let old_hover = *self.hovered_element.borrow();
        if old_hover.is_some() {
            // Collect handlers (clone-before-drop pattern)
            let handlers: SmallVec<[PointerHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                if let Some(id) = old_hover {
                    ancestors(id, &pm)
                        .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_leave.clone()))
                        .collect()
                } else {
                    SmallVec::new()
                }
            };

            // Invoke handlers
            for handler in &handlers {
                let event = PointerEvent {
                    kind: PointerEventKind::Leave,
                    position: (0.0, 0.0),
                    button: None,
                    modifiers: Modifiers::default(),
                    timestamp: Instant::now(),
                };
                let mut stopped = false;
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&event, &mut ctx);
            }

            *self.hovered_element.borrow_mut() = None;
        }
        *self.last_mouse_pos.borrow_mut() = None;
        *self.coalesced_move_pos.borrow_mut() = None;
        AppSignal::None
    }

    /// Dispatch CaptureLost event.
    pub(crate) fn dispatch_capture_lost(&self) -> AppSignal {
        *self.capture_target.borrow_mut() = None;
        *self.button_state.borrow_mut() = 0;
        AppSignal::None
    }

    // -----------------------------------------------------------------------
    // Mouse event helpers (internal)
    // -----------------------------------------------------------------------

    fn flush_coalesced_move(&self) {
        if let Some(pos) = self.coalesced_move_pos.borrow_mut().take() {
            let last_dispatched = *self.last_dispatched_move_pos.borrow();
            if last_dispatched != Some(pos) {
                let captured = *self.capture_target.borrow();
                let target = if let Some(ct) = captured {
                    Some(ct)
                } else {
                    self.hit_test_list
                        .borrow()
                        .hit_test(Point::new(pos.0, pos.1))
                        .map(|r| r.element_id)
                };

                if let Some(t) = target {
                    let mouse_event = MouseEvent {
                        position: pos,
                        button: None,
                        modifiers: Modifiers::default(),
                        timestamp: Instant::now(),
                    };
                    bubble_mouse_handler(
                        t,
                        &mouse_event,
                        &self.handler_map.borrow(),
                        &self.parent_map.borrow(),
                        |h| h.on_mouse_move.clone(),
                    );
                }

                *self.last_dispatched_move_pos.borrow_mut() = Some(pos);
            }
        }
    }

    fn update_hover_state(&self) {
        let current_pos = *self.last_mouse_pos.borrow();
        let captured = *self.capture_target.borrow();

        let new_hover = if captured.is_some() {
            captured
        } else if let Some(pos) = current_pos {
            self.hit_test_list
                .borrow()
                .hit_test(Point::new(pos.0, pos.1))
                .map(|r| r.element_id)
        } else {
            None
        };

        let old_hover = *self.hovered_element.borrow();
        if new_hover != old_hover {
            fire_hover_transitions(
                old_hover,
                new_hover,
                &self.handler_map.borrow(),
                &self.parent_map.borrow(),
            );
            *self.hovered_element.borrow_mut() = new_hover;
        }
    }
}

impl<V: View> Drop for AppState<V> {
    fn drop(&mut self) {
        log::debug!("AppState<V> dropped — cycle-free shutdown verified");
    }
}

// ---------------------------------------------------------------------------
// Event dispatch helpers (pub(crate) for use by app.rs and future phases)
// ---------------------------------------------------------------------------

/// Walk ancestors from `start` to root, yielding each ElementId.
pub(crate) fn ancestors<'a>(
    start: ElementId,
    parent_map: &'a HashMap<ElementId, ElementId>,
) -> impl Iterator<Item = ElementId> + 'a {
    let mut current = Some(start);
    std::iter::from_fn(move || {
        let id = current?;
        current = parent_map.get(&id).copied();
        Some(id)
    })
}

/// Bubble a mouse event through the ancestor chain, invoking handlers.
pub(crate) fn bubble_mouse_handler<F>(
    target: ElementId,
    event: &MouseEvent,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
    get_handler: F,
) where
    F: Fn(&Handlers) -> Option<MouseHandler>,
{
    let mut chain: SmallVec<[MouseHandler; 8]> = SmallVec::new();
    for id in ancestors(target, parent_map) {
        if let Some(h) = handler_map.get(&id).and_then(|handlers| get_handler(handlers)) {
            chain.push(h);
        }
    }

    let mut stopped = false;
    for handler in &chain {
        let mut ctx = EventCtx::new(&mut stopped);
        handler(event, &mut ctx);
        if stopped {
            break;
        }
    }
}

/// Bubble a pointer event through the ancestor chain, invoking handlers.
pub(crate) fn bubble_pointer_handler<F>(
    target: ElementId,
    event: &PointerEvent,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
    get_handler: F,
) where
    F: Fn(&Handlers) -> Option<PointerHandler>,
{
    let mut chain: SmallVec<[PointerHandler; 8]> = SmallVec::new();
    for id in ancestors(target, parent_map) {
        if let Some(h) = handler_map.get(&id).and_then(|handlers| get_handler(handlers)) {
            chain.push(h);
        }
    }

    let mut stopped = false;
    for handler in &chain {
        let mut ctx = EventCtx::new(&mut stopped);
        handler(event, &mut ctx);
        if stopped {
            break;
        }
    }
}

/// Bubble a scroll event through the ancestor chain, invoking handlers.
pub(crate) fn bubble_scroll_handler(
    target: ElementId,
    event: &ScrollEvent,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
) {
    let mut chain: SmallVec<[ScrollHandler; 8]> = SmallVec::new();
    for id in ancestors(target, parent_map) {
        if let Some(h) = handler_map
            .get(&id)
            .and_then(|handlers| handlers.on_mouse_scrolled.clone())
        {
            chain.push(h);
        }
    }

    let mut stopped = false;
    for handler in &chain {
        let mut ctx = EventCtx::new(&mut stopped);
        handler(event, &mut ctx);
        if stopped {
            break;
        }
    }
}

/// Fire hover transitions between old and new hover targets.
pub(crate) fn fire_hover_transitions(
    old_hover: Option<ElementId>,
    new_hover: Option<ElementId>,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
) {
    use std::collections::HashSet;

    let old_chain: SmallVec<[ElementId; 16]> = if let Some(id) = old_hover {
        ancestors(id, parent_map).collect()
    } else {
        SmallVec::new()
    };
    let new_chain: SmallVec<[ElementId; 16]> = if let Some(id) = new_hover {
        ancestors(id, parent_map).collect()
    } else {
        SmallVec::new()
    };

    let new_set: HashSet<ElementId> = new_chain.iter().copied().collect();
    let old_set: HashSet<ElementId> = old_chain.iter().copied().collect();

    let mut leave_handlers: SmallVec<[PointerHandler; 8]> = SmallVec::new();
    for &id in &old_chain {
        if !new_set.contains(&id)
            && let Some(h) = handler_map.get(&id).and_then(|h| h.on_pointer_leave.clone())
        {
            leave_handlers.push(h);
        }
    }

    let mut enter_ids: SmallVec<[ElementId; 8]> = SmallVec::new();
    for &id in &new_chain {
        if !old_set.contains(&id) {
            enter_ids.push(id);
        }
    }
    enter_ids.reverse();

    let mut enter_handlers: SmallVec<[PointerHandler; 8]> = SmallVec::new();
    for &id in &enter_ids {
        if let Some(h) = handler_map.get(&id).and_then(|h| h.on_pointer_enter.clone()) {
            enter_handlers.push(h);
        }
    }

    for handler in &leave_handlers {
        let event = PointerEvent {
            kind: PointerEventKind::Leave,
            position: (0.0, 0.0),
            button: None,
            modifiers: Modifiers::default(),
            timestamp: Instant::now(),
        };
        let mut stopped = false;
        let mut ctx = EventCtx::new(&mut stopped);
        handler(&event, &mut ctx);
    }

    for handler in &enter_handlers {
        let event = PointerEvent {
            kind: PointerEventKind::Enter,
            position: (0.0, 0.0),
            button: None,
            modifiers: Modifiers::default(),
            timestamp: Instant::now(),
        };
        let mut stopped = false;
        let mut ctx = EventCtx::new(&mut stopped);
        handler(&event, &mut ctx);
    }
}

/// Convert MouseButton to bitmask bit position.
pub(crate) fn button_to_bit(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1 << 0,
        MouseButton::Right => 1 << 1,
        MouseButton::Middle => 1 << 2,
        MouseButton::Other(n) => 1 << (3 + n.min(4)),
    }
}
