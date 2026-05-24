//! macOS composition target wrapping wgpu::Surface (CAMetalLayer-backed).

#![cfg(target_os = "macos")]

use std::sync::Arc;

use objc2_quartz_core::CATransaction;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use slate_platform::Window;
use wgpu::{
    Device, Instance, PollType, PresentMode, Surface, SurfaceConfiguration, SurfaceTargetUnsafe,
    TextureFormat, TextureUsages, TextureViewDescriptor,
};

use crate::RendererError;
use crate::surface_target::{
    AcquiredFrame, AcquiredFrameInner, CompositionTarget, ConfigureError, FrameAcquireError,
    PresentError,
};

pub(crate) struct MacSurface {
    surface: Surface<'static>,
    config: SurfaceConfiguration,
    _window: Arc<dyn Window>,
}

impl MacSurface {
    pub fn new(
        instance: &Instance,
        adapter: &wgpu::Adapter,
        window: Arc<dyn Window>,
    ) -> Result<Self, RendererError> {
        let surface_target = {
            let raw_window = window
                .window_handle()
                .map_err(|e| RendererError::RawHandle(e.to_string()))?
                .as_raw();
            let raw_display = window
                .display_handle()
                .map_err(|e| RendererError::RawHandle(e.to_string()))?
                .as_raw();
            SurfaceTargetUnsafe::RawHandle {
                raw_window_handle: raw_window,
                raw_display_handle: Some(raw_display),
            }
        };
        let surface: Surface<'static> = unsafe { instance.create_surface_unsafe(surface_target)? };

        let caps = surface.get_capabilities(adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| *f == TextureFormat::Bgra8UnormSrgb)
            .unwrap_or(caps.formats[0]);

        let (w, h) = window.physical_size();
        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: w.max(1),
            height: h.max(1),
            present_mode: PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 3,
        };

        Ok(Self {
            surface,
            config,
            _window: window,
        })
    }

    /// Present a previously acquired frame INSIDE AppKit's currently open
    /// CATransaction (the one opened by `setFrameSize:` for the current drag
    /// tick). Drains pending GPU work non-blocking, submits the Metal present,
    /// then `CATransaction::flush` commits the open transaction with the new
    /// pixels in place — closing the wrong-side-of-transaction race that
    /// otherwise lets WindowServer composite the new bounds with stale
    /// drawable contents.
    pub fn present_sync(
        &mut self,
        frame: AcquiredFrame,
        device: &Device,
    ) -> Result<(), PresentError> {
        // Drain submitted command-buffer callbacks without blocking. Cheap
        // when there's no work queued; matches Tristan-Hume / Zed pattern
        // step 1. Escalation tree (Wait, then [commandBuffer waitUntilScheduled]
        // via wgpu-hal) if residual flicker appears.
        let _ = device.poll(PollType::Poll);
        match frame.inner {
            AcquiredFrameInner::Mac(tex) => tex.present(),
            #[cfg(target_os = "windows")]
            AcquiredFrameInner::Win { .. } => unreachable!(),
        }
        // Force AppKit's currently open implicit transaction to commit
        // immediately, landing the new framebuffer in the same transaction
        // as the bounds change. NOT `begin()+commit()` — that would open a
        // separate transaction and reproduce the bug.
        CATransaction::flush();
        Ok(())
    }
}

impl CompositionTarget for MacSurface {
    fn configure(
        &mut self,
        device: &Device,
        width: u32,
        height: u32,
    ) -> Result<(), ConfigureError> {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(device, &self.config);
        Ok(())
    }

    fn acquire_frame(&mut self) -> Result<AcquiredFrame, FrameAcquireError> {
        use wgpu::CurrentSurfaceTexture;
        match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(tex) | CurrentSurfaceTexture::Suboptimal(tex) => {
                let view = tex.texture.create_view(&TextureViewDescriptor::default());
                Ok(AcquiredFrame {
                    view,
                    inner: AcquiredFrameInner::Mac(tex),
                })
            }
            CurrentSurfaceTexture::Outdated => Err(FrameAcquireError::Outdated),
            CurrentSurfaceTexture::Lost => {
                Err(FrameAcquireError::DeviceLost("surface lost".to_owned()))
            }
            CurrentSurfaceTexture::Occluded => Err(FrameAcquireError::Occluded),
            CurrentSurfaceTexture::Timeout => Err(FrameAcquireError::Timeout),
            CurrentSurfaceTexture::Validation => {
                Err(FrameAcquireError::Failed("validation error".to_owned()))
            }
        }
    }

    fn present(&mut self, frame: AcquiredFrame) -> Result<(), PresentError> {
        match frame.inner {
            AcquiredFrameInner::Mac(tex) => tex.present(),
            #[cfg(target_os = "windows")]
            AcquiredFrameInner::Win { .. } => unreachable!(),
        }
        Ok(())
    }

    fn format(&self) -> TextureFormat {
        self.config.format
    }

    fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    fn set_minimized(&mut self, _minimized: bool) {
        // No-op on macOS - Surface handles occlusion automatically
    }
}
