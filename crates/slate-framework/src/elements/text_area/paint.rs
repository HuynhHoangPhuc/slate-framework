//! TextArea paint pass: glyphs, caret, and the handler-side layout cache.
//!
//! Split from `mod.rs` so the element module stays focused on types + Element
//! wiring (mirrors how `text_field` extracts `handlers.rs`). The three concerns
//! here — rasterizing each visual line, drawing the blinking caret, and caching
//! the layout onto `ImeState` for later hit-testing — share one borrow scope on
//! the IME registry, so they live together.

use std::rc::Rc;
use std::time::Instant;

use slate_renderer::Lpx;
use slate_renderer::scene::RectInstance;
use slate_text::MultilineLayout;

use crate::context::PaintCtx;
use crate::text_system::PlatformFont;
use crate::types::{Bounds, ElementId};

use super::TextAreaStyle;

/// Render `layout` for the element at `bounds`: optional background, one glyph
/// run per visual line, and (when `focused`) the blinking caret at the byte the
/// shared editor state reports.
pub(super) fn paint(
    style: &TextAreaStyle,
    font: &PlatformFont,
    layout: &Rc<MultilineLayout>,
    element_id: ElementId,
    focused: bool,
    bounds: Bounds,
    cx: &mut PaintCtx,
) {
    // Optional background spanning the full wrapped height.
    if let Some(bg) = style.background {
        cx.scene.push_rect(RectInstance {
            rect: [
                Lpx(bounds.origin.x),
                Lpx(bounds.origin.y),
                Lpx(style.width),
                Lpx(layout.total_height_lpx),
            ],
            color: bg,
            corner_radius: Lpx(0.0),
            _pad: [0.0; 3],
        });
    }

    // Caret byte from the shared editor state; falls back to the document end
    // when no ImeState exists yet (pre-prepaint or unmounted).
    let caret_byte = match cx.ime_registry.borrow().get(element_id) {
        Some(rc) => rc.borrow().caret,
        None => layout.lines.last().map(|l| l.byte_end).unwrap_or(0),
    };

    // 1. Glyphs — one rasterize pass per visual line at its own baseline.
    for vline in &layout.lines {
        if vline.line.glyphs.is_empty() {
            continue;
        }
        let baseline = [
            bounds.origin.x,
            bounds.origin.y + vline.line.y_offset_lpx + vline.line.ascent_lpx,
        ];
        match cx.text.rasterize_text_run(
            font,
            &vline.line,
            baseline,
            style.color,
            cx.glyph_atlas,
            cx.queue,
        ) {
            Ok(glyphs) => {
                for glyph in glyphs {
                    cx.scene.push_glyph(glyph);
                }
            }
            Err(e) => {
                log::error!("TextArea: rasterize_text_run failed: {e}");
            }
        }
    }

    // 2. Caret — resolve the byte to its line + pixel-x, then advance blink.
    let (_, caret_x, caret_y) = layout.caret_position(caret_byte);

    let mut caret_visible = false;
    if let Some(state_rc) = cx.ime_registry.borrow().get(element_id)
        && let Ok(mut state) = state_rc.try_borrow_mut()
    {
        let (visible, next_deadline) = crate::elements::text_edit::blink::advance_blink(
            &mut state.blink,
            focused,
            Instant::now(),
        );
        caret_visible = visible;
        if let Some(deadline) = next_deadline {
            cx.schedule_redraw_at(deadline);
        }
    }
    if focused && caret_visible {
        cx.scene.push_rect(RectInstance {
            rect: [
                Lpx(bounds.origin.x + caret_x),
                Lpx(bounds.origin.y + caret_y),
                Lpx(1.0),
                Lpx(layout.line_height_lpx),
            ],
            color: style.caret_color,
            corner_radius: Lpx(0.0),
            _pad: [0.0; 3],
        });
    }

    // 3. Cache the layout + paint origin on the editor state so handlers can
    //    map bytes↔(line, x) without re-shaping (used by later phases).
    if let Some(state_rc) = cx.ime_registry.borrow().get(element_id)
        && let Ok(mut state) = state_rc.try_borrow_mut()
    {
        state.last_layout = Some(layout.clone());
        state.paint_origin_x = bounds.origin.x;
    }
}
