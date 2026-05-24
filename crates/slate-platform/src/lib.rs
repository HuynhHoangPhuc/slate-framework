//! Platform abstraction for window + event loop.
//!
//! Implementations: macOS (`objc2-app-kit`), Windows (`windows-rs`).
//! Linux backends are planned post-Phase-0; the trait is shaped so an
//! X11/Wayland impl can plug in without disturbing existing callers.

use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

/// Cross-platform window/event-loop driver.
///
/// One `Platform` per process. `run` enters the OS-native event loop and
/// blocks until [`Platform::quit`] is invoked.
pub trait Platform: 'static {
    type Window: Window;

    /// Initialize the platform layer (e.g. shared `NSApplication` on
    /// macOS, register `WNDCLASS` on Windows). Must be called from the
    /// main OS thread.
    fn new() -> Self
    where
        Self: Sized;

    /// Create a window. The renderer holds a clone of the returned `Arc`
    /// for the surface lifetime.
    fn create_window(&self, opts: WindowOptions) -> Arc<Self::Window>;

    /// Enter the native run loop. Calls `handler` for every dispatched
    /// [`Event`]. Blocks until [`Platform::quit`] returns control.
    ///
    /// Borrows `&self` so the caller can keep the platform handle (and
    /// invoke `quit` from inside the closure). Handler lifetime is tied
    /// to that borrow, *not* `'static`.
    fn run<F>(&self, handler: F)
    where
        F: FnMut(Event);

    /// Request the run loop to exit. Idempotent.
    /// macOS: stops the shared `NSApplication`. Windows: `PostQuitMessage(0)`.
    fn quit(&self);
}

/// Window handle.
///
/// Intentionally **not** `Send + Sync`: AppKit `NSWindow`/`NSView` and
/// Win32 `HWND` are tied to the thread that created them. Cross-thread
/// requests must travel through the (future) event channel rather than
/// a shared `&Window` reference.
pub trait Window: HasWindowHandle + HasDisplayHandle + 'static {
    /// Logical size of the drawable area in points.
    fn logical_size(&self) -> (u32, u32);
    /// Physical pixel size of the drawable area.
    fn physical_size(&self) -> (u32, u32);
    /// Backing scale factor (1.0 standard, 2.0 Retina, etc.).
    fn scale_factor(&self) -> f64;
    /// Schedule a redraw. The next `Event::WindowRedrawRequested` for
    /// this window will fire after the OS draws the next frame.
    fn request_redraw(&self);
    fn set_title(&self, title: &str);
    /// Begin closing the window. The `WindowDestroyed` event fires
    /// asynchronously when the OS releases native resources.
    fn close(&self);
    /// Stable id for routing events.
    fn id(&self) -> WindowId;
    /// Install a render delegate to receive synchronous resize and redraw
    /// callbacks. Held as `Weak<dyn>` to avoid `AppState ↔ Window` cycles.
    /// Setting to a `Weak` whose `Rc` has been dropped is a no-op on next call.
    fn set_render_delegate(&self, delegate: std::rc::Weak<dyn WindowRenderDelegate>);

    /// Install an IME query delegate to satisfy synchronous OS queries during
    /// composition (caret rect for candidate-window positioning, marked /
    /// selected range, text near caret). Held as `Weak<dyn>` to mirror the
    /// render delegate; setting to a dropped `Weak` is a no-op on next call.
    ///
    /// Implementers MUST follow the cache-then-query contract documented on
    /// [`WindowImeDelegate`]; the platform layer will invoke these methods
    /// from inside Obj-C / Win32 callbacks that may already hold borrows on
    /// framework state.
    ///
    /// Default panics; concrete platform impls override.
    fn set_ime_delegate(&self, _delegate: std::rc::Weak<dyn WindowImeDelegate>) {
        panic!("set_ime_delegate not implemented for this Window");
    }

    /// LUID of the DXGI adapter that drives the monitor the window currently
    /// occupies (Windows only). Used by the renderer to pick the adapter that
    /// matches the window's monitor so cross-monitor drag does not silently
    /// switch the swap-chain across adapters (`DXGI_ERROR_DEVICE_REMOVED`).
    ///
    /// Returns `None` on platforms where monitor-bound adapter selection is
    /// not meaningful (macOS Metal manages this transparently) or when the
    /// LUID cannot be determined (probe failure, window minimized).
    fn current_monitor_luid(&self) -> Option<u64> {
        None
    }

    /// True while the window is inside a modal size/move loop
    /// (`WM_ENTERSIZEMOVE..WM_EXITSIZEMOVE` on Win32). Non-Windows platforms
    /// have no modal-loop concept and return `false`.
    fn in_size_move(&self) -> bool {
        false
    }

    /// Schedule a one-shot redraw at or shortly after `deadline`.
    ///
    /// Used by long-period animation drivers like the caret blink: the
    /// caller arms one timer for the next desired tick, then re-arms on
    /// the redraw it triggered. Single-shot semantics avoid leaking a
    /// repeating timer when the focused element changes or the window
    /// loses focus mid-animation.
    ///
    /// `deadline` in the past still fires — it is clamped up to the
    /// platform's minimum timer resolution (Win32 `USER_TIMER_MINIMUM` =
    /// 10ms; macOS GCD = effectively immediate) rather than dropped, so
    /// "fire ASAP" callers never silently no-op. Calling this while a
    /// previous timer for the same window is still pending REPLACES it
    /// (no double-fire). Replacement is per-window, not per-caller;
    /// callers that share a window must coordinate.
    ///
    /// Default impl is a no-op so headless test harnesses and mock
    /// `Window` impls compile without animation support. Concrete
    /// platform impls override.
    fn schedule_redraw_at(&self, deadline: std::time::Instant) {
        let _ = deadline;
    }
}

#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub title: String,
    /// Logical size in points.
    pub size: (u32, u32),
    pub min_size: Option<(u32, u32)>,
    pub resizable: bool,
    /// Whether the window is visible on creation. `false` produces a borderless,
    /// disabled, off-screen window suitable for headless DXGI/swap-chain tests.
    pub visible: bool,
    /// Top-left position in screen coordinates. `None` defers to the OS default
    /// (`CW_USEDEFAULT` on Win32).
    pub position: Option<(i32, i32)>,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: String::from("slate"),
            size: (800, 600),
            min_size: None,
            resizable: true,
            visible: true,
            position: None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

/// Mouse button identifier.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    /// Additional buttons (X1=0, X2=1, etc.). Values beyond 4 are clamped.
    Other(u8),
}

/// Modifier key state at event time.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Physical key identifier (positional, layout-independent).
///
/// Mirrors the W3C UI Events `KeyboardEvent.code` namespace — `KeyA` is
/// the physical key at QWERTY-A's location regardless of active layout
/// (Dvorak, AZERTY, etc).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KeyCode {
    KeyA,
    KeyB,
    KeyC,
    KeyD,
    KeyE,
    KeyF,
    KeyG,
    KeyH,
    KeyI,
    KeyJ,
    KeyK,
    KeyL,
    KeyM,
    KeyN,
    KeyO,
    KeyP,
    KeyQ,
    KeyR,
    KeyS,
    KeyT,
    KeyU,
    KeyV,
    KeyW,
    KeyX,
    KeyY,
    KeyZ,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Space,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    MetaLeft,
    MetaRight,
    Backquote,
    Minus,
    Equal,
    BracketLeft,
    BracketRight,
    Backslash,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
    Unidentified,
}

/// Logical-name keys that aren't textual characters.
/// Used inside [`Key::Named`] for keys like Enter, Tab, arrows, function keys.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NamedKey {
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Space,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Shift,
    Control,
    Alt,
    Meta,
}

/// Logical key value (after layout + modifier translation).
///
/// `Character` carries produced text (`"A"`, `"é"`, `"🦀"`). `Named`
/// carries non-textual keys (Enter, arrows, F-keys, modifiers). Allocation
/// is rare (one alloc per keypress, mostly 1–4 bytes); revisit with
/// `SmolStr` if profiling demands it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Character(String),
    Named(NamedKey),
    Unidentified,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
    /// Sent once after platform init, before the first paint.
    Resumed,
    WindowResized {
        window: WindowId,
        logical_size: (u32, u32),
        physical_size: (u32, u32),
        scale_factor: f64,
    },
    WindowCloseRequested {
        window: WindowId,
    },
    WindowDestroyed {
        window: WindowId,
    },
    WindowRedrawRequested {
        window: WindowId,
    },
    /// Background task completed; poll foreground executor and consider redraw.
    Wake,
    /// Sent immediately before `run` returns.
    Exiting,
    // -------------------------------------------------------------------------
    // Mouse events
    // -------------------------------------------------------------------------
    MouseDown {
        window: WindowId,
        /// Logical position in view coordinates (top-left origin).
        position: (f32, f32),
        button: MouseButton,
        modifiers: Modifiers,
    },
    MouseUp {
        window: WindowId,
        position: (f32, f32),
        button: MouseButton,
        modifiers: Modifiers,
    },
    MouseMoved {
        window: WindowId,
        position: (f32, f32),
        modifiers: Modifiers,
    },
    /// Scroll wheel or trackpad scroll gesture.
    /// Positive delta_y = scroll up (content moves down, wheel rolled away).
    MouseScrolled {
        window: WindowId,
        position: (f32, f32),
        delta_x: f32,
        delta_y: f32,
        /// True for trackpad/Magic Mouse (pixel precision), false for discrete wheel (line-based).
        precise: bool,
        modifiers: Modifiers,
    },
    /// Cursor exited the window bounds. Framework synthesizes Enter from hit-test diff.
    MouseExited {
        window: WindowId,
    },
    /// Windows-only: system stole mouse capture (e.g. Alt-Tab, another app took focus).
    /// Framework should clear its capture_target state when this fires.
    CaptureLost {
        window: WindowId,
    },
    // -------------------------------------------------------------------------
    // Keyboard events
    // -------------------------------------------------------------------------
    /// Physical key pressed. Carries both the layout-independent [`KeyCode`]
    /// and the logical [`Key`] value.
    KeyDown {
        window: WindowId,
        /// Physical key (layout-independent positional code).
        code: KeyCode,
        /// Logical key value (layout + modifier applied).
        key: Key,
        modifiers: Modifiers,
        /// True if generated by OS auto-repeat (key held down).
        is_repeat: bool,
    },
    /// Physical key released.
    KeyUp {
        window: WindowId,
        code: KeyCode,
        key: Key,
        modifiers: Modifiers,
    },
    /// Composed text from a single keypress (or surrogate pair on Windows).
    /// IME pre-edit is routed via `ImePreedit` / `ImeCommit` / `ImeEnabled` / `ImeDisabled`.
    TextInput {
        window: WindowId,
        text: String,
    },
    // -------------------------------------------------------------------------
    // Keyboard IME events
    // -------------------------------------------------------------------------
    /// IME composition session started. Fires exactly once per session at
    /// the first `setMarkedText:` (macOS, synthesised from marked-range
    /// transition) or `WM_IME_STARTCOMPOSITION` (Windows, 1:1).
    /// Always paired with a later [`Event::ImeDisabled`].
    ImeEnabled {
        window: WindowId,
    },
    /// IME composition in progress — replaces any prior preedit.
    /// `cursor_byte_offset` is a UTF-8 byte offset into `text`.
    /// `selection`, when present, is the IME-highlighted target-converted
    /// range (UTF-8 byte range into `text`).
    ImePreedit {
        window: WindowId,
        text: String,
        cursor_byte_offset: usize,
        selection: Option<core::ops::Range<usize>>,
    },
    /// IME composition finalised. `text` is the committed text to insert
    /// at the caret. **Empty `text` is the canonical "clear preedit, no
    /// insert" event** (macOS `unmarkText`) — framework consumers should
    /// only clear the preedit overlay and skip `Signal::set`.
    ImeCommit {
        window: WindowId,
        text: String,
    },
    /// IME composition session ended. Fires exactly once per session at
    /// commit or `unmarkText` (macOS) / `WM_IME_ENDCOMPOSITION` (Windows,
    /// 1:1) / window destroy with active composition (synthesised).
    /// Always preceded by [`Event::ImeEnabled`].
    ImeDisabled {
        window: WindowId,
    },
    // -------------------------------------------------------------------------
    // Device recovery events
    // -------------------------------------------------------------------------
    /// GPU device was lost (driver reset, TDR, adapter change).
    /// If `fatal: true`, recovery failed after max attempts - app should close window.
    DeviceLost {
        window: WindowId,
        fatal: bool,
    },
    /// GPU device was successfully recovered after a device-lost event.
    DeviceRestored {
        window: WindowId,
    },
}

mod render_delegate;
pub use render_delegate::{PhysicalRect, PhysicalSize, WindowImeDelegate, WindowRenderDelegate};

pub mod clipboard;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::{MacPlatform as DefaultPlatform, MacWindow as DefaultWindow, wake_run_loop};
#[cfg(all(target_os = "macos", feature = "test-hooks"))]
pub use mac::{
    clear_event_handler_for_test, dispatch_event_for_test, install_event_handler_for_test,
};

#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
pub use win::{WinPlatform as DefaultPlatform, WinWindow as DefaultWindow, wake_run_loop};
#[cfg(all(target_os = "windows", feature = "test-hooks"))]
pub use win::{
    clear_event_handler_for_test, dispatch_event_for_test, install_event_handler_for_test,
};
