//! Win32 message loop — wndproc trampoline and WinWindowInner message handling.

use std::cell::{Cell, RefCell};
use std::rc::Weak;
use std::sync::Arc;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dxgi::IDXGIFactory2;
use windows::Win32::Graphics::Gdi::{
    HMONITOR, InvalidateRect, MONITOR_DEFAULTTONULL, MonitorFromWindow, ScreenToClient,
    ValidateRect,
};
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT};
use windows::Win32::UI::Controls::{HOVER_DEFAULT, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::Ime::{GCS_COMPSTR, GCS_RESULTSTR};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_LWIN,
    VK_MENU, VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, DefWindowProcW, GWLP_USERDATA, GetClientRect, GetWindowLongPtrW, KillTimer,
    MINMAXINFO, MSG, PM_REMOVE, PeekMessageW, PostQuitMessage, SIZE_MINIMIZED, SWP_NOACTIVATE,
    SWP_NOZORDER, SetTimer, SetWindowLongPtrW, SetWindowPos, USER_TIMER_MINIMUM, WM_CAPTURECHANGED,
    WM_CHAR, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ENTERSIZEMOVE,
    WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION,
    WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDBLCLK, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SIZE,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WM_UNICHAR, WM_WINDOWPOSCHANGED, WM_XBUTTONDBLCLK,
    WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use super::ime::{
    ImmContextGuard, find_target_converted_run, read_composition_attrs, read_composition_cursor,
    read_composition_string_utf8, set_composition_window, utf16_units_to_utf8_bytes,
};
use super::keymap;
use super::{
    IN_SIZE_MOVE, REDRAW_TIMER_ID, SIZE_MOVE_TIMER_ID, WM_APP_WAKE, clear_wake_hwnd, dispatch_event,
};
use crate::{
    Event, Key, Modifiers, MouseButton, PhysicalSize, WindowId, WindowImeDelegate,
    WindowRenderDelegate,
};

// ---------------------------------------------------------------------------
// Mouse event decode helpers
// ---------------------------------------------------------------------------

/// Decode mouse position from lparam with i16 sign extension.
/// Divide by scale to get logical coordinates.
fn decode_pos(lparam: LPARAM, scale: f32) -> (f32, f32) {
    let raw = lparam.0 as i32 as u32;
    let x = (raw & 0xFFFF) as i16 as f32 / scale;
    let y = ((raw >> 16) & 0xFFFF) as i16 as f32 / scale;
    (x, y)
}

/// Decode modifier keys from wparam and GetKeyState.
fn decode_modifiers(wparam: WPARAM) -> Modifiers {
    let w = wparam.0 as u32;
    Modifiers {
        shift: w & MK_SHIFT.0 != 0,
        ctrl: w & MK_CONTROL.0 != 0,
        alt: unsafe { GetKeyState(VK_MENU.0 as i32) } < 0,
        meta: unsafe { GetKeyState(VK_LWIN.0 as i32) } < 0
            || unsafe { GetKeyState(VK_RWIN.0 as i32) } < 0,
    }
}

/// Button bit positions for capture tracking.
const BUTTON_BIT_LEFT: u8 = 1 << 0;
const BUTTON_BIT_RIGHT: u8 = 1 << 1;
const BUTTON_BIT_MIDDLE: u8 = 1 << 2;
const BUTTON_BIT_X1: u8 = 1 << 3;
const BUTTON_BIT_X2: u8 = 1 << 4;

fn button_to_bit(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => BUTTON_BIT_LEFT,
        MouseButton::Right => BUTTON_BIT_RIGHT,
        MouseButton::Middle => BUTTON_BIT_MIDDLE,
        MouseButton::Other(0) => BUTTON_BIT_X1,
        MouseButton::Other(1) => BUTTON_BIT_X2,
        MouseButton::Other(_) => 0,
    }
}

// ---------------------------------------------------------------------------
// WinWindowInner — actual HWND state (Arc'd, OS holds raw ptr via GWLP_USERDATA)
// ---------------------------------------------------------------------------

pub struct WinWindowInner {
    pub(crate) hwnd: HWND,
    pub(crate) hinstance: windows::Win32::Foundation::HINSTANCE,
    pub(crate) id: WindowId,
    pub(crate) min_size: Option<(u32, u32)>,
    /// Bitmask of currently captured mouse buttons.
    pub(crate) captured_buttons: Cell<u8>,
    /// True if TrackMouseEvent is armed for WM_MOUSELEAVE.
    pub(crate) is_tracking_hover: Cell<bool>,
    /// Render delegate for sync resize/redraw callbacks (Phase 4).
    pub(crate) delegate: RefCell<Option<Weak<dyn WindowRenderDelegate>>>,
    /// IME query delegate for sync OS composition queries (Phase 9c).
    /// Wired into `WM_IME_*` arms in Phase 3.
    pub(crate) ime_delegate: RefCell<Option<Weak<dyn WindowImeDelegate>>>,
    /// True iff a Win32 IME composition is currently active on this window.
    /// Flipped on by `WM_IME_STARTCOMPOSITION`, off by `WM_IME_ENDCOMPOSITION`.
    /// `WM_DESTROY` reads this to synthesise `Event::ImeDisabled` so the
    /// one-Disabled-per-session contract holds across window teardown.
    pub(crate) composition_active: Cell<bool>,
    /// True during modal size-move loop; WM_SIZE skips delegate (WM_TIMER handles it).
    pub(crate) in_size_move: Cell<bool>,
    /// Phase 5: Deferred device probe triggered during modal loop.
    /// Set by WM_DISPLAYCHANGE/WM_DPICHANGED when in_size_move; fired in WM_EXITSIZEMOVE.
    pub(crate) pending_display_change: Cell<bool>,
    /// Phase 2.1: fire-once gates so T2 diagnostics don't flood logs (~239×/drag).
    pub(crate) logged_no_delegate: Cell<bool>,
    pub(crate) logged_weak_upgrade_none: Cell<bool>,
    /// Cached DXGI factory for monitor→adapter LUID mapping. Created once at
    /// window construction; reused for every `current_monitor_luid` probe so
    /// the per-redraw LUID check stays microseconds, not a fresh COM factory
    /// each call.
    pub(crate) dxgi_factory: Option<IDXGIFactory2>,
    /// HMONITOR the window most recently occupied. Compared against
    /// `MonitorFromWindow` in `WM_WINDOWPOSCHANGED` to detect cross-monitor
    /// moves that bypass `WM_DISPLAYCHANGE` / `WM_DPICHANGED` (same-DPI
    /// monitor pair). Initialized to the real HMONITOR at construction so
    /// the first WM_WINDOWPOSCHANGED does not fire a spurious change.
    pub(crate) last_monitor: Cell<HMONITOR>,
    /// Deferred monitor-change flag set by `WM_WINDOWPOSCHANGED` when the
    /// window is inside a modal size/move loop. Drained in `WM_EXITSIZEMOVE`.
    /// Distinct from `pending_display_change` — different semantics, different
    /// probe call site.
    pub(crate) pending_monitor_change: Cell<bool>,
    /// High half of a pending UTF-16 surrogate pair received via `WM_CHAR`.
    /// Joined with the low half on the next `WM_CHAR`; cleared on any
    /// `WM_KEYUP`/`WM_SYSKEYUP` so orphan highs from stalled sequences do not
    /// leak into the next keypress.
    pub(crate) pending_high_surrogate: Cell<Option<u16>>,
    /// True iff a one-shot redraw timer is currently armed via
    /// `Window::schedule_redraw_at` (Phase 10a.6). Used to `KillTimer`
    /// before re-arming so a fast caller replaces an in-flight timer
    /// without double-fire, and to clean up on `WM_DESTROY`.
    pub(crate) redraw_timer_armed: Cell<bool>,
    /// Set immediately before our own `ReleaseCapture()` call so the
    /// following `WM_CAPTURECHANGED` is recognized as a voluntary release,
    /// not theft by another window. The wndproc consumes (and clears) it.
    /// Without this gate, every mouse-up would dispatch `CaptureLost`,
    /// zeroing the multi-click run between consecutive clicks.
    pub(crate) releasing_capture: Cell<bool>,
}

impl WinWindowInner {
    /// Invoke the render delegate if present. Clone-and-drop pattern ensures
    /// no RefCell borrow is held across the trait method call (re-entrancy safe).
    fn with_delegate(&self, f: impl FnOnce(&dyn WindowRenderDelegate)) {
        let weak = self.delegate.borrow().clone();
        if let Some(weak) = weak {
            if let Some(strong) = weak.upgrade() {
                f(&*strong);
            } else if !self.logged_weak_upgrade_none.replace(true) {
                // Phase 2.1 T2: surface silent no-op when AppState was already dropped.
                // Fire-once per window to keep diagnostic logs readable.
                log::trace!(target: "slate::win", "with_delegate: weak upgrade returned None (logged once)");
            }
        } else if !self.logged_no_delegate.replace(true) {
            // Phase 2.1 T2: delegate slot never installed (e.g. example bypasses AppState).
            // Fire-once per window — otherwise spams ~239×/drag.
            log::trace!(target: "slate::win", "with_delegate: no delegate installed (logged once)");
        }
    }

    /// Translate a Win32 message into a Slate [`Event`] and dispatch it.
    pub(crate) fn handle_message(
        &self,
        hwnd: HWND,
        msg: u32,
        _wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CLOSE => {
                dispatch_event(Event::WindowCloseRequested { window: self.id });
                // SAFETY: default proc is always safe to call.
                unsafe { DefWindowProcW(hwnd, msg, _wparam, lparam) }
            }
            WM_DESTROY => {
                // Synthesise ImeDisabled if a composition was still active at
                // teardown, so consumers always see a clean session bracket.
                if self.composition_active.replace(false) {
                    dispatch_event(Event::ImeDisabled { window: self.id });
                }
                // Cancel any in-flight redraw timer so the OS does not raise
                // a stray WM_TIMER against a destroyed window (Phase 10a.6).
                if self.redraw_timer_armed.replace(false) {
                    // SAFETY: hwnd is still valid inside WM_DESTROY.
                    let _ = unsafe { KillTimer(Some(hwnd), REDRAW_TIMER_ID) };
                }
                dispatch_event(Event::WindowDestroyed { window: self.id });
                // SAFETY: PostQuitMessage is always safe to call from a WM_DESTROY handler.
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            WM_SIZE => {
                // Ignore minimize: lparam carries (0, 0) which would stage a
                // zero-dim resize and trigger a swapchain reconfigure to 0×0.
                if _wparam.0 == SIZE_MINIMIZED as usize {
                    return LRESULT(0);
                }
                // lParam carries physical client size under PMv2.
                let lp = lparam.0 as u32;
                let pw = (lp & 0xFFFF).max(1);
                let ph = ((lp >> 16) & 0xFFFF).max(1);
                // SAFETY: hwnd is valid.
                let dpi = unsafe { GetDpiForWindow(hwnd) };
                let scale = dpi as f64 / 96.0;
                let lw = (pw as f64 / scale).round() as u32;
                let lh = (ph as f64 / scale).round() as u32;
                let in_size_move = IN_SIZE_MOVE.with(|f| f.get());
                log::trace!(target: "slate::win", "WM_SIZE pw={pw} ph={ph} in_size_move={in_size_move}");
                // Sync delegate: skip during size-move (WM_TIMER handles live-resize).
                if !self.in_size_move.get() {
                    self.with_delegate(|d| d.on_resize_sync(self.id, PhysicalSize::new(pw, ph)));
                }
                dispatch_event(Event::WindowResized {
                    window: self.id,
                    logical_size: (lw, lh),
                    physical_size: (pw, ph),
                    scale_factor: scale,
                });
                if !in_size_move {
                    dispatch_event(Event::WindowRedrawRequested { window: self.id });
                }
                LRESULT(0)
            }
            WM_TIMER if _wparam.0 == SIZE_MOVE_TIMER_ID && IN_SIZE_MOVE.with(|f| f.get()) => {
                self.with_delegate(|d| d.on_redraw(self.id));
                dispatch_event(Event::WindowRedrawRequested { window: self.id });
                let _ = unsafe { ValidateRect(Some(hwnd), None) };
                LRESULT(0)
            }
            // One-shot redraw scheduled via Window::schedule_redraw_at (10a.6).
            // KillTimer immediately so the id is reusable for the next arm and
            // a stuck-set "armed" flag can't outlive the fire.
            WM_TIMER if _wparam.0 == REDRAW_TIMER_ID => {
                // SAFETY: hwnd is valid; KillTimer is idempotent.
                let _ = unsafe { KillTimer(Some(hwnd), REDRAW_TIMER_ID) };
                self.redraw_timer_armed.set(false);
                // SAFETY: hwnd is valid; None invalidates the entire client
                // area, matching `Window::request_redraw`. Letting WM_PAINT
                // handle the actual redraw keeps a single redraw code path.
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                self.with_delegate(|d| d.on_redraw(self.id));
                dispatch_event(Event::WindowRedrawRequested { window: self.id });
                let _ = unsafe { ValidateRect(Some(hwnd), None) };
                LRESULT(0)
            }
            WM_ENTERSIZEMOVE => {
                self.in_size_move.set(true);
                IN_SIZE_MOVE.with(|f| f.set(true));
                let id =
                    unsafe { SetTimer(Some(hwnd), SIZE_MOVE_TIMER_ID, USER_TIMER_MINIMUM, None) };
                if id == 0 {
                    log::error!(
                        "SetTimer failed for size-move loop; live-resize rendering disabled this drag"
                    );
                }
                LRESULT(0)
            }
            WM_EXITSIZEMOVE => {
                self.in_size_move.set(false);
                IN_SIZE_MOVE.with(|f| f.set(false));
                let _ = unsafe { KillTimer(Some(hwnd), SIZE_MOVE_TIMER_ID) };

                let mut msg = MSG::default();
                while unsafe {
                    PeekMessageW(&mut msg, Some(hwnd), WM_TIMER, WM_TIMER, PM_REMOVE).as_bool()
                } {
                    // discard
                }

                // Phase 5: Fire deferred device probe if display changed during modal loop.
                if self.pending_display_change.get() {
                    self.pending_display_change.set(false);
                    log::trace!(target: "slate::win", "WM_EXITSIZEMOVE: firing deferred display change probe");
                    self.with_delegate(|d| d.on_display_change(self.id));
                }
                // Cross-monitor drag completed inside the modal loop — request a
                // redraw so the framework's per-frame adapter-LUID probe runs
                // and migrates the renderer to the new monitor's adapter if
                // needed. Deliberately separate from `pending_display_change`
                // (different probe semantics: GPU health vs. adapter LUID).
                if self.pending_monitor_change.get() {
                    self.pending_monitor_change.set(false);
                    log::trace!(target: "slate::win", "WM_EXITSIZEMOVE: draining deferred monitor-change redraw");
                    dispatch_event(Event::WindowRedrawRequested { window: self.id });
                }
                // Phase 2.1 T1: prove we re-emerge from the if-block.
                log::trace!(target: "slate::win", "WM_EXITSIZEMOVE: post-probe checkpoint reached");

                self.with_delegate(|d| d.on_size_move_end(self.id));
                self.with_delegate(|d| d.on_redraw(self.id));
                dispatch_event(Event::WindowRedrawRequested { window: self.id });
                LRESULT(0)
            }
            // Phase 5: Proactive device health check on monitor topology change.
            WM_DISPLAYCHANGE => {
                if self.in_size_move.get() || IN_SIZE_MOVE.with(|f| f.get()) {
                    log::trace!(target: "slate::win", "WM_DISPLAYCHANGE: in modal loop, deferring probe");
                    self.pending_display_change.set(true);
                } else {
                    log::trace!(target: "slate::win", "WM_DISPLAYCHANGE: probing device health");
                    self.with_delegate(|d| d.on_display_change(self.id));
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                // SAFETY: Win32 guarantees lParam is a valid *const RECT for WM_DPICHANGED.
                let suggested = unsafe { &*(lparam.0 as *const RECT) };
                // SetWindowPos is unconditional per constraint #10 (must honour suggested rect).
                let _ = unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        suggested.left,
                        suggested.top,
                        suggested.right - suggested.left,
                        suggested.bottom - suggested.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                };
                // In-size-move guard (Phase 5 / constraint #10): suppress resize dispatch during modal.
                if !IN_SIZE_MOVE.with(|f| f.get()) {
                    // Suggested RECT is frame coords; use GetClientRect for client size.
                    let mut rect = RECT::default();
                    let _ = unsafe { GetClientRect(hwnd, &mut rect) };
                    let pw = (rect.right - rect.left) as u32;
                    let ph = (rect.bottom - rect.top) as u32;
                    // SAFETY: hwnd is valid.
                    let dpi = unsafe { GetDpiForWindow(hwnd) };
                    let scale = dpi as f64 / 96.0;
                    let lw = (pw as f64 / scale).round() as u32;
                    let lh = (ph as f64 / scale).round() as u32;
                    // Sync delegate first so framebuffer is ready before observers see resize.
                    self.with_delegate(|d| d.on_resize_sync(self.id, PhysicalSize::new(pw, ph)));
                    dispatch_event(Event::WindowResized {
                        window: self.id,
                        logical_size: (lw, lh),
                        physical_size: (pw, ph),
                        scale_factor: scale,
                    });
                }
                // Phase 5 / Q2: WM_DPICHANGED also probes for device loss (cross-monitor mid-drag).
                if self.in_size_move.get() || IN_SIZE_MOVE.with(|f| f.get()) {
                    log::trace!(target: "slate::win", "WM_DPICHANGED: in modal loop, deferring probe");
                    self.pending_display_change.set(true);
                } else {
                    log::trace!(target: "slate::win", "WM_DPICHANGED: probing device health");
                    self.with_delegate(|d| d.on_display_change(self.id));
                }
                LRESULT(0)
            }
            // Cross-monitor drag without DPI change (same-DPI monitor pair) does
            // not produce WM_DISPLAYCHANGE or WM_DPICHANGED. Compare the
            // window's current HMONITOR against the cached one and request a
            // redraw on change so the framework's per-frame adapter-LUID probe
            // can migrate the renderer. MONITOR_DEFAULTTONULL — we want a real
            // HMONITOR or nothing, never a stale fallback.
            WM_WINDOWPOSCHANGED => {
                // SAFETY: hwnd is valid for the message lifetime.
                let hmon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL) };
                let last = self.last_monitor.get();
                if !hmon.is_invalid() && hmon != last {
                    self.last_monitor.set(hmon);
                    if self.in_size_move.get() || IN_SIZE_MOVE.with(|f| f.get()) {
                        log::trace!(target: "slate::win", "WM_WINDOWPOSCHANGED: monitor changed in modal loop, deferring redraw");
                        self.pending_monitor_change.set(true);
                    } else {
                        log::trace!(target: "slate::win", "WM_WINDOWPOSCHANGED: monitor changed, requesting redraw");
                        dispatch_event(Event::WindowRedrawRequested { window: self.id });
                    }
                }
                // SAFETY: default proc must run so it generates the follow-up
                // WM_SIZE / WM_MOVE messages.
                unsafe { DefWindowProcW(hwnd, msg, _wparam, lparam) }
            }
            WM_GETMINMAXINFO => {
                if let Some((min_w, min_h)) = self.min_size {
                    // SAFETY: lParam is a valid *mut MINMAXINFO for WM_GETMINMAXINFO.
                    let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                    info.ptMinTrackSize.x = min_w as i32;
                    info.ptMinTrackSize.y = min_h as i32;
                }
                LRESULT(0)
            }
            // -----------------------------------------------------------------
            // Mouse events (Phase 5a)
            // -----------------------------------------------------------------
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                self.handle_button_down(hwnd, _wparam, lparam, MouseButton::Left);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                self.handle_button_up(hwnd, _wparam, lparam, MouseButton::Left);
                LRESULT(0)
            }
            WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => {
                self.handle_button_down(hwnd, _wparam, lparam, MouseButton::Right);
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                self.handle_button_up(hwnd, _wparam, lparam, MouseButton::Right);
                LRESULT(0)
            }
            WM_MBUTTONDOWN | WM_MBUTTONDBLCLK => {
                self.handle_button_down(hwnd, _wparam, lparam, MouseButton::Middle);
                LRESULT(0)
            }
            WM_MBUTTONUP => {
                self.handle_button_up(hwnd, _wparam, lparam, MouseButton::Middle);
                LRESULT(0)
            }
            WM_XBUTTONDOWN | WM_XBUTTONDBLCLK => {
                let xbutton = ((_wparam.0 >> 16) & 0xFFFF) as u16;
                let button = if xbutton == 1 {
                    MouseButton::Other(0)
                } else {
                    MouseButton::Other(1)
                };
                self.handle_button_down(hwnd, _wparam, lparam, button);
                LRESULT(1) // X-button messages must return TRUE
            }
            WM_XBUTTONUP => {
                let xbutton = ((_wparam.0 >> 16) & 0xFFFF) as u16;
                let button = if xbutton == 1 {
                    MouseButton::Other(0)
                } else {
                    MouseButton::Other(1)
                };
                self.handle_button_up(hwnd, _wparam, lparam, button);
                LRESULT(1) // X-button messages must return TRUE
            }
            WM_MOUSEMOVE => {
                let scale = self.get_dpi_scale(hwnd);
                let position = decode_pos(lparam, scale);
                let modifiers = decode_modifiers(_wparam);
                // Arm TrackMouseEvent if not already tracking
                if !self.is_tracking_hover.get() {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: HOVER_DEFAULT,
                    };
                    let _ = unsafe { TrackMouseEvent(&mut tme) };
                    self.is_tracking_hover.set(true);
                }
                dispatch_event(Event::MouseMoved {
                    window: self.id,
                    position,
                    modifiers,
                });
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let scale = self.get_dpi_scale(hwnd);
                // lparam is in screen coords for wheel events
                let x_screen = (lparam.0 as i32 as u32 & 0xFFFF) as i16 as i32;
                let y_screen = ((lparam.0 as i32 as u32 >> 16) & 0xFFFF) as i16 as i32;
                let mut pt = POINT {
                    x: x_screen,
                    y: y_screen,
                };
                let _ = unsafe { ScreenToClient(hwnd, &mut pt) };
                let position = (pt.x as f32 / scale, pt.y as f32 / scale);
                let wheel_delta = ((_wparam.0 >> 16) as i16 as f32) / 120.0;
                let modifiers = decode_modifiers(_wparam);
                dispatch_event(Event::MouseScrolled {
                    window: self.id,
                    position,
                    delta_x: 0.0,
                    delta_y: wheel_delta,
                    precise: false,
                    modifiers,
                });
                LRESULT(0)
            }
            WM_MOUSEHWHEEL => {
                let scale = self.get_dpi_scale(hwnd);
                // lparam is in screen coords for wheel events
                let x_screen = (lparam.0 as i32 as u32 & 0xFFFF) as i16 as i32;
                let y_screen = ((lparam.0 as i32 as u32 >> 16) & 0xFFFF) as i16 as i32;
                let mut pt = POINT {
                    x: x_screen,
                    y: y_screen,
                };
                let _ = unsafe { ScreenToClient(hwnd, &mut pt) };
                let position = (pt.x as f32 / scale, pt.y as f32 / scale);
                let wheel_delta = ((_wparam.0 >> 16) as i16 as f32) / 120.0;
                let modifiers = decode_modifiers(_wparam);
                dispatch_event(Event::MouseScrolled {
                    window: self.id,
                    position,
                    delta_x: wheel_delta,
                    delta_y: 0.0,
                    precise: false,
                    modifiers,
                });
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                self.is_tracking_hover.set(false);
                dispatch_event(Event::MouseExited { window: self.id });
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                // Win32 fires WM_CAPTURECHANGED for both voluntary release
                // (our own ReleaseCapture call in handle_button_up) and theft
                // by another window. The flag is set before our ReleaseCapture
                // so consume-and-clear here suppresses the spurious CaptureLost.
                if self.releasing_capture.replace(false) {
                    LRESULT(0)
                } else if lparam.0 != hwnd.0 as isize {
                    self.captured_buttons.set(0);
                    self.is_tracking_hover.set(false);
                    dispatch_event(Event::CaptureLost { window: self.id });
                    LRESULT(0)
                } else {
                    LRESULT(0)
                }
            }
            // -----------------------------------------------------------------
            // Keyboard events (Phase 9a)
            // -----------------------------------------------------------------
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = _wparam.0 as u32;
                let lp = lparam.0 as u32;
                let scancode = (lp >> 16) & 0xFF;
                let extended = (lp >> 24) & 0x01 != 0;
                let is_repeat = (lp >> 30) & 0x01 != 0;
                let code = keymap::decode_keycode(vk, scancode, extended);
                let key = keymap::vk_to_named_key(vk)
                    .map(Key::Named)
                    .or_else(|| keymap::vk_to_character(vk, scancode).map(Key::Character))
                    .unwrap_or(Key::Unidentified);
                let modifiers = keymap::read_modifiers();
                dispatch_event(Event::KeyDown {
                    window: self.id,
                    code,
                    key,
                    modifiers,
                    is_repeat,
                });
                if msg == WM_SYSKEYDOWN {
                    // Fall through so Alt-menu activation (Alt+F, Alt+Space, Alt+F4)
                    // continues to work via the system's default handling.
                    // SAFETY: default proc is always safe to call.
                    unsafe { DefWindowProcW(hwnd, msg, _wparam, lparam) }
                } else {
                    LRESULT(0)
                }
            }
            WM_KEYUP | WM_SYSKEYUP => {
                let vk = _wparam.0 as u32;
                let lp = lparam.0 as u32;
                let scancode = (lp >> 16) & 0xFF;
                let extended = (lp >> 24) & 0x01 != 0;
                let code = keymap::decode_keycode(vk, scancode, extended);
                let key = keymap::vk_to_named_key(vk)
                    .map(Key::Named)
                    .or_else(|| keymap::vk_to_character(vk, scancode).map(Key::Character))
                    .unwrap_or(Key::Unidentified);
                let modifiers = keymap::read_modifiers();
                // Drop any orphan high surrogate left by a stalled WM_CHAR sequence.
                self.pending_high_surrogate.set(None);
                dispatch_event(Event::KeyUp {
                    window: self.id,
                    code,
                    key,
                    modifiers,
                });
                if msg == WM_SYSKEYUP {
                    // SAFETY: default proc is always safe to call.
                    unsafe { DefWindowProcW(hwnd, msg, _wparam, lparam) }
                } else {
                    LRESULT(0)
                }
            }
            WM_CHAR => {
                let code_unit = _wparam.0 as u16;
                // Filter ASCII control range except Tab (0x09) and CR (0x0D, normalized to LF).
                if (code_unit < 0x20 && code_unit != 0x09 && code_unit != 0x0D) || code_unit == 0x7F
                {
                    return LRESULT(0);
                }
                let text: Option<String> = if (0xD800..=0xDBFF).contains(&code_unit) {
                    // High surrogate — stash for join on next WM_CHAR.
                    self.pending_high_surrogate.set(Some(code_unit));
                    None
                } else if (0xDC00..=0xDFFF).contains(&code_unit) {
                    // Low surrogate — join with pending high.
                    if let Some(high) = self.pending_high_surrogate.take() {
                        let cp = 0x10000
                            + (((high - 0xD800) as u32) << 10)
                            + ((code_unit - 0xDC00) as u32);
                        char::from_u32(cp).map(|c| c.to_string())
                    } else {
                        // Orphan low — drop silently.
                        None
                    }
                } else {
                    // BMP code unit; normalize CR → LF for text input.
                    let unit = if code_unit == 0x0D { 0x0A } else { code_unit };
                    String::from_utf16(&[unit]).ok().filter(|s| !s.is_empty())
                };
                if let Some(text) = text {
                    dispatch_event(Event::TextInput {
                        window: self.id,
                        text,
                    });
                }
                LRESULT(0)
            }
            WM_UNICHAR => {
                const UNICODE_NOCHAR: usize = 0xFFFF;
                if _wparam.0 == UNICODE_NOCHAR {
                    // Advertise UTF-32 support to senders probing for it.
                    return LRESULT(1);
                }
                let cp = _wparam.0 as u32;
                if (cp < 0x20 && cp != 0x09 && cp != 0x0D) || cp == 0x7F {
                    return LRESULT(0);
                }
                let cp = if cp == 0x0D { 0x0A } else { cp };
                if let Some(c) = char::from_u32(cp) {
                    dispatch_event(Event::TextInput {
                        window: self.id,
                        text: c.to_string(),
                    });
                }
                LRESULT(0)
            }
            // -----------------------------------------------------------------
            // IME composition (Phase 9c)
            // -----------------------------------------------------------------
            WM_IME_STARTCOMPOSITION => {
                self.composition_active.set(true);
                dispatch_event(Event::ImeEnabled { window: self.id });
                // Return 0 to suppress the IMM built-in preedit window; we
                // render the composition ourselves inside the focused element.
                LRESULT(0)
            }
            WM_IME_COMPOSITION => {
                let flags = lparam.0 as u32;
                let Some(guard) = ImmContextGuard::acquire(hwnd) else {
                    // SAFETY: default proc is always safe to call.
                    return unsafe { DefWindowProcW(hwnd, msg, _wparam, lparam) };
                };
                let imc = guard.handle();
                let mut handled = false;

                if flags & GCS_RESULTSTR.0 != 0 {
                    if let Some((text, _)) = read_composition_string_utf8(imc, GCS_RESULTSTR) {
                        dispatch_event(Event::ImeCommit {
                            window: self.id,
                            text,
                        });
                    }
                    handled = true;
                }
                if flags & GCS_COMPSTR.0 != 0 {
                    if let Some((text, utf16)) = read_composition_string_utf8(imc, GCS_COMPSTR) {
                        let cursor_u16 = read_composition_cursor(imc).unwrap_or(0);
                        let cursor_byte_offset = utf16_units_to_utf8_bytes(&utf16, cursor_u16);
                        let selection = read_composition_attrs(imc)
                            .and_then(|attrs| find_target_converted_run(&attrs, &utf16));
                        dispatch_event(Event::ImePreedit {
                            window: self.id,
                            text,
                            cursor_byte_offset,
                            selection,
                        });
                    } else {
                        // GCS_COMPSTR is flagged but the IMM reports a zero-length
                        // string: the composition was deleted back to empty (the
                        // user backspaced the last char) while the session stays
                        // open. `read_composition_string_utf8` returns None for
                        // length 0, so emit an explicit empty preedit — otherwise
                        // the framework keeps painting the last non-empty
                        // composition, which sticks on screen and blocks further
                        // deletion.
                        dispatch_event(Event::ImePreedit {
                            window: self.id,
                            text: String::new(),
                            cursor_byte_offset: 0,
                            selection: None,
                        });
                    }
                    // Anchor the candidate window at the caret. The caret rect is
                    // already in client-relative physical pixels (the element
                    // paints the caret in scene/client space, and under
                    // Per-Monitor-v2 awareness those are physical px), and
                    // ImmSetCompositionWindow's COMPOSITIONFORM expects client
                    // coordinates — so feed the rect through unchanged. (No
                    // ScreenToClient: the rect was never in screen space, and
                    // subtracting the window origin again would fling the
                    // candidate window to a screen corner.)
                    let weak = self.ime_delegate.borrow().clone();
                    if let Some(weak) = weak
                        && let Some(strong) = weak.upgrade()
                        && let Some(rect) = strong.ime_caret_rect(self.id)
                    {
                        let client_rect = RECT {
                            left: rect.x,
                            top: rect.y,
                            right: rect.x + rect.width as i32,
                            bottom: rect.y + rect.height as i32,
                        };
                        set_composition_window(imc, client_rect);
                    }
                    handled = true;
                }
                drop(guard);
                if handled {
                    LRESULT(0)
                } else {
                    // SAFETY: default proc is always safe to call.
                    unsafe { DefWindowProcW(hwnd, msg, _wparam, lparam) }
                }
            }
            WM_IME_ENDCOMPOSITION => {
                self.composition_active.set(false);
                // Clear any lingering preedit: a cancelled composition ends
                // without a final empty GCS_COMPSTR, and no element wires an
                // `on_ime_disabled` handler, so the overlay would otherwise stay
                // painted. A preceding commit already cleared it, making this a
                // no-op there.
                dispatch_event(Event::ImePreedit {
                    window: self.id,
                    text: String::new(),
                    cursor_byte_offset: 0,
                    selection: None,
                });
                dispatch_event(Event::ImeDisabled { window: self.id });
                // SAFETY: default proc must run for IMM cleanup.
                unsafe { DefWindowProcW(hwnd, msg, _wparam, lparam) }
            }
            _ if msg == WM_APP_WAKE => {
                dispatch_event(Event::Wake);
                LRESULT(0)
            }
            _ => {
                // SAFETY: default proc is always safe to call.
                unsafe { DefWindowProcW(hwnd, msg, _wparam, lparam) }
            }
        }
    }

    /// Get DPI scale factor for coordinate conversion.
    fn get_dpi_scale(&self, hwnd: HWND) -> f32 {
        use windows::Win32::UI::HiDpi::GetDpiForWindow;
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        dpi as f32 / 96.0
    }

    /// Handle mouse button down: acquire capture, emit MouseDown.
    fn handle_button_down(&self, hwnd: HWND, wparam: WPARAM, lparam: LPARAM, button: MouseButton) {
        let scale = self.get_dpi_scale(hwnd);
        let position = decode_pos(lparam, scale);
        let modifiers = decode_modifiers(wparam);

        // Acquire capture on first button down
        let bit = button_to_bit(button);
        let old = self.captured_buttons.get();
        if old == 0 {
            let _ = unsafe { SetCapture(hwnd) };
        }
        self.captured_buttons.set(old | bit);

        dispatch_event(Event::MouseDown {
            window: self.id,
            position,
            button,
            modifiers,
        });
    }

    /// Handle mouse button up: release capture if all buttons up, emit MouseUp.
    fn handle_button_up(&self, hwnd: HWND, wparam: WPARAM, lparam: LPARAM, button: MouseButton) {
        let scale = self.get_dpi_scale(hwnd);
        let position = decode_pos(lparam, scale);
        let modifiers = decode_modifiers(wparam);

        // Release capture when all buttons are up
        let bit = button_to_bit(button);
        let old = self.captured_buttons.get();
        let new = old & !bit;
        self.captured_buttons.set(new);
        if new == 0 && old != 0 {
            // Mark the upcoming WM_CAPTURECHANGED as voluntary BEFORE the call —
            // Win32 may dispatch it synchronously from inside ReleaseCapture.
            self.releasing_capture.set(true);
            let _ = unsafe { ReleaseCapture() };
        }

        dispatch_event(Event::MouseUp {
            window: self.id,
            position,
            button,
            modifiers,
        });
    }
}

// ---------------------------------------------------------------------------
// WndProc trampoline — extern "system" callback with catch_unwind + abort (C4)
// ---------------------------------------------------------------------------

/// # SAFETY
/// This is the Win32 window procedure. Called by the OS on the main thread.
/// All Rust dispatch is wrapped in `catch_unwind`; any panic aborts the process
/// rather than unwinding into Win32 (which is UB on stable Rust).
pub(crate) unsafe extern "system" fn wnd_proc_trampoline(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // C4 fix: catch_unwind boundary — panicking across Win32 dispatch is UB on stable.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: lparam for WM_NCCREATE points to a CREATESTRUCTW as specified
        // by Win32. We immediately store the pointer and increment the Arc count.
        if msg == WM_NCCREATE {
            let cs = lparam.0 as *const CREATESTRUCTW;
            let inner_ptr = unsafe { (*cs).lpCreateParams as *const WinWindowInner };
            // C2 fix: increment strong count — OS now holds a logical Arc reference.
            unsafe { Arc::increment_strong_count(inner_ptr) };
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, inner_ptr as isize) };
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }

        let inner_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WinWindowInner;
        if inner_ptr.is_null() {
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }

        // SAFETY: GWLP_USERDATA is non-null only between WM_NCCREATE and
        // WM_NCDESTROY (where we clear it). Strong count >= 1 throughout.
        let inner = unsafe { &*inner_ptr };
        let res = inner.handle_message(hwnd, msg, wparam, lparam);

        if msg == WM_NCDESTROY {
            // C1 fix: last message this HWND will ever receive.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            unsafe { Arc::decrement_strong_count(inner_ptr) };
            // H1 fix: clear wake HWND to prevent posting to dead window.
            clear_wake_hwnd(hwnd);
        }

        res
    }));

    match result {
        Ok(lr) => lr,
        Err(_) => {
            log::error!("slate: Rust panic crossed Win32 wnd_proc; aborting");
            std::process::abort();
        }
    }
}
