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

// RT-11: Prevent `test-hooks` feature from leaking into release builds.
// The feature exposes `force_device_lost` and `fire_device_lost_callback_for_test`
// which can DoS the GPU. Belt-and-suspenders against workspace members accidentally
// enabling the feature via `[dependencies]` (not `[dev-dependencies]`).
#[cfg(all(not(debug_assertions), feature = "test-hooks"))]
compile_error!(
    "the `test-hooks` feature must NEVER be enabled in release builds — \
     it exposes `force_device_lost` and `fire_device_lost_callback_for_test` \
     as public API and would allow any caller to DoS the GPU"
);

// Public modules
pub mod atlas;
pub mod color;
pub mod device_lost_reason;
pub mod glyph_pipeline;
pub mod image_pipeline;
pub mod instanced_rect_pipeline;
pub mod observer;
pub mod pipeline_shared;
pub mod renderer;
pub mod scene;
pub mod shadow_pipeline;
pub mod surface_target;
pub mod units;

// Platform-specific modules (internal)
#[cfg(target_os = "macos")]
pub(crate) mod mac_surface;
#[cfg(target_os = "windows")]
pub(crate) mod win_compose;

// Adapter selection (logic is cross-platform-testable; LUID interop is Windows-only)
pub(crate) mod adapter_selection;

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

// Re-exports from device_lost_reason
pub use device_lost_reason::DeviceLostReason;

// Re-exports from observer
pub use observer::RendererObserver;

// Re-exports from units
pub use units::Lpx;

// Re-exports from renderer
pub use renderer::{RenderError, Renderer, RendererError};
