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

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

pub use slate_platform::{Key, KeyCode, Modifiers, MouseButton, NamedKey, WindowId};

use crate::ime::{ImeRegistry, ImeState};
use crate::types::ElementId;

/// Cross-platform "command modifier" check for keyboard shortcuts.
///
/// Returns `true` when the platform's command modifier is held: `Cmd` on macOS,
/// `Ctrl` elsewhere. Use this in shortcut handlers so the keybinding code stays
/// free of `cfg(target_os)` branches.
#[inline]
pub fn is_command_modifier(m: &Modifiers) -> bool {
    #[cfg(target_os = "macos")]
    {
        m.meta
    }
    #[cfg(not(target_os = "macos"))]
    {
        m.ctrl
    }
}

/// Handler closure type for mouse events (click, down, up, move).
pub(crate) type MouseHandler = Arc<dyn Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static>;

/// Per-element keyboard handler closure (used by `Div::on_key_down / on_key_up`).
pub(crate) type ElementKeyHandler = Arc<dyn Fn(&KeyEvent, &mut EventCtx) + Send + Sync + 'static>;

/// Per-element text-input handler closure (used by `Div::on_text_input`).
pub(crate) type ElementTextInputHandler =
    Arc<dyn Fn(&TextInputEvent, &mut EventCtx) + Send + Sync + 'static>;

/// Per-element handler for `ImePreedit` events (used by `Div::on_ime_preedit`).
pub(crate) type ElementImePreeditHandler =
    Arc<dyn Fn(&ImePreeditEvent, &mut EventCtx) + Send + Sync + 'static>;

/// Per-element handler for `ImeCommit` events (used by `Div::on_ime_commit`).
pub(crate) type ElementImeCommitHandler =
    Arc<dyn Fn(&ImeCommitEvent, &mut EventCtx) + Send + Sync + 'static>;

/// Per-element handler for `ImeEnabled` / `ImeDisabled` events. Both lifecycle
/// edges share the same payload (no data) and the same handler shape.
pub(crate) type ElementImeLifecycleHandler =
    Arc<dyn Fn(&ImeLifecycleEvent, &mut EventCtx) + Send + Sync + 'static>;

/// Per-element IME handler bundle. Mirrors [`KeyHandlers`]; stored in
/// `AppState::ime_handler_map` and looked up during the focused-chain
/// bubble dispatch for IME events.
#[allow(dead_code)] // Populated by Div::prepaint and consumed by AppState dispatchers.
#[derive(Clone, Default)]
#[doc(hidden)]
pub struct ImeHandlers {
    pub on_ime_preedit: Option<ElementImePreeditHandler>,
    pub on_ime_commit: Option<ElementImeCommitHandler>,
    pub on_ime_enabled: Option<ElementImeLifecycleHandler>,
    pub on_ime_disabled: Option<ElementImeLifecycleHandler>,
}

impl ImeHandlers {
    #[allow(dead_code)] // Used by dispatch_ime_* to skip empty bundles.
    pub fn has_any(&self) -> bool {
        self.on_ime_preedit.is_some()
            || self.on_ime_commit.is_some()
            || self.on_ime_enabled.is_some()
            || self.on_ime_disabled.is_some()
    }
}

/// Per-element mouse handler bundle. Mirrors [`KeyHandlers`] / [`ImeHandlers`];
/// stored in `AppState::mouse_handler_map` and consulted by
/// `dispatch_mouse_down` / `dispatch_mouse_up` / the coalesced-move flush in
/// addition to the legacy `Handlers` map. Lets focused elements (TextField,
/// future TextArea) register a focused, three-method bundle without paying for
/// Div's full Handlers struct surface (click, pointer, scroll). Bubble order is
/// shared with `handler_map`: MouseHandlers entries fire before Handlers
/// entries at each ancestor.
#[allow(dead_code)] // Populated by elements that opt in; consumed by AppState dispatchers.
#[derive(Clone, Default)]
#[doc(hidden)]
pub struct MouseHandlers {
    pub on_mouse_down: Option<MouseHandler>,
    pub on_mouse_move: Option<MouseHandler>,
    pub on_mouse_up: Option<MouseHandler>,
}

impl MouseHandlers {
    #[allow(dead_code)] // Used by registration helper to skip empty bundles.
    pub fn has_any(&self) -> bool {
        self.on_mouse_down.is_some() || self.on_mouse_move.is_some() || self.on_mouse_up.is_some()
    }
}

/// Per-element keyboard handler bundle. Mirrors [`Handlers`] for mouse; stored
/// in `AppState::key_handler_map` (Phase 3) and looked up during the focused-chain
/// bubble dispatch.
#[allow(dead_code)] // Populated by Div::prepaint and consumed by AppState in Phase 3.
#[derive(Clone, Default)]
#[doc(hidden)]
pub struct KeyHandlers {
    pub on_key_down: Option<ElementKeyHandler>,
    pub on_key_up: Option<ElementKeyHandler>,
    pub on_text_input: Option<ElementTextInputHandler>,
}

impl KeyHandlers {
    #[allow(dead_code)] // Used by AppState::dispatch_key_* in Phase 3 to skip empty bundles.
    pub fn has_any(&self) -> bool {
        self.on_key_down.is_some() || self.on_key_up.is_some() || self.on_text_input.is_some()
    }
}

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

/// Closure type for App-level `KeyDown` / `KeyUp` handlers. Not `Send` —
/// keyboard dispatch runs on the UI thread.
///
/// Phase 9b: signature gained `&mut EventCtx` for parity with per-element
/// keyboard handlers — App-level handlers can now call `cx.stop_propagation()`,
/// `cx.request_focus(id)`, and `cx.focused_element()`.
pub type KeyHandler = Box<dyn FnMut(&KeyEvent, &mut EventCtx)>;

/// Closure type for App-level `TextInput` handlers.
pub type TextInputHandler = Box<dyn FnMut(&TextInputEvent, &mut EventCtx)>;

/// App-level handler for IME preedit composition events. Not `Send` — IME
/// dispatch runs on the UI thread.
pub type ImePreeditHandler = Box<dyn FnMut(&ImePreeditEvent, &mut EventCtx)>;

/// App-level handler for IME commit events (composition finalised).
pub type ImeCommitHandler = Box<dyn FnMut(&ImeCommitEvent, &mut EventCtx)>;

/// App-level handler for IME enable/disable lifecycle edges.
pub type ImeLifecycleHandler = Box<dyn FnMut(&ImeLifecycleEvent, &mut EventCtx)>;

/// Keyboard event payload for `KeyDown` / `KeyUp` handlers.
///
/// Carries both the layout-independent [`KeyCode`] (positional, W3C
/// `KeyboardEvent.code`) and the logical [`Key`] value (after layout + modifier
/// translation, W3C `KeyboardEvent.key`). Use `code` for shortcut matching that
/// must survive layout changes (Dvorak, AZERTY); use `key` for human-readable
/// inspection.
///
/// `is_repeat` is set when the OS auto-repeat generated this event. For `KeyUp`
/// this is always `false`.
#[derive(Clone, Debug)]
pub struct KeyEvent {
    /// Physical key (layout-independent positional code).
    pub code: KeyCode,
    /// Logical key value (layout + modifier applied).
    pub key: Key,
    /// Modifier keys held at event time.
    pub modifiers: Modifiers,
    /// True if generated by OS auto-repeat (key held down). Always false for KeyUp.
    pub is_repeat: bool,
    /// When the event was received by the framework.
    pub timestamp: Instant,
}

/// Composed text payload for `on_text_input` handlers.
///
/// Carries the UTF-8 text produced by a single keystroke (or surrogate pair on
/// Windows). Cross-platform consumers wanting "what did the user type" should
/// observe `TextInputEvent` rather than `KeyEvent.key`, since the latter is
/// platform-asymmetric: macOS may emit `Key::Character` inline via UCKeyTranslate
/// while Windows routes character data exclusively through `WM_CHAR`.
#[derive(Clone, Debug)]
pub struct TextInputEvent {
    /// Composed text (UTF-8). 1+ characters per event.
    pub text: String,
    /// When the event was received by the framework.
    pub timestamp: Instant,
}

/// IME composition update payload. Replaces any prior preedit on the
/// focused element each time it fires.
#[derive(Clone, Debug)]
pub struct ImePreeditEvent {
    /// Composition text (UTF-8).
    pub text: String,
    /// UTF-8 byte offset of the IME caret inside `text`.
    pub cursor_byte_offset: usize,
    /// IME-highlighted target-converted range, if the OS provided one.
    pub selection: Option<Range<usize>>,
    /// When the event was received by the framework.
    pub timestamp: Instant,
}

/// IME commit payload. Empty `text` is the canonical "clear preedit, no
/// insert" signal (macOS `unmarkText`); consumers skip `Signal::set` when
/// `text.is_empty()`.
#[derive(Clone, Debug)]
pub struct ImeCommitEvent {
    /// Text to insert at the focused element's caret.
    pub text: String,
    /// When the event was received by the framework.
    pub timestamp: Instant,
}

/// IME lifecycle edge payload (enabled / disabled). Carries only the
/// timestamp; the edge itself is communicated by which handler ran.
#[derive(Clone, Copy, Debug)]
pub struct ImeLifecycleEvent {
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

/// Deferred focus operation. Set by handlers via [`EventCtx::request_focus`] or
/// [`EventCtx::blur`]; drained by `AppState` after the handler chain unwinds so
/// focus only mutates at a single, well-defined point per dispatch.
#[derive(Clone, Copy, Debug)]
pub enum PendingFocusOp {
    /// Focus the given element after the chain completes.
    Focus(ElementId),
    /// Clear focus after the chain completes.
    Blur,
}

/// Deferred mouse-capture operation. Set by handlers via
/// [`EventCtx::set_capture`] / [`EventCtx::release_capture`]; drained by
/// `AppState` after the handler chain unwinds (same lifecycle as
/// [`PendingFocusOp`]). An explicit `Set` makes capture sticky — it survives
/// mouse-up so multi-step drags across button releases keep routing to the
/// captured element until `Release` is requested or the element is dropped.
#[derive(Clone, Copy, Debug)]
pub enum PendingCaptureOp {
    /// Capture the pointer to the given element after the chain completes.
    Set(ElementId),
    /// Release any active capture after the chain completes.
    Release,
}

/// Event dispatch context passed to handlers.
///
/// Provides control over event propagation and focus changes. Handlers can call
/// `stop_propagation()` to prevent bubbling, `request_focus(id)` / `blur()` to
/// queue a focus change (applied after the chain), and `focused_element()` to
/// read the focus snapshot taken at dispatch start.
pub struct EventCtx<'a> {
    propagation_stopped: &'a mut bool,
    pending_focus_op: &'a mut Option<PendingFocusOp>,
    pending_capture_op: &'a mut Option<PendingCaptureOp>,
    window_id: WindowId,
    focused: Option<ElementId>,
    /// Element this handler is bound to (Phase 9c). `None` for App-level and
    /// non-element dispatch sites; `Some(id)` when the dispatch loop is in
    /// per-element bubble mode.
    element_id: Option<ElementId>,
    /// Reference to the framework IME registry (Phase 9c). `None` outside of
    /// IME dispatch; per-element IME handlers receive `Some(_)` so they can
    /// query/mutate their own `ImeState` via [`EventCtx::ime_state`].
    ime_registry: Option<&'a RefCell<ImeRegistry>>,
}

impl<'a> EventCtx<'a> {
    /// Create a new EventCtx. Crate-private — dispatch sites in `app_state.rs`
    /// allocate the backing state then thread the borrow through the handler
    /// chain.
    pub(crate) fn new(
        propagation_stopped: &'a mut bool,
        pending_focus_op: &'a mut Option<PendingFocusOp>,
        pending_capture_op: &'a mut Option<PendingCaptureOp>,
        window_id: WindowId,
        focused: Option<ElementId>,
    ) -> EventCtx<'a> {
        EventCtx {
            propagation_stopped,
            pending_focus_op,
            pending_capture_op,
            window_id,
            focused,
            element_id: None,
            ime_registry: None,
        }
    }

    /// Attach the per-element id + IME registry reference (Phase 9c).
    /// Called by IME dispatch loops on the EventCtx they construct so
    /// handlers can resolve their own `ImeState`.
    pub(crate) fn with_ime(
        mut self,
        element_id: ElementId,
        ime_registry: &'a RefCell<ImeRegistry>,
    ) -> EventCtx<'a> {
        self.element_id = Some(element_id);
        self.ime_registry = Some(ime_registry);
        self
    }

    /// Element this handler is bound to, if dispatch surfaced one.
    pub fn element_id(&self) -> Option<ElementId> {
        self.element_id
    }

    /// Look up the `ImeState` for `id` from the framework registry. Returns
    /// `None` outside of IME dispatch (no registry bound), when the registry
    /// is currently being borrowed elsewhere, or when `id` isn't registered.
    pub fn ime_state(&self, id: ElementId) -> Option<Rc<RefCell<ImeState>>> {
        let registry = self.ime_registry?;
        registry.try_borrow().ok()?.get(id)
    }

    /// Stop event propagation. Prevents the event from bubbling to parent elements.
    pub fn stop_propagation(&mut self) {
        *self.propagation_stopped = true;
    }

    /// Check if propagation has been stopped.
    pub fn is_propagation_stopped(&self) -> bool {
        *self.propagation_stopped
    }

    /// Queue a focus change. Applied by `AppState` after the handler chain
    /// completes — multiple `request_focus` calls in a chain are last-write-wins.
    ///
    /// Inside a handler, prefer this over `AppContext::set_focus` (which is the
    /// outside-handler entry point and applies immediately).
    pub fn request_focus(&mut self, id: ElementId) {
        *self.pending_focus_op = Some(PendingFocusOp::Focus(id));
    }

    /// Queue a focus clear. Applied after the handler chain completes.
    pub fn blur(&mut self) {
        *self.pending_focus_op = Some(PendingFocusOp::Blur);
    }

    /// Capture the pointer to `id`. Applied by `AppState` after the handler
    /// chain completes; subsequent pointer events route to `id` until capture
    /// is released. Explicit capture is sticky — it survives mouse-up, so a
    /// handler can drive a multi-step drag across button releases. The
    /// framework auto-releases if the captured element is dropped from the tree.
    pub fn set_capture(&mut self, id: ElementId) {
        *self.pending_capture_op = Some(PendingCaptureOp::Set(id));
    }

    /// Release an active pointer capture. Applied after the handler chain
    /// completes. Pairs with [`set_capture`](Self::set_capture).
    pub fn release_capture(&mut self) {
        *self.pending_capture_op = Some(PendingCaptureOp::Release);
    }

    /// The id of the window this event originated in. Single-window today —
    /// the id is available to read but is never used to key per-window state.
    pub fn window_id(&self) -> WindowId {
        self.window_id
    }

    /// Currently focused element id, snapshot at dispatch start. Does NOT
    /// reflect `request_focus` / `blur` calls made during this chain — focus
    /// only mutates after the chain unwinds.
    pub fn focused_element(&self) -> Option<ElementId> {
        self.focused
    }

    /// Take the queued focus op, leaving `None` behind. Crate-private — used
    /// by AppState to apply the op after a handler chain completes.
    #[allow(dead_code)] // Drained by AppState in Phase 3 after dispatch chain unwinds.
    pub(crate) fn take_pending_focus_op(&mut self) -> Option<PendingFocusOp> {
        self.pending_focus_op.take()
    }

    /// Take the queued capture op, leaving `None` behind. Crate-private — used
    /// by AppState to apply the op after a handler chain completes.
    #[allow(dead_code)] // Drained by AppState after the dispatch chain unwinds.
    pub(crate) fn take_pending_capture_op(&mut self) -> Option<PendingCaptureOp> {
        self.pending_capture_op.take()
    }
}

impl std::fmt::Debug for EventCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventCtx")
            .field("propagation_stopped", &*self.propagation_stopped)
            .field("pending_focus_op", &*self.pending_focus_op)
            .field("pending_capture_op", &*self.pending_capture_op)
            .field("window_id", &self.window_id)
            .field("focused", &self.focused)
            .finish()
    }
}
