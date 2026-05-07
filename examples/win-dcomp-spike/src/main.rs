#[cfg(not(target_os = "windows"))]
fn main() {
    compile_error!("win-dcomp-spike is Windows-only");
}

#[cfg(target_os = "windows")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let use_wgpu = args.iter().any(|a| a == "--wgpu");

    if use_wgpu {
        println!("Running with wgpu backend...");
        wgpu_spike::run();
    } else {
        println!("Running with raw D3D12 backend (use --wgpu for wgpu)...");
        d3d12_spike::run();
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_op_in_unsafe_fn)]
mod d3d12_spike {
    use std::cell::RefCell;
    use std::mem::{size_of, zeroed};

    use windows::core::{w, Interface, PCWSTR};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Direct3D12::{
        D3D12CreateDevice, ID3D12CommandAllocator, ID3D12CommandQueue, ID3D12DescriptorHeap,
        ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList, ID3D12Resource,
        D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_CPU_DESCRIPTOR_HANDLE,
        D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_FENCE_FLAG_NONE,
        D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
        D3D12_RESOURCE_BARRIER_FLAG_NONE, D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        D3D12_RESOURCE_STATES, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_PRESENT,
        D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_TRANSITION_BARRIER,
    };
    use windows::Win32::Graphics::DirectComposition::{
        DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
    };
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
    };
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory2, IDXGIFactory6, IDXGISwapChain1, IDXGISwapChain3,
        DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
        DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    };
    use windows::Win32::Graphics::Gdi::{ValidateRect, HBRUSH};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW,
        GetClientRect, GetMessageW, HCURSOR, HICON, IDC_ARROW, KillTimer, LoadCursorW, MSG,
        PostQuitMessage, RegisterClassExW, SIZE_MINIMIZED, SWP_NOCOPYBITS,
        SetTimer, TranslateMessage, USER_TIMER_MINIMUM, WINDOWPOS, WM_DESTROY, WM_ENTERSIZEMOVE,
        WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_PAINT, WM_SIZE, WM_TIMER, WM_WINDOWPOSCHANGING,
        WNDCLASSEXW, WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    const BUFFER_COUNT: u32 = 3;
    const SIZE_MOVE_TIMER_ID: usize = 1;
    const CLEAR_COLOR: [f32; 4] = [0.05, 0.10, 0.20, 1.0];

    struct Renderer {
        command_queue: ID3D12CommandQueue,
        command_allocator: ID3D12CommandAllocator,
        command_list: ID3D12GraphicsCommandList,
        swap_chain: IDXGISwapChain3,
        rtv_heap: ID3D12DescriptorHeap,
        rtv_descriptor_size: u32,
        back_buffers: [Option<ID3D12Resource>; BUFFER_COUNT as usize],
        is_first_use: [bool; BUFFER_COUNT as usize],
        fence: ID3D12Fence,
        fence_value: u64,
        fence_event: windows::Win32::Foundation::HANDLE,
        width: u32,
        height: u32,
        device: ID3D12Device,
        _comp_device: IDCompositionDevice,
        _comp_target: IDCompositionTarget,
        _comp_visual: IDCompositionVisual,
    }

    impl Renderer {
        unsafe fn new(hwnd: HWND, width: u32, height: u32) -> Self {
            let device: ID3D12Device = {
                let mut device = None;
                D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut device)
                    .expect("D3D12CreateDevice failed");
                device.unwrap()
            };

            let command_queue: ID3D12CommandQueue = {
                let desc = D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                };
                device
                    .CreateCommandQueue(&desc)
                    .expect("CreateCommandQueue failed")
            };

            let command_allocator: ID3D12CommandAllocator = device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .expect("CreateCommandAllocator failed");

            let command_list: ID3D12GraphicsCommandList = {
                let list: ID3D12GraphicsCommandList = device
                    .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &command_allocator, None)
                    .expect("CreateCommandList failed");
                list.Close().expect("Close command list failed");
                list
            };

            let factory: IDXGIFactory6 =
                CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).expect("CreateDXGIFactory2 failed");

            let swap_chain_desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
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

            let swap_chain: IDXGISwapChain3 = {
                let swap_chain1: IDXGISwapChain1 = factory
                    .CreateSwapChainForComposition(&command_queue, &swap_chain_desc, None)
                    .expect("CreateSwapChainForComposition failed");
                swap_chain1.cast().expect("cast to IDXGISwapChain3")
            };

            let (comp_device, comp_target, comp_visual) = {
                let comp_device: IDCompositionDevice =
                    DCompositionCreateDevice(None).expect("DCompositionCreateDevice failed");
                let comp_target: IDCompositionTarget = comp_device
                    .CreateTargetForHwnd(hwnd, true)
                    .expect("CreateTargetForHwnd failed");
                let comp_visual: IDCompositionVisual =
                    comp_device.CreateVisual().expect("CreateVisual failed");

                comp_visual
                    .SetContent(&swap_chain)
                    .expect("SetContent failed");
                comp_target.SetRoot(&comp_visual).expect("SetRoot failed");
                comp_device.Commit().expect("Commit failed");

                (comp_device, comp_target, comp_visual)
            };

            let rtv_heap: ID3D12DescriptorHeap = {
                let desc = D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                    NumDescriptors: BUFFER_COUNT,
                    ..Default::default()
                };
                device
                    .CreateDescriptorHeap(&desc)
                    .expect("CreateDescriptorHeap failed")
            };
            let rtv_descriptor_size =
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV);

            let fence: ID3D12Fence = device
                .CreateFence(0, D3D12_FENCE_FLAG_NONE)
                .expect("CreateFence failed");
            let fence_event = CreateEventW(None, false, false, None).expect("CreateEventW failed");

            let mut renderer = Self {
                device,
                command_queue,
                command_allocator,
                command_list,
                swap_chain,
                rtv_heap,
                rtv_descriptor_size,
                back_buffers: [None, None, None],
                is_first_use: [true; BUFFER_COUNT as usize],
                fence,
                fence_value: 0,
                fence_event,
                width,
                height,
                _comp_device: comp_device,
                _comp_target: comp_target,
                _comp_visual: comp_visual,
            };
            renderer.acquire_back_buffers();
            renderer
        }

        unsafe fn acquire_back_buffers(&mut self) {
            let rtv_start = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
            for i in 0..BUFFER_COUNT {
                let buffer: ID3D12Resource = self.swap_chain.GetBuffer(i).expect("GetBuffer failed");
                let rtv_handle = D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: rtv_start.ptr + (i * self.rtv_descriptor_size) as usize,
                };
                self.device
                    .CreateRenderTargetView(&buffer, None, rtv_handle);
                self.back_buffers[i as usize] = Some(buffer);
            }
            self.is_first_use = [true; BUFFER_COUNT as usize];
        }

        fn release_back_buffers(&mut self) {
            for buffer in &mut self.back_buffers {
                *buffer = None;
            }
        }

        unsafe fn wait_for_gpu(&mut self) {
            self.fence_value += 1;
            self.command_queue
                .Signal(&self.fence, self.fence_value)
                .expect("Signal failed");
            if self.fence.GetCompletedValue() < self.fence_value {
                self.fence
                    .SetEventOnCompletion(self.fence_value, self.fence_event)
                    .expect("SetEventOnCompletion failed");
                WaitForSingleObject(self.fence_event, INFINITE);
            }
        }

        unsafe fn resize(&mut self, width: u32, height: u32) {
            if width == 0 || height == 0 {
                return;
            }
            self.wait_for_gpu();
            self.release_back_buffers();

            self.swap_chain
                .ResizeBuffers(
                    BUFFER_COUNT,
                    width,
                    height,
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
                .expect("ResizeBuffers failed");

            self.width = width;
            self.height = height;
            self.acquire_back_buffers();
        }

        unsafe fn render(&mut self) {
            if self.width == 0 || self.height == 0 {
                return;
            }

            let back_buffer_idx = self.swap_chain.GetCurrentBackBufferIndex() as usize;
            let back_buffer = self.back_buffers[back_buffer_idx]
                .as_ref()
                .expect("back buffer missing");

            self.command_allocator
                .Reset()
                .expect("Reset allocator failed");
            self.command_list
                .Reset(&self.command_allocator, None)
                .expect("Reset command list failed");

            let before_state = if self.is_first_use[back_buffer_idx] {
                D3D12_RESOURCE_STATE_COMMON
            } else {
                D3D12_RESOURCE_STATE_PRESENT
            };
            self.is_first_use[back_buffer_idx] = false;

            let barrier_to_rt =
                transition_barrier(back_buffer, before_state, D3D12_RESOURCE_STATE_RENDER_TARGET);
            self.command_list.ResourceBarrier(&[barrier_to_rt]);

            let rtv_start = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
            let rtv_handle = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: rtv_start.ptr + (back_buffer_idx as u32 * self.rtv_descriptor_size) as usize,
            };
            self.command_list
                .ClearRenderTargetView(rtv_handle, &CLEAR_COLOR, None);

            let barrier_to_present = transition_barrier(
                back_buffer,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_PRESENT,
            );
            self.command_list.ResourceBarrier(&[barrier_to_present]);

            self.command_list.Close().expect("Close command list failed");

            let command_lists = [Some(self.command_list.cast().unwrap())];
            self.command_queue.ExecuteCommandLists(&command_lists);

            let _ = self.swap_chain.Present(1, DXGI_PRESENT(0));

            self.wait_for_gpu();
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
                    pResource: unsafe { std::mem::transmute_copy(resource) },
                    Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                    StateBefore: before,
                    StateAfter: after,
                }),
            },
        }
    }

    thread_local! {
        static RENDERER: RefCell<Option<Renderer>> = const { RefCell::new(None) };
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_SIZE => {
                let size_type = wparam.0 as u32;
                if size_type == SIZE_MINIMIZED {
                    return LRESULT(0);
                }
                let width = (lparam.0 & 0xFFFF) as u32;
                let height = ((lparam.0 >> 16) & 0xFFFF) as u32;
                RENDERER.with(|r| {
                    if let Some(renderer) = r.borrow_mut().as_mut() {
                        renderer.resize(width, height);
                    }
                });
                LRESULT(0)
            }
            WM_PAINT | WM_TIMER => {
                RENDERER.with(|r| {
                    if let Some(renderer) = r.borrow_mut().as_mut() {
                        renderer.render();
                    }
                });
                let _ = ValidateRect(Some(hwnd), None);
                LRESULT(0)
            }
            WM_ENTERSIZEMOVE => {
                let _ = SetTimer(Some(hwnd), SIZE_MOVE_TIMER_ID, USER_TIMER_MINIMUM, None);
                LRESULT(0)
            }
            WM_EXITSIZEMOVE => {
                let _ = KillTimer(Some(hwnd), SIZE_MOVE_TIMER_ID);
                LRESULT(0)
            }
            WM_WINDOWPOSCHANGING => {
                let pos = lparam.0 as *mut WINDOWPOS;
                (*pos).flags |= SWP_NOCOPYBITS;
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    pub fn run() {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

            let hinstance: HINSTANCE =
                GetModuleHandleW(None).expect("GetModuleHandleW failed").into();
            let class_name = w!("WinDCompSpikeClass");

            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: HICON::default(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or(HCURSOR::default()),
                hbrBackground: HBRUSH::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: class_name,
                hIconSm: HICON::default(),
            };

            let atom = RegisterClassExW(&wc);
            assert!(atom != 0, "RegisterClassExW failed");

            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                class_name,
                w!("DComp Spike - D3D12 Clear Color"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                1280,
                800,
                None,
                None,
                Some(hinstance),
                None,
            )
            .expect("CreateWindowExW failed");

            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect).expect("GetClientRect failed");
            let width = (rect.right - rect.left) as u32;
            let height = (rect.bottom - rect.top) as u32;

            RENDERER.with(|r| {
                *r.borrow_mut() = Some(Renderer::new(hwnd, width, height));
            });

            RENDERER.with(|r| {
                if let Some(renderer) = r.borrow_mut().as_mut() {
                    renderer.render();
                }
            });

            let mut msg: MSG = zeroed();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            RENDERER.with(|r| {
                if let Some(renderer) = r.borrow_mut().as_mut() {
                    renderer.wait_for_gpu();
                }
                *r.borrow_mut() = None;
            });
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_op_in_unsafe_fn)]
mod wgpu_spike {
    use std::cell::RefCell;
    use std::mem::{size_of, zeroed};

    use wgpu::hal::api::Dx12;
    use windows::core::{w, Interface, PCWSTR};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Direct3D12::{
        ID3D12CommandAllocator, ID3D12CommandQueue, ID3D12Fence, ID3D12GraphicsCommandList,
        ID3D12Resource, D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_FENCE_FLAG_NONE,
        D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
        D3D12_RESOURCE_BARRIER_FLAG_NONE, D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        D3D12_RESOURCE_STATES, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_PRESENT,
        D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_TRANSITION_BARRIER,
    };
    use windows::Win32::Graphics::DirectComposition::{
        DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
    };
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
    };
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory2, IDXGIFactory2, IDXGISwapChain1, IDXGISwapChain3,
        DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
        DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    };
    use windows::Win32::Graphics::Gdi::{ValidateRect, HBRUSH};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW,
        GetClientRect, GetMessageW, HCURSOR, HICON, IDC_ARROW, KillTimer, LoadCursorW, MSG,
        PostQuitMessage, RegisterClassExW, SIZE_MINIMIZED, SWP_NOCOPYBITS,
        SetTimer, TranslateMessage, USER_TIMER_MINIMUM, WINDOWPOS, WM_DESTROY, WM_ENTERSIZEMOVE,
        WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_PAINT, WM_SIZE, WM_TIMER, WM_WINDOWPOSCHANGING,
        WNDCLASSEXW, WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    const BUFFER_COUNT: u32 = 3;
    const SIZE_MOVE_TIMER_ID: usize = 1;
    const CLEAR_COLOR: wgpu::Color = wgpu::Color {
        r: 0.05,
        g: 0.10,
        b: 0.20,
        a: 1.0,
    };

    struct WgpuRenderer {
        wgpu_device: wgpu::Device,
        wgpu_queue: wgpu::Queue,
        raw_queue: ID3D12CommandQueue,
        barrier_allocator: ID3D12CommandAllocator,
        barrier_list: ID3D12GraphicsCommandList,
        swap_chain: IDXGISwapChain3,
        back_buffers: [Option<ID3D12Resource>; BUFFER_COUNT as usize],
        wgpu_textures: [Option<wgpu::Texture>; BUFFER_COUNT as usize],
        is_first_use: [bool; BUFFER_COUNT as usize],
        fence: ID3D12Fence,
        fence_value: u64,
        fence_event: windows::Win32::Foundation::HANDLE,
        width: u32,
        height: u32,
        _comp_device: IDCompositionDevice,
        _comp_target: IDCompositionTarget,
        _comp_visual: IDCompositionVisual,
    }

    impl WgpuRenderer {
        fn new(hwnd: HWND, width: u32, height: u32) -> Self {
            let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
            instance_desc.backends = wgpu::Backends::DX12;
            let instance = wgpu::Instance::new(instance_desc);

            let adapter =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                }))
                .expect("No adapter found");

            let (wgpu_device, wgpu_queue) = pollster::block_on(adapter.request_device(
                &wgpu::DeviceDescriptor::default(),
            ))
            .expect("Failed to create device");

            let (raw_device, raw_queue) = unsafe {
                let hal_device_guard = wgpu_device.as_hal::<Dx12>().expect("dx12 device");
                let raw_device = hal_device_guard.raw_device().clone();
                let raw_queue = hal_device_guard.raw_queue().clone();
                (raw_device, raw_queue)
            };

            let raw_factory: IDXGIFactory2 =
                unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).expect("CreateDXGIFactory2 failed") };

            let barrier_allocator: ID3D12CommandAllocator = unsafe {
                raw_device
                    .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                    .expect("CreateCommandAllocator failed")
            };

            let barrier_list: ID3D12GraphicsCommandList = unsafe {
                let list: ID3D12GraphicsCommandList = raw_device
                    .CreateCommandList(
                        0,
                        D3D12_COMMAND_LIST_TYPE_DIRECT,
                        &barrier_allocator,
                        None,
                    )
                    .expect("CreateCommandList failed");
                list.Close().expect("Close command list failed");
                list
            };

            let swap_chain_desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
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
                    .expect("CreateSwapChainForComposition failed");
                swap_chain1.cast().expect("cast to IDXGISwapChain3")
            };

            let (comp_device, comp_target, comp_visual) = unsafe {
                let comp_device: IDCompositionDevice =
                    DCompositionCreateDevice(None).expect("DCompositionCreateDevice failed");
                let comp_target: IDCompositionTarget = comp_device
                    .CreateTargetForHwnd(hwnd, true)
                    .expect("CreateTargetForHwnd failed");
                let comp_visual: IDCompositionVisual =
                    comp_device.CreateVisual().expect("CreateVisual failed");

                comp_visual
                    .SetContent(&swap_chain)
                    .expect("SetContent failed");
                comp_target.SetRoot(&comp_visual).expect("SetRoot failed");
                comp_device.Commit().expect("Commit failed");

                (comp_device, comp_target, comp_visual)
            };

            let fence: ID3D12Fence = unsafe {
                raw_device
                    .CreateFence(0, D3D12_FENCE_FLAG_NONE)
                    .expect("CreateFence failed")
            };
            let fence_event =
                unsafe { CreateEventW(None, false, false, None).expect("CreateEventW failed") };

            let mut renderer = Self {
                wgpu_device,
                wgpu_queue,
                raw_queue,
                barrier_allocator,
                barrier_list,
                swap_chain,
                back_buffers: [None, None, None],
                wgpu_textures: [None, None, None],
                is_first_use: [true; BUFFER_COUNT as usize],
                fence,
                fence_value: 0,
                fence_event,
                width,
                height,
                _comp_device: comp_device,
                _comp_target: comp_target,
                _comp_visual: comp_visual,
            };
            renderer.acquire_back_buffers();
            renderer
        }

        fn acquire_back_buffers(&mut self) {
            for i in 0..BUFFER_COUNT {
                let buffer: ID3D12Resource =
                    unsafe { self.swap_chain.GetBuffer(i).expect("GetBuffer failed") };

                let hal_tex = unsafe {
                    wgpu::hal::dx12::Device::texture_from_raw(
                        buffer.clone(),
                        wgpu::TextureFormat::Bgra8Unorm,
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
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                };

                let wgpu_tex =
                    unsafe { self.wgpu_device.create_texture_from_hal::<Dx12>(hal_tex, &desc) };

                self.back_buffers[i as usize] = Some(buffer);
                self.wgpu_textures[i as usize] = Some(wgpu_tex);
            }
            self.is_first_use = [true; BUFFER_COUNT as usize];
        }

        fn release_back_buffers(&mut self) {
            for tex in &mut self.wgpu_textures {
                *tex = None;
            }
            for buffer in &mut self.back_buffers {
                *buffer = None;
            }
        }

        fn wait_for_gpu(&mut self) {
            self.fence_value += 1;
            unsafe {
                self.raw_queue
                    .Signal(&self.fence, self.fence_value)
                    .expect("Signal failed");
                if self.fence.GetCompletedValue() < self.fence_value {
                    self.fence
                        .SetEventOnCompletion(self.fence_value, self.fence_event)
                        .expect("SetEventOnCompletion failed");
                    WaitForSingleObject(self.fence_event, INFINITE);
                }
            }
        }

        fn resize(&mut self, width: u32, height: u32) {
            if width == 0 || height == 0 {
                return;
            }

            // Wait for any pending wgpu work
            let _ = self.wgpu_device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
            // Wait for GPU to complete all queued work
            self.wait_for_gpu();
            // Release wgpu texture wrappers and D3D12 resources
            self.release_back_buffers();
            // Poll again to ensure wgpu processes texture destruction
            let _ = self.wgpu_device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });

            let result = unsafe {
                self.swap_chain.ResizeBuffers(
                    BUFFER_COUNT,
                    width,
                    height,
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
            };

            if let Err(e) = result {
                eprintln!("ResizeBuffers failed: {:?} - will retry", e);
                let _ = self.wgpu_device.poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                });
                self.wait_for_gpu();
                unsafe {
                    self.swap_chain
                        .ResizeBuffers(
                            BUFFER_COUNT,
                            width,
                            height,
                            DXGI_FORMAT_B8G8R8A8_UNORM,
                            DXGI_SWAP_CHAIN_FLAG(0),
                        )
                        .expect("ResizeBuffers failed on retry");
                }
            }

            self.width = width;
            self.height = height;
            self.acquire_back_buffers();
        }

        fn render(&mut self) {
            if self.width == 0 || self.height == 0 {
                return;
            }

            let back_buffer_idx = unsafe { self.swap_chain.GetCurrentBackBufferIndex() as usize };
            let back_buffer = self.back_buffers[back_buffer_idx]
                .as_ref()
                .expect("back buffer missing");

            let before_state = if self.is_first_use[back_buffer_idx] {
                D3D12_RESOURCE_STATE_COMMON
            } else {
                D3D12_RESOURCE_STATE_PRESENT
            };
            self.is_first_use[back_buffer_idx] = false;

            unsafe {
                self.barrier_allocator
                    .Reset()
                    .expect("Reset allocator failed");
                self.barrier_list
                    .Reset(&self.barrier_allocator, None)
                    .expect("Reset command list failed");

                let barrier_to_rt = transition_barrier(
                    back_buffer,
                    before_state,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                );
                self.barrier_list.ResourceBarrier(&[barrier_to_rt]);
                self.barrier_list
                    .Close()
                    .expect("Close command list failed");

                let command_lists = [Some(self.barrier_list.cast().unwrap())];
                self.raw_queue.ExecuteCommandLists(&command_lists);
            }

            let wgpu_tex = self.wgpu_textures[back_buffer_idx]
                .as_ref()
                .expect("wgpu texture missing");
            let view = wgpu_tex.create_view(&wgpu::TextureViewDescriptor::default());

            let mut encoder = self
                .wgpu_device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("render-encoder"),
                });

            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("clear-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            }

            self.wgpu_queue.submit(std::iter::once(encoder.finish()));

            unsafe {
                self.barrier_allocator
                    .Reset()
                    .expect("Reset allocator failed");
                self.barrier_list
                    .Reset(&self.barrier_allocator, None)
                    .expect("Reset command list failed");

                let barrier_to_present = transition_barrier(
                    back_buffer,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                    D3D12_RESOURCE_STATE_PRESENT,
                );
                self.barrier_list.ResourceBarrier(&[barrier_to_present]);
                self.barrier_list
                    .Close()
                    .expect("Close command list failed");

                let command_lists = [Some(self.barrier_list.cast().unwrap())];
                self.raw_queue.ExecuteCommandLists(&command_lists);

                let _ = self.swap_chain.Present(1, DXGI_PRESENT(0));
            }

            self.wait_for_gpu();
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
                    pResource: unsafe { std::mem::transmute_copy(resource) },
                    Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                    StateBefore: before,
                    StateAfter: after,
                }),
            },
        }
    }

    thread_local! {
        static RENDERER: RefCell<Option<WgpuRenderer>> = const { RefCell::new(None) };
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_SIZE => {
                let size_type = wparam.0 as u32;
                if size_type == SIZE_MINIMIZED {
                    return LRESULT(0);
                }
                let width = (lparam.0 & 0xFFFF) as u32;
                let height = ((lparam.0 >> 16) & 0xFFFF) as u32;
                RENDERER.with(|r| {
                    if let Some(renderer) = r.borrow_mut().as_mut() {
                        renderer.resize(width, height);
                    }
                });
                LRESULT(0)
            }
            WM_PAINT | WM_TIMER => {
                RENDERER.with(|r| {
                    if let Some(renderer) = r.borrow_mut().as_mut() {
                        renderer.render();
                    }
                });
                let _ = ValidateRect(Some(hwnd), None);
                LRESULT(0)
            }
            WM_ENTERSIZEMOVE => {
                let _ = SetTimer(Some(hwnd), SIZE_MOVE_TIMER_ID, USER_TIMER_MINIMUM, None);
                LRESULT(0)
            }
            WM_EXITSIZEMOVE => {
                let _ = KillTimer(Some(hwnd), SIZE_MOVE_TIMER_ID);
                LRESULT(0)
            }
            WM_WINDOWPOSCHANGING => {
                let pos = lparam.0 as *mut WINDOWPOS;
                (*pos).flags |= SWP_NOCOPYBITS;
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    pub fn run() {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

            let hinstance: HINSTANCE =
                GetModuleHandleW(None).expect("GetModuleHandleW failed").into();
            let class_name = w!("WinDCompWgpuSpikeClass");

            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: HICON::default(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or(HCURSOR::default()),
                hbrBackground: HBRUSH::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: class_name,
                hIconSm: HICON::default(),
            };

            let atom = RegisterClassExW(&wc);
            assert!(atom != 0, "RegisterClassExW failed");

            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                class_name,
                w!("DComp Spike - wgpu Clear Color"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                1280,
                800,
                None,
                None,
                Some(hinstance),
                None,
            )
            .expect("CreateWindowExW failed");

            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect).expect("GetClientRect failed");
            let width = (rect.right - rect.left) as u32;
            let height = (rect.bottom - rect.top) as u32;

            RENDERER.with(|r| {
                *r.borrow_mut() = Some(WgpuRenderer::new(hwnd, width, height));
            });

            RENDERER.with(|r| {
                if let Some(renderer) = r.borrow_mut().as_mut() {
                    renderer.render();
                }
            });

            let mut msg: MSG = zeroed();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            RENDERER.with(|r| {
                if let Some(renderer) = r.borrow_mut().as_mut() {
                    renderer.wait_for_gpu();
                }
                *r.borrow_mut() = None;
            });
        }
    }
}
