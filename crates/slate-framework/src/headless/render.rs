//! `HeadlessApp` render pipeline: layout → prepaint → paint → GPU encode →
//! texture-to-buffer readback into an `image::RgbaImage`.

use std::num::NonZeroU32;

use image::RgbaImage;
use wgpu::{
    Color, CommandEncoderDescriptor, Extent3d, LoadOp, MapMode, Operations, Origin3d, PollType,
    RenderPassColorAttachment, RenderPassDescriptor, StoreOp, TexelCopyBufferInfo,
    TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
};

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::AnyElement;
use crate::layout::{compute_layout, resolve_bounds};
use crate::types::Size;
use crate::view::View;

use super::{HeadlessApp, HeadlessError};

impl HeadlessApp {
    /// Render an element tree and return the result as an RGBA image.
    pub fn render(&mut self, mut root: AnyElement) -> Result<RgbaImage, HeadlessError> {
        // Drain effects before render
        self.runtime.drain_effects();

        let w = self.width;
        let h = self.height;

        // 1. Layout pass
        #[cfg(feature = "profiling")]
        let _layout_start = std::time::Instant::now();
        self.layout_tree.clear();
        let root_id = {
            let mut cx = LayoutCtx::new(
                self.layout_tree.inner_mut(),
                &mut self.text_system,
                &self.executor.foreground,
                self.scale_factor,
            );
            compute_layout(&mut root, &mut cx, Size::new(w as f32, h as f32))
        };
        #[cfg(feature = "profiling")]
        crate::profiling::redraw_counters::record_layout(_layout_start.elapsed());

        let Some(root_id) = root_id else {
            log::warn!("HeadlessApp: layout computation failed");
            return Err(HeadlessError::InvalidDimensions);
        };

        // 2. Resolve bounds
        let root_bounds = resolve_bounds(self.layout_tree.inner(), root_id);
        let Some(root_bounds) = root_bounds else {
            log::warn!("HeadlessApp: bounds resolution failed");
            return Err(HeadlessError::InvalidDimensions);
        };

        // 3. Prepaint pass
        self.hit_test_list.clear();
        self.a11y_nodes.clear();
        self.handler_map.clear();
        self.parent_map.clear();
        self.key_handler_map.clear();
        self.focus_registry.clear();
        self.focus_bounds.clear();
        self.ime_handler_map.clear();
        self.ime_registered_ids.clear();
        self.ime_registry.borrow_mut().clear();
        {
            let mut cx = PrepaintCtx::new(
                self.layout_tree.inner(),
                &mut self.hit_test_list,
                &mut self.a11y_nodes,
                &mut self.text_system,
                &self.executor.foreground,
                self.scale_factor,
                &mut self.state_registry,
                &mut self.text_shaping_cache,
                &mut self.handler_map,
                &mut self.mouse_handler_map,
                &mut self.parent_map,
                &mut self.key_handler_map,
                &mut self.focus_registry,
                &mut self.focus_bounds,
                &self.ime_registry,
                &mut self.ime_handler_map,
                &mut self.ime_registered_ids,
            );

            // Initialize tree-position keying for stable ElementIds
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

        // Advance frame counter and GC stale state slots
        self.state_registry.advance_frame();
        self.state_registry.gc();

        // Advance frame counter and GC stale text shaping cache
        self.text_shaping_cache.advance_frame();
        self.text_shaping_cache.gc();

        // 4. Paint pass
        #[cfg(feature = "profiling")]
        let _paint_start = std::time::Instant::now();
        self.scene.clear();

        // Resolve the simulated pointer to a hover/press target via the hit
        // regions registered during the prepaint above, mirroring the windowed
        // hover-diff that runs before the paint pass.
        let interaction = {
            let hovered = self.cursor_pos.and_then(|(x, y)| {
                self.hit_test_list
                    .hit_test(crate::types::Point::new(x, y))
                    .map(|r| r.element_id)
            });
            let pressed = if self.pressing { hovered } else { None };
            crate::interaction_state::InteractionSnapshot::new(hovered, pressed, &self.parent_map)
        };
        {
            let mut cx = PaintCtx::new(
                self.layout_tree.inner(),
                &mut self.scene,
                &mut self.text_system,
                &mut self.glyph_cache,
                &mut self.glyph_atlas,
                &mut self.image_atlas,
                &mut self.image_cache,
                &self.queue,
                &self.executor.foreground,
                self.scale_factor,
                &self.ime_registry,
                None,
                interaction,
            );
            root.paint(root_bounds, &mut cx);
        }
        // Drop entries for unmounted ime-capable elements.
        self.ime_registry
            .borrow_mut()
            .prune_missing(&self.ime_registered_ids);
        #[cfg(feature = "profiling")]
        crate::profiling::redraw_counters::record_paint(_paint_start.elapsed());

        // 5. GPU render
        #[cfg(feature = "profiling")]
        let _present_start = std::time::Instant::now();
        self.scene.finish();
        self.image_atlas.begin_frame();
        self.glyph_atlas.begin_frame();

        // Prepare pipelines
        self.shadow_pipeline
            .prepare(&self.device, &self.queue, &self.scene.shadows);
        self.rect_pipeline
            .prepare(&self.device, &self.queue, &self.scene.rects);
        self.image_pipeline
            .prepare(&self.device, &self.queue, &self.scene.images);
        self.glyph_pipeline
            .prepare(&self.device, &self.queue, &self.scene.glyphs);

        // Record render pass
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("slate-headless-encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("slate-headless-pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &self.target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color::TRANSPARENT),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None::<NonZeroU32>,
            });

            // Clip: mirror the windowed submit path — one scissor update per
            // layer, `None` resets to the full attachment, an off-screen clip
            // skips the layer. `width`/`height` are the headless layout size;
            // this is exact at `scale_factor == 1.0` (every current headless
            // test). A DPI-aware (scale != 1.0) headless clip test would need
            // a separate physical target size — revisit when one is added.
            //
            // Layer draw order mirrors the windowed path's `DepthBucketOrder`:
            // walk layers lowest-depth-first (stable among equal depths) so
            // golden images match on-screen z-order. All-depth-`0` scenes walk
            // in push order — byte-identical to the pre-depth headless path.
            let (target_w, target_h) = (self.width, self.height);
            let scale = self.scale_factor as f32;
            let depths: Vec<i32> = self.scene.layers.iter().map(|l| l.depth).collect();
            for idx in slate_renderer::depth_sorted_layers(&depths) {
                let layer = &self.scene.layers[idx];
                match layer.clip {
                    Some(clip) => match clip.to_scissor_px(scale, target_w, target_h) {
                        Some((x, y, w, h)) => pass.set_scissor_rect(x, y, w, h),
                        None => continue,
                    },
                    None => pass.set_scissor_rect(0, 0, target_w.max(1), target_h.max(1)),
                }
                self.shadow_pipeline.record(
                    &mut pass,
                    &self.viewport_bg,
                    &self.unit_quad,
                    layer.shadows.clone(),
                );
                self.rect_pipeline.record(
                    &mut pass,
                    &self.viewport_bg,
                    &self.unit_quad,
                    layer.rects.clone(),
                );
                self.glyph_pipeline.record(
                    &mut pass,
                    &self.viewport_bg,
                    &self.unit_quad,
                    layer.glyphs.clone(),
                );
                self.image_pipeline.record(
                    &mut pass,
                    &self.viewport_bg,
                    &self.unit_quad,
                    layer.images.clone(),
                );
            }
        }

        // Copy texture to staging buffer
        let bytes_per_row = Self::aligned_bytes_per_row(w);
        encoder.copy_texture_to_buffer(
            TexelCopyTextureInfo {
                texture: &self.target_texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            TexelCopyBufferInfo {
                buffer: &self.staging_buffer,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(h),
                },
            },
            Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        // Read back pixels
        let buffer_slice = self.staging_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(MapMode::Read, move |result| {
            tx.send(result).ok();
        });

        let _ = self.device.poll(PollType::wait_indefinitely());

        rx.recv()
            .map_err(|_| HeadlessError::BufferMapping)?
            .map_err(|_| HeadlessError::BufferMapping)?;

        // Copy to image buffer (removing row padding)
        let data = buffer_slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);

        for row in 0..h {
            let start = (row * bytes_per_row) as usize;
            let end = start + (w * 4) as usize;
            pixels.extend_from_slice(&data[start..end]);
        }

        drop(data);
        self.staging_buffer.unmap();
        #[cfg(feature = "profiling")]
        crate::profiling::redraw_counters::record_present(_present_start.elapsed());

        // image crate expects sRGB, which matches our target_texture format
        RgbaImage::from_raw(w, h, pixels).ok_or(HeadlessError::InvalidDimensions)
    }

    /// Render a View with reactive observer wrapping.
    ///
    /// Use this for testing reactive Views. The view's `render()` method is called
    /// inside `with_observer(...)` so that signals read during render auto-subscribe
    /// to the headless observer.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut app = HeadlessApp::new(400, 200)?;
    /// let count = Signal::new(app.runtime(), 0u32);
    /// let mut view = CounterView { count: count.clone() };
    /// let img = app.render_view(&mut view)?;
    /// count.set(1);
    /// let img2 = app.render_view(&mut view)?;
    /// ```
    pub fn render_view<V: View>(&mut self, view: &mut V) -> Result<RgbaImage, HeadlessError> {
        // Build element tree inside observer scope for reactive subscriptions
        let mut render_cx = crate::RenderCx::new(slate_platform::WindowId(0));
        #[cfg(feature = "profiling")]
        let _vr_start = std::time::Instant::now();
        let root = slate_reactive::with_observer(self.observer_id, || view.render(&mut render_cx));
        #[cfg(feature = "profiling")]
        crate::profiling::redraw_counters::record_view_render(_vr_start.elapsed());
        self.render(root)
    }
}
