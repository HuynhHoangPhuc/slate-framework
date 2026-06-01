//! Keyboard + click handlers for [`Tree`](super::Tree).
//!
//! The visible nodes form a 1-D focus ring. Up/Down/Home/End move real focus
//! along it (a screen reader follows). Right/Left act on the *hierarchy*: Right
//! expands a collapsed parent (then a second Right steps into its first child);
//! Left collapses an expanded parent, or on a leaf/collapsed node moves focus to
//! the parent. Enter / Space (and a click) commit the selection.

use std::collections::HashSet;
use std::sync::Arc;

use slate_platform::{Key, NamedKey};

use crate::event::{ElementKeyHandler, EventCtx, Handlers, KeyEvent, KeyHandlers, MouseEvent, MouseHandler};
use crate::reactive::Signal;
use crate::types::ElementId;

use super::TreeActivate;

/// Per-visible-node navigation facts derived from the flattened tree, captured
/// by the handler so it can resolve hierarchy moves without re-walking.
pub(super) struct TreeNav {
    pub node_id: u64,
    pub has_children: bool,
    pub expanded: bool,
    /// Flat index of the nearest shallower ancestor, if any.
    pub parent: Option<usize>,
    /// Flat index of the first child (only when expanded with children).
    pub first_child: Option<usize>,
}

/// Derive [`TreeNav`] for each visible row from its depth/expansion facts.
pub(super) fn build_nav(rows: &[(u64, usize, bool, bool)]) -> Vec<TreeNav> {
    // rows: (node_id, depth, has_children, expanded)
    rows.iter()
        .enumerate()
        .map(|(i, &(node_id, depth, has_children, expanded))| {
            let parent = rows[..i].iter().rposition(|&(_, d, _, _)| d < depth);
            let first_child = (has_children && expanded && rows.get(i + 1).is_some_and(|&(_, d, _, _)| d == depth + 1))
                .then_some(i + 1);
            TreeNav { node_id, has_children, expanded, parent, first_child }
        })
        .collect()
}

fn commit(node_id: u64, selected: &Signal<Option<u64>>, on_activate: &TreeActivate, cx: &mut EventCtx) {
    selected.set(Some(node_id));
    on_activate(node_id, cx);
}

/// Build the navigation + expand/collapse + commit handler shared by the
/// container and every visible row.
pub(super) fn roving_handler(
    active: Signal<usize>,
    expanded: Signal<HashSet<u64>>,
    selected: Signal<Option<u64>>,
    row_ids: Arc<Vec<ElementId>>,
    nav: Arc<Vec<TreeNav>>,
    on_activate: TreeActivate,
) -> ElementKeyHandler {
    Arc::new(move |ev: &KeyEvent, cx: &mut EventCtx| {
        let n = nav.len();
        if n == 0 {
            return;
        }
        let cur = active.get().min(n - 1);
        let info = &nav[cur];
        let move_to = |target: usize, cx: &mut EventCtx| {
            if target != cur {
                active.set(target);
            }
            if let Some(&id) = row_ids.get(target) {
                cx.request_focus(id);
            }
        };
        match ev.key {
            Key::Named(NamedKey::ArrowDown) => move_to((cur + 1).min(n - 1), cx),
            Key::Named(NamedKey::ArrowUp) => move_to(cur.saturating_sub(1), cx),
            Key::Named(NamedKey::Home) => move_to(0, cx),
            Key::Named(NamedKey::End) => move_to(n - 1, cx),
            Key::Named(NamedKey::ArrowRight) => {
                if info.has_children && !info.expanded {
                    let id = info.node_id;
                    expanded.update(|set| {
                        set.insert(id);
                    });
                } else if let Some(child) = info.first_child {
                    move_to(child, cx);
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if info.has_children && info.expanded {
                    let id = info.node_id;
                    expanded.update(|set| {
                        set.remove(&id);
                    });
                } else if let Some(parent) = info.parent {
                    move_to(parent, cx);
                }
            }
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => {
                if !ev.is_repeat {
                    commit(info.node_id, &selected, &on_activate, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    })
}

/// Wrap a key handler as the `on_key_down`-only [`KeyHandlers`].
pub(super) fn key_handlers(h: ElementKeyHandler) -> KeyHandlers {
    KeyHandlers { on_key_down: Some(h), ..Default::default() }
}

/// Build the click handler for the visible row at `index`: commits its node
/// (selection + callback) and stops propagation.
pub(super) fn click_handler(
    node_id: u64,
    selected: Signal<Option<u64>>,
    on_activate: TreeActivate,
) -> Handlers {
    let on_click: MouseHandler = Arc::new(move |_ev: &MouseEvent, cx: &mut EventCtx| {
        commit(node_id, &selected, &on_activate, cx);
        cx.stop_propagation();
    });
    Handlers { on_click: Some(on_click), ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::build_nav;

    // (node_id, depth, has_children, expanded)
    #[test]
    fn nav_resolves_parent_and_first_child() {
        // root(0,expanded) > child-a(1) , child-b(1,expanded) > leaf(2)
        let rows = [(1, 0, true, true), (2, 1, false, false), (3, 1, true, true), (4, 2, false, false)];
        let nav = build_nav(&rows);
        assert_eq!(nav[0].parent, None);
        assert_eq!(nav[0].first_child, Some(1), "root's first child is index 1");
        assert_eq!(nav[1].parent, Some(0), "child-a parent is root");
        assert_eq!(nav[2].parent, Some(0), "child-b parent is root");
        assert_eq!(nav[2].first_child, Some(3), "child-b's child is the leaf");
        assert_eq!(nav[3].parent, Some(2), "leaf parent is child-b");
        assert_eq!(nav[3].first_child, None, "leaf has no child");
    }

    #[test]
    fn collapsed_parent_has_no_first_child() {
        // A parent that is collapsed exposes no first_child even though the model
        // has children (they aren't in the visible list).
        let rows = [(1, 0, true, false), (2, 0, false, false)];
        let nav = build_nav(&rows);
        assert_eq!(nav[0].first_child, None);
        assert!(nav[0].has_children && !nav[0].expanded);
    }
}
