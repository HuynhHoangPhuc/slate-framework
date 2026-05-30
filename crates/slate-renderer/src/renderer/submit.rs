//! Frame submission: scene render, clear-only render, and the shared frame
//! acquisition loop.
//!
//! Submodule for `Renderer`. Holds:
//!
//! - `render_scene` / `render_scene_sync` / `render_scene_with_present_mode`:
//!   the full per-frame path (prepare atlases, prepare pipelines, acquire
//!   frame with Outdated retry, record draws via `LayerOrdering`, submit,
//!   present).
//! - `render`: the stub clear-only path.
//! - `draw_clear_pass`: shared clear-pass helper.

use std::num::NonZeroU32;
use std::sync::atomic::Ordering;

use wgpu::{
    Color, CommandEncoderDescriptor, LoadOp, Operations, RenderPassColorAttachment,
    RenderPassDescriptor, StoreOp,
};

use crate::layer_ordering::{DefaultPainterOrder, LayerOrdering, ScenePrimitive};
use crate::scene::Scene;
use crate::surface_target::{ConfigureError, FrameAcquireError};
// On macOS, `Renderer.target` is the concrete `Box<MacSurface>` (not a trait
// object), so the trait must be in scope to call `acquire_frame`, `configure`,
// and `present`. On Windows the field is already `Box<dyn CompositionTarget>`.
#[cfg(target_os = "macos")]
use crate::surface_target::CompositionTarget;

use super::{RenderError, Renderer};

impl Renderer {
    /// Render a scene: all layers in push order, fixed primitive order within
    /// each layer (shadows → rects → glyphs → images).
    ///
    /// Takes `&mut Scene` so it can call `finish()` internally — callers never
    /// need to remember.
    pub fn render_scene(&mut self, scene: &mut Scene) -> Result<(), RenderError> {
        self.render_scene_with_present_mode(scene, false)
    }

    /// macOS-only: render + present inside AppKit's currently open resize
    /// CATransaction. Use during live resize to land the new framebuffer in
    /// the same transaction as the bounds change. See
    /// [`crate::mac_surface::MacSurface::present_sync`] for the GPU-sync details.
    #[cfg(target_os = "macos")]
    pub fn render_scene_sync(&mut self, scene: &mut Scene) -> Result<(), RenderError> {
        self.render_scene_with_present_mode(scene, true)
    }

    fn render_scene_with_present_mode(
        &mut self,
        scene: &mut Scene,
        sync_present: bool,
    ) -> Result<(), RenderError> {
        scene.finish();
        self.image_atlas.begin_frame();
        self.glyph_atlas.begin_frame();

        // Phase A: PREPARE — all mutable upload work, no pass alive.
        self.shadow_pipeline
            .prepare(&self.device, &self.queue, &scene.shadows);
        self.rect_pipeline
            .prepare(&self.device, &self.queue, &scene.rects);
        self.image_pipeline
            .prepare(&self.device, &self.queue, &scene.images);
        self.glyph_pipeline
            .prepare(&self.device, &self.queue, &scene.glyphs);

        // Phase B: RECORD — try acquire, with one retry if Outdated.
        let frame = {
            let mut last_outdated = false;
            let mut acquired = None;
            for _attempt in 0..2 {
                match self.target.acquire_frame() {
                    Ok(f) => {
                        acquired = Some(f);
                        break;
                    }
                    Err(FrameAcquireError::Outdated) => {
                        let (w, h) = self._window.physical_size();
                        if let Err(e) = self.target.configure(&self.device, w.max(1), h.max(1)) {
                            match &e {
                                ConfigureError::ResizeBuffersFailed(hr)
                                | ConfigureError::BackBufferFailed(hr) => {
                                    if self.check_hr_for_device_lost(*hr) {
                                        return Err(RenderError::DeviceLost(format!("{:?}", e)));
                                    }
                                }
                            }
                        }
                        last_outdated = true;
                    }
                    Err(
                        FrameAcquireError::Occluded
                        | FrameAcquireError::Minimized
                        | FrameAcquireError::Timeout,
                    ) => {
                        return Ok(());
                    }
                    Err(FrameAcquireError::DeviceLost(reason)) => {
                        self.device_lost.store(true, Ordering::Release);
                        return Err(RenderError::DeviceLost(reason));
                    }
                    Err(other) => return Err(RenderError::AcquireFailed(other.to_string())),
                }
            }
            match acquired {
                Some(f) => f,
                None => {
                    if last_outdated {
                        log::warn!("renderer: surface still Outdated after retry");
                    }
                    return Ok(());
                }
            }
        };
        let view = &frame.view;
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("slate-frame-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("slate-frame"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view,
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

            // Draw order is driven through the `LayerOrdering` seam so the
            // strategy (depth buckets, BoundsTree) can be swapped post-v1
            // without touching this pass-recording code. The default is the v1
            // painter's walk; monomorphized here, so no per-frame vtable.
            //
            // Clip: each layer may carry an `Option<ClipRect>` (logical px).
            // The scissor is render-pass state, so we update it once per layer
            // (when the layer index changes — `for_each_draw` yields a layer's
            // four primitive kinds consecutively, which the `idx != last_idx`
            // guard relies on; both shipped orderings emit per-layer kinds back
            // to back). A `None` clip resets to the full attachment; a clip
            // that lands fully off-screen skips the layer's draws entirely.
            let (target_w, target_h) = self._window.physical_size();
            let scale = self._window.scale_factor() as f32;
            let mut last_idx = usize::MAX;
            let mut skip_layer = false;
            DefaultPainterOrder.for_each_draw(scene.layers.len(), |idx, kind| {
                let layer = &scene.layers[idx];
                if idx != last_idx {
                    last_idx = idx;
                    skip_layer = false;
                    match layer.clip {
                        Some(clip) => match clip.to_scissor_px(scale, target_w, target_h) {
                            Some((x, y, w, h)) => pass.set_scissor_rect(x, y, w, h),
                            None => skip_layer = true,
                        },
                        None => pass.set_scissor_rect(0, 0, target_w.max(1), target_h.max(1)),
                    }
                }
                if skip_layer {
                    return;
                }
                match kind {
                    ScenePrimitive::Shadow => self.shadow_pipeline.record(
                        &mut pass,
                        &self.viewport_bg,
                        &self.unit_quad,
                        layer.shadows.clone(),
                    ),
                    ScenePrimitive::Rect => self.rect_pipeline.record(
                        &mut pass,
                        &self.viewport_bg,
                        &self.unit_quad,
                        layer.rects.clone(),
                    ),
                    ScenePrimitive::Glyph => self.glyph_pipeline.record(
                        &mut pass,
                        &self.viewport_bg,
                        &self.unit_quad,
                        layer.glyphs.clone(),
                    ),
                    ScenePrimitive::Image => self.image_pipeline.record(
                        &mut pass,
                        &self.viewport_bg,
                        &self.unit_quad,
                        layer.images.clone(),
                    ),
                }
            });
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        // Present — on macOS, route through `present_sync` during live resize
        // so the new framebuffer lands inside AppKit's open CATransaction.
        let present_result = {
            #[cfg(target_os = "macos")]
            {
                if sync_present {
                    self.target.present_sync(frame, &self.device)
                } else {
                    self.target.present(frame)
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = sync_present; // unused off-macOS
                self.target.present(frame)
            }
        };
        if let Err(e) = present_result {
            if self.check_hr_for_device_lost(e.hr()) {
                return Err(RenderError::DeviceLost(format!("present failed: {:?}", e)));
            }
            log::warn!(target: "slate::render", "present failed: {:?}", e);
        }
        Ok(())
    }

    /// Acquire the next frame, submit a clear-color pass, and present.
    ///
    /// Stub clear-only path — use [`Renderer::render_scene`] for Scene-driven
    /// rendering.
    pub fn render(&mut self) -> Result<(), RenderError> {
        // Try acquire with one retry if Outdated.
        let frame = {
            let mut last_outdated = false;
            let mut acquired = None;
            for _attempt in 0..2 {
                match self.target.acquire_frame() {
                    Ok(f) => {
                        acquired = Some(f);
                        break;
                    }
                    Err(FrameAcquireError::Outdated) => {
                        let (w, h) = self._window.physical_size();
                        if let Err(e) = self.target.configure(&self.device, w.max(1), h.max(1)) {
                            match &e {
                                ConfigureError::ResizeBuffersFailed(hr)
                                | ConfigureError::BackBufferFailed(hr) => {
                                    if self.check_hr_for_device_lost(*hr) {
                                        return Err(RenderError::DeviceLost(format!("{:?}", e)));
                                    }
                                }
                            }
                        }
                        last_outdated = true;
                    }
                    Err(
                        FrameAcquireError::Occluded
                        | FrameAcquireError::Minimized
                        | FrameAcquireError::Timeout,
                    ) => {
                        return Ok(());
                    }
                    Err(FrameAcquireError::DeviceLost(reason)) => {
                        self.device_lost.store(true, Ordering::Release);
                        return Err(RenderError::DeviceLost(reason));
                    }
                    Err(other) => return Err(RenderError::AcquireFailed(other.to_string())),
                }
            }
            match acquired {
                Some(f) => f,
                None => {
                    if last_outdated {
                        log::warn!("renderer: surface still Outdated after retry");
                    }
                    return Ok(());
                }
            }
        };
        self.draw_clear_pass(&frame.view);

        // Handle present errors - check for device-lost
        if let Err(e) = self.target.present(frame) {
            if self.check_hr_for_device_lost(e.hr()) {
                return Err(RenderError::DeviceLost(format!("present failed: {:?}", e)));
            }
            log::warn!(target: "slate::render", "present failed: {:?}", e);
        }
        Ok(())
    }

    /// Encode and submit a clear-color render pass (dark gray).
    fn draw_clear_pass(&self, view: &wgpu::TextureView) {
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("slate-frame-encoder"),
            });
        {
            let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("slate-clear-pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color {
                            r: 0.1,
                            g: 0.1,
                            b: 0.15,
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None::<NonZeroU32>,
            });
        }
        self.queue.submit(std::iter::once(encoder.finish()));
    }
}
