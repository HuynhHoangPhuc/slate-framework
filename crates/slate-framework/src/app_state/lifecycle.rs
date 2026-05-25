//! `AppState` construction and high-level lifecycle handlers.
//!
//! - `new`: deferred-init constructor; renderer/text_system/view are filled in
//!   during `Event::Resumed` by the view factory.
//! - `handle_wake`: drain the foreground executor on background-task wake.
//! - `handle_window_destroyed`: platform close cleanup; idempotent.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use slate_platform::DefaultWindow;
use slate_reactive::Signal;
use slate_renderer::Scene;

use crate::executor::{Executor, RedrawRequester};
use crate::focus::FocusRegistry;
use crate::hit_test::HitTestList;
use crate::image_cache::{ImageCache, ImageSystemObserver};
use crate::ime::{CachedImeQuery, ImeRegistry};
use crate::layout::LayoutTree;
use crate::paint_cache::{TextShapingCache, TextShapingCacheObserver};
use crate::reactive_state::StateRegistry;
use crate::text_system::TextSystemObserver;
use crate::view::View;

use super::state::AppState;
use super::types::{AppSignal, RecoveryState};

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

        // Create Rc-wrapped caches for observer weak references
        let text_system = Rc::new(RefCell::new(None));
        let text_shaping_cache = Rc::new(RefCell::new(TextShapingCache::new()));

        // Create observers with weak references to the caches
        let text_system_observer = Rc::new(TextSystemObserver::new(Rc::downgrade(&text_system)));
        let text_shaping_cache_observer = Rc::new(TextShapingCacheObserver::new(Rc::downgrade(
            &text_shaping_cache,
        )));

        // Image cache + observer
        let image_cache = Rc::new(RefCell::new(ImageCache::new()));
        let image_system_observer = Rc::new(ImageSystemObserver::new(Rc::downgrade(&image_cache)));

        Self {
            renderer: RefCell::new(None),
            text_system,
            view: RefCell::new(None),

            layout_tree: RefCell::new(LayoutTree::new()),
            hit_test_list: RefCell::new(HitTestList::new()),
            a11y_nodes: RefCell::new(Vec::new()),
            scene: RefCell::new(Scene::new()),

            handler_map: RefCell::new(HashMap::new()),
            mouse_handler_map: RefCell::new(HashMap::new()),
            parent_map: RefCell::new(HashMap::new()),
            hovered_element: RefCell::new(None),
            button_state: RefCell::new(0),
            capture_target: RefCell::new(None),
            explicit_capture: RefCell::new(false),
            last_mouse_pos: RefCell::new(None),

            coalesced_move_pos: RefCell::new(None),
            last_dispatched_move_pos: RefCell::new(None),

            runtime: runtime.clone(),
            view_observer_id,

            state_registry: RefCell::new(state_registry),
            text_shaping_cache,

            text_system_observer,
            text_shaping_cache_observer,
            image_cache,
            image_system_observer,

            executor,
            redraw_requester,
            window,

            recovery_state: RefCell::new(RecoveryState::NotLost),
            skip_draws: Cell::new(false),
            last_successful_recovery_at: Cell::new(None),
            last_wgpu_callback_loss_at: Cell::new(None),
            last_adapter_check_at: Cell::new(None),
            renderer_generation: Signal::new(runtime, 0u64),
            rendering: Cell::new(false),
            pending_quit: Cell::new(false),
            sync_resize: Cell::new(false),
            last_resize_size: Cell::new(None),
            on_key_down: RefCell::new(Vec::new()),
            on_key_up: RefCell::new(Vec::new()),
            on_text_input: RefCell::new(Vec::new()),
            key_handler_map: RefCell::new(HashMap::new()),
            focus_registry: Rc::new(RefCell::new(FocusRegistry::new())),
            focus_bounds: RefCell::new(HashMap::new()),

            ime_registry: RefCell::new(ImeRegistry::new()),
            ime_handler_map: RefCell::new(HashMap::new()),
            ime_registered_ids: RefCell::new(HashSet::new()),
            cached_ime_query: RefCell::new(CachedImeQuery::default()),
            pending_ime_ops: RefCell::new(Vec::new()),
            on_ime_preedit: RefCell::new(Vec::new()),
            on_ime_commit: RefCell::new(Vec::new()),
            on_ime_enabled: RefCell::new(Vec::new()),
            on_ime_disabled: RefCell::new(Vec::new()),
        }
    }

    /// Handle background task completion (Event::Wake).
    pub fn handle_wake(&self) -> AppSignal {
        self.executor.foreground.poll();
        AppSignal::RequestRedraw
    }

    /// Handle window close by platform (Event::WindowDestroyed).
    /// Cleans up view and logs. Idempotent.
    pub fn handle_window_destroyed(&self) -> AppSignal {
        log::debug!("WindowDestroyed received in AppState");
        *self.view.borrow_mut() = None;
        AppSignal::RequestQuit
    }
}
