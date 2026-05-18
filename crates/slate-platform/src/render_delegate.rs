//! Sync render boundary trait — invoked from platform OS callbacks
//! (Windows `WM_SIZE`/`WM_PAINT`, macOS `setFrameSize:`/`drawRect:`)
//! BEFORE the framebuffer commits.
//!
//! # Ownership
//!
//! `WindowRenderDelegate` is held by platform [`Window`](crate::Window) implementations
//! as `Weak<dyn WindowRenderDelegate>` (installed via
//! [`Window::set_render_delegate`](crate::Window)). The framework's `AppState<V>`
//! implements this trait to run the full layout + GPU pipeline
//! synchronously inside the OS resize/redraw callback. Holding `Weak`
//! prevents the `AppState ↔ Window` reference cycle that would otherwise
//! arise from `AppState` owning `Arc<Window>` strong.
//!
//! # Threading (NOT `Send + Sync` — by design)
//!
//! - **Trait has no `Send`/`Sync` bound.** Implementers may use interior
//!   mutability (`RefCell`, `Cell`) freely.
//! - **All trait methods are invoked on the main thread**, from inside
//!   the OS platform callback. The framework guarantees this; cross-thread
//!   redraws use the async `RedrawRequester` mechanism, not the delegate.
//! - On macOS, `DisplayLink` fires on a high-priority background thread;
//!   the platform code is responsible for marshalling the callback to the
//!   main queue (via `Queue::main().exec_async`) BEFORE invoking the
//!   delegate.
//! - **Implementers must not `Send` themselves across threads** to bypass
//!   this — the trait does not advertise `Send`, and `Rc<AppState>`
//!   cannot cross thread boundaries anyway.
//!
//! # Re-entrancy
//!
//! Methods take `&self` (not `&mut`) so the implementer can use interior
//! mutability. Do NOT re-enter the same `Window`'s delegate synchronously
//! from inside a delegate method — the framework's AppState guards
//! against this via a `rendering: Cell<bool>` flag, but other implementers
//! must follow the same discipline.

use crate::WindowId;

/// Physical pixel dimensions of a window's drawable surface.
///
/// Distinct from logical size (which is divided by scale factor).
/// Phase 4 dispatch sites pass the post-DPI physical extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct PhysicalSize {
    pub width: u32,
    pub height: u32,
}

impl PhysicalSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Convert to `(width, height)` tuple as expected by renderer APIs.
    pub const fn as_tuple(self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl From<(u32, u32)> for PhysicalSize {
    fn from((width, height): (u32, u32)) -> Self {
        Self { width, height }
    }
}

impl From<PhysicalSize> for (u32, u32) {
    fn from(size: PhysicalSize) -> Self {
        (size.width, size.height)
    }
}

/// Physical pixel rectangle in screen coordinates.
///
/// Used by the IME delegate query channel ([`WindowImeDelegate::ime_caret_rect`])
/// to report the caret position to the OS for candidate-window placement.
/// Origin is top-left; coordinates are physical pixels in **screen space**
/// (not window-relative). Platform impls perform any final OS-specific
/// conversions (Y-flip on macOS, ScreenToClient on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PhysicalRect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Build a physical-pixel rect from a logical-pixel rect by scaling.
    ///
    /// `scale` is the window's HiDPI scale factor (1.0 on @1x, 2.0 on Retina).
    /// X/Y are rounded; width is floored with a 1-px minimum so a 1-lpx caret
    /// stroke never collapses to 0 at fractional scales. Height matches the
    /// rounding rule used in `TextField`'s caret-rect synthesis.
    pub fn from_lpx_rect(x: f32, y: f32, w: f32, h: f32, scale: f32) -> Self {
        let phys_x = (x * scale).round() as i32;
        let phys_y = (y * scale).round() as i32;
        let phys_w = (w * scale).max(1.0) as u32;
        let phys_h = (h * scale).round() as u32;
        Self::new(phys_x, phys_y, phys_w, phys_h)
    }
}

/// Receives synchronous render notifications from a [`Window`](crate::Window).
///
/// Invoked from OS resize callbacks (Windows `WM_SIZE`/`WM_NCCALCSIZE`/
/// `WM_DPICHANGED`, macOS `windowDidResize:` /
/// `windowDidChangeBackingProperties:`) and redraw callbacks
/// (`WM_PAINT`, `drawRect:`, DisplayLink-driven redraw) **before the
/// framebuffer commits**.
///
/// The framework's `AppState<V>` is the canonical implementer. Platform
/// `Window` impls hold a `Weak<dyn WindowRenderDelegate>` to avoid
/// `AppState` ↔ `Window` reference cycles.
///
/// # Re-entrancy
///
/// Methods take `&self` (not `&mut`) so the implementer can use interior
/// mutability (`RefCell`). Do not re-enter the same `Window` synchronously
/// from inside a delegate method.
///
/// # Threading
///
/// See the module-level docs. Methods are main-thread-only.
pub trait WindowRenderDelegate {
    /// Called by the platform when the window's drawable surface must resize.
    /// Implementer should run layout + GPU submit inside this call so the
    /// new framebuffer is ready before the OS commits the new bounds.
    fn on_resize_sync(&self, window_id: WindowId, new_size: PhysicalSize);

    /// Called by the platform when the window needs to redraw (paint cycle).
    /// Implementer runs the full render pipeline synchronously.
    fn on_redraw(&self, window_id: WindowId);

    /// Called on WM_DISPLAYCHANGE (monitor topology change). Default: no-op.
    /// AppState overrides to probe device health proactively.
    fn on_display_change(&self, _window_id: WindowId) {}

    /// Called on WM_EXITSIZEMOVE (modal size-move loop ended). Default: no-op.
    /// AppState overrides to fire deferred device probes.
    fn on_size_move_end(&self, _window_id: WindowId) {}
}

/// Sync IME query boundary trait — invoked from platform OS callbacks
/// (macOS `NSTextInputClient` methods on `MetalView`, Windows `WM_IME_*`
/// arms in `wnd_proc_trampoline`) **during composition**.
///
/// # ADR-001 amendment (cache-then-query, Phase 9c)
///
/// Implementers MUST read all four query methods from a pre-published
/// snapshot (e.g. `AppState.cached_ime_query: RefCell<CachedImeQuery>`)
/// and MUST NOT traverse the live focus / IME / parent registries from
/// inside these methods. The OS query callbacks can fire mid-`keyDown:`
/// dispatch, re-entering framework state with active borrows; the
/// cache-then-query pattern eliminates the re-entrancy hazard
/// structurally.
///
/// The cache is republished at deterministic points (end of
/// `dispatch_ime_preedit`, end of every paint), so query results lag at
/// most one frame — well within IME tolerance (the OS candidate window
/// already lags one frame).
///
/// # Ownership
///
/// Held by platform [`Window`](crate::Window) implementations as
/// `Weak<dyn WindowImeDelegate>` (installed via
/// [`Window::set_ime_delegate`](crate::Window)). Mirrors the
/// [`WindowRenderDelegate`] ownership pattern; `Weak` prevents the
/// `AppState ↔ Window` cycle.
///
/// # Threading
///
/// `?Send + ?Sync` (no bound). All methods invoked on the main thread
/// only, from inside OS query callbacks during composition. Matches
/// [`WindowRenderDelegate`] threading.
pub trait WindowImeDelegate {
    /// Screen-coord physical-pixel rect of the active caret. The OS uses
    /// this to position the candidate / suggestion window.
    ///
    /// Returns `None` when no focused element is IME-capable, or when
    /// the cache has not been published yet (pre-first-paint).
    fn ime_caret_rect(&self, window_id: WindowId) -> Option<PhysicalRect>;

    /// Text at the given byte range in the focused element's buffer.
    /// Used by macOS `attributedSubstringForProposedRange:` for IME
    /// context queries.
    ///
    /// Returns `None` if the range is outside the cached text window
    /// or if no focused element claims IME.
    fn ime_text(&self, window_id: WindowId, range: std::ops::Range<usize>) -> Option<String>;

    /// Current selection in the focused element. Caret-only collapses
    /// to an empty range at the caret position. Used by macOS
    /// `selectedRange` query.
    ///
    /// Returns `None` if no focused element claims IME.
    fn ime_selected_range(&self, window_id: WindowId) -> Option<std::ops::Range<usize>>;

    /// Current preedit (marked text) range, or `None` if no composition
    /// is active on the focused element. Used by macOS `markedRange`
    /// query.
    fn ime_marked_range(&self, window_id: WindowId) -> Option<std::ops::Range<usize>>;
}
