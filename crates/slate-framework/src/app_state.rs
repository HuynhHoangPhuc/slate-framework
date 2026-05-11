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

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use smallvec::SmallVec;
use slate_platform::{DefaultWindow, Modifiers, MouseButton, PhysicalSize, Window, WindowId};
use slate_reactive::ObserverId;
use slate_renderer::{Renderer, Scene};

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

/// Shared application state accessible by both event handler and resize callback.
///
/// Generic over `V: View` to hold the user's root view type.
/// Each field keeps its own `RefCell<T>` wrapper to preserve fine-grained borrow scope.
pub struct AppState<V: View> {
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
    /// Will be used in sync resize callback path for cross-thread redraw signaling (Phase 2)
    #[allow(dead_code)]
    pub redraw_requester: RedrawRequester,

    // Window reference for size queries
    pub window: Arc<DefaultWindow>,
}

impl<V: View> AppState<V> {
    /// Create a new AppState with uninitialized renderer/text_system/view.
    ///
    /// These are set during `Event::Resumed` via the view factory.
    pub fn new(
        window: Arc<DefaultWindow>,
        executor: Executor,
        redraw_requester: RedrawRequester,
        runtime: Arc<slate_reactive::Runtime>,
    ) -> Self {
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
        }
    }
}

/// Run the redraw pipeline (layout → prepaint → paint → render).
///
/// Shared by both the async `Event::WindowRedrawRequested` path and the
/// sync resize callback path.
pub fn run_redraw<V: View>(state: &AppState<V>) {
    // Skip if not initialized
    if state.renderer.borrow().is_none() {
        return;
    }

    let (lw, lh) = state.window.logical_size();
    let scale_factor = state.window.scale_factor();

    // Drain reactive effects
    state.runtime.drain_dirty();
    state.runtime.drain_effects();

    // 1. Build element tree
    let mut root = {
        let mut v = state.view.borrow_mut();
        let v = v.as_mut().expect("view not initialized");
        slate_reactive::with_observer(state.view_observer_id, || v.render())
    };

    // 2. Layout pass
    let root_id = {
        let mut tree = state.layout_tree.borrow_mut();
        tree.clear();

        let mut ts = state.text_system.borrow_mut();
        let ts = ts.as_mut().expect("text system not initialized");

        let mut cx = LayoutCtx::new(
            tree.inner_mut(),
            ts,
            &state.executor.foreground,
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
        let tree = state.layout_tree.borrow();
        resolve_bounds(tree.inner(), root_id)
    };

    let Some(root_bounds) = root_bounds else {
        log::warn!("bounds resolution failed");
        return;
    };

    // 4. Prepaint pass
    // NOTE: Intentionally holds multiple RefCell borrows simultaneously here.
    // This diverges from ADR-001's sequential borrow guideline but is safe because:
    // 1. Single-threaded execution - no concurrent access
    // 2. No re-entrancy during prepaint - no callbacks that could borrow these fields
    // 3. All borrows released at block end before any other operations
    {
        let tree = state.layout_tree.borrow();
        let mut hit = state.hit_test_list.borrow_mut();
        let mut a11y = state.a11y_nodes.borrow_mut();
        let mut ts = state.text_system.borrow_mut();
        let ts = ts.as_mut().expect("text system not initialized");
        let mut sr = state.state_registry.borrow_mut();
        let mut tsc = state.text_shaping_cache.borrow_mut();
        let mut hm = state.handler_map.borrow_mut();
        let mut pm = state.parent_map.borrow_mut();

        hit.clear();
        a11y.clear();
        hm.clear();
        pm.clear();

        let mut cx = PrepaintCtx::new(
            tree.inner(),
            &mut hit,
            &mut a11y,
            ts,
            &state.executor.foreground,
            scale_factor,
            &mut sr,
            &mut tsc,
            &mut hm,
            &mut pm,
        );

        cx.init_root_frame();
        root.prepaint(root_bounds, &mut cx);
    }

    // 4a. Coalesced move flush
    flush_coalesced_move(state);

    // 4b. Hover diff
    update_hover_state(state);

    // 5. Paint pass
    {
        let tree = state.layout_tree.borrow();
        let mut s = state.scene.borrow_mut();
        let mut r = state.renderer.borrow_mut();
        let r = r.as_mut().expect("renderer not initialized");
        let mut ts = state.text_system.borrow_mut();
        let ts = ts.as_mut().expect("text system not initialized");

        s.clear();

        let (atlas, queue) = r.glyph_atlas_and_queue();
        let mut cx = PaintCtx::new(
            tree.inner(),
            &mut s,
            ts,
            atlas,
            queue,
            &state.executor.foreground,
            scale_factor,
        );

        root.paint(root_bounds, &mut cx);
    }

    // 6. Render
    {
        let mut s = state.scene.borrow_mut();
        let mut r = state.renderer.borrow_mut();
        let r = r.as_mut().expect("renderer not initialized");

        if let Err(e) = r.render_scene(&mut s) {
            log::warn!("render skipped: {e:?}");
        }
    }

    // 7. Poll async executor
    state.executor.foreground.poll();

    // 8. GC stale state slots
    {
        let mut sr = state.state_registry.borrow_mut();
        sr.advance_frame();
        sr.gc();
    }

    // 9. GC text shaping cache
    {
        let mut tsc = state.text_shaping_cache.borrow_mut();
        tsc.advance_frame();
        tsc.gc();
    }
}

/// Run synchronous resize: resize the renderer then redraw.
///
/// Called from the platform's sync resize callback (setFrameSize: on macOS,
/// WM_SIZE/WM_NCCALCSIZE on Windows).
pub fn run_resize_sync<V: View>(state: &AppState<V>, _window_id: WindowId, size: PhysicalSize) {
    // Resize renderer
    if let Some(r) = state.renderer.borrow_mut().as_mut() {
        r.resize(size.as_tuple());
    }

    // Run full redraw pipeline
    run_redraw(state);
}

// ---------------------------------------------------------------------------
// Mouse event helpers (moved from app.rs)
// ---------------------------------------------------------------------------

fn flush_coalesced_move<V: View>(state: &AppState<V>) {
    if let Some(pos) = state.coalesced_move_pos.borrow_mut().take() {
        let last_dispatched = *state.last_dispatched_move_pos.borrow();
        if last_dispatched != Some(pos) {
            let captured = *state.capture_target.borrow();
            let target = if let Some(ct) = captured {
                Some(ct)
            } else {
                state
                    .hit_test_list
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
                    &state.handler_map.borrow(),
                    &state.parent_map.borrow(),
                    |h| h.on_mouse_move.clone(),
                );
            }

            *state.last_dispatched_move_pos.borrow_mut() = Some(pos);
        }
    }
}

fn update_hover_state<V: View>(state: &AppState<V>) {
    let current_pos = *state.last_mouse_pos.borrow();
    let captured = *state.capture_target.borrow();

    let new_hover = if captured.is_some() {
        captured
    } else if let Some(pos) = current_pos {
        state
            .hit_test_list
            .borrow()
            .hit_test(Point::new(pos.0, pos.1))
            .map(|r| r.element_id)
    } else {
        None
    };

    let old_hover = *state.hovered_element.borrow();
    if new_hover != old_hover {
        fire_hover_transitions(
            old_hover,
            new_hover,
            &state.handler_map.borrow(),
            &state.parent_map.borrow(),
        );
        *state.hovered_element.borrow_mut() = new_hover;
    }
}

/// Walk ancestors from `start` to root, yielding each ElementId.
pub fn ancestors<'a>(
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
pub fn bubble_mouse_handler<F>(
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
pub fn bubble_pointer_handler<F>(
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
pub fn bubble_scroll_handler(
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
pub fn fire_hover_transitions(
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
pub fn button_to_bit(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1 << 0,
        MouseButton::Right => 1 << 1,
        MouseButton::Middle => 1 << 2,
        MouseButton::Other(n) => 1 << (3 + n.min(4)),
    }
}
