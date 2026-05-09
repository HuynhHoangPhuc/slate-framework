//! Input event types for mouse, scroll, and pointer interactions.
//!
//! This module defines the framework-side event surface that user code touches.
//! Platform events from `slate_platform` are translated into these types before
//! being dispatched to element handlers.
//!
//! # Example
//!
//! ```ignore
//! use slate_framework::{MouseEvent, EventCtx};
//!
//! Div::new().on_mouse_down(|event: &MouseEvent, ctx: &mut EventCtx| {
//!     println!("Clicked at {:?}", event.position);
//!     ctx.stop_propagation();
//! })
//! ```

use std::sync::Arc;
use std::time::Instant;

pub use slate_platform::{Modifiers, MouseButton};

/// Handler closure type for mouse events (click, down, up, move).
pub(crate) type MouseHandler = Arc<dyn Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static>;

/// Handler closure type for scroll events.
pub(crate) type ScrollHandler = Arc<dyn Fn(&ScrollEvent, &mut EventCtx) + Send + Sync + 'static>;

/// Handler closure type for pointer events (raw stream, enter, leave).
pub(crate) type PointerHandler = Arc<dyn Fn(&PointerEvent, &mut EventCtx) + Send + Sync + 'static>;

/// Per-element handler collection for dispatch.
///
/// Mirrors the handler fields in `Div`. Stored in a `HashMap<ElementId, Handlers>`
/// for O(1) lookup during event dispatch.
#[derive(Clone, Default)]
pub(crate) struct Handlers {
    pub on_click: Option<MouseHandler>,
    pub on_mouse_down: Option<MouseHandler>,
    pub on_mouse_up: Option<MouseHandler>,
    pub on_mouse_move: Option<MouseHandler>,
    pub on_mouse_scrolled: Option<ScrollHandler>,
    pub on_pointer_event: Option<PointerHandler>,
    pub on_pointer_enter: Option<PointerHandler>,
    pub on_pointer_leave: Option<PointerHandler>,
}

impl Handlers {
    /// Returns true if any handler is registered.
    pub fn has_any(&self) -> bool {
        self.on_click.is_some()
            || self.on_mouse_down.is_some()
            || self.on_mouse_up.is_some()
            || self.on_mouse_move.is_some()
            || self.on_mouse_scrolled.is_some()
            || self.on_pointer_event.is_some()
            || self.on_pointer_enter.is_some()
            || self.on_pointer_leave.is_some()
    }
}

/// Mouse button event (down, up, or synthesized click).
///
/// Dispatched to `on_mouse_down`, `on_mouse_up`, and `on_click` handlers.
/// The handler name carries the event kind; this struct provides the payload.
#[derive(Clone, Copy, Debug)]
pub struct MouseEvent {
    /// Window-relative position, top-left origin, logical px (DPI-divided).
    pub position: (f32, f32),
    /// The button that triggered this event. Some for down/up/click; None for move.
    pub button: Option<MouseButton>,
    /// Modifier keys held at event time.
    pub modifiers: Modifiers,
    /// When the event was received by the framework.
    pub timestamp: Instant,
}

/// Scroll wheel or trackpad scroll gesture.
///
/// Positive `delta_y` = scroll up (content moves down, wheel rolled away from user).
#[derive(Clone, Copy, Debug)]
pub struct ScrollEvent {
    /// Window-relative position, top-left origin, logical px (DPI-divided).
    pub position: (f32, f32),
    /// Horizontal scroll delta.
    pub delta_x: f32,
    /// Vertical scroll delta. Positive = scroll up (content moves down).
    pub delta_y: f32,
    /// True for trackpad/Magic Mouse (pixel precision), false for discrete wheel.
    pub precise: bool,
    /// Modifier keys held at event time.
    pub modifiers: Modifiers,
    /// When the event was received by the framework.
    pub timestamp: Instant,
}

/// Low-level pointer event for users who need the full event stream.
///
/// Dispatched to `on_pointer_event` handlers. Includes all pointer interactions
/// without distinguishing by handler name.
#[derive(Clone, Copy, Debug)]
pub struct PointerEvent {
    /// The kind of pointer event.
    pub kind: PointerEventKind,
    /// Window-relative position, top-left origin, logical px (DPI-divided).
    pub position: (f32, f32),
    /// The button that triggered this event. None for move/enter/leave.
    pub button: Option<MouseButton>,
    /// Modifier keys held at event time.
    pub modifiers: Modifiers,
    /// When the event was received by the framework.
    pub timestamp: Instant,
}

/// The kind of pointer event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerEventKind {
    /// Pointer button pressed.
    Down,
    /// Pointer button released.
    Up,
    /// Pointer moved (with or without buttons held).
    Move,
    /// Pointer entered the element's hit region.
    Enter,
    /// Pointer left the element's hit region.
    Leave,
}

/// Event dispatch context passed to handlers.
///
/// Provides control over event propagation. The handler can call
/// `stop_propagation()` to prevent the event from bubbling to parent elements.
pub struct EventCtx<'a> {
    propagation_stopped: &'a mut bool,
}

impl EventCtx<'_> {
    /// Create a new EventCtx. Crate-private — only app.rs creates these.
    pub(crate) fn new(flag: &mut bool) -> EventCtx<'_> {
        EventCtx {
            propagation_stopped: flag,
        }
    }

    /// Stop event propagation. Prevents the event from bubbling to parent elements.
    pub fn stop_propagation(&mut self) {
        *self.propagation_stopped = true;
    }

    /// Check if propagation has been stopped.
    pub fn is_propagation_stopped(&self) -> bool {
        *self.propagation_stopped
    }
}

impl std::fmt::Debug for EventCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventCtx")
            .field("propagation_stopped", &*self.propagation_stopped)
            .finish()
    }
}
