//! Prepaint for [`List`](super::List): resolves the roving-focus index from
//! `use_state`, allocates stable per-row ids, wires the shared roving/commit
//! handlers, and builds the `List` → `ListItem` accessibility subtree.

use std::sync::Arc;

use crate::context::PrepaintCtx;
use crate::elements::list::core::{list_container_info, list_item_node};
use crate::focus::FocusableEntry;
use crate::hit_test::{CursorStyle, HitRegion};
use crate::layout::resolve_child_bounds;
use crate::types::{Bounds, ElementId};

use super::keys::{click_handler, key_handlers, roving_handler};
use super::{List, ListLayout};

/// Distinct marker so row ids never collide with the container id (the id hash
/// includes the `TypeId`).
struct RowMarker;

/// Build the focus/handler registrations + a11y subtree for one frame.
pub(super) fn build(list: &mut List, bounds: Bounds, layout: &mut ListLayout, cx: &mut PrepaintCtx) {
    let n = list.entries.len();
    let container_id = cx.allocate_id::<List>();
    list.last_id = Some(container_id);

    // Roving active index: transient per-mount state. Starts on the committed
    // selection (so the focus ring opens where the user left it) or the first
    // row, then `use_state` persists the live value across rebuilds.
    let start = list.selected_value.unwrap_or(0).min(n.saturating_sub(1));
    let active = cx.state_registry.use_state::<usize>(container_id, move || start);
    list.active_value = active.get().min(n.saturating_sub(1));

    cx.push_frame(container_id);

    // Stable row ids up front so the roving handler can capture them.
    let mut row_ids: Vec<ElementId> = Vec::with_capacity(n);
    for i in 0..n {
        cx.set_next_key(format!("item-{i}"));
        row_ids.push(cx.allocate_id::<RowMarker>());
    }
    let row_ids = Arc::new(row_ids);
    let disabled = Arc::new(list.entries.iter().map(|e| e.disabled).collect::<Vec<_>>());
    let handler = roving_handler(
        active,
        list.selected.clone(),
        row_ids.clone(),
        disabled,
        list.on_activate.clone(),
    );

    // Container = the tab stop + autofocus target. Registered before rows so its
    // hit region sits behind them; its handler routes the first arrow into a row.
    cx.register_focusable(
        FocusableEntry { id: container_id, tab_index: 0, focus_ring: false },
        bounds,
        0.0,
    );
    cx.register_key_handlers(container_id, key_handlers(handler.clone()));
    cx.register_hit_region(HitRegion::new(container_id, bounds, 0));

    cx.prepaint_node_open(container_id, bounds, list_container_info(list.label.as_deref(), n));
    for i in 0..n {
        let rb =
            resolve_child_bounds(cx.taffy, layout.container, i, bounds.origin).unwrap_or(Bounds::ZERO);
        let id = row_ids[i];
        let disabled = list.entries[i].disabled;
        // `Some(false)` on unselected rows so AT announces "not selected".
        let selected = Some(list.selected_value == Some(i));
        cx.register_a11y_node(list_item_node(id, rb, &list.entries[i].label, i, selected, disabled));
        if !disabled {
            cx.register_focusable(FocusableEntry { id, tab_index: -1, focus_ring: true }, rb, 4.0);
            cx.register_key_handlers(id, key_handlers(handler.clone()));
            cx.register_handlers(
                id,
                click_handler(i, list.selected.clone(), list.on_activate.clone()),
            );
            cx.register_hit_region(HitRegion::new(id, rb, 0).with_cursor(CursorStyle::Arrow));
        }
    }
    cx.prepaint_node_close();

    cx.pop_frame();
}
