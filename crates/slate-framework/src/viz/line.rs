//! [`Sparkline`] — a single polyline over a normalized `&[f32]` series in a
//! fixed box, drawn with the thin-quad [`polyline`](super::polyline) helper (no
//! line pipeline). A leaf, non-interactive element with an `Image` a11y summary.
//! Fill-under and axes are intentionally omitted (YAGNI / charter OUT).

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{Element, IntoElement, Sealed};
use crate::style::Style;
use crate::theme::theme;
use crate::types::{
    AccessibilityInfo, AccessibilityNode, AccessibilityRole, Bounds, ElementId, LayoutId,
    NodeContext, Point,
};

use super::polyline::emit_polyline;
use super::series_summary;

/// A sparkline / line chart over a fixed `&[f32]` series (see module docs).
pub struct Sparkline {
    values: Vec<f32>,
    width: f32,
    height: f32,
    stroke: [f32; 4],
    stroke_width: f32,
    label: Option<String>,
    last_id: Option<ElementId>,
}

impl Sparkline {
    /// Create a sparkline over `values`, themed with the `accent` stroke.
    pub fn new(values: impl IntoIterator<Item = f32>) -> Self {
        let t = theme();
        Self {
            values: values.into_iter().collect(),
            width: 240.0,
            height: 64.0,
            stroke: t.accent.into(),
            stroke_width: 1.5,
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

    /// Map the series into screen points inside `bounds` (normalized min→max,
    /// y inverted so larger values sit higher). Empty/flat series are handled.
    fn points(&self, bounds: Bounds) -> Vec<Point> {
        let n = self.values.len();
        if n == 0 {
            return Vec::new();
        }
        let min = self.values.iter().copied().fold(f32::MAX, f32::min);
        let max = self.values.iter().copied().fold(f32::MIN, f32::max);
        let span = (max - min).max(f32::MIN_POSITIVE);
        let pad = self.stroke_width;
        let usable_h = (bounds.size.height - 2.0 * pad).max(0.0);
        let step = if n > 1 { bounds.size.width / (n - 1) as f32 } else { 0.0 };
        self.values
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let frac = (v - min) / span;
                let x = bounds.origin.x + i as f32 * step;
                let y = bounds.origin.y + pad + (1.0 - frac) * usable_h;
                Point::new(x, y)
            })
            .collect()
    }
}

impl Element for Sparkline {
    type LayoutState = ();
    type PaintState = ();

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, ()) {
        let style = Style::default().width(self.width).height(self.height);
        let node = cx
            .taffy
            .new_leaf(taffy::Style::from(&style))
            .unwrap_or_else(|e| {
                log::error!("Sparkline: new_leaf failed ({e})");
                taffy::NodeId::from(u64::MAX)
            });
        let _ = cx.taffy.set_node_context(node, Some(NodeContext::None));
        (LayoutId(node), ())
    }

    fn prepaint(&mut self, bounds: Bounds, _: &mut (), cx: &mut PrepaintCtx) {
        let id = cx.allocate_id::<Sparkline>();
        self.last_id = Some(id);
        cx.register_a11y_node(AccessibilityNode {
            id,
            bounds,
            info: AccessibilityInfo {
                role: AccessibilityRole::Image,
                label: Some(series_summary("Sparkline", self.label.as_deref(), &self.values)),
                ..Default::default()
            },
            children: Vec::new(),
            actions: Vec::new(),
        });
    }

    fn paint(&mut self, bounds: Bounds, _: &mut (), _: &mut (), cx: &mut PaintCtx) {
        emit_polyline(cx.scene, &self.points(bounds), self.stroke_width, self.stroke);
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

impl Sealed for Sparkline {}

impl IntoElement for Sparkline {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
