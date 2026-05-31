//! Div `Element` trait implementation — layout, prepaint, paint, accessibility.

use slate_renderer::Lpx;
use slate_renderer::scene::RectInstance;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::Element;
use crate::event::{Handlers, KeyHandlers};
use crate::focus::FocusableEntry;
use crate::hit_test::{CursorStyle, HitRegion};
use crate::layout::resolve_child_bounds;
use crate::types::{
    AccessibilityInfo, AccessibilityRole, Bounds, ElementId, LayoutId, NodeContext,
};

use super::{Div, DivLayoutState, DivPaintState};

impl Div {
    /// Resolve the painted background + corner radius against the current
    /// interaction state. Returns the base visual unchanged when there are no
    /// per-state overrides — the zero-cost fast path that performs no state
    /// lookups and paints byte-identically to a plain styled `Div`.
    ///
    /// Resolution order is base → hover → active(pressed) → disabled, so a
    /// disabled override always wins.
    fn resolved_visual(&self, cx: &PaintCtx) -> (Option<[f32; 4]>, f32) {
        let mut background = self.visual.background;
        let mut corner_radius = self.visual.corner_radius;
        let Some(styles) = self.state_styles.as_deref() else {
            return (background, corner_radius);
        };
        if let Some(id) = self.last_id {
            if cx.is_hovered(id) {
                styles.hover.apply(&mut background, &mut corner_radius);
            }
            if cx.is_pressed(id) {
                styles.active.apply(&mut background, &mut corner_radius);
            }
        }
        if self.disabled {
            styles.disabled.apply(&mut background, &mut corner_radius);
        }
        (background, corner_radius)
    }
}

impl Element for Div {
    type LayoutState = DivLayoutState;
    type PaintState = DivPaintState;

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        // Layout children first, collect their node IDs
        let mut child_nodes = Vec::with_capacity(self.children.len());
        for child in &mut self.children {
            let layout_id = child.request_layout(cx);
            child_nodes.push(layout_id.0);
        }

        // Convert unified Style to taffy::Style
        let taffy_style = taffy::Style::from(&self.layout_style);
        let node_id = match cx.taffy.new_with_children(taffy_style, &child_nodes) {
            Ok(id) => id,
            Err(e) => {
                log::error!("Div: failed to create Taffy node: {e}; rendering empty");
                // Sentinel: empty leaf node so layout completes; widget renders zero-size
                match cx.taffy.new_leaf(taffy::Style::default()) {
                    Ok(id) => id,
                    Err(e2) => {
                        log::error!("Div: Taffy new_leaf also failed ({e2}) — pathological state");
                        taffy::NodeId::from(u64::MAX)
                    }
                }
            }
        };

        // Set node context (container) — non-fatal if fails
        if let Err(e) = cx.taffy.set_node_context(node_id, Some(NodeContext::None)) {
            log::error!("Div: failed to set node context: {e}; layout proceeds without context");
        }

        (LayoutId(node_id), DivLayoutState { node_id })
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        // Tree-position keying: set user key if provided, allocate stable ID
        if let Some(k) = self.user_key.take() {
            cx.set_next_key(k);
        }
        let element_id = cx.allocate_id::<Div>();
        self.last_id = Some(element_id);

        // Report painted bounds into the caller's signal for overlay
        // anchor-to-element. Guard the write: `Signal::set` always notifies and
        // marks the runtime dirty, so an unconditional set every prepaint would
        // spin the whole-view rebuild loop. Read untracked (no observer here).
        if let Some(sig) = &self.track_bounds
            && sig.get_untracked() != bounds
        {
            sig.set(bounds);
        }

        // Disabled elements are non-interactive: skip registering event,
        // keyboard, IME, and focus participation so dispatch never reaches them
        // (the hit region below is still registered so they block fall-through).
        let interactive = !self.disabled;

        // Register event handlers for dispatch
        if interactive {
            cx.register_handlers(
                element_id,
                Handlers {
                    on_click: self.on_click.clone(),
                    on_mouse_down: self.on_mouse_down.clone(),
                    on_mouse_up: self.on_mouse_up.clone(),
                    on_mouse_move: self.on_mouse_move.clone(),
                    on_mouse_scrolled: self.on_mouse_scrolled.clone(),
                    on_pointer_event: self.on_pointer_event.clone(),
                    on_pointer_enter: self.on_pointer_enter.clone(),
                    on_pointer_leave: self.on_pointer_leave.clone(),
                },
            );

            // Register per-element keyboard handlers + focusable entry.
            cx.register_key_handlers(
                element_id,
                KeyHandlers {
                    on_key_down: self.on_key_down.clone(),
                    on_key_up: self.on_key_up.clone(),
                    on_text_input: self.on_text_input.clone(),
                },
            );

            // Register IME state + handlers when the element opts in.
            if self.ime_capable {
                cx.register_ime_state(element_id);
                cx.register_ime_handlers(
                    element_id,
                    crate::event::ImeHandlers {
                        on_ime_preedit: self.on_ime_preedit.clone(),
                        on_ime_commit: self.on_ime_commit.clone(),
                        on_ime_enabled: None,
                        on_ime_disabled: None,
                    },
                );
            }
        }
        if self.focusable && interactive {
            cx.register_focusable(
                FocusableEntry {
                    id: element_id,
                    tab_index: self.tab_index,
                    focus_ring: self.focus_ring,
                },
                bounds,
                self.visual.corner_radius,
            );
        }

        // Register hit region if Div has background OR any mouse handler.
        // Transparent divs with handlers still need hit regions.
        let has_any_handler = self.on_click.is_some()
            || self.on_mouse_down.is_some()
            || self.on_mouse_up.is_some()
            || self.on_mouse_move.is_some()
            || self.on_mouse_scrolled.is_some()
            || self.on_pointer_event.is_some()
            || self.on_pointer_enter.is_some()
            || self.on_pointer_leave.is_some();
        if self.visual.background.is_some() || has_any_handler {
            cx.register_hit_region(
                HitRegion::new(element_id, bounds, 0).with_cursor(CursorStyle::Arrow),
            );
        }

        // Open a11y node before children (will accumulate children's a11y nodes)
        // Order: prepaint_node_open → push_frame → recurse → pop_frame → prepaint_node_close
        //
        // PANIC SAFETY: If child.prepaint() panics, a11y_stack will be left unbalanced.
        // - Debug builds: frame-end debug_assert catches this before render
        // - Release builds: corrupted a11y tree is non-fatal (screen reader sees flat tree)
        // - Child prepaint implementations should not panic in normal operation
        let opened_a11y = if let Some(info) = self.accessibility() {
            cx.prepaint_node_open(element_id, bounds, info);
            true
        } else {
            false
        };

        // Push frame so children's IDs derive from this Div's ID
        cx.push_frame(element_id);

        // Prepaint children with their resolved bounds
        for (i, child) in self.children.iter_mut().enumerate() {
            if let Some(child_bounds) =
                resolve_child_bounds(cx.taffy, layout_state.node_id, i, bounds.origin)
            {
                child.prepaint(child_bounds, cx);
            }
        }

        // Pop frame after children are processed
        cx.pop_frame();

        // Close a11y node after children (children are now nested)
        if opened_a11y {
            cx.prepaint_node_close();
        }

        DivPaintState
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        _paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        // Resolve the painted visual against interaction state. The fast path
        // (no per-state overrides) skips all state lookups and is byte-identical
        // to a plain styled Div.
        let (background, corner_radius) = self.resolved_visual(cx);

        // Paint background if set. Scene wire format is in logical pixels;
        // the renderer's viewport maps lpx → NDC, so no `* scale_factor` here.
        if let Some(color) = background {
            cx.scene.push_rect(RectInstance {
                rect: [
                    Lpx(bounds.origin.x),
                    Lpx(bounds.origin.y),
                    Lpx(bounds.size.width),
                    Lpx(bounds.size.height),
                ],
                color,
                corner_radius: Lpx(corner_radius),
                _pad: [0.0; 3],
            });
        }

        // Paint children
        for (i, child) in self.children.iter_mut().enumerate() {
            if let Some(child_bounds) =
                resolve_child_bounds(cx.taffy, layout_state.node_id, i, bounds.origin)
            {
                child.paint(child_bounds, cx);
            }
        }
    }

    fn accessibility(&self) -> Option<AccessibilityInfo> {
        Some(AccessibilityInfo {
            role: AccessibilityRole::Group,
            is_disabled: self.disabled,
            ..Default::default()
        })
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}
