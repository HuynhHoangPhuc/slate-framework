//! Interaction-state styling: transient per-element state (hover / press /
//! disabled) plus the per-state visual style overrides resolved at paint time.
//!
//! Two pieces live here:
//! - [`StateStyle`] / [`StateStyles`] — the per-state visual deltas an element
//!   stores (`None` fields fall through to the base style). Values are resolved
//!   from [`Reactive`] under the render observer, exactly like
//!   [`crate::elements::Div::background`], so a signal-backed override
//!   re-resolves on the whole-view rebuild (Strategy A).
//! - [`InteractionSnapshot`] — the paint-time view of which elements are
//!   currently hovered / pressed. Hover and press each carry the target *and
//!   its ancestors*, so a container reflects the state when a descendant is
//!   hovered/pressed (matching CSS `:hover` / `:active`).

use std::collections::HashMap;

use smallvec::SmallVec;

use crate::color::Color;
use crate::reactive_value::Reactive;
use crate::types::ElementId;

/// Per-state visual style overrides. A `None` field falls through to the
/// element's base style; a `Some` field replaces it while that state is active.
///
/// Built inside the closure passed to [`Div::hover`](crate::elements::Div::hover),
/// [`Div::active`](crate::elements::Div::active), or
/// [`Div::disabled_style`](crate::elements::Div::disabled_style).
#[derive(Clone, Debug, Default)]
pub struct StateStyle {
    pub(crate) background: Option<[f32; 4]>,
    pub(crate) corner_radius: Option<f32>,
}

impl StateStyle {
    /// Override the background color while this state is active.
    ///
    /// Accepts a plain color or a signal-backed [`Reactive`]; the value is read
    /// under the render observer, mirroring [`Div::background`](crate::elements::Div::background).
    pub fn background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        let value: Reactive<Color> = color.into();
        self.background = Some(value.resolve().0);
        self
    }

    /// Override the corner radius (logical pixels) while this state is active.
    pub fn corner_radius(mut self, radius: impl Into<Reactive<f32>>) -> Self {
        let value: Reactive<f32> = radius.into();
        self.corner_radius = Some(value.resolve());
        self
    }

    /// Apply this delta's set fields onto the running `(background, radius)`.
    pub(crate) fn apply(&self, background: &mut Option<[f32; 4]>, corner_radius: &mut f32) {
        if let Some(bg) = self.background {
            *background = Some(bg);
        }
        if let Some(r) = self.corner_radius {
            *corner_radius = r;
        }
    }
}

/// The full set of per-state style deltas an interactive element may carry.
///
/// Stored on an element behind `Option<Box<StateStyles>>` so the common case
/// (no interaction overrides) keeps a null fast path and allocates nothing.
#[derive(Clone, Debug, Default)]
pub(crate) struct StateStyles {
    pub(crate) hover: StateStyle,
    pub(crate) active: StateStyle,
    pub(crate) disabled: StateStyle,
}

/// Paint-time snapshot of the hover and press chains for a window.
///
/// Each chain is the target element plus its ancestors, so membership tests
/// light up containers when a descendant is the direct target.
#[derive(Default)]
pub(crate) struct InteractionSnapshot {
    hovered: SmallVec<[ElementId; 8]>,
    pressed: SmallVec<[ElementId; 8]>,
}

impl InteractionSnapshot {
    /// Build a snapshot from the current hovered / pressed targets and the
    /// parent map produced during prepaint.
    pub(crate) fn new(
        hovered: Option<ElementId>,
        pressed: Option<ElementId>,
        parent_map: &HashMap<ElementId, ElementId>,
    ) -> Self {
        Self {
            hovered: collect_chain(hovered, parent_map),
            pressed: collect_chain(pressed, parent_map),
        }
    }

    /// True when `id` is the hovered element or an ancestor of it.
    pub(crate) fn is_hovered(&self, id: ElementId) -> bool {
        self.hovered.contains(&id)
    }

    /// True when `id` is the pressed element or an ancestor of it.
    pub(crate) fn is_pressed(&self, id: ElementId) -> bool {
        self.pressed.contains(&id)
    }
}

/// Walk `start` to the root via `parent_map`, collecting the chain.
fn collect_chain(
    start: Option<ElementId>,
    parent_map: &HashMap<ElementId, ElementId>,
) -> SmallVec<[ElementId; 8]> {
    let mut chain = SmallVec::new();
    let mut current = start;
    while let Some(id) = current {
        chain.push(id);
        current = parent_map.get(&id).copied();
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_style_apply_overrides_only_set_fields() {
        let mut bg = Some([1.0, 0.0, 0.0, 1.0]);
        let mut radius = 4.0;
        StateStyle::default()
            .background([0.0, 0.0, 1.0, 1.0])
            .apply(&mut bg, &mut radius);
        assert_eq!(bg, Some([0.0, 0.0, 1.0, 1.0]));
        assert_eq!(radius, 4.0, "corner_radius untouched when delta omits it");
    }

    #[test]
    fn snapshot_chain_includes_ancestors() {
        let parent = ElementId::from_hash(1);
        let child = ElementId::from_hash(2);
        let mut pm = HashMap::new();
        pm.insert(child, parent);
        let snap = InteractionSnapshot::new(Some(child), None, &pm);
        assert!(snap.is_hovered(child));
        assert!(snap.is_hovered(parent), "ancestor reflects descendant hover");
        assert!(!snap.is_pressed(child));
    }

    #[test]
    fn empty_snapshot_matches_nothing() {
        let snap = InteractionSnapshot::default();
        assert!(!snap.is_hovered(ElementId::from_hash(1)));
        assert!(!snap.is_pressed(ElementId::from_hash(1)));
    }
}
