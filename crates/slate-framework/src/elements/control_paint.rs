//! Shared paint + interaction helpers for the form-control widgets
//! (Button, Checkbox, Switch, Slider).
//!
//! Two concerns live here so each control doesn't re-derive them:
//! - **Interaction-state fill** — hover/press shading of a base theme colour, so
//!   a control lights up under the pointer without needing extra theme tokens.
//! - **Toggle interaction** — the click + Space handlers a boolean control
//!   (Checkbox, Switch) shares, flipping a caller-owned `Signal<bool>`
//!   (Strategy A: the handler writes, the next whole-view rebuild reads).

use std::sync::Arc;

use slate_platform::{Key, NamedKey};
use slate_renderer::Lpx;
use slate_renderer::scene::RectInstance;

use crate::context::PaintCtx;
use crate::element::AnyElement;
use crate::elements::Text;
use crate::event::{
    ElementKeyHandler, EventCtx, Handlers, KeyEvent, KeyHandlers, MouseEvent, MouseHandler,
};
use crate::reactive::Signal;
use crate::types::{Bounds, ElementId};

/// Build a presentational label child: it renders pixels but emits no a11y node.
/// Shared by Checkbox / Switch, whose own roled node already carries the
/// accessible name, so a nested `Label` node would double-announce.
pub(crate) fn presentational_label(text: &str, color: [f32; 4], size: f32) -> AnyElement {
    AnyElement::new(
        Text::new(text)
            .font_size(size)
            .color(color)
            .presentational(),
    )
}

/// Push one filled, rounded rect (logical-pixel coords) to the scene. Shared by
/// the controls that paint their own indicators (Checkbox box, Switch track +
/// thumb, Slider track + thumb).
pub(crate) fn push_rect(cx: &mut PaintCtx, b: Bounds, color: [f32; 4], radius: f32) {
    cx.scene.push_rect(RectInstance {
        rect: [
            Lpx(b.origin.x),
            Lpx(b.origin.y),
            Lpx(b.size.width),
            Lpx(b.size.height),
        ],
        color,
        corner_radius: Lpx(radius),
        _pad: [0.0; 3],
    });
}

/// Scale the RGB channels of a premultiplied colour by `factor`, keeping alpha.
///
/// `factor < 1.0` darkens. Alpha is left untouched so the result stays a valid
/// premultiplied colour (scaling RGB and A together would change the hue's
/// coverage, not just its brightness).
pub(crate) fn scale_rgb(c: [f32; 4], factor: f32) -> [f32; 4] {
    [c[0] * factor, c[1] * factor, c[2] * factor, c[3]]
}

/// Resolve a control fill against the hover/press interaction snapshot: pressed
/// darkens most, hover slightly, resting returns `base`. Mirrors the resolution
/// order used by `Div` interaction-state styling and the scrollbar thumb
/// (pressed wins over hover).
pub(crate) fn interactive_fill(cx: &PaintCtx, id: ElementId, base: [f32; 4]) -> [f32; 4] {
    if cx.is_pressed(id) {
        scale_rgb(base, 0.82)
    } else if cx.is_hovered(id) {
        scale_rgb(base, 0.92)
    } else {
        base
    }
}

/// Build the click + Space-key handlers that flip `value`.
///
/// Shared by Checkbox and Switch: both toggle a boolean on primary click and on
/// Space (ignoring OS auto-repeat), and stop propagation so the gesture doesn't
/// also drive Tab navigation or an ancestor's handler.
pub(crate) fn toggle_handlers(value: Signal<bool>) -> (Handlers, KeyHandlers) {
    let click_value = value.clone();
    let on_click: MouseHandler = Arc::new(move |_ev: &MouseEvent, cx: &mut EventCtx| {
        click_value.set(!click_value.get());
        cx.stop_propagation();
    });

    let key_value = value;
    let on_key_down: ElementKeyHandler = Arc::new(move |ev: &KeyEvent, cx: &mut EventCtx| {
        if ev.key == Key::Named(NamedKey::Space) && !ev.is_repeat {
            key_value.set(!key_value.get());
            cx.stop_propagation();
        }
    });

    (
        Handlers {
            on_click: Some(on_click),
            ..Default::default()
        },
        KeyHandlers {
            on_key_down: Some(on_key_down),
            ..Default::default()
        },
    )
}
