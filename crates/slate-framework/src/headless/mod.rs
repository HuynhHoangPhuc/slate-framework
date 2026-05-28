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

mod init;
mod render;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use wgpu::{BindGroup, Buffer, Device, Queue, RequestDeviceError, Texture, TextureView};
use slate_reactive::{ObserverId, Runtime};
use slate_renderer::Scene;
use slate_renderer::atlas::Atlas;
use slate_renderer::glyph_pipeline::GlyphPipeline;
use slate_renderer::image_pipeline::ImagePipeline;
use slate_renderer::instanced_rect_pipeline::InstancedRectPipeline;
use slate_renderer::shadow_pipeline::ShadowPipeline;
use crate::event::{Handlers, ImeHandlers, KeyHandlers, MouseHandlers};
use crate::executor::Executor;
use crate::focus::FocusRegistry;
use crate::focus_ring::FocusBounds;
use crate::hit_test::HitTestList;
use slate_text::GlyphCache;
use crate::image_cache::ImageCache;
use crate::ime::ImeRegistry;
use crate::layout::LayoutTree;
use crate::reactive_state::StateRegistry;
use crate::text_system::TextSystem;
use crate::types::{AccessibilityNode, ElementId};

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

    // Reactive runtime for headless tests
    runtime: Arc<Runtime>,
    observer_id: ObserverId,

    // StateRegistry for element-level reactive state
    state_registry: StateRegistry,

    // TextShapingCache for text shaping optimization
    text_shaping_cache: crate::paint_cache::TextShapingCache,

    // Image cache for uploaded images
    image_cache: ImageCache,

    // Glyph cache (atlas-scoped; pairs with `glyph_atlas`).
    glyph_cache: GlyphCache,

    // Event handler collection (for headless testing parity)
    handler_map: HashMap<ElementId, Handlers>,
    mouse_handler_map: HashMap<ElementId, MouseHandlers>,
    parent_map: HashMap<ElementId, ElementId>,

    // Keyboard + focus state collected each prepaint (headless parity).
    key_handler_map: HashMap<ElementId, KeyHandlers>,
    focus_registry: FocusRegistry,
    focus_bounds: HashMap<ElementId, FocusBounds>,

    // IME state collected each prepaint (headless parity).
    ime_registry: RefCell<ImeRegistry>,
    ime_handler_map: HashMap<ElementId, ImeHandlers>,
    ime_registered_ids: HashSet<ElementId>,
}

/// Error creating or rendering with HeadlessApp.
#[derive(thiserror::Error, Debug)]
pub enum HeadlessError {
    /// No GPU adapter that supports the headless surface format was found.
    #[error("no compatible GPU adapter found")]
    NoAdapter,
    /// `Adapter::request_device` rejected the device descriptor.
    #[error("failed to open GPU device: {0}")]
    Device(#[from] RequestDeviceError),
    /// Failed to initialize the text shaping/rasterization backend.
    #[error("text system init failed: {0}")]
    TextSystem(String),
    /// Mapping the readback buffer for CPU access failed.
    #[error("buffer mapping failed")]
    BufferMapping,
    /// Image dimensions are zero or exceed renderer limits.
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

    /// Bytes per row aligned to 256 (wgpu requirement for buffer copies).
    fn aligned_bytes_per_row(width: u32) -> u32 {
        let bytes_per_pixel = 4u32; // RGBA8
        let unaligned = width * bytes_per_pixel;
        let alignment = 256u32;
        unaligned.div_ceil(alignment) * alignment
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

    /// Snapshot the most recently built accessibility tree.
    ///
    /// Returns a clone of the completed `Vec<AccessibilityNode>` produced
    /// during the last `render()` / `render_view()` call. Empty until the
    /// first render runs.
    pub fn a11y_nodes(&self) -> Vec<AccessibilityNode> {
        self.a11y_nodes.clone()
    }

    /// Get the reactive runtime for creating signals in tests.
    pub fn runtime(&self) -> Arc<Runtime> {
        self.runtime.clone()
    }

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

    /// Inject an `ImeState` for `id` directly into the headless IME registry.
    ///
    /// Bypasses the platform dispatch path; intended for visual regression
    /// tests that need to render preedit / caret states without a real IME
    /// session. `ime_registry` is the source of truth here — no separate
    /// republish step required.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_ime_state(&mut self, id: ElementId, state: crate::ime::ImeState) {
        let rc = self.ime_registry.borrow_mut().register(id);
        *rc.borrow_mut() = state;
    }
}
