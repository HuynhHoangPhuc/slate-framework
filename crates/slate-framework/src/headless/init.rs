//! `HeadlessApp` construction: wgpu instance/device, atlases, pipelines, and
//! framework state setup. Implements `HeadlessApp::with_scale_factor`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use wgpu::{
    Backends, BindGroupDescriptor, BindGroupEntry, BindingResource, BufferDescriptor, BufferUsages,
    DeviceDescriptor, ExperimentalFeatures, Features, Instance, InstanceDescriptor, Limits,
    MemoryHints, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, Trace,
};

use slate_reactive::Runtime;
use slate_renderer::atlas::{Atlas, Format};
use slate_renderer::glyph_pipeline::GlyphPipeline;
use slate_renderer::image_pipeline::ImagePipeline;
use slate_renderer::instanced_rect_pipeline::InstancedRectPipeline;
use slate_renderer::pipeline_shared::{self, ViewportUniform};
use slate_renderer::shadow_pipeline::ShadowPipeline;
use slate_renderer::{Lpx, Scene};

use crate::executor::{Executor, RedrawRequester};
use crate::focus::FocusRegistry;
use crate::hit_test::HitTestList;
use crate::image_cache::ImageCache;
use crate::ime::ImeRegistry;
use crate::layout::LayoutTree;
use crate::reactive_state::StateRegistry;
use crate::text_system::TextSystem;

use super::{HeadlessApp, HeadlessError};

impl HeadlessApp {
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

        // Runtime for reactive headless tests (no redraw wiring needed)
        let runtime = Runtime::new();
        let observer_id = runtime.next_observer_id();

        // StateRegistry for element-level reactive state
        let state_registry = StateRegistry::new(runtime.clone());

        // TextShapingCache for text shaping optimization
        let text_shaping_cache = crate::paint_cache::TextShapingCache::new();

        // Event handler collection (for headless testing parity)
        let handler_map = HashMap::new();
        let parent_map = HashMap::new();
        let key_handler_map = HashMap::new();
        let focus_registry = FocusRegistry::new();
        let focus_bounds = HashMap::new();

        // Image cache
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
            mouse_handler_map: HashMap::new(),
            parent_map,
            key_handler_map,
            focus_registry,
            focus_bounds,
            ime_registry: RefCell::new(ImeRegistry::new()),
            ime_handler_map: HashMap::new(),
            ime_registered_ids: HashSet::new(),
        })
    }
}
