//! Slider interaction: pointer drag (click-to-position + thumb drag) and the
//! keyboard adjustment handler.
//!
//! The drag anchor lives in a per-element [`StateRegistry`] slot (a
//! `Signal<SliderDrag>`) so it survives the Strategy-A whole-view rebuild each
//! `value.set` triggers mid-drag — same discipline as the scrollbar thumb. The
//! pointer is mapped to an *absolute* value (not a delta), so a press anywhere
//! on the track jumps the thumb there and then drags from it.
//!
//! [`StateRegistry`]: crate::reactive_state::StateRegistry

use std::sync::Arc;

use slate_platform::{Key, NamedKey};

use crate::event::{
    ElementKeyHandler, EventCtx, KeyEvent, MouseEvent, MouseHandler, MouseHandlers,
};
use crate::reactive::Signal;

use super::value::SliderRange;

/// Drag anchor persisted across rebuilds: only "are we dragging" is needed since
/// the pointer maps to an absolute value each move.
#[derive(Clone, Copy, Default)]
pub(super) struct SliderDrag {
    pub active: bool,
}

/// Set `value` from a pointer X if it actually changed (avoids spinning the
/// rebuild loop on identical sets).
fn set_from_pointer(
    value: &Signal<f32>,
    range: &SliderRange,
    pointer_x: f32,
    track_x: f32,
    track_w: f32,
) {
    let v = range.value_at_pointer(pointer_x, track_x, track_w);
    if (v - value.get()).abs() > f32::EPSILON {
        value.set(v);
    }
}

/// Build the down/move/up pointer handlers. `track_x` / `track_w` are this
/// frame's resolved track geometry, re-captured every rebuild.
pub(super) fn build_mouse_handlers(
    value: Signal<f32>,
    drag: Signal<SliderDrag>,
    range: SliderRange,
    track_x: f32,
    track_w: f32,
) -> MouseHandlers {
    let on_mouse_down = {
        let value = value.clone();
        let drag = drag.clone();
        Arc::new(move |ev: &MouseEvent, cx: &mut EventCtx| {
            drag.set(SliderDrag { active: true });
            set_from_pointer(&value, &range, ev.position.0, track_x, track_w);
            cx.stop_propagation();
        }) as MouseHandler
    };

    let on_mouse_move = {
        let value = value.clone();
        let drag = drag.clone();
        Arc::new(move |ev: &MouseEvent, cx: &mut EventCtx| {
            if !drag.get().active {
                return;
            }
            set_from_pointer(&value, &range, ev.position.0, track_x, track_w);
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

/// Build the keyboard adjustment handler: Arrows step by one, PageUp/Down by a
/// larger amount, Home/End jump to the ends. All results are clamped + snapped.
pub(super) fn build_key_handler(value: Signal<f32>, range: SliderRange) -> ElementKeyHandler {
    Arc::new(move |ev: &KeyEvent, cx: &mut EventCtx| {
        let cur = value.get();
        let next = match &ev.key {
            Key::Named(NamedKey::ArrowRight) | Key::Named(NamedKey::ArrowUp) => {
                cur + range.step_amount()
            }
            Key::Named(NamedKey::ArrowLeft) | Key::Named(NamedKey::ArrowDown) => {
                cur - range.step_amount()
            }
            Key::Named(NamedKey::PageUp) => cur + range.page_amount(),
            Key::Named(NamedKey::PageDown) => cur - range.page_amount(),
            Key::Named(NamedKey::Home) => range.min,
            Key::Named(NamedKey::End) => range.max,
            _ => return,
        };
        let next = range.clamp_step(next);
        if (next - cur).abs() > f32::EPSILON {
            value.set(next);
        }
        cx.stop_propagation();
    })
}
