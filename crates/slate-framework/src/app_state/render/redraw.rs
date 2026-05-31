//! Inner redraw pipeline: layout → prepaint → paint → render.
//!
//! `dispatch_redraw` (in `dispatch.rs`) is the re-entrancy-guarded entry that
//! drives the device-lost recovery state machine and delegates here when the
//! state machine clears for a normal paint.

use slate_platform::{Window, WindowId};

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::layout::{compute_layout, resolve_bounds};
use crate::render_cx::RenderCx;
use crate::types::Size;

use super::super::state::AppState;

impl AppState {
    /// Run the redraw pipeline (layout → prepaint → paint → render) for
    /// a single window. The re-entrancy guard and device-lost recovery wrapper
    /// live in `dispatch_redraw`, not here.
    pub(crate) fn run_redraw(&self, window_id: WindowId) {
        // All per-window borrows are taken individually below. We never hold
        // the outer `windows` RefCell borrow across any user callback.

        // Guard: skip if not initialized.
        {
            let guard = self.windows.borrow();
            if guard
                .get(&window_id)
                .map(|w| w.renderer.borrow().is_none())
                .unwrap_or(true)
            {
                return;
            }
        }

        // skip_draws gate — suppress one frame after recovery.
        {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id)
                && win.skip_draws.get()
            {
                log::debug!(target: "slate::device_lost", "skip_draws active — present suppressed");
                win.skip_draws.set(false);
                return;
            }
        }

        // Snapshot window-size parameters (cheap — no borrow conflict).
        let (lw, lh, scale_factor, win_id_stamp) = {
            let guard = self.windows.borrow();
            match guard.get(&window_id) {
                Some(win) => {
                    let (lw, lh) = win.window.logical_size();
                    let sf = win.window.scale_factor();
                    (lw, lh, sf, win.window.id())
                }
                None => return,
            }
        };

        // Drain reactive effects (process-wide, not per-window).
        self.runtime.drain_dirty();
        self.runtime.drain_effects();

        // 1. Build element tree — borrows view + view_observer_id.
        #[cfg(feature = "profiling")]
        let _vr_start = std::time::Instant::now();
        let mut root = {
            let guard = self.windows.borrow();
            let win = match guard.get(&window_id) {
                Some(w) => w,
                None => return,
            };
            let observer_id = win.view_observer_id;
            let mut v = win.view.borrow_mut();
            let v = v.as_mut().expect("view not initialized");
            let mut render_cx = RenderCx::new(win_id_stamp);
            slate_reactive::with_observer(observer_id, || v.render(&mut render_cx))
        };
        #[cfg(feature = "profiling")]
        crate::profiling::redraw_counters::record_view_render(_vr_start.elapsed());

        // 2. Layout pass.
        #[cfg(feature = "profiling")]
        let _layout_start = std::time::Instant::now();
        let root_id = {
            let guard = self.windows.borrow();
            let win = match guard.get(&window_id) {
                Some(w) => w,
                None => return,
            };
            let mut tree = win.layout_tree.borrow_mut();
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
        #[cfg(feature = "profiling")]
        crate::profiling::redraw_counters::record_layout(_layout_start.elapsed());

        let Some(root_id) = root_id else {
            log::warn!("layout computation failed");
            return;
        };

        // 3. Resolve root bounds.
        let root_bounds = {
            let guard = self.windows.borrow();
            let win = match guard.get(&window_id) {
                Some(w) => w,
                None => return,
            };
            let tree = win.layout_tree.borrow();
            resolve_bounds(tree.inner(), root_id)
        };

        let Some(root_bounds) = root_bounds else {
            log::warn!("bounds resolution failed");
            return;
        };

        // 4. Prepaint pass — needs many simultaneous borrows from WindowState.
        {
            let guard = self.windows.borrow();
            let win = match guard.get(&window_id) {
                Some(w) => w,
                None => return,
            };

            let tree = win.layout_tree.borrow();
            let mut hit = win.hit_test_list.borrow_mut();
            let mut a11y = win.a11y_nodes.borrow_mut();
            let mut ts = self.text_system.borrow_mut();
            let ts = ts.as_mut().expect("text system not initialized");
            let mut sr = self.state_registry.borrow_mut();
            let mut tsc = self.text_shaping_cache.borrow_mut();
            let mut hm = win.handler_map.borrow_mut();
            let mut mhm = win.mouse_handler_map.borrow_mut();
            let mut pm = win.parent_map.borrow_mut();
            let mut khm = win.key_handler_map.borrow_mut();
            let mut fr = win.focus_registry.borrow_mut();
            let mut fb = win.focus_bounds.borrow_mut();
            let mut ihm = win.ime_handler_map.borrow_mut();
            let mut iri = win.ime_registered_ids.borrow_mut();
            let mut ovr = win.overlay_registry.borrow_mut();

            hit.clear();
            a11y.clear();
            hm.clear();
            mhm.clear();
            pm.clear();
            khm.clear();
            fr.clear();
            fb.clear();
            ihm.clear();
            iri.clear();
            ovr.clear();
            win.ime_registry.borrow_mut().clear();

            let mut cx = PrepaintCtx::new(
                tree.inner(),
                &mut hit,
                &mut a11y,
                ts,
                &self.executor.foreground,
                scale_factor,
                Size::new(lw as f32, lh as f32),
                &mut sr,
                &mut tsc,
                &mut hm,
                &mut mhm,
                &mut pm,
                &mut khm,
                &mut fr,
                &mut fb,
                &win.ime_registry,
                &mut ihm,
                &mut iri,
                &mut ovr,
            );

            cx.init_root_frame();
            root.prepaint(root_bounds, &mut cx);

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

            fr.prune_missing();
            // Modal focus lifecycle: auto-focus into newly-opened modals, restore
            // prior focus when one closes. Runs after prune so a restore target
            // that was unmounted is correctly skipped.
            fr.reconcile_modal_focus(&mut win.modal_focus_stack.borrow_mut());
            win.ime_registry.borrow_mut().prune_missing(&iri);
            AppState::release_capture_if_unmounted(win, &hit);
        }

        // 4a. Coalesced move flush.
        self.flush_coalesced_move(window_id);

        // 4b. Hover diff.
        self.update_hover_state(window_id);

        // 5. Paint pass.
        // Clone Rc + Arc so PaintCtx does not borrow through the outer guard,
        // allowing us to drop the guard before republish_ime_cache.
        #[cfg(feature = "profiling")]
        let _paint_start = std::time::Instant::now();
        let (ime_rc_for_paint, window_arc_for_paint) = {
            let guard = self.windows.borrow();
            match guard.get(&window_id) {
                Some(w) => (w.ime_registry.clone(), w.window.clone()),
                None => return,
            }
        };
        {
            let guard = self.windows.borrow();
            let win = match guard.get(&window_id) {
                Some(w) => w,
                None => return,
            };

            let tree = win.layout_tree.borrow();
            let mut s = win.scene.borrow_mut();
            let mut r = win.renderer.borrow_mut();
            let r = r.as_mut().expect("renderer not initialized");
            let mut ts = self.text_system.borrow_mut();
            let ts = ts.as_mut().expect("text system not initialized");

            s.clear();

            let (glyph_atlas, image_atlas, queue) = r.atlases_and_queue();
            // Per-window caches paired with per-window atlases. Mixing them
            // across windows produces token-collision glyph corruption — see
            // WindowState::glyph_cache docs.
            let mut gc = win.glyph_cache.borrow_mut();
            let mut ic = win.image_cache.borrow_mut();

            // Interaction-state snapshot for per-state style resolution. Hover
            // comes from the just-updated hover diff; press is the auto-capture
            // target while the primary (left) button is held. Explicit (sticky)
            // capture is excluded — it can point at an element that was never
            // pressed, which must not light up `:active`.
            let interaction = {
                let hovered = *win.hovered_element.borrow();
                let left_down = (*win.button_state.borrow() & 0b1) != 0;
                let pressed = if left_down && !*win.explicit_capture.borrow() {
                    *win.capture_target.borrow()
                } else {
                    None
                };
                let pm = win.parent_map.borrow();
                crate::interaction_state::InteractionSnapshot::new(hovered, pressed, &pm)
            };

            let mut cx = PaintCtx::new(
                tree.inner(),
                &mut s,
                ts,
                &mut gc,
                glyph_atlas,
                image_atlas,
                &mut ic,
                queue,
                &self.executor.foreground,
                scale_factor,
                &ime_rc_for_paint,
                Some(window_arc_for_paint.as_ref()),
                interaction,
            );

            root.paint(root_bounds, &mut cx);
            // All borrows from guard/win end here as block exits.
        }
        self.republish_ime_cache(window_id);

        // Re-borrow for focus ring overlay — emitted last so it sits on top.
        let guard2 = self.windows.borrow();
        if let Some(win2) = guard2.get(&window_id) {
            let focused = win2.focus_registry.borrow().focused();
            if let Some(id) = focused {
                let registry = win2.focus_registry.borrow();
                let show_ring = registry.entry(id).map(|e| e.focus_ring).unwrap_or(false);
                drop(registry);
                if show_ring && let Some(info) = win2.focus_bounds.borrow().get(&id).copied() {
                    let mut s = win2.scene.borrow_mut();
                    // Emit into a top-most layer so a ring on an element *inside*
                    // an overlay sorts above the overlay (overlays use high depth;
                    // the post-walk default layer is depth 0).
                    s.push_layer_with_depth(i32::MAX);
                    crate::focus_ring::emit_focus_ring(&mut s, info);
                }
            }
        }
        drop(guard2);
        #[cfg(feature = "profiling")]
        crate::profiling::redraw_counters::record_paint(_paint_start.elapsed());

        // Re-borrow for render step.
        #[cfg(feature = "profiling")]
        let _present_start = std::time::Instant::now();
        let guard3 = self.windows.borrow();
        if let Some(win3) = guard3.get(&window_id) {
            let mut s = win3.scene.borrow_mut();
            let mut r = win3.renderer.borrow_mut();
            let r = r.as_mut().expect("renderer not initialized");

            #[cfg(target_os = "macos")]
            let render_result = if win3.sync_resize.get() {
                r.render_scene_sync(&mut s)
            } else {
                r.render_scene(&mut s)
            };
            #[cfg(not(target_os = "macos"))]
            let render_result = r.render_scene(&mut s);

            if let Err(e) = render_result {
                log::warn!("render skipped: {e:?}");
            }
        }
        drop(guard3);
        #[cfg(feature = "profiling")]
        crate::profiling::redraw_counters::record_present(_present_start.elapsed());

        // Poll async executor.
        self.executor.foreground.poll();

        // GC stale state slots.
        {
            let mut sr = self.state_registry.borrow_mut();
            sr.advance_frame();
            sr.gc();
        }

        // GC text shaping cache.
        {
            let mut tsc = self.text_shaping_cache.borrow_mut();
            tsc.advance_frame();
            tsc.gc();
        }
    }
}
