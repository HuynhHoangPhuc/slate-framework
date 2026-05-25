//! Inner redraw pipeline: layout → prepaint → paint → render.
//!
//! `dispatch_redraw` (in `dispatch.rs`) is the re-entrancy-guarded entry that
//! drives the device-lost recovery state machine and delegates here when the
//! state machine clears for a normal paint.

use slate_platform::Window;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::layout::{compute_layout, resolve_bounds};
use crate::render_cx::RenderCx;
use crate::types::Size;
use crate::view::View;

use super::super::state::AppState;

impl<V: View> AppState<V> {
    /// Run the redraw pipeline (layout → prepaint → paint → render).
    ///
    /// This is the inner body called by `dispatch_redraw`. The re-entrancy guard
    /// and device-lost recovery wrapper live in `dispatch_redraw`, not here.
    pub(crate) fn run_redraw(&self) {
        // Skip if not initialized
        if self.renderer.borrow().is_none() {
            return;
        }

        // skip_draws gate - suppress one frame after recovery
        if self.skip_draws.get() {
            log::debug!(target: "slate::device_lost", "skip_draws active — present suppressed");
            self.skip_draws.set(false);
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
            let mut render_cx = RenderCx::new(self.window.id());
            slate_reactive::with_observer(self.view_observer_id, || v.render(&mut render_cx))
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
            let mut mhm = self.mouse_handler_map.borrow_mut();
            let mut pm = self.parent_map.borrow_mut();
            let mut khm = self.key_handler_map.borrow_mut();
            let mut fr = self.focus_registry.borrow_mut();
            let mut fb = self.focus_bounds.borrow_mut();
            let mut ihm = self.ime_handler_map.borrow_mut();
            let mut iri = self.ime_registered_ids.borrow_mut();

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
            // Clear the dirty bit; entries are pruned after the walk
            // using `ime_registered_ids` so per-element `Rc<RefCell<ImeState>>`
            // handles survive across frames for surviving elements.
            self.ime_registry.borrow_mut().clear();

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
                &mut mhm,
                &mut pm,
                &mut khm,
                &mut fr,
                &mut fb,
                &self.ime_registry,
                &mut ihm,
                &mut iri,
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

            // Clear focus if the focused element was unmounted this frame.
            fr.prune_missing();
            // Drop IME entries for unmounted elements.
            self.ime_registry.borrow_mut().prune_missing(&iri);

            // Auto-release mouse capture if the captured element was unmounted
            // this frame (no hit region produced). An unmounted element can no
            // longer receive pointer events, so a sticky explicit capture must
            // not strand input on a dead id.
            self.release_capture_if_unmounted(&hit);
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

            let (glyph_atlas, image_atlas, queue) = r.atlases_and_queue();
            let mut ic = self.image_cache.borrow_mut();
            let mut cx = PaintCtx::new(
                tree.inner(),
                &mut s,
                ts,
                glyph_atlas,
                image_atlas,
                &mut ic,
                queue,
                &self.executor.foreground,
                scale_factor,
                &self.ime_registry,
                Some(self.window.as_ref()),
            );

            root.paint(root_bounds, &mut cx);

            // Refresh the IME query cache after every paint so the platform
            // delegate sees the freshly-painted `caret_client_rect`.
            // NLL drops `cx`'s borrows here since it's not used past this point.
            self.republish_ime_cache();

            // Focus ring overlay — emitted last so it sits on top of
            // element content. Only painted when the focused element opted into
            // a visible ring via `focus_ring(true)` (default for `focusable`).
            let focused = self.focus_registry.borrow().focused();
            if let Some(id) = focused {
                let registry = self.focus_registry.borrow();
                let show_ring = registry.entry(id).map(|e| e.focus_ring).unwrap_or(false);
                drop(registry);
                if show_ring && let Some(info) = self.focus_bounds.borrow().get(&id).copied() {
                    crate::focus_ring::emit_focus_ring(&mut s, info);
                }
            }
        }

        // 6. Render
        {
            let mut s = self.scene.borrow_mut();
            let mut r = self.renderer.borrow_mut();
            let r = r.as_mut().expect("renderer not initialized");

            // On macOS during a sync-resize tick, present inside AppKit's
            // open CATransaction so the new framebuffer lands in the same
            // transaction as the bounds change.
            #[cfg(target_os = "macos")]
            let render_result = if self.sync_resize.get() {
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
}
