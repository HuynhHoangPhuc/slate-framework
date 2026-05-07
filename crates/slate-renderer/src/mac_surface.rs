//! macOS composition target wrapping wgpu::Surface (CAMetalLayer-backed).

#![cfg(target_os = "macos")]

use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use slate_platform::Window;
use wgpu::{
    Device, Instance, PresentMode, Surface, SurfaceConfiguration, SurfaceTargetUnsafe,
    TextureFormat, TextureUsages, TextureViewDescriptor,
};

use crate::RendererError;
use crate::surface_target::{
    AcquiredFrame, AcquiredFrameInner, CompositionTarget, FrameAcquireError,
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

        let (w, h) = window.size();
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
}

impl CompositionTarget for MacSurface {
    fn configure(&mut self, device: &Device, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(device, &self.config);
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

    fn present(&mut self, frame: AcquiredFrame) {
        match frame.inner {
            AcquiredFrameInner::Mac(tex) => tex.present(),
            #[cfg(target_os = "windows")]
            AcquiredFrameInner::Win { .. } => unreachable!(),
        }
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
