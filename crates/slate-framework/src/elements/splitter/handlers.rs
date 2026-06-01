//! Splitter divider interaction: pointer drag and keyboard resize.
//!
//! The drag anchor lives in a per-element [`StateRegistry`] slot (a
//! `Signal<SplitterDrag>`) so it survives the Strategy-A whole-view rebuild each
//! `ratio.set` triggers mid-drag — same discipline as the slider/scrollbar
//! thumbs. The pointer maps to an *absolute* ratio (not a delta), so the divider
//! tracks the cursor directly and a press anywhere starts a drag from there.
//!
//! [`StateRegistry`]: crate::reactive_state::StateRegistry

use std::sync::Arc;

use slate_platform::{Key, NamedKey};

use crate::event::{
    ElementKeyHandler, EventCtx, KeyEvent, MouseEvent, MouseHandler, MouseHandlers,
};
use crate::reactive::Signal;

use super::SplitAxis;
use super::value::SplitRange;

/// Drag anchor persisted across rebuilds: only "are we dragging" is needed since
/// the pointer maps to an absolute ratio each move.
#[derive(Clone, Copy, Default)]
pub(super) struct SplitterDrag {
    pub active: bool,
}

/// Pull the main-axis coordinate out of a pointer position for this axis.
fn pointer_main(ev: &MouseEvent, axis: SplitAxis) -> f32 {
    match axis {
        SplitAxis::Horizontal => ev.position.0,
        SplitAxis::Vertical => ev.position.1,
    }
}

/// Set `ratio` from a pointer position if it actually changed (avoids spinning
/// the rebuild loop on identical sets).
fn set_from_pointer(
    ratio: &Signal<f32>,
    range: &SplitRange,
    origin_main: f32,
    pointer: f32,
) {
    let r = range.ratio_at_pointer(pointer, origin_main);
    if (r - ratio.get()).abs() > f32::EPSILON {
        ratio.set(r);
    }
}

/// Build the down/move/up pointer handlers. `origin_main` + `range` are this
/// frame's resolved geometry, re-captured every rebuild.
pub(super) fn build_mouse_handlers(
    ratio: Signal<f32>,
    drag: Signal<SplitterDrag>,
    range: SplitRange,
    origin_main: f32,
    axis: SplitAxis,
) -> MouseHandlers {
    let on_mouse_down = {
        let ratio = ratio.clone();
        let drag = drag.clone();
        Arc::new(move |ev: &MouseEvent, cx: &mut EventCtx| {
            drag.set(SplitterDrag { active: true });
            set_from_pointer(&ratio, &range, origin_main, pointer_main(ev, axis));
            cx.stop_propagation();
        }) as MouseHandler
    };

    let on_mouse_move = {
        let ratio = ratio.clone();
        let drag = drag.clone();
        Arc::new(move |ev: &MouseEvent, cx: &mut EventCtx| {
            if !drag.get().active {
                return;
            }
            set_from_pointer(&ratio, &range, origin_main, pointer_main(ev, axis));
            cx.stop_propagation();
        }) as MouseHandler
    };

    let on_mouse_up = {
        let drag = drag.clone();
        Arc::new(move |_ev: &MouseEvent, cx: &mut EventCtx| {
            let mut d = drag.get();
            if d.active {
                d.active = false;
                drag.set(d);
            }
            cx.stop_propagation();
        }) as MouseHandler
    };

    MouseHandlers {
        on_mouse_down: Some(on_mouse_down),
        on_mouse_move: Some(on_mouse_move),
        on_mouse_up: Some(on_mouse_up),
    }
}

/// Build the keyboard resize handler. The decrease/increase keys are axis-aware
/// (Left/Right for horizontal, Up/Down for vertical); Home/End jump to the
/// minimum/maximum allowed first-pane size. All results are clamped.
pub(super) fn build_key_handler(
    ratio: Signal<f32>,
    range: SplitRange,
    axis: SplitAxis,
) -> ElementKeyHandler {
    Arc::new(move |ev: &KeyEvent, cx: &mut EventCtx| {
        let (dec, inc) = match axis {
            SplitAxis::Horizontal => (NamedKey::ArrowLeft, NamedKey::ArrowRight),
            SplitAxis::Vertical => (NamedKey::ArrowUp, NamedKey::ArrowDown),
        };
        let cur = ratio.get();
        let next = match &ev.key {
            Key::Named(k) if *k == dec => cur - range.step_ratio(),
            Key::Named(k) if *k == inc => cur + range.step_ratio(),
            Key::Named(NamedKey::Home) => range.clamp_ratio(0.0),
            Key::Named(NamedKey::End) => range.clamp_ratio(1.0),
            _ => return,
        };
        let next = range.clamp_ratio(next);
        if (next - cur).abs() > f32::EPSILON {
            ratio.set(next);
        }
        cx.stop_propagation();
    })
}
