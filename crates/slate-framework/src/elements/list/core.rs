//! Shared data + accessibility builders for the list family
//! ([`List`](super::List) and [`VirtualList`](super::VirtualList)).
//!
//! Both widgets render the same row shape (a presentational label per row), the
//! same `List` → `ListItem` accessibility subtree (container carries the true
//! `row_count`; each item carries its zero-based `row_index` + `is_selected`),
//! and the same caller-owned single-selection model. The pieces that don't
//! depend on whether rows are fully materialized (List) or windowed
//! (VirtualList) live here so neither re-derives them.

use crate::types::{
    AccessibilityAction, AccessibilityInfo, AccessibilityNode, AccessibilityRole, Bounds, ElementId,
};

/// One row of a list: a visible/accessible label and a disabled flag.
#[derive(Clone)]
pub struct ListEntry {
    /// Visible text + accessible name of the row.
    pub label: String,
    /// Disabled rows are skipped by roving focus, ignore selection, and report
    /// `is_disabled` to assistive tech.
    pub disabled: bool,
}

impl ListEntry {
    /// Create an enabled entry with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), disabled: false }
    }

    /// Mark this entry disabled.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl From<&str> for ListEntry {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for ListEntry {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

/// Accessibility node for the list container: `List` role carrying the **true**
/// total `row_count` (even when only a window of rows is materialized, as in
/// [`VirtualList`](super::VirtualList)).
pub(crate) fn list_container_info(label: Option<&str>, row_count: usize) -> AccessibilityInfo {
    AccessibilityInfo {
        role: AccessibilityRole::List,
        label: label.map(str::to_string),
        row_count: Some(row_count),
        ..Default::default()
    }
}

/// Accessibility node for one list row: `ListItem` carrying its zero-based
/// absolute `row_index`, accessible name, selected state, and the Focus/Click
/// actions assistive tech can invoke.
///
/// `selected` is `Some(false)` (not `None`) on the unselected rows of a list
/// that has a selection model, so AT announces "not selected" rather than
/// staying silent; pass `None` only for a list with no selection concept.
pub(crate) fn list_item_node(
    id: ElementId,
    bounds: Bounds,
    label: &str,
    row_index: usize,
    selected: Option<bool>,
    disabled: bool,
) -> AccessibilityNode {
    AccessibilityNode {
        id,
        bounds,
        info: AccessibilityInfo {
            role: AccessibilityRole::ListItem,
            label: Some(label.to_string()),
            is_disabled: disabled,
            is_selected: selected,
            row_index: Some(row_index),
            ..Default::default()
        },
        children: Vec::new(),
        actions: vec![AccessibilityAction::Focus, AccessibilityAction::Click],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_reports_role_and_true_row_count() {
        let info = list_container_info(Some("Processes"), 250);
        assert_eq!(info.role, AccessibilityRole::List);
        assert_eq!(info.label.as_deref(), Some("Processes"));
        assert_eq!(info.row_count, Some(250), "true total, not the windowed count");
    }

    #[test]
    fn item_carries_zero_based_index_selection_and_actions() {
        let b = Bounds::from_origin_size(0.0, 0.0, 10.0, 10.0);
        let node = list_item_node(ElementId::from_raw(7), b, "Safari", 3, Some(true), false);
        assert_eq!(node.info.role, AccessibilityRole::ListItem);
        assert_eq!(node.info.label.as_deref(), Some("Safari"));
        assert_eq!(node.info.row_index, Some(3));
        assert_eq!(node.info.is_selected, Some(true));
        assert!(!node.info.is_disabled);
        assert!(node.actions.contains(&AccessibilityAction::Focus));
        assert!(node.actions.contains(&AccessibilityAction::Click));
    }

    #[test]
    fn unselected_item_announces_not_selected() {
        // `Some(false)` (not `None`) so AT says "not selected" rather than silent.
        let b = Bounds::ZERO;
        let node = list_item_node(ElementId::from_raw(1), b, "row", 0, Some(false), false);
        assert_eq!(node.info.is_selected, Some(false));
    }

    #[test]
    fn list_entry_from_str_is_enabled_by_default() {
        let e: ListEntry = "CPU".into();
        assert_eq!(e.label, "CPU");
        assert!(!e.disabled);
        assert!(ListEntry::new("x").disabled(true).disabled);
    }
}
