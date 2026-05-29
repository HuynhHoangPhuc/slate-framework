//! Platform-agnostic composition target abstraction.
//!
//! On macOS this wraps a wgpu Surface (CAMetalLayer-backed).
//! On Windows this wraps a DXGI composition swap chain bound to an
//! IDCompositionVisual (no DWM redirection bitmap).

use wgpu::{Device, TextureFormat, TextureView};

/// A handle to one acquired frame.
///
/// The frame OWNS the references the renderer needs for the duration of the
/// render pass. `view` is exposed directly so `Renderer::render_scene` can
/// pass `&frame.view` into a `RenderPassColorAttachment`. Platform-specific
/// state is hidden in `inner` and consumed by `CompositionTarget::present`.
pub struct AcquiredFrame {
    pub view: TextureView,
    pub(crate) inner: AcquiredFrameInner,
}

pub(crate) enum AcquiredFrameInner {
    #[cfg(target_os = "macos")]
    Mac(wgpu::SurfaceTexture),
    #[cfg(target_os = "windows")]
    Win { back_buffer_index: u32 },
}

pub trait CompositionTarget {
    /// Configure (or reconfigure on resize). Width/height are physical pixels.
    /// Implementations MUST drop any cached views/textures before calling
    /// `ResizeBuffers`/`surface.configure` and rebuild them after.
    ///
    /// Returns `Ok(())` on success, or `Err` with an HRESULT if resize failed
    /// (Windows only — macOS always returns `Ok`). Caller should check for
    /// device-lost HRESULTs and handle recovery.
    fn configure(&mut self, device: &Device, width: u32, height: u32)
    -> Result<(), ConfigureError>;

    /// Acquire the next frame's render target. Caller renders into `view`,
    /// then calls `present`. On Windows, this drives the resource-state
    /// barrier prologue (COMMON/PRESENT → RENDER_TARGET).
    fn acquire_frame(&mut self) -> Result<AcquiredFrame, FrameAcquireError>;

    /// Present the previously acquired frame. Consumes `frame`. On Windows
    /// this issues the trailing RENDER_TARGET → PRESENT barrier and
    /// `IDXGISwapChain::Present(1, 0)`. No-op when the window is minimized.
    ///
    /// Returns `Ok(())` on success, or `Err` with an HRESULT if Present failed
    /// (Windows only — macOS always returns `Ok`). Caller should check for
    /// device-lost HRESULTs.
    fn present(&mut self, frame: AcquiredFrame) -> Result<(), PresentError>;

    /// The texture format used for rendering (Bgra8UnormSrgb on macOS,
    /// Bgra8Unorm on Windows). Renderer pipelines are built against this.
    fn format(&self) -> TextureFormat;

    /// Current configured size (physical pixels).
    fn size(&self) -> (u32, u32);

    /// Tell the impl whether the window is currently minimized. When true,
    /// `present` becomes a no-op (skip barrier work + Present call).
    fn set_minimized(&mut self, minimized: bool);
}

/// Error from `configure` — typically a failed `ResizeBuffers` on Windows.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // variants constructed only on Windows; macOS surfaces never fail configure
pub enum ConfigureError {
    #[error("resize buffers failed: {0}")]
    ResizeBuffersFailed(i32),
    #[error("back buffer acquisition failed: {0}")]
    BackBufferFailed(i32),
}

/// Error from `present` — typically a failed `IDXGISwapChain::Present` on Windows.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // variant constructed only on Windows; macOS surfaces never fail present
pub enum PresentError {
    #[error("present failed: {0}")]
    PresentFailed(i32),
}

impl PresentError {
    /// Extract the HRESULT code for device-lost checking.
    pub fn hr(&self) -> i32 {
        match self {
            PresentError::PresentFailed(hr) => *hr,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // variants reserved for surface-acquire failure paths
pub enum FrameAcquireError {
    /// Swap chain is mid-resize / window resized between configure and
    /// acquire — drop the frame and reconfigure on the next tick.
    #[error("swap chain outdated")]
    Outdated,
    /// Window is occluded (DXGI_STATUS_OCCLUDED). Skip render this frame.
    #[error("window occluded")]
    Occluded,
    /// Window is currently minimized — caller should not render.
    #[error("window minimized")]
    Minimized,
    /// Frame acquire timed out.
    #[error("frame acquire timed out")]
    Timeout,
    /// Device is lost. Caller MUST drop and rebuild the entire Renderer.
    #[error("device lost: {0}")]
    DeviceLost(String),
    /// Other unrecoverable failure.
    #[error("acquire failed: {0}")]
    Failed(String),
}
