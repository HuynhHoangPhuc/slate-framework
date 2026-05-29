//! Win32 mouse handlers: WM_*BUTTON*, WM_MOUSEMOVE, WM_MOUSEWHEEL/HWHEEL,
//! WM_MOUSELEAVE, WM_CAPTURECHANGED.

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT};
use windows::Win32::UI::Controls::{HOVER_DEFAULT, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_LWIN,
    VK_MENU, VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    WM_CAPTURECHANGED, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDBLCLK,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_XBUTTONDBLCLK, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use super::super::dispatch_event;
use super::WinWindowInner;
use crate::{Event, Modifiers, MouseButton};

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

impl WinWindowInner {
    /// Dispatch a mouse-family Win32 message; returns `None` if `msg` is not
    /// a recognized mouse message so the caller can try other families.
    pub(super) fn dispatch_mouse(
        &self,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        match msg {
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                self.handle_button_down(hwnd, wparam, lparam, MouseButton::Left);
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                self.handle_button_up(hwnd, wparam, lparam, MouseButton::Left);
                Some(LRESULT(0))
            }
            WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => {
                self.handle_button_down(hwnd, wparam, lparam, MouseButton::Right);
                Some(LRESULT(0))
            }
            WM_RBUTTONUP => {
                self.handle_button_up(hwnd, wparam, lparam, MouseButton::Right);
                Some(LRESULT(0))
            }
            WM_MBUTTONDOWN | WM_MBUTTONDBLCLK => {
                self.handle_button_down(hwnd, wparam, lparam, MouseButton::Middle);
                Some(LRESULT(0))
            }
            WM_MBUTTONUP => {
                self.handle_button_up(hwnd, wparam, lparam, MouseButton::Middle);
                Some(LRESULT(0))
            }
            WM_XBUTTONDOWN | WM_XBUTTONDBLCLK => {
                let xbutton = ((wparam.0 >> 16) & 0xFFFF) as u16;
                let button = if xbutton == 1 {
                    MouseButton::Other(0)
                } else {
                    MouseButton::Other(1)
                };
                self.handle_button_down(hwnd, wparam, lparam, button);
                Some(LRESULT(1)) // X-button messages must return TRUE
            }
            WM_XBUTTONUP => {
                let xbutton = ((wparam.0 >> 16) & 0xFFFF) as u16;
                let button = if xbutton == 1 {
                    MouseButton::Other(0)
                } else {
                    MouseButton::Other(1)
                };
                self.handle_button_up(hwnd, wparam, lparam, button);
                Some(LRESULT(1))
            }
            WM_MOUSEMOVE => Some(self.on_mouse_move(hwnd, wparam, lparam)),
            WM_MOUSEWHEEL => Some(self.on_mouse_wheel(hwnd, wparam, lparam, false)),
            WM_MOUSEHWHEEL => Some(self.on_mouse_wheel(hwnd, wparam, lparam, true)),
            WM_MOUSELEAVE => {
                self.is_tracking_hover.set(false);
                dispatch_event(Event::MouseExited { window: self.id });
                Some(LRESULT(0))
            }
            WM_CAPTURECHANGED => Some(self.on_capture_changed(hwnd, lparam)),
            _ => None,
        }
    }

    /// Get DPI scale factor for coordinate conversion.
    fn get_dpi_scale(&self, hwnd: HWND) -> f32 {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        dpi as f32 / 96.0
    }

    fn on_mouse_move(&self, hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let scale = self.get_dpi_scale(hwnd);
        let position = decode_pos(lparam, scale);
        let modifiers = decode_modifiers(wparam);
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

    fn on_mouse_wheel(
        &self,
        hwnd: HWND,
        wparam: WPARAM,
        lparam: LPARAM,
        horizontal: bool,
    ) -> LRESULT {
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
        let wheel_delta = ((wparam.0 >> 16) as i16 as f32) / 120.0;
        let modifiers = decode_modifiers(wparam);
        let (delta_x, delta_y) = if horizontal {
            (wheel_delta, 0.0)
        } else {
            (0.0, wheel_delta)
        };
        dispatch_event(Event::MouseScrolled {
            window: self.id,
            position,
            delta_x,
            delta_y,
            precise: false,
            modifiers,
        });
        LRESULT(0)
    }

    fn on_capture_changed(&self, hwnd: HWND, lparam: LPARAM) -> LRESULT {
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
