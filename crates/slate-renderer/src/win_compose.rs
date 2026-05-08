//! Windows composition target using DXGI swap chain + DirectComposition.
//!
//! Bypasses DWM's redirection bitmap for smooth drag-resize behavior.

#![cfg(target_os = "windows")]
#![allow(unsafe_op_in_unsafe_fn)]

use std::sync::Arc;

use raw_window_handle::RawWindowHandle;
use slate_platform::Window;
use wgpu::hal::api::Dx12;
use wgpu::{Device, TextureFormat, TextureViewDescriptor};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_FENCE_FLAG_NONE, D3D12_RESOURCE_BARRIER,
    D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
    D3D12_RESOURCE_BARRIER_FLAG_NONE, D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
    D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET,
    D3D12_RESOURCE_STATES, D3D12_RESOURCE_TRANSITION_BARRIER, ID3D12CommandAllocator,
    ID3D12CommandQueue, ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList, ID3D12Resource,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory2, IDXGISwapChain1, IDXGISwapChain3,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::Interface;

use crate::RendererError;
use crate::surface_target::{
    AcquiredFrame, AcquiredFrameInner, CompositionTarget, FrameAcquireError,
};

const BUFFER_COUNT: u32 = 3;

pub(crate) struct WinCompose {
    width: u32,
    height: u32,
    format: TextureFormat,
    is_minimized: bool,
    is_first_use: [bool; BUFFER_COUNT as usize],
    /// Per-back-buffer fence checkpoint. `fence_values[idx]` is the fence value
    /// Signal'd after the present that consumed that buffer.
    fence_values: [u64; BUFFER_COUNT as usize],
    /// Next fence value to assign on Signal — monotonic, never reset.
    next_fence_value: u64,
    back_buffer_textures: [Option<wgpu::Texture>; BUFFER_COUNT as usize],
    back_buffer_resources: [Option<ID3D12Resource>; BUFFER_COUNT as usize],
    barrier_cmd_list: ID3D12GraphicsCommandList,
    barrier_cmd_alloc: ID3D12CommandAllocator,
    fence: ID3D12Fence,
    fence_event: windows::Win32::Foundation::HANDLE,
    swap_chain: IDXGISwapChain3,
    #[allow(dead_code)] // kept alive for COM ownership
    comp_visual: IDCompositionVisual,
    #[allow(dead_code)] // kept alive for COM ownership
    comp_target: IDCompositionTarget,
    #[allow(dead_code)] // kept alive for COM ownership
    comp_device: IDCompositionDevice,
    raw_queue: ID3D12CommandQueue,
    #[allow(dead_code)] // kept alive for COM ownership
    raw_device: ID3D12Device,
    _window: Arc<dyn Window>,
}

impl WinCompose {
    pub fn new(device: &Device, window: Arc<dyn Window>) -> Result<Self, RendererError> {
        let hwnd = extract_hwnd(&*window)?;
        let (w, h) = window.size();

        let (raw_device, raw_queue) = unsafe {
            let hal_device_guard = device
                .as_hal::<Dx12>()
                .ok_or_else(|| RendererError::RawHandle("not a DX12 device".to_owned()))?;
            let raw_device = hal_device_guard.raw_device().clone();
            let raw_queue = hal_device_guard.raw_queue().clone();
            (raw_device, raw_queue)
        };

        let raw_factory: IDXGIFactory2 = unsafe {
            CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))
                .map_err(|e| RendererError::RawHandle(format!("CreateDXGIFactory2: {e}")))?
        };

        let barrier_cmd_alloc: ID3D12CommandAllocator = unsafe {
            raw_device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .map_err(|e| RendererError::RawHandle(format!("CreateCommandAllocator: {e}")))?
        };

        let barrier_cmd_list: ID3D12GraphicsCommandList = unsafe {
            let list: ID3D12GraphicsCommandList = raw_device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &barrier_cmd_alloc, None)
                .map_err(|e| RendererError::RawHandle(format!("CreateCommandList: {e}")))?;
            list.Close()
                .map_err(|e| RendererError::RawHandle(format!("Close command list: {e}")))?;
            list
        };

        let swap_chain_desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w.max(1),
            Height: h.max(1),
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: BUFFER_COUNT,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };

        let swap_chain: IDXGISwapChain3 = unsafe {
            let swap_chain1: IDXGISwapChain1 = raw_factory
                .CreateSwapChainForComposition(&raw_queue, &swap_chain_desc, None)
                .map_err(|e| {
                    RendererError::RawHandle(format!("CreateSwapChainForComposition: {e}"))
                })?;
            swap_chain1
                .cast()
                .map_err(|e| RendererError::RawHandle(format!("cast to IDXGISwapChain3: {e}")))?
        };

        let (comp_device, comp_target, comp_visual) = unsafe {
            let comp_device: IDCompositionDevice = DCompositionCreateDevice(None)
                .map_err(|e| RendererError::RawHandle(format!("DCompositionCreateDevice: {e}")))?;
            let comp_target: IDCompositionTarget = comp_device
                .CreateTargetForHwnd(hwnd, true)
                .map_err(|e| RendererError::RawHandle(format!("CreateTargetForHwnd: {e}")))?;
            let comp_visual: IDCompositionVisual = comp_device
                .CreateVisual()
                .map_err(|e| RendererError::RawHandle(format!("CreateVisual: {e}")))?;
            comp_visual
                .SetContent(&swap_chain)
                .map_err(|e| RendererError::RawHandle(format!("SetContent: {e}")))?;
            comp_target
                .SetRoot(&comp_visual)
                .map_err(|e| RendererError::RawHandle(format!("SetRoot: {e}")))?;
            comp_device
                .Commit()
                .map_err(|e| RendererError::RawHandle(format!("Commit: {e}")))?;
            (comp_device, comp_target, comp_visual)
        };

        let fence: ID3D12Fence = unsafe {
            raw_device
                .CreateFence(0, D3D12_FENCE_FLAG_NONE)
                .map_err(|e| RendererError::RawHandle(format!("CreateFence: {e}")))?
        };
        let fence_event = unsafe {
            CreateEventW(None, false, false, None)
                .map_err(|e| RendererError::RawHandle(format!("CreateEventW: {e}")))?
        };

        Ok(Self {
            width: w.max(1),
            height: h.max(1),
            format: TextureFormat::Bgra8UnormSrgb,
            is_minimized: false,
            is_first_use: [true; BUFFER_COUNT as usize],
            fence_values: [0; BUFFER_COUNT as usize],
            next_fence_value: 0,
            back_buffer_textures: [None, None, None],
            back_buffer_resources: [None, None, None],
            barrier_cmd_list,
            barrier_cmd_alloc,
            fence,
            fence_event,
            swap_chain,
            comp_visual,
            comp_target,
            comp_device,
            raw_queue,
            raw_device,
            _window: window,
        })
    }

    fn acquire_back_buffers(&mut self, device: &Device) {
        for i in 0..BUFFER_COUNT {
            let buffer: ID3D12Resource =
                unsafe { self.swap_chain.GetBuffer(i).expect("GetBuffer failed") };

            let hal_tex = unsafe {
                wgpu::hal::dx12::Device::texture_from_raw(
                    buffer.clone(),
                    wgpu::TextureFormat::Bgra8UnormSrgb,
                    wgpu::TextureDimension::D2,
                    wgpu::Extent3d {
                        width: self.width,
                        height: self.height,
                        depth_or_array_layers: 1,
                    },
                    1,
                    1,
                )
            };

            let desc = wgpu::TextureDescriptor {
                label: Some("composition-back-buffer"),
                size: wgpu::Extent3d {
                    width: self.width,
                    height: self.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            };

            let wgpu_tex = unsafe { device.create_texture_from_hal::<Dx12>(hal_tex, &desc) };
            self.back_buffer_resources[i as usize] = Some(buffer);
            self.back_buffer_textures[i as usize] = Some(wgpu_tex);
        }
        self.is_first_use = [true; BUFFER_COUNT as usize];
    }

    fn release_back_buffers(&mut self, _device: &Device) {
        // Release wgpu textures first, then D3D12 resources
        for tex in &mut self.back_buffer_textures {
            *tex = None;
        }
        for buffer in &mut self.back_buffer_resources {
            *buffer = None;
        }
    }

    /// Wait until the GPU has reached `target` on `self.fence`.
    fn wait_for_fence_value(&mut self, target: u64) {
        if target == 0 {
            return;
        }
        unsafe {
            if self.fence.GetCompletedValue() < target {
                self.fence
                    .SetEventOnCompletion(target, self.fence_event)
                    .ok();
                WaitForSingleObject(self.fence_event, INFINITE);
            }
        }
    }

    /// Signal a fresh fence value on the queue, return the value used.
    fn signal_next(&mut self) -> u64 {
        self.next_fence_value += 1;
        let v = self.next_fence_value;
        unsafe {
            self.raw_queue.Signal(&self.fence, v).ok();
        }
        v
    }

    /// Full flush — used on resize + Drop. Equivalent to old wait_for_gpu.
    fn flush(&mut self) {
        let v = self.signal_next();
        self.wait_for_fence_value(v);
    }
}

impl CompositionTarget for WinCompose {
    fn configure(&mut self, device: &Device, width: u32, height: u32) {
        let w = width.max(1);
        let h = height.max(1);
        if self.width == w && self.height == h && self.back_buffer_textures[0].is_some() {
            return;
        }

        // Match win-dcomp-spike resize flow: poll → wait_for_gpu → release → poll
        // 1. Poll wgpu to process pending callbacks
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });

        // 2. Wait for GPU to complete all queued work
        self.flush();

        // 3. Release back buffer references
        self.release_back_buffers(device);

        // 4. Poll again to ensure wgpu processes texture destruction
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });

        let result = unsafe {
            self.swap_chain.ResizeBuffers(
                BUFFER_COUNT,
                w,
                h,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            )
        };
        if let Err(e) = result {
            // ResizeBuffers can fail if DXGI still holds internal references to back buffers.
            // This is a known issue with wgpu + DirectComposition swap chains.
            // Continue with old size - compositor will stretch the content.
            // TODO: Investigate proper synchronization to fix resize.
            log::warn!("ResizeBuffers failed: {:?} - continuing with old size", e);
            self.acquire_back_buffers(device);
            return;
        }

        self.width = w;
        self.height = h;
        self.acquire_back_buffers(device);
        self.fence_values = [0; BUFFER_COUNT as usize];
    }

    fn acquire_frame(&mut self) -> Result<AcquiredFrame, FrameAcquireError> {
        if self.is_minimized {
            return Err(FrameAcquireError::Minimized);
        }

        let idx = unsafe { self.swap_chain.GetCurrentBackBufferIndex() as usize };

        // Early check that buffer exists
        if self.back_buffer_resources[idx].is_none() {
            return Err(FrameAcquireError::Failed(
                "back buffer not acquired".to_owned(),
            ));
        }

        let before_state = if self.is_first_use[idx] {
            D3D12_RESOURCE_STATE_COMMON
        } else {
            D3D12_RESOURCE_STATE_PRESENT
        };
        self.is_first_use[idx] = false;

        // Wait for this specific buffer to be GPU-idle before reusing
        self.wait_for_fence_value(self.fence_values[idx]);

        // Now borrow back_buffer after the mutable borrow of self is done
        let back_buffer = self.back_buffer_resources[idx].as_ref().unwrap();

        unsafe {
            self.barrier_cmd_alloc.Reset().ok();
            self.barrier_cmd_list
                .Reset(&self.barrier_cmd_alloc, None)
                .ok();
            let mut barrier = transition_barrier(
                back_buffer,
                before_state,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
            );
            self.barrier_cmd_list
                .ResourceBarrier(std::slice::from_ref(&barrier));
            self.barrier_cmd_list.Close().ok();
            let command_lists = [Some(self.barrier_cmd_list.cast().unwrap())];
            self.raw_queue.ExecuteCommandLists(&command_lists);
            std::mem::ManuallyDrop::drop(&mut barrier.Anonymous.Transition);
        }

        let texture = self.back_buffer_textures[idx]
            .as_ref()
            .ok_or_else(|| FrameAcquireError::Failed("wgpu texture not acquired".to_owned()))?;
        let view = texture.create_view(&TextureViewDescriptor::default());

        Ok(AcquiredFrame {
            view,
            inner: AcquiredFrameInner::Win {
                back_buffer_index: idx as u32,
            },
        })
    }

    fn present(&mut self, frame: AcquiredFrame) {
        if self.is_minimized {
            return;
        }

        let idx = match frame.inner {
            #[cfg(target_os = "macos")]
            AcquiredFrameInner::Mac(_) => unreachable!(),
            AcquiredFrameInner::Win {
                back_buffer_index, ..
            } => back_buffer_index as usize,
        };

        let back_buffer = match self.back_buffer_resources[idx].as_ref() {
            Some(b) => b,
            None => return,
        };

        unsafe {
            self.barrier_cmd_alloc.Reset().ok();
            self.barrier_cmd_list
                .Reset(&self.barrier_cmd_alloc, None)
                .ok();
            let mut barrier = transition_barrier(
                back_buffer,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_PRESENT,
            );
            self.barrier_cmd_list
                .ResourceBarrier(std::slice::from_ref(&barrier));
            self.barrier_cmd_list.Close().ok();
            let command_lists = [Some(self.barrier_cmd_list.cast().unwrap())];
            self.raw_queue.ExecuteCommandLists(&command_lists);
            let _ = self.swap_chain.Present(1, DXGI_PRESENT(0));
            std::mem::ManuallyDrop::drop(&mut barrier.Anonymous.Transition);
        }
        // Signal and immediately wait for fence - ensures GPU is idle after each frame.
        // This is required for reliable ResizeBuffers on composition swap chains.
        let fence_val = self.signal_next();
        self.wait_for_fence_value(fence_val);
        self.fence_values[idx] = fence_val;
    }

    fn format(&self) -> TextureFormat {
        self.format
    }

    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn set_minimized(&mut self, minimized: bool) {
        self.is_minimized = minimized;
    }
}

impl Drop for WinCompose {
    fn drop(&mut self) {
        self.flush();
        unsafe {
            windows::Win32::Foundation::CloseHandle(self.fence_event).ok();
        }
    }
}

fn extract_hwnd(window: &dyn Window) -> Result<HWND, RendererError> {
    let handle = window
        .window_handle()
        .map_err(|e| RendererError::RawHandle(e.to_string()))?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => Ok(HWND(h.hwnd.get() as *mut _)),
        _ => Err(RendererError::RawHandle(
            "expected Win32 window handle".to_owned(),
        )),
    }
}

fn transition_barrier(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: std::mem::ManuallyDrop::new(Some(resource.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}
