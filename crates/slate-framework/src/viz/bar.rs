//! [`BarChart`] — vertical bars from a `&[f32]` series, drawn with `Rect`
//! primitives only. A leaf, non-interactive element exposing an `Image` a11y
//! node with a generated summary (no per-bar a11y — charts are read as a whole).

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{Element, IntoElement, Sealed};
use crate::style::Style;
use crate::theme::theme;
use crate::types::{
    AccessibilityInfo, AccessibilityNode, AccessibilityRole, Bounds, ElementId, LayoutId,
    NodeContext,
};

use super::{fill_rect, series_summary};

/// A vertical bar chart over a fixed `&[f32]` series (see module docs).
pub struct BarChart {
    values: Vec<f32>,
    width: f32,
    height: f32,
    fill: [f32; 4],
    baseline: [f32; 4],
    label: Option<String>,
    last_id: Option<ElementId>,
}

impl BarChart {
    /// Create a bar chart over `values`, themed with the `accent` fill. Bars are
    /// normalized against the series max; negative values are clamped to a
    /// zero-height bar (this is a magnitude chart, not a signed axis).
    pub fn new(values: impl IntoIterator<Item = f32>) -> Self {
        let t = theme();
        Self {
            values: values.into_iter().collect(),
            width: 240.0,
            height: 120.0,
            fill: t.accent.into(),
            baseline: t.border.into(),
            label: None,
            last_id: None,
        }
    }

    /// Set the chart's fixed size in logical pixels.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = width.max(0.0);
        self.height = height.max(0.0);
        self
    }

    /// Set the accessible name (prefixed onto the generated summary).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl Element for BarChart {
    type LayoutState = ();
    type PaintState = ();

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, ()) {
        let style = Style::default().width(self.width).height(self.height);
        let node = cx
            .taffy
            .new_leaf(taffy::Style::from(&style))
            .unwrap_or_else(|e| {
                log::error!("BarChart: new_leaf failed ({e})");
                taffy::NodeId::from(u64::MAX)
            });
        let _ = cx.taffy.set_node_context(node, Some(NodeContext::None));
        (LayoutId(node), ())
    }

    fn prepaint(&mut self, bounds: Bounds, _: &mut (), cx: &mut PrepaintCtx) {
        let id = cx.allocate_id::<BarChart>();
        self.last_id = Some(id);
        cx.register_a11y_node(AccessibilityNode {
            id,
            bounds,
            info: AccessibilityInfo {
                role: AccessibilityRole::Image,
                label: Some(series_summary("Bar chart", self.label.as_deref(), &self.values)),
                ..Default::default()
            },
            children: Vec::new(),
            actions: Vec::new(),
        });
    }

    fn paint(&mut self, bounds: Bounds, _: &mut (), _: &mut (), cx: &mut PaintCtx) {
        let n = self.values.len();
        // Baseline hairline along the bottom.
        fill_rect(
            cx.scene,
            Bounds::from_origin_size(bounds.origin.x, bounds.origin.y + bounds.size.height - 1.0, bounds.size.width, 1.0),
            self.baseline,
        );
        if n == 0 {
            return;
        }
        let max = self.values.iter().copied().fold(f32::MIN, f32::max).max(f32::MIN_POSITIVE);
        let slot = bounds.size.width / n as f32;
        let gap = (slot * 0.2).min(4.0);
        let bar_w = (slot - gap).max(1.0);
        for (i, &v) in self.values.iter().enumerate() {
            let frac = (v.max(0.0) / max).clamp(0.0, 1.0);
            let h = frac * (bounds.size.height - 1.0);
            let x = bounds.origin.x + i as f32 * slot + gap * 0.5;
            let y = bounds.origin.y + (bounds.size.height - 1.0) - h;
            fill_rect(cx.scene, Bounds::from_origin_size(x, y, bar_w, h), self.fill);
        }
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

impl Sealed for BarChart {}

impl IntoElement for BarChart {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
