//! Tree — a keyboard-navigable hierarchy of expand/collapse nodes.
//!
//! [`Tree`] renders a caller-owned forest of [`TreeNode`]s as an indented column
//! of rows. The currently-visible nodes (a parent's children show only when the
//! parent is expanded) form a 1-D roving-focus ring — the single-element pattern
//! [`List`](crate::List) and the S0 grid spike share, so the container allocates
//! every visible-node id and arrow keys move *real* focus for a screen reader to
//! follow. It emits a `Tree` → `TreeItem` accessibility subtree, with
//! `is_expanded` + Expand/Collapse actions on the expandable nodes.
//!
//! ## State is caller-owned (Strategy A)
//!
//! Expansion is a `Signal<HashSet<u64>>` of expanded node ids; selection is a
//! `Signal<Option<u64>>` of the chosen node id. Create both outside `render` so
//! they survive the whole-view rebuild each toggle triggers. The roving "active"
//! row is transient per-mount state, re-clamped after every expand/collapse so a
//! collapse never strands focus past the shortened visible list.

mod keys;
mod layout;
mod node;
mod prepaint;

use std::collections::HashSet;
use std::sync::Arc;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{AnyElement, Element, IntoElement, Sealed};
use crate::elements::control_paint::presentational_label;
use crate::event::EventCtx;
use crate::reactive::Signal;
use crate::theme::theme;
use crate::types::{Bounds, ElementId, LayoutId};

pub use node::TreeNode;
use node::{VisibleNode, flatten_visible};

/// Activation callback fired on Enter / click, *after* the selection signal is
/// written. Receives the activated node's id and the [`EventCtx`].
pub(crate) type TreeActivate = Arc<dyn Fn(u64, &mut EventCtx) + Send + Sync + 'static>;

/// A keyboard-navigable hierarchy of expand/collapse nodes.
///
/// # Example
/// ```ignore
/// // `expanded` and `selected` are created once (outside `render`).
/// Tree::new(roots, expanded.clone(), selected.clone())
///     .label("Files")
///     .on_activate(|id, _cx| log::info!("activated node {id}"))
/// ```
pub struct Tree {
    /// The currently-visible nodes (flattened depth-first from the roots under
    /// the current expansion), resolved under the render observer at build time.
    visible: Vec<VisibleNode>,
    /// Presentational `Text` label per visible row (twisty glyph + node name);
    /// the `TreeItem` node carries the clean accessible name.
    rows: Vec<AnyElement>,
    /// Caller-owned expansion set (expanded node ids).
    expanded: Signal<HashSet<u64>>,
    /// Caller-owned selection (selected node id).
    selected: Signal<Option<u64>>,
    /// Roving-focus index resolved from `use_state` during prepaint, reused at
    /// paint.
    active_value: usize,
    on_activate: TreeActivate,
    label: Option<String>,
    accent: [f32; 4],
    /// Per-row height, base horizontal padding, and per-depth indent (logical px).
    row_h: f32,
    pad_x: f32,
    indent: f32,
    last_id: Option<ElementId>,
}

/// Layout state for [`Tree`] — the column node plus one wrapper node per visible row.
pub struct TreeLayout {
    container: taffy::NodeId,
    rows: Vec<taffy::NodeId>,
}

/// Leading glyph for a row: open/closed twisty for parents, blank for leaves
/// (kept in the visible label so it lines up; the a11y name stays clean).
fn twisty(has_children: bool, expanded: bool) -> &'static str {
    match (has_children, expanded) {
        (true, true) => "\u{25BE} ",  // ▾
        (true, false) => "\u{25B8} ", // ▸
        (false, _) => "   ",
    }
}

impl Tree {
    /// Create a tree from its root [`TreeNode`]s, bound to caller-owned
    /// expansion and selection signals.
    pub fn new(
        roots: Vec<TreeNode>,
        expanded: Signal<HashSet<u64>>,
        selected: Signal<Option<u64>>,
    ) -> Self {
        let expanded_now = expanded.get(); // subscribe the render observer
        let selected_now = selected.get();
        let visible = flatten_visible(&roots, &expanded_now, selected_now);
        let t = theme();
        let fg: [f32; 4] = t.fg.into();
        let rows = visible
            .iter()
            .map(|v| {
                let text = format!("{}{}", twisty(v.has_children, v.expanded), v.label);
                presentational_label(&text, fg, t.typography.body.size)
            })
            .collect();
        Self {
            visible,
            rows,
            expanded,
            selected,
            active_value: 0,
            on_activate: Arc::new(|_, _| {}),
            label: None,
            accent: t.accent.into(),
            row_h: t.typography.body.size + t.spacing.md,
            pad_x: t.spacing.md,
            indent: t.spacing.lg,
            last_id: None,
        }
    }

    /// Set the activation callback (node id + [`EventCtx`]), fired on Enter /
    /// click after the selection signal is written.
    pub fn on_activate<F>(mut self, f: F) -> Self
    where
        F: Fn(u64, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_activate = Arc::new(f);
        self
    }

    /// Set the accessible name announced for the tree container.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl Element for Tree {
    type LayoutState = TreeLayout;
    type PaintState = ();

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        layout::request(self, cx)
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        prepaint::build(self, bounds, layout_state, cx);
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        _paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        layout::paint(self, bounds, layout_state, cx);
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

impl Sealed for Tree {}

impl IntoElement for Tree {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
