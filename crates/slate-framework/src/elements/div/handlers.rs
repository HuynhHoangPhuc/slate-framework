//! Div event handler, focus, and IME builders.

use std::sync::Arc;

use crate::event::{
    EventCtx, KeyEvent, MouseEvent, PointerEvent, ScrollEvent, TextInputEvent,
};

use super::Div;

impl Div {
    // -------------------------------------------------------------------------
    // Event handler builders
    // -------------------------------------------------------------------------

    /// Register a click handler (fires when mouse down+up lands on same target).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let count = Signal::new(cx.runtime(), 0);
    /// Div::new().on_click(move |_, _| count.set(count.get() + 1))
    /// ```
    pub fn on_click<F>(mut self, handler: F) -> Self
    where
        F: Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_click = Some(Arc::new(handler));
        self
    }

    /// Register a mouse down handler.
    ///
    /// # Example
    ///
    /// ```ignore
    /// Div::new().on_mouse_down(|event, ctx| {
    ///     println!("Down at {:?}", event.position);
    ///     ctx.stop_propagation();
    /// })
    /// ```
    pub fn on_mouse_down<F>(mut self, handler: F) -> Self
    where
        F: Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_mouse_down = Some(Arc::new(handler));
        self
    }

    /// Register a mouse up handler.
    pub fn on_mouse_up<F>(mut self, handler: F) -> Self
    where
        F: Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_mouse_up = Some(Arc::new(handler));
        self
    }

    /// Register a mouse move handler (coalesced, one call per frame).
    pub fn on_mouse_move<F>(mut self, handler: F) -> Self
    where
        F: Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_mouse_move = Some(Arc::new(handler));
        self
    }

    /// Register a scroll wheel/trackpad handler.
    ///
    /// # Example
    ///
    /// ```ignore
    /// Div::new().on_mouse_scrolled(|event, _| {
    ///     let scale = if event.precise { 1.0 } else { 12.0 };
    ///     scroll_offset.set(scroll_offset.get() + event.delta_y * scale);
    /// })
    /// ```
    pub fn on_mouse_scrolled<F>(mut self, handler: F) -> Self
    where
        F: Fn(&ScrollEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_mouse_scrolled = Some(Arc::new(handler));
        self
    }

    /// Register a raw pointer event handler (no coalescing).
    ///
    /// Receives all pointer events (down, up, move, enter, leave) in order.
    pub fn on_pointer_event<F>(mut self, handler: F) -> Self
    where
        F: Fn(&PointerEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_pointer_event = Some(Arc::new(handler));
        self
    }

    /// Register a pointer enter handler.
    pub fn on_pointer_enter<F>(mut self, handler: F) -> Self
    where
        F: Fn(&PointerEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_pointer_enter = Some(Arc::new(handler));
        self
    }

    /// Register a pointer leave handler.
    pub fn on_pointer_leave<F>(mut self, handler: F) -> Self
    where
        F: Fn(&PointerEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_pointer_leave = Some(Arc::new(handler));
        self
    }

    // -------------------------------------------------------------------------
    // Focus configuration builders
    // -------------------------------------------------------------------------

    /// Opt this element in to keyboard focus.
    ///
    /// Without this call (or with `false`), the element is invisible to the
    /// focus system — it will not register with `FocusRegistry`, will not be
    /// reachable via Tab, and `AppContext::set_focus` will fail for its id.
    pub fn focusable(mut self, focusable: bool) -> Self {
        self.focusable = focusable;
        self
    }

    /// Set the W3C-style `tab_index`.
    ///
    /// - `0` (default): joins the Tab cycle at registration order.
    /// - Positive: hoisted earlier in the Tab cycle by ascending value, then
    ///   registration order within equal indices.
    /// - Negative: excluded from the Tab cycle but still focusable via
    ///   `AppContext::set_focus` or `EventCtx::request_focus`.
    pub fn tab_index(mut self, tab_index: i32) -> Self {
        self.tab_index = tab_index;
        self
    }

    /// Toggle the framework's hardcoded 2px accent-color focus ring.
    ///
    /// Defaults to `true` and is only consulted when [`Div::focusable`] is
    /// also `true`. Set to `false` when the element renders its own focus
    /// indicator.
    pub fn focus_ring(mut self, focus_ring: bool) -> Self {
        self.focus_ring = focus_ring;
        self
    }

    // -------------------------------------------------------------------------
    // Per-element keyboard handler builders
    // -------------------------------------------------------------------------

    /// Register a `KeyDown` handler that fires while this element is on the
    /// focused chain.
    ///
    /// # Example
    /// ```ignore
    /// Div::new()
    ///     .focusable(true)
    ///     .on_key_down(|ev, cx| {
    ///         if ev.key == Key::Named(NamedKey::Enter) {
    ///             // ...
    ///             cx.stop_propagation();
    ///         }
    ///     })
    /// ```
    pub fn on_key_down<F>(mut self, handler: F) -> Self
    where
        F: Fn(&KeyEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_key_down = Some(Arc::new(handler));
        self
    }

    /// Register a `KeyUp` handler. See [`Div::on_key_down`].
    pub fn on_key_up<F>(mut self, handler: F) -> Self
    where
        F: Fn(&KeyEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_key_up = Some(Arc::new(handler));
        self
    }

    /// Register a composed text-input handler. Fires once per keystroke that
    /// produces visible text (or per surrogate pair on Windows).
    pub fn on_text_input<F>(mut self, handler: F) -> Self
    where
        F: Fn(&TextInputEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_text_input = Some(Arc::new(handler));
        self
    }

    /// Opt this element into IME composition. Required for `on_ime_preedit` /
    /// `on_ime_commit` handlers to fire and for the OS sync-query channel to
    /// see the element's caret rect and committed buffer.
    pub fn ime_capable(mut self, ime_capable: bool) -> Self {
        self.ime_capable = ime_capable;
        self
    }

    /// Register an IME preedit handler. Receives composition updates while
    /// this element is on the focused chain.
    pub fn on_ime_preedit<F>(mut self, handler: F) -> Self
    where
        F: Fn(&crate::event::ImePreeditEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_ime_preedit = Some(Arc::new(handler));
        self
    }

    /// Register an IME commit handler. Empty `text` is the "clear preedit, no
    /// insert" signal — handlers MUST skip `Signal::set` in that case.
    pub fn on_ime_commit<F>(mut self, handler: F) -> Self
    where
        F: Fn(&crate::event::ImeCommitEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_ime_commit = Some(Arc::new(handler));
        self
    }
}
