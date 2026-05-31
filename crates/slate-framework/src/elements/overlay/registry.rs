//! Per-frame registry of open overlays + the dismiss decision logic.
//!
//! An [`Overlay`](super::Overlay) is "open" iff it is rendered this frame; there
//! is no separate open/closed flag in the framework. To *dismiss* one (Esc,
//! outside-click, modal scrim click) the framework cannot mutate that state
//! itself — it invokes a caller-supplied `on_dismiss` callback, and the caller
//! flips its own `open` signal so the overlay drops out of the next rebuild.
//!
//! This registry is rebuilt every prepaint (mirroring [`FocusRegistry`]): each
//! open overlay registers its solved content rect, anchor rect, modality, depth
//! and dismiss callback. The dispatch path then queries it to decide what an Esc
//! keypress or a mouse-down should do. Decisions consider only the **top-most**
//! (highest-depth) overlay — a single gesture dismisses one overlay.
//!
//! [`FocusRegistry`]: crate::focus::FocusRegistry

use std::rc::Rc;

use crate::types::{Bounds, ElementId, Point};

/// A dismiss callback: invoked (after dispatch borrows unwind) when the user
/// requests an overlay be dismissed. Typically `move || open.set(false)`.
pub(crate) type DismissFn = Rc<dyn Fn()>;

/// One open overlay's per-frame record.
pub(crate) struct OverlayEntry {
    /// The overlay element's stable id (for tie-breaking / debugging).
    #[allow(dead_code)]
    pub id: ElementId,
    /// Absolute (window-relative) rect the content actually paints at this frame
    /// (the anchor-solved origin + measured content size).
    pub content_bounds: Bounds,
    /// The target rect the overlay anchors to (e.g. the trigger button). A click
    /// here is treated as "inside" for a non-modal overlay so the trigger can
    /// toggle without the outside-click path also firing dismiss.
    pub anchor_bounds: Bounds,
    /// Modal overlays paint a full-viewport scrim: any click outside the content
    /// is on the scrim, which both dismisses and blocks the click from the tree.
    pub modal: bool,
    /// Z-depth; the highest-depth open overlay is the dismiss target.
    pub depth: i32,
    /// Caller's dismiss callback. `None` = the overlay opts out of dismissal.
    pub dismiss: Option<DismissFn>,
}

/// What a mouse-down at a point should do with respect to open overlays.
#[derive(Clone)]
pub(crate) enum ClickOutcome {
    /// No open overlay is relevant — dispatch the click normally.
    Pass,
    /// The click is outside the top-most overlay's content: fire `callback`
    /// (if any). When `block` is set (modal scrim), the click is swallowed and
    /// must NOT reach the base tree; otherwise normal dispatch continues so the
    /// click still acts on whatever sits beneath the overlay.
    Dismiss {
        /// The top-most overlay's dismiss callback, if it supplied one.
        callback: Option<DismissFn>,
        /// Swallow the click from the base tree (modal input-blocking).
        block: bool,
    },
}

/// Per-frame list of open overlays. Rebuilt each prepaint.
#[derive(Default)]
pub(crate) struct OverlayRegistry {
    entries: Vec<OverlayEntry>,
}

impl OverlayRegistry {
    /// Drop all entries (call at the start of every prepaint).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Register an open overlay during prepaint.
    pub fn register(&mut self, entry: OverlayEntry) {
        self.entries.push(entry);
    }

    /// Index of the top-most overlay: highest depth, ties broken by latest
    /// registration (later in the prepaint walk paints on top).
    fn topmost_index(&self) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .max_by_key(|(i, e)| (e.depth, *i))
            .map(|(i, _)| i)
    }

    /// Dismiss callback of the top-most overlay (Esc target), if any is open.
    /// The outer `Option` is `None` when no overlay is open; the inner is `None`
    /// when the top-most overlay opted out of dismissal.
    pub fn topmost_dismiss(&self) -> Option<Option<DismissFn>> {
        self.topmost_index().map(|i| self.entries[i].dismiss.clone())
    }

    /// Decide what a mouse-down at `point` should do against the top-most overlay.
    ///
    /// - Inside the content rect → [`ClickOutcome::Pass`] (the content handles it).
    /// - Outside, modal → dismiss **and** block (scrim swallows the click).
    /// - Outside, non-modal, but inside the anchor → `Pass` (let the trigger
    ///   toggle; avoids the outside-click path racing the trigger's own handler).
    /// - Outside, non-modal, outside the anchor → dismiss without blocking.
    pub fn click_outcome(&self, point: Point) -> ClickOutcome {
        let Some(i) = self.topmost_index() else {
            return ClickOutcome::Pass;
        };
        let e = &self.entries[i];
        if e.content_bounds.contains(point) {
            return ClickOutcome::Pass;
        }
        if !e.modal && e.anchor_bounds.contains(point) {
            return ClickOutcome::Pass;
        }
        ClickOutcome::Dismiss {
            callback: e.dismiss.clone(),
            block: e.modal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn id(n: u64) -> ElementId {
        ElementId::from_raw(n)
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds {
        Bounds::from_origin_size(x, y, w, h)
    }

    /// A dismiss callback that records how many times it fired.
    fn counter() -> (Rc<Cell<u32>>, DismissFn) {
        let c = Rc::new(Cell::new(0u32));
        let c2 = c.clone();
        (c, Rc::new(move || c2.set(c2.get() + 1)))
    }

    fn entry(id_n: u64, content: Bounds, anchor: Bounds, modal: bool, depth: i32) -> OverlayEntry {
        OverlayEntry {
            id: id(id_n),
            content_bounds: content,
            anchor_bounds: anchor,
            modal,
            depth,
            dismiss: None,
        }
    }

    #[test]
    fn empty_registry_passes_and_has_no_dismiss() {
        let r = OverlayRegistry::default();
        assert!(matches!(r.click_outcome(Point::new(5.0, 5.0)), ClickOutcome::Pass));
        assert!(r.topmost_dismiss().is_none());
    }

    #[test]
    fn click_inside_content_passes() {
        let mut r = OverlayRegistry::default();
        r.register(entry(1, rect(100.0, 100.0, 50.0, 50.0), Bounds::ZERO, false, 1000));
        assert!(matches!(
            r.click_outcome(Point::new(120.0, 120.0)),
            ClickOutcome::Pass
        ));
    }

    #[test]
    fn nonmodal_click_outside_dismisses_without_block() {
        let mut r = OverlayRegistry::default();
        let (count, cb) = counter();
        let mut e = entry(1, rect(100.0, 100.0, 50.0, 50.0), Bounds::ZERO, false, 1000);
        e.dismiss = Some(cb);
        r.register(e);
        match r.click_outcome(Point::new(10.0, 10.0)) {
            ClickOutcome::Dismiss { callback, block } => {
                assert!(!block, "non-modal must not block");
                callback.unwrap()();
            }
            ClickOutcome::Pass => panic!("expected dismiss"),
        }
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn nonmodal_click_on_anchor_passes() {
        // Clicking the trigger must not also fire the outside-click dismiss.
        let mut r = OverlayRegistry::default();
        let (_count, cb) = counter();
        let mut e = entry(
            1,
            rect(100.0, 130.0, 50.0, 50.0),
            rect(100.0, 100.0, 60.0, 20.0), // anchor (the button)
            false,
            1000,
        );
        e.dismiss = Some(cb);
        r.register(e);
        assert!(matches!(
            r.click_outcome(Point::new(110.0, 105.0)), // inside anchor
            ClickOutcome::Pass
        ));
    }

    #[test]
    fn modal_click_outside_dismisses_and_blocks() {
        let mut r = OverlayRegistry::default();
        let (count, cb) = counter();
        let mut e = entry(1, rect(100.0, 100.0, 50.0, 50.0), Bounds::ZERO, true, 1000);
        e.dismiss = Some(cb);
        r.register(e);
        match r.click_outcome(Point::new(10.0, 10.0)) {
            ClickOutcome::Dismiss { callback, block } => {
                assert!(block, "modal scrim click must block the base tree");
                callback.unwrap()();
            }
            ClickOutcome::Pass => panic!("expected dismiss"),
        }
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn modal_ignores_anchor_when_outside_content() {
        // A modal has no meaningful anchor; an "anchor" hit still dismisses+blocks.
        let mut r = OverlayRegistry::default();
        let e = entry(
            1,
            rect(100.0, 130.0, 50.0, 50.0),
            rect(0.0, 0.0, 20.0, 20.0),
            true,
            1000,
        );
        r.register(e);
        assert!(matches!(
            r.click_outcome(Point::new(5.0, 5.0)),
            ClickOutcome::Dismiss { block: true, .. }
        ));
    }

    #[test]
    fn topmost_by_depth_is_the_dismiss_target() {
        let mut r = OverlayRegistry::default();
        // Lower-depth tooltip + higher-depth dialog both open.
        r.register(entry(1, rect(0.0, 0.0, 10.0, 10.0), Bounds::ZERO, false, 1000));
        let (count, cb) = counter();
        let mut top = entry(2, rect(200.0, 200.0, 50.0, 50.0), Bounds::ZERO, true, 2000);
        top.dismiss = Some(cb);
        r.register(top);

        // Esc targets the deepest (id 2).
        let d = r.topmost_dismiss().expect("an overlay is open");
        d.expect("top has a callback")();
        assert_eq!(count.get(), 1);

        // A click outside the deep dialog blocks (modal), regardless of the
        // lower tooltip beneath it.
        assert!(matches!(
            r.click_outcome(Point::new(5.0, 5.0)),
            ClickOutcome::Dismiss { block: true, .. }
        ));
    }

    #[test]
    fn topmost_tie_breaks_to_latest_registration() {
        let mut r = OverlayRegistry::default();
        r.register(entry(1, rect(0.0, 0.0, 10.0, 10.0), Bounds::ZERO, false, 1000));
        let (count, cb) = counter();
        let mut later = entry(2, rect(0.0, 0.0, 10.0, 10.0), Bounds::ZERO, false, 1000);
        later.dismiss = Some(cb);
        r.register(later); // same depth, registered later → wins
        r.topmost_dismiss().flatten().expect("later wins")();
        assert_eq!(count.get(), 1);
    }
}
