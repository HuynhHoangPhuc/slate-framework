//! Prepaint for [`MenuList`](super::MenuList): resolves the roving-focus index
//! from `use_state`, allocates stable per-row ids, wires the shared nav/click
//! handlers, and builds the `Menu` → `MenuItem` accessibility subtree. Split
//! from `mod.rs` to stay under the file-size budget.

use std::sync::Arc;

use crate::context::PrepaintCtx;
use crate::focus::FocusableEntry;
use crate::hit_test::{CursorStyle, HitRegion};
use crate::types::{
    AccessibilityAction, AccessibilityInfo, AccessibilityNode, AccessibilityRole, Bounds, ElementId,
};

use super::keys::{click_handler, roving_handler};
use super::{MenuList, MenuListLayout};
use crate::event::KeyHandlers;
use crate::layout::resolve_child_bounds;

/// Distinct marker so row ids never collide with the container id (the id hash
/// includes the `TypeId`).
struct RowMarker;

/// Build the focus/handler registrations + a11y subtree for one frame.
pub(super) fn build(
    menu: &mut MenuList,
    bounds: Bounds,
    layout: &mut MenuListLayout,
    cx: &mut PrepaintCtx,
) {
    let n = menu.entries.len();
    let container_id = cx.allocate_id::<MenuList>();
    menu.last_id = Some(container_id);

    // Roving active index: transient per-mount state, reset to `initial_active`
    // each time the menu (re)appears after GC. Snapshot for paint.
    let start = menu.initial_active.min(n.saturating_sub(1));
    let active = cx.state_registry.use_state::<usize>(container_id, move || start);
    menu.active_value = active.get().min(n.saturating_sub(1));

    cx.push_frame(container_id);

    // Stable row ids up front so the nav handler can capture them.
    let mut row_ids: Vec<ElementId> = Vec::with_capacity(n);
    for i in 0..n {
        cx.set_next_key(format!("item-{i}"));
        row_ids.push(cx.allocate_id::<RowMarker>());
    }
    let row_ids = Arc::new(row_ids);
    let disabled = Arc::new(menu.entries.iter().map(|e| e.disabled).collect::<Vec<_>>());
    let handler = roving_handler(active, row_ids.clone(), disabled, menu.on_activate.clone());

    // Container = the tab stop + autofocus target. Registered before rows so its
    // hit region sits behind them; its handler routes the first arrow into a row.
    cx.register_focusable(
        FocusableEntry { id: container_id, tab_index: 0, focus_ring: false },
        bounds,
        0.0,
    );
    cx.register_key_handlers(container_id, key_handlers(handler.clone()));
    cx.register_hit_region(HitRegion::new(container_id, bounds, 0));

    cx.prepaint_node_open(container_id, bounds, menu_info());
    for i in 0..n {
        let rb = resolve_child_bounds(cx.taffy, layout.container, i, bounds.origin)
            .unwrap_or(Bounds::ZERO);
        let id = row_ids[i];
        let disabled = menu.entries[i].disabled;
        cx.register_a11y_node(item_node(menu, id, rb, i, disabled));
        if !disabled {
            cx.register_focusable(
                FocusableEntry { id, tab_index: -1, focus_ring: true },
                rb,
                4.0,
            );
            cx.register_key_handlers(id, key_handlers(handler.clone()));
            cx.register_handlers(id, click_handler(i, menu.on_activate.clone()));
            cx.register_hit_region(HitRegion::new(id, rb, 0).with_cursor(CursorStyle::Arrow));
        }
    }
    cx.prepaint_node_close();

    // Prepaint each presentational row label (no a11y node, but paint requires
    // the ElementState to have been prepainted first).
    for i in 0..menu.rows.len() {
        let rb = resolve_child_bounds(cx.taffy, layout.container, i, bounds.origin).unwrap_or(Bounds::ZERO);
        if let Some(lb) = resolve_child_bounds(cx.taffy, layout.rows[i], 0, rb.origin) {
            menu.rows[i].prepaint(lb, cx);
        }
    }

    cx.pop_frame();
}

fn key_handlers(h: crate::event::ElementKeyHandler) -> KeyHandlers {
    KeyHandlers { on_key_down: Some(h), ..Default::default() }
}

fn menu_info() -> AccessibilityInfo {
    AccessibilityInfo {
        role: AccessibilityRole::Menu,
        ..Default::default()
    }
}

fn item_node(menu: &MenuList, id: ElementId, bounds: Bounds, index: usize, disabled: bool) -> AccessibilityNode {
    AccessibilityNode {
        id,
        bounds,
        info: AccessibilityInfo {
            role: AccessibilityRole::MenuItem,
            label: Some(menu.entries[index].label.clone()),
            is_disabled: disabled,
            // `Some(false)` on non-selected rows of a selectable menu so AT
            // announces the state; `None` for action menus (selected == None).
            is_selected: menu.selected.map(|s| s == index),
            ..Default::default()
        },
        children: Vec::new(),
        actions: vec![AccessibilityAction::Focus, AccessibilityAction::Click],
    }
}
