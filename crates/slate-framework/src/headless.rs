//! Headless rendering — offscreen wgpu rendering without a platform window.
//!
//! `HeadlessApp` runs the full layout → prepaint → paint pipeline against an
//! offscreen texture, then reads back the pixels as an `image::RgbaImage`.
//!
//! # Example
//!
//! ```ignore
//! let mut app = HeadlessApp::new(800, 600)?;
//! let ui = Div::new().child(Text::new("Hello!"));
//! let image = app.render(ui.into_any())?;
//! image.save("output.png")?;
//! ```

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;

use image::RgbaImage;
use wgpu::{
    Backends, BindGroup, BindGroupDescriptor, BindGroupEntry, BindingResource, Buffer,
    BufferDescriptor, BufferUsages, Color, CommandEncoderDescriptor, Device, DeviceDescriptor,
    ExperimentalFeatures, Extent3d, Features, Instance, InstanceDescriptor, Limits, LoadOp,
    MapMode, MemoryHints, Operations, Origin3d, PollType, Queue, RenderPassColorAttachment,
    RenderPassDescriptor, RequestDeviceError, StoreOp, TexelCopyBufferInfo, TexelCopyBufferLayout,
    TexelCopyTextureInfo, Texture, TextureAspect, TextureDescriptor, TextureDimension,
    TextureFormat, TextureUsages, TextureView, Trace,
};

use std::sync::Arc;

use slate_reactive::{ObserverId, Runtime};
use slate_renderer::{Lpx, Scene};
use slate_renderer::atlas::{Atlas, Format};
use slate_renderer::glyph_pipeline::GlyphPipeline;
use slate_renderer::image_pipeline::ImagePipeline;
use slate_renderer::instanced_rect_pipeline::InstancedRectPipeline;
use slate_renderer::pipeline_shared::{self, ViewportUniform};
use slate_renderer::shadow_pipeline::ShadowPipeline;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::AnyElement;
use crate::event::{Handlers, ImeHandlers, KeyHandlers};
use crate::executor::{Executor, RedrawRequester};
use crate::focus::FocusRegistry;
use crate::focus_ring::FocusBounds;
use crate::hit_test::HitTestList;
use crate::image_cache::ImageCache;
use crate::ime::ImeRegistry;
use crate::layout::{LayoutTree, compute_layout, resolve_bounds};
use crate::reactive_state::StateRegistry;
use crate::text_system::TextSystem;
use crate::types::{AccessibilityNode, ElementId, Size};
use crate::view::View;

/// Headless application for offscreen rendering.
///
/// Renders elements to an in-memory texture without requiring a platform window.
pub struct HeadlessApp {
    device: Device,
    queue: Queue,
    width: u32,
    height: u32,
    scale_factor: f64,

    // Render target
    target_texture: Texture,
    target_view: TextureView,
    staging_buffer: Buffer,

    // Shared GPU resources
    #[allow(dead_code)]
    viewport_buf: Buffer,
    viewport_bg: BindGroup,
    unit_quad: Buffer,

    // Atlases
    image_atlas: Atlas,
    glyph_atlas: Atlas,

    // Pipelines
    rect_pipeline: InstancedRectPipeline,
    shadow_pipeline: ShadowPipeline,
    image_pipeline: ImagePipeline,
    glyph_pipeline: GlyphPipeline,

    // Framework state
    text_system: TextSystem,
    layout_tree: LayoutTree,
    hit_test_list: HitTestList,
    a11y_nodes: Vec<AccessibilityNode>,
    scene: Scene,
    executor: Executor,

    // Phase 5: Reactive runtime for headless tests
    runtime: Arc<Runtime>,
    observer_id: ObserverId,

    // Phase 4: StateRegistry for element-level reactive state
    state_registry: StateRegistry,

    // Phase 6: TextShapingCache for text shaping optimization
    text_shaping_cache: crate::paint_cache::TextShapingCache,

    // Phase 2: Image cache for uploaded images
    image_cache: ImageCache,

    // Phase 5a: Event handler collection (for headless testing parity)
    handler_map: HashMap<ElementId, Handlers>,
    parent_map: HashMap<ElementId, ElementId>,

    // Phase 9b: keyboard + focus state collected each prepaint (headless parity).
    key_handler_map: HashMap<ElementId, KeyHandlers>,
    focus_registry: FocusRegistry,
    focus_bounds: HashMap<ElementId, FocusBounds>,

    // Phase 9c: IME state collected each prepaint (headless parity).
    ime_registry: RefCell<ImeRegistry>,
    ime_handler_map: HashMap<ElementId, ImeHandlers>,
    ime_registered_ids: HashSet<ElementId>,
}

/// Error creating or rendering with HeadlessApp.
#[derive(thiserror::Error, Debug)]
pub enum HeadlessError {
    #[error("no compatible GPU adapter found")]
    NoAdapter,
    #[error("failed to open GPU device: {0}")]
    Device(#[from] RequestDeviceError),
    #[error("text system init failed: {0}")]
    TextSystem(String),
    #[error("buffer mapping failed")]
    BufferMapping,
    #[error("invalid image dimensions")]
    InvalidDimensions,
}

impl HeadlessApp {
    /// Create a new headless app with the given dimensions (logical pixels).
    ///
    /// Uses scale_factor of 1.0 by default. Call `with_scale_factor` to change.
    pub fn new(width: u32, height: u32) -> Result<Self, HeadlessError> {
        Self::with_scale_factor(width, height, 1.0)
    }

    /// Create a new headless app with custom scale factor.
    pub fn with_scale_factor(
        width: u32,
        height: u32,
        scale_factor: f64,
    ) -> Result<Self, HeadlessError> {
        // Use PRIMARY backends for headless (works on all platforms)
        let instance = Instance::new(InstanceDescriptor {
            backends: Backends::PRIMARY,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: Default::default(),
            display: None,
        });

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower, // Better for headless/CI
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .map_err(|_| HeadlessError::NoAdapter)?;

        log::info!(
            "HeadlessApp: GPU adapter selected: {:?}",
            adapter.get_info()
        );

        let (device, queue) = pollster::block_on(
            adapter.request_device(&DeviceDescriptor {
                label: Some("slate-headless-device"),
                required_features: Features::empty(),
                required_limits: Limits::downlevel_defaults()
                    .using_resolution(adapter.limits())
                    .using_alignment(adapter.limits()),
                memory_hints: MemoryHints::Performance,
                trace: Trace::Off,
                experimental_features: ExperimentalFeatures::disabled(),
            }),
        )?;

        let format = TextureFormat::Rgba8UnormSrgb;

        // Create offscreen render target
        let target_texture = device.create_texture(&TextureDescriptor {
            label: Some("slate-headless-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let target_view = target_texture.create_view(&Default::default());

        // Create staging buffer for readback (4 bytes per pixel, aligned to 256)
        let bytes_per_row = Self::aligned_bytes_per_row(width);
        let buffer_size = (bytes_per_row * height) as u64;
        let staging_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("slate-headless-staging"),
            size: buffer_size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Shared resources
        let viewport_bgl = pipeline_shared::viewport_bind_group_layout(&device);
        let viewport_buf = device.create_buffer(&BufferDescriptor {
            label: Some("slate-viewport-buf"),
            size: std::mem::size_of::<ViewportUniform>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &viewport_buf,
            0,
            bytemuck::bytes_of(&ViewportUniform {
                size: [Lpx(width as f32), Lpx(height as f32)],
                _pad: [0.0; 2],
            }),
        );
        let viewport_bg = device.create_bind_group(&BindGroupDescriptor {
            label: Some("slate-viewport-bg"),
            layout: &viewport_bgl,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer(viewport_buf.as_entire_buffer_binding()),
            }],
        });
        let unit_quad = pipeline_shared::create_unit_quad(&device);

        // Atlases
        let image_atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);
        let glyph_atlas = Atlas::new(&device, Format::R8Unorm);

        // Pipelines
        let rect_pipeline = InstancedRectPipeline::new(&device, format, &viewport_bgl);
        let shadow_pipeline = ShadowPipeline::new(&device, format, &viewport_bgl);
        let image_pipeline = ImagePipeline::new(&device, format, &viewport_bgl, &image_atlas);
        let glyph_pipeline = GlyphPipeline::new(&device, format, &viewport_bgl, &glyph_atlas);

        // Framework state
        let text_system =
            TextSystem::new().map_err(|e| HeadlessError::TextSystem(e.to_string()))?;
        let layout_tree = LayoutTree::new();
        let hit_test_list = HitTestList::new();
        let a11y_nodes = Vec::new();
        let scene = Scene::new();
        let redraw_requester = RedrawRequester::new(|| {}); // No-op for headless
        let executor = Executor::new(redraw_requester);

        // Phase 5: Runtime for reactive headless tests (no redraw wiring needed)
        let runtime = Runtime::new();
        let observer_id = runtime.next_observer_id();

        // Phase 4: StateRegistry for element-level reactive state
        let state_registry = StateRegistry::new(runtime.clone());

        // Phase 6: TextShapingCache for text shaping optimization
        let text_shaping_cache = crate::paint_cache::TextShapingCache::new();

        // Phase 5a: Event handler collection (for headless testing parity)
        let handler_map = HashMap::new();
        let parent_map = HashMap::new();
        let key_handler_map = HashMap::new();
        let focus_registry = FocusRegistry::new();
        let focus_bounds = HashMap::new();

        // Phase 2: Image cache
        let image_cache = ImageCache::new();

        Ok(Self {
            device,
            queue,
            width,
            height,
            scale_factor,
            target_texture,
            target_view,
            staging_buffer,
            viewport_buf,
            viewport_bg,
            unit_quad,
            image_atlas,
            glyph_atlas,
            rect_pipeline,
            shadow_pipeline,
            image_pipeline,
            glyph_pipeline,
            text_system,
            layout_tree,
            hit_test_list,
            a11y_nodes,
            scene,
            executor,
            runtime,
            observer_id,
            state_registry,
            text_shaping_cache,
            image_cache,
            handler_map,
            parent_map,
            key_handler_map,
            focus_registry,
            focus_bounds,
            ime_registry: RefCell::new(ImeRegistry::new()),
            ime_handler_map: HashMap::new(),
            ime_registered_ids: HashSet::new(),
        })
    }

    /// Bytes per row aligned to 256 (wgpu requirement for buffer copies).
    fn aligned_bytes_per_row(width: u32) -> u32 {
        let bytes_per_pixel = 4u32; // RGBA8
        let unaligned = width * bytes_per_pixel;
        let alignment = 256u32;
        unaligned.div_ceil(alignment) * alignment
    }

    /// Render an element tree and return the result as an RGBA image.
    pub fn render(&mut self, mut root: AnyElement) -> Result<RgbaImage, HeadlessError> {
        // Phase 5: Drain effects before render
        self.runtime.drain_effects();

        let w = self.width;
        let h = self.height;

        // 1. Layout pass
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

        // Phase 4: Advance frame counter and GC stale state slots
        self.state_registry.advance_frame();
        self.state_registry.gc();

        // Phase 6: Advance frame counter and GC stale text shaping cache
        self.text_shaping_cache.advance_frame();
        self.text_shaping_cache.gc();

        // 4. Paint pass
        self.scene.clear();
        {
            let mut cx = PaintCtx::new(
                self.layout_tree.inner(),
                &mut self.scene,
                &mut self.text_system,
                &mut self.glyph_atlas,
                &mut self.image_atlas,
                &mut self.image_cache,
                &self.queue,
                &self.executor.foreground,
                self.scale_factor,
                &self.ime_registry,
            );
            root.paint(root_bounds, &mut cx);
        }
        // Phase 9c: drop entries for unmounted ime-capable elements.
        self.ime_registry
            .borrow_mut()
            .prune_missing(&self.ime_registered_ids);

        // 5. GPU render
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

            for layer in &self.scene.layers {
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

        // Convert from SRGB to linear for image crate (it expects linear)
        // Actually image crate expects sRGB, so we're good
        RgbaImage::from_raw(w, h, pixels).ok_or(HeadlessError::InvalidDimensions)
    }

    /// Get the current dimensions.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Get the scale factor.
    pub fn scale_factor(&self) -> f64 {
        self.scale_factor
    }

    /// Mutable access to the text system.
    pub fn text_system_mut(&mut self) -> &mut TextSystem {
        &mut self.text_system
    }

    /// Inspect the most recently rendered scene. Used by wire-format
    /// regression tests to verify that pushed instances carry lpx coordinates
    /// regardless of `scale_factor`.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Get the reactive runtime for creating signals in tests.
    pub fn runtime(&self) -> Arc<Runtime> {
        self.runtime.clone()
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
        // Phase 5: Build element tree inside observer scope for reactive subscriptions
        let root = slate_reactive::with_observer(self.observer_id, || view.render());
        self.render(root)
    }

    // =========================================================================
    // Phase 6: Test helpers for TextShapingCache metrics
    // =========================================================================

    /// Get text shaping cache hit count (for testing).
    pub fn text_shaping_cache_hits(&self) -> u64 {
        self.text_shaping_cache.hit_count()
    }

    /// Get text shaping cache miss count (for testing).
    pub fn text_shaping_cache_misses(&self) -> u64 {
        self.text_shaping_cache.miss_count()
    }

    /// Get number of entries in the text shaping cache (for testing).
    pub fn text_shaping_cache_len(&self) -> usize {
        self.text_shaping_cache.len()
    }

    /// Get memory used by text shaping cache in bytes (for testing).
    pub fn text_shaping_cache_memory(&self) -> usize {
        self.text_shaping_cache.memory_used()
    }

    // =========================================================================
    // Phase 9c: IME test hook
    // =========================================================================

    /// Inject an `ImeState` for `id` directly into the headless IME registry,
    /// then republish the cached query snapshot.
    ///
    /// This bypasses the platform dispatch path and is intended for visual
    /// regression tests that need to render preedit / caret states without a
    /// real IME session.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_ime_state(&mut self, id: ElementId, state: crate::ime::ImeState) {
        let rc = self.ime_registry.borrow_mut().register(id);
        *rc.borrow_mut() = state;
        // HeadlessApp has no WindowImeDelegate cache; `ime_registry` itself is
        // the source of truth — no separate republish step required here.
    }
}
