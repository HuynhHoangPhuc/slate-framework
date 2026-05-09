//! Application container and frame loop.
//!
//! `App` owns all framework resources and provides `run()` to enter
//! the platform event loop with a View.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use smallvec::SmallVec;
use slate_platform::{
    DefaultPlatform, DefaultWindow, Event, Platform, Window, WindowOptions, wake_run_loop,
};
use slate_renderer::{Renderer, Scene};

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::event::{
    EventCtx, Handlers, MouseEvent, MouseHandler, PointerEvent, PointerEventKind, PointerHandler,
    ScrollEvent, ScrollHandler,
};
use crate::executor::{BackgroundExecutor, Executor, RedrawRequester};
use crate::hit_test::HitTestList;
use crate::layout::{LayoutTree, compute_layout, resolve_bounds};
use crate::reactive_state::StateRegistry;
use crate::text_system::TextSystem;
use crate::types::{AccessibilityNode, ElementId, Point, Size};
use crate::view::View;
use slate_platform::{Modifiers, MouseButton};

// ---------------------------------------------------------------------------
// Mouse event dispatch helpers (Phase 5a)
// ---------------------------------------------------------------------------

/// Convert MouseButton to bitmask bit position.
fn button_to_bit(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1 << 0,
        MouseButton::Right => 1 << 1,
        MouseButton::Middle => 1 << 2,
        MouseButton::Other(n) => 1 << (3 + n.min(4)),
    }
}

/// Walk ancestors from `start` to root, yielding each ElementId.
fn ancestors<'a>(
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
fn bubble_mouse_handler<F>(
    target: ElementId,
    event: &MouseEvent,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
    get_handler: F,
) where
    F: Fn(&Handlers) -> Option<MouseHandler>,
{
    // Collect handlers into a SmallVec to release the borrow before invoking
    let mut chain: SmallVec<[MouseHandler; 8]> = SmallVec::new();
    for id in ancestors(target, parent_map) {
        if let Some(h) = handler_map.get(&id).and_then(|handlers| get_handler(handlers)) {
            chain.push(h);
        }
    }

    // Invoke handlers in order (leaf to root)
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
fn bubble_pointer_handler<F>(
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
fn bubble_scroll_handler(
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
///
/// Computes leave/enter chains via LCA (lowest common ancestor) algorithm:
/// - Leave: fire on_pointer_leave from old leaf up to (not including) LCA, deepest-first
/// - Enter: fire on_pointer_enter from child-of-LCA down to new leaf, outermost-first
fn fire_hover_transitions(
    old_hover: Option<ElementId>,
    new_hover: Option<ElementId>,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
) {
    use std::collections::HashSet;

    // Build ancestor chains (leaf to root order)
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

    // Build set of new ancestors for O(1) lookup
    let new_set: HashSet<ElementId> = new_chain.iter().copied().collect();
    let old_set: HashSet<ElementId> = old_chain.iter().copied().collect();

    // Collect leave handlers (deepest-first = old_chain order)
    // Leave elements in old chain that are not in new chain
    let mut leave_handlers: SmallVec<[PointerHandler; 8]> = SmallVec::new();
    for &id in &old_chain {
        if !new_set.contains(&id)
            && let Some(h) = handler_map.get(&id).and_then(|h| h.on_pointer_leave.clone())
        {
            leave_handlers.push(h);
        }
    }

    // Collect enter handlers (outermost-first = reverse of new_chain order)
    // Enter elements in new chain that are not in old chain
    let mut enter_ids: SmallVec<[ElementId; 8]> = SmallVec::new();
    for &id in &new_chain {
        if !old_set.contains(&id) {
            enter_ids.push(id);
        }
    }
    enter_ids.reverse(); // outermost-first

    let mut enter_handlers: SmallVec<[PointerHandler; 8]> = SmallVec::new();
    for &id in &enter_ids {
        if let Some(h) = handler_map.get(&id).and_then(|h| h.on_pointer_enter.clone()) {
            enter_handlers.push(h);
        }
    }

    // Fire leave events (deepest-first)
    for handler in &leave_handlers {
        let event = PointerEvent {
            kind: PointerEventKind::Leave,
            position: (0.0, 0.0), // Position not meaningful for leave
            button: None,
            modifiers: Modifiers::default(),
            timestamp: Instant::now(),
        };
        let mut stopped = false;
        let mut ctx = EventCtx::new(&mut stopped);
        handler(&event, &mut ctx);
        // Note: leave events don't propagate — each is a single-element call
    }

    // Fire enter events (outermost-first)
    for handler in &enter_handlers {
        let event = PointerEvent {
            kind: PointerEventKind::Enter,
            position: (0.0, 0.0), // Position not meaningful for enter
            button: None,
            modifiers: Modifiers::default(),
            timestamp: Instant::now(),
        };
        let mut stopped = false;
        let mut ctx = EventCtx::new(&mut stopped);
        handler(&event, &mut ctx);
        // Note: enter events don't propagate — each is a single-element call
    }
}

/// Application context passed to the view factory.
///
/// Provides access to the reactive runtime and background executor for constructing
/// signals and spawning background tasks.
///
/// Note: `ForegroundExecutor` is intentionally not exposed here because it's `!Send`
/// and bound to the UI thread. UI-thread tasks should use the foreground executor
/// available in element context methods.
///
/// # Example
///
/// ```ignore
/// App::new(options).run(|cx| {
///     let count = Signal::new(cx.runtime(), 0u32);
///     cx.background_executor().spawn(async move { /* ... */ }).detach();
///     MyView { count }
/// });
/// ```
#[derive(Clone)]
pub struct AppContext {
    runtime: Arc<slate_reactive::Runtime>,
    background_executor: BackgroundExecutor,
}

impl AppContext {
    /// Get the reactive runtime for creating signals.
    pub fn runtime(&self) -> Arc<slate_reactive::Runtime> {
        self.runtime.clone()
    }

    /// Get the background executor for spawning async tasks.
    pub fn background_executor(&self) -> BackgroundExecutor {
        self.background_executor.clone()
    }
}

// Debug-mode borrow-order discipline (see ADR-001)
// Detects borrow-order violations before they ship.
#[cfg(debug_assertions)]
thread_local! {
    static BORROW_ORDER: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(debug_assertions)]
fn reset_borrow_order() {
    BORROW_ORDER.with(|c| c.set(0));
}

#[cfg(debug_assertions)]
#[allow(dead_code)] // Infrastructure for future simultaneous-borrow detection
fn check_borrow_order(slot: u8) {
    BORROW_ORDER.with(|c| {
        let last = c.get();
        debug_assert!(
            slot > last,
            "RefCell borrow-order violation: tried slot {} after slot {}; see ADR-001",
            slot,
            last
        );
        c.set(slot);
    });
}

#[cfg(not(debug_assertions))]
fn reset_borrow_order() {}

#[cfg(not(debug_assertions))]
#[allow(dead_code)] // Infrastructure for future simultaneous-borrow detection
fn check_borrow_order(_slot: u8) {}

/// Application container.
///
/// Owns all framework resources: platform, window, renderer, executor,
/// layout tree, hit-test list, accessibility tree, and text system.
///
/// # Example
///
/// ```ignore
/// App::new(WindowOptions {
///     title: "My App".into(),
///     size: (800, 600),
///     ..Default::default()
/// })
/// .run(|cx| {
///     let count = Signal::new(cx.runtime(), 0);
///     MyView::new(count)
/// });
/// ```
pub struct App {
    platform: DefaultPlatform,
    window: Arc<DefaultWindow>,
}

impl App {
    /// Create a new application with the given window options.
    ///
    /// The renderer and text system are initialized lazily on `Event::Resumed`.
    pub fn new(options: WindowOptions) -> Self {
        let platform = DefaultPlatform::new();
        let window = platform.create_window(options);

        Self { platform, window }
    }

    /// Run the application with the given view factory.
    ///
    /// `view_fn` receives an [`AppContext`] with access to the reactive runtime
    /// and background executor for constructing signals and spawning tasks.
    ///
    /// `view_fn` is `FnMut` because `Event::Resumed` can fire multiple times
    /// (e.g., after suspend on mobile platforms).
    ///
    /// This method enters the platform event loop and does not return until
    /// the application exits.
    pub fn run<V: View>(self, mut view_fn: impl FnMut(&AppContext) -> V + 'static) {
        let App { platform, window } = self;

        // Deferred initialization (set up in Event::Resumed)
        let renderer: RefCell<Option<Renderer>> = RefCell::new(None);
        let text_system: RefCell<Option<TextSystem>> = RefCell::new(None);
        let view: RefCell<Option<V>> = RefCell::new(None);

        // Per-frame state (always allocated)
        let redraw_requester = RedrawRequester::new(wake_run_loop);
        let executor = Executor::new(redraw_requester.clone());
        let layout_tree: RefCell<LayoutTree> = RefCell::new(LayoutTree::new());
        let hit_test_list: RefCell<HitTestList> = RefCell::new(HitTestList::new());
        let a11y_nodes: RefCell<Vec<AccessibilityNode>> = RefCell::new(Vec::new());
        let scene: RefCell<Scene> = RefCell::new(Scene::new());

        // Phase 5a: Mouse event dispatch state
        let handler_map: RefCell<HashMap<ElementId, Handlers>> = RefCell::new(HashMap::new());
        let parent_map: RefCell<HashMap<ElementId, ElementId>> = RefCell::new(HashMap::new());
        let hovered_element: RefCell<Option<ElementId>> = RefCell::new(None);
        let button_state: RefCell<u8> = RefCell::new(0);
        let capture_target: RefCell<Option<ElementId>> = RefCell::new(None);
        let last_mouse_pos: RefCell<Option<(f32, f32)>> = RefCell::new(None);

        // Phase 7: Move coalescing state
        let coalesced_move_pos: RefCell<Option<(f32, f32)>> = RefCell::new(None);
        let last_dispatched_move_pos: RefCell<Option<(f32, f32)>> = RefCell::new(None);

        // Phase 5: Reactive runtime with redraw wiring
        let runtime = slate_reactive::Runtime::new();
        runtime.install_redraw({
            let req = redraw_requester.clone();
            Arc::new(move || req.request())
        });
        let view_observer_id = runtime.next_observer_id();

        // Phase 4: StateRegistry for element-level reactive state (borrow slot 8)
        let state_registry: RefCell<StateRegistry> =
            RefCell::new(StateRegistry::new(runtime.clone()));

        // Phase 6: TextShapingCache for text shaping optimization (borrow slot 9)
        let text_shaping_cache: RefCell<crate::paint_cache::TextShapingCache> =
            RefCell::new(crate::paint_cache::TextShapingCache::new());

        // AppContext for view factory
        let cx = AppContext {
            runtime: runtime.clone(),
            background_executor: executor.background.clone(),
        };

        let platform_ref = &platform;
        let window_ref = window.clone();
        let executor_ref = &executor;

        platform.run(move |event| match event {
            Event::Resumed => {
                // Initialize renderer
                let r = match pollster::block_on(Renderer::new(window_ref.clone())) {
                    Ok(r) => r,
                    Err(e) => {
                        log::error!("renderer init failed: {e}");
                        platform_ref.quit();
                        return;
                    }
                };

                // Initialize text system
                let ts = match TextSystem::new() {
                    Ok(ts) => ts,
                    Err(e) => {
                        log::error!("text system init failed: {e}");
                        platform_ref.quit();
                        return;
                    }
                };

                log::info!("renderer and text system ready");

                *renderer.borrow_mut() = Some(r);
                *text_system.borrow_mut() = Some(ts);
                *view.borrow_mut() = Some(view_fn(&cx));

                // Request initial redraw
                window_ref.request_redraw();
            }

            Event::WindowResized { size, .. } => {
                if let Some(r) = renderer.borrow_mut().as_mut() {
                    r.resize(size);
                }
            }

            Event::WindowRedrawRequested { .. } => {
                // BORROW ORDER (do not reorder; see ADR-001 in plans/260508-1245-*):
                // 1. view  2. layout_tree  3. text_system  4. hit_test_list
                // 5. a11y_nodes  6. scene  7. renderer
                //
                // Each borrow lives in the smallest scope possible — guard drops
                // before next acquire. Phase 4 signals depend on this discipline
                // to avoid deadlock during read-during-render.
                //
                // Debug builds reset the borrow-order cookie here; if future changes
                // introduce simultaneous borrows, add check_borrow_order(N) calls.
                reset_borrow_order();

                // Skip if not initialized
                if renderer.borrow().is_none() {
                    return;
                }

                let (w, h) = window_ref.size();
                let scale_factor = window_ref.scale_factor();

                // Phase 5: Drain dirty bit and effects before render
                runtime.drain_dirty();
                runtime.drain_effects();

                // 1. Build element tree (inside observer scope for reactive subscriptions)
                let mut root = {
                    let mut v = view.borrow_mut();
                    let v = v.as_mut().expect("view not initialized");
                    slate_reactive::with_observer(view_observer_id, || v.render())
                };

                // 2. Layout pass
                let root_id = {
                    let mut tree = layout_tree.borrow_mut();
                    tree.clear();

                    let mut ts = text_system.borrow_mut();
                    let ts = ts.as_mut().expect("text system not initialized");

                    let mut cx = LayoutCtx::new(
                        tree.inner_mut(),
                        ts,
                        &executor_ref.foreground,
                        scale_factor,
                    );

                    compute_layout(&mut root, &mut cx, Size::new(w as f32, h as f32))
                };

                let Some(root_id) = root_id else {
                    log::warn!("layout computation failed");
                    return;
                };

                // 3. Resolve root bounds
                let root_bounds = {
                    let tree = layout_tree.borrow();
                    resolve_bounds(tree.inner(), root_id)
                };

                let Some(root_bounds) = root_bounds else {
                    log::warn!("bounds resolution failed");
                    return;
                };

                // 4. Prepaint pass (borrow slots 8-9: state_registry, text_shaping_cache)
                {
                    let tree = layout_tree.borrow();
                    let mut hit = hit_test_list.borrow_mut();
                    let mut a11y = a11y_nodes.borrow_mut();
                    let mut ts = text_system.borrow_mut();
                    let ts = ts.as_mut().expect("text system not initialized");
                    let mut sr = state_registry.borrow_mut();
                    let mut tsc = text_shaping_cache.borrow_mut();
                    let mut hm = handler_map.borrow_mut();
                    let mut pm = parent_map.borrow_mut();

                    hit.clear();
                    a11y.clear();
                    hm.clear();
                    pm.clear();

                    let mut cx = PrepaintCtx::new(
                        tree.inner(),
                        &mut hit,
                        &mut a11y,
                        ts,
                        &executor_ref.foreground,
                        scale_factor,
                        &mut sr,
                        &mut tsc,
                        &mut hm,
                        &mut pm,
                    );

                    // Initialize tree-position keying for stable ElementIds
                    cx.init_root_frame();

                    root.prepaint(root_bounds, &mut cx);

                    // Verify prepaint frames are balanced (unbalanced = bug in element prepaint)
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

                // 4a. Coalesced move flush (Phase 7)
                // Fire on_mouse_move at most once per frame with the latest position.
                if let Some(pos) = coalesced_move_pos.borrow_mut().take() {
                    let last_dispatched = *last_dispatched_move_pos.borrow();
                    if last_dispatched != Some(pos) {
                        // Route through capture_target if captured, else hit-test
                        let captured = *capture_target.borrow();
                        let target = if let Some(ct) = captured {
                            Some(ct)
                        } else {
                            hit_test_list
                                .borrow()
                                .hit_test(Point::new(pos.0, pos.1))
                                .map(|r| r.element_id)
                        };

                        if let Some(t) = target {
                            let mouse_event = MouseEvent {
                                position: pos,
                                button: None, // move events have no button
                                modifiers: Modifiers::default(),
                                timestamp: Instant::now(),
                            };
                            bubble_mouse_handler(
                                t,
                                &mouse_event,
                                &handler_map.borrow(),
                                &parent_map.borrow(),
                                |h| h.on_mouse_move.clone(),
                            );
                        }

                        *last_dispatched_move_pos.borrow_mut() = Some(pos);
                    }
                }

                // 4b. Hover diff (Phase 7)
                // Compute enter/leave transitions from frame-to-frame hover changes.
                {
                    let current_pos = *last_mouse_pos.borrow();
                    let captured = *capture_target.borrow();

                    // Determine new hover target
                    let new_hover = if captured.is_some() {
                        // While captured, only the capture target can be hovered
                        // (simplified: cursor "owns" the captured element)
                        captured
                    } else if let Some(pos) = current_pos {
                        hit_test_list
                            .borrow()
                            .hit_test(Point::new(pos.0, pos.1))
                            .map(|r| r.element_id)
                    } else {
                        None
                    };

                    let old_hover = *hovered_element.borrow();
                    if new_hover != old_hover {
                        // Fire leave/enter transitions
                        fire_hover_transitions(
                            old_hover,
                            new_hover,
                            &handler_map.borrow(),
                            &parent_map.borrow(),
                        );
                        *hovered_element.borrow_mut() = new_hover;
                    }
                }

                // 5. Paint pass (split borrow on renderer for atlas/queue)
                {
                    let tree = layout_tree.borrow();
                    let mut s = scene.borrow_mut();
                    let mut r = renderer.borrow_mut();
                    let r = r.as_mut().expect("renderer not initialized");
                    let mut ts = text_system.borrow_mut();
                    let ts = ts.as_mut().expect("text system not initialized");

                    s.clear();

                    let (atlas, queue) = r.glyph_atlas_and_queue();
                    let mut cx = PaintCtx::new(
                        tree.inner(),
                        &mut s,
                        ts,
                        atlas,
                        queue,
                        &executor_ref.foreground,
                        scale_factor,
                    );

                    root.paint(root_bounds, &mut cx);
                }

                // 6. Render
                {
                    let mut s = scene.borrow_mut();
                    let mut r = renderer.borrow_mut();
                    let r = r.as_mut().expect("renderer not initialized");

                    if let Err(e) = r.render_scene(&mut s) {
                        log::warn!("render skipped: {e:?}");
                    }
                }

                // 7. Poll async executor
                executor_ref.foreground.poll();

                // 8. Phase 4: Advance frame counter and GC stale state slots
                // Slots not accessed for 2+ consecutive frames are dropped.
                {
                    let mut sr = state_registry.borrow_mut();
                    sr.advance_frame();
                    sr.gc();
                }

                // 9. Phase 6: Advance frame counter and GC stale text shaping cache
                // Entries not accessed for 2+ consecutive frames are dropped.
                {
                    let mut tsc = text_shaping_cache.borrow_mut();
                    tsc.advance_frame();
                    tsc.gc();
                }
            }

            Event::WindowCloseRequested { .. } => {
                platform_ref.quit();
            }

            Event::Exiting => {
                log::info!("exiting");
            }

            Event::Wake => {
                // Background task completed — poll executor to process results,
                // then request redraw so any state changes render this frame.
                // Order matters: poll first (drains completed tasks), then redraw.
                executor_ref.foreground.poll();
                window_ref.request_redraw();
            }

            // -----------------------------------------------------------------
            // Mouse events (Phase 5a)
            // -----------------------------------------------------------------
            Event::MouseDown {
                position,
                button,
                modifiers,
                ..
            } => {
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
                let old_state = *button_state.borrow();
                *button_state.borrow_mut() = old_state | bit;

                // Determine dispatch target: if already captured, use capture_target;
                // else hit-test and set capture on first button down
                let captured = *capture_target.borrow();
                let target = if let Some(ct) = captured {
                    // Already captured — dispatch to capture target's chain
                    Some(ct)
                } else {
                    // First button down — hit-test and set capture
                    let hit = hit_test_list
                        .borrow()
                        .hit_test(Point::new(position.0, position.1));
                    if let Some(result) = hit {
                        *capture_target.borrow_mut() = Some(result.element_id);
                        Some(result.element_id)
                    } else {
                        None
                    }
                };

                if let Some(t) = target {
                    // Bubble on_mouse_down handlers
                    bubble_mouse_handler(
                        t,
                        &mouse_event,
                        &handler_map.borrow(),
                        &parent_map.borrow(),
                        |h| h.on_mouse_down.clone(),
                    );

                    // Bubble on_pointer_event handlers (independent propagation)
                    bubble_pointer_handler(
                        t,
                        &pointer_event,
                        &handler_map.borrow(),
                        &parent_map.borrow(),
                        |h| h.on_pointer_event.clone(),
                    );
                }

                *last_mouse_pos.borrow_mut() = Some(position);
                window_ref.request_redraw();
            }

            Event::MouseUp {
                position,
                button,
                modifiers,
                ..
            } => {
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
                let old_state = *button_state.borrow();
                let new_state = old_state & !bit;
                *button_state.borrow_mut() = new_state;

                // Determine dispatch target: use capture_target if captured, else hit-test
                let captured = *capture_target.borrow();
                let up_hit = hit_test_list
                    .borrow()
                    .hit_test(Point::new(position.0, position.1))
                    .map(|r| r.element_id);
                let target = captured.or(up_hit);

                if let Some(t) = target {
                    // Bubble on_mouse_up handlers
                    bubble_mouse_handler(
                        t,
                        &mouse_event,
                        &handler_map.borrow(),
                        &parent_map.borrow(),
                        |h| h.on_mouse_up.clone(),
                    );

                    // Bubble on_pointer_event handlers (independent propagation)
                    bubble_pointer_handler(
                        t,
                        &pointer_event,
                        &handler_map.borrow(),
                        &parent_map.borrow(),
                        |h| h.on_pointer_event.clone(),
                    );

                    // Click synthesis (native rule): fire on_click if left button AND
                    // up lands on same target as capture (hit-test at up pos == captured)
                    if button == MouseButton::Left && up_hit == captured && captured.is_some() {
                        bubble_mouse_handler(
                            t,
                            &mouse_event,
                            &handler_map.borrow(),
                            &parent_map.borrow(),
                            |h| h.on_click.clone(),
                        );
                    }
                }

                // Release capture when all buttons are up
                if new_state == 0 {
                    *capture_target.borrow_mut() = None;
                }

                *last_mouse_pos.borrow_mut() = Some(position);
                window_ref.request_redraw();
            }

            Event::MouseMoved {
                position,
                modifiers,
                ..
            } => {
                let pointer_event = PointerEvent {
                    kind: PointerEventKind::Move,
                    position,
                    button: None,
                    modifiers,
                    timestamp: Instant::now(),
                };

                // Route through capture_target if captured, else hit-test
                let captured = *capture_target.borrow();
                let target = if let Some(ct) = captured {
                    Some(ct)
                } else {
                    hit_test_list
                        .borrow()
                        .hit_test(Point::new(position.0, position.1))
                        .map(|r| r.element_id)
                };

                if let Some(t) = target {
                    // Bubble on_pointer_event handlers (uncoalesced raw stream)
                    bubble_pointer_handler(
                        t,
                        &pointer_event,
                        &handler_map.borrow(),
                        &parent_map.borrow(),
                        |h| h.on_pointer_event.clone(),
                    );
                }

                // Stash position for coalesced move flush (Phase 7)
                *coalesced_move_pos.borrow_mut() = Some(position);
                *last_mouse_pos.borrow_mut() = Some(position);
            }

            Event::MouseScrolled {
                position,
                delta_x,
                delta_y,
                precise,
                modifiers,
                ..
            } => {
                let scroll_event = ScrollEvent {
                    position,
                    delta_x,
                    delta_y,
                    precise,
                    modifiers,
                    timestamp: Instant::now(),
                };

                // Hit test to find target element
                let hit = hit_test_list.borrow().hit_test(Point::new(position.0, position.1));
                if let Some(result) = hit {
                    let target = result.element_id;

                    // Bubble on_mouse_scrolled handlers
                    bubble_scroll_handler(
                        target,
                        &scroll_event,
                        &handler_map.borrow(),
                        &parent_map.borrow(),
                    );
                }

                window_ref.request_redraw();
            }

            Event::MouseExited { .. } => {
                // Fire leave events on entire hovered chain (cursor left window)
                let old_hover = *hovered_element.borrow();
                if old_hover.is_some() {
                    fire_hover_transitions(
                        old_hover,
                        None, // new_hover = None (cursor left)
                        &handler_map.borrow(),
                        &parent_map.borrow(),
                    );
                    *hovered_element.borrow_mut() = None;
                }
                *last_mouse_pos.borrow_mut() = None;
                *coalesced_move_pos.borrow_mut() = None;
            }

            Event::CaptureLost { .. } => {
                // Clear capture state — handlers see up events stop arriving
                *capture_target.borrow_mut() = None;
                *button_state.borrow_mut() = 0;
            }

            _ => {}
        });
    }
}
