//! GPU renderer for the slate-framework UI framework, built on `wgpu`.
//!
//! The primary entry point is [`Renderer::render_scene`], which takes a
//! [`Scene`] and draws all layers (shadows → rects → glyphs → images per
//! layer) in a single render pass with one `Queue::submit`.
//!
//! # Lifecycle contract
//!
//! [`Renderer`] **must** be constructed from inside an `Event::Resumed` handler,
//! after the platform run-loop is alive. On macOS, wgpu surface creation requires
//! a running `NSApplication` (CAMetalLayer attachment + `mainScreen` lookups).
//! Constructing before `Platform::run` returns will panic or produce a null surface.
//!
//! Use `pollster::block_on` inside the event handler:
//!
//! ```ignore
//! platform.run(|event| {
//!     if let Event::Resumed = event {
//!         let renderer = pollster::block_on(Renderer::new(Arc::clone(&window)))
//!             .expect("failed to init renderer");
//!     }
//! });
//! ```

// Public modules
pub mod atlas;
pub mod color;
pub mod glyph_pipeline;
pub mod image_pipeline;
pub mod instanced_rect_pipeline;
pub mod pipeline_shared;
pub mod renderer;
pub mod scene;
pub mod shadow_pipeline;
pub mod surface_target;

// Platform-specific modules (internal)
#[cfg(target_os = "macos")]
pub(crate) mod mac_surface;
#[cfg(target_os = "windows")]
pub(crate) mod win_compose;

// Re-exports from color
pub use color::{
    linear_to_srgb_channel, linear_to_srgb_u8, srgb_channel_to_linear, srgb_to_linear,
    srgb_u8_to_linear, srgb_u8_to_linear_premul,
};

// Re-exports from pipelines
pub use glyph_pipeline::{GlyphPipeline, allocate_glyph};
pub use image_pipeline::ImagePipeline;
pub use instanced_rect_pipeline::InstancedRectPipeline;
pub use pipeline_shared::{ViewportUniform, create_unit_quad, viewport_bind_group_layout};
pub use shadow_pipeline::ShadowPipeline;

// Re-exports from scene
pub use scene::{GlyphInstance, ImageInstance, Layer, RectInstance, Scene, ShadowInstance};

// Re-exports from renderer
pub use renderer::{Renderer, RendererError, RenderError};
