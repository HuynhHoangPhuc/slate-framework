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
///
/// While an IME composition is active, the committed text + preedit are spliced
/// into a *display* string, re-shaped, and re-wrapped so the candidate text
/// renders inline (with an underline and an optional target highlight) and a
/// long composition can push following text onto the next visual line —
/// matching native textarea + IME-panel behavior. The display layout is what
/// gets cached for hit-testing; the mouse/key handlers no-op during composition,
/// so the preedit-shifted byte offsets never reach a click.
pub(super) fn paint(
    style: &TextAreaStyle,
    font: &PlatformFont,
    layout: &Rc<MultilineLayout>,
    element_id: ElementId,
    focused: bool,
    bounds: Bounds,
    cx: &mut PaintCtx,
) {
    // Snapshot the editor state in one borrow, then release it before any
    // re-shaping (which needs `cx.text`) or the later cache write-back. The
    // caret falls back to the document end when no ImeState exists yet
    // (pre-prepaint or unmounted).
    let (committed_text, caret_byte, caret_affinity, selection_anchor, preedit) =
        match cx.ime_registry.borrow().get(element_id) {
            Some(rc) => {
                let s = rc.borrow();
                (
                    s.text.clone(),
                    s.caret,
                    s.caret_affinity,
                    s.selection_anchor,
                    s.preedit.clone(),
                )
            }
            None => (
                String::new(),
                layout.lines.last().map(|l| l.byte_end).unwrap_or(0),
                slate_text::Affinity::Downstream,
                None,
                None,
            ),
        };

    // Compose the display layout. With a preedit, splice it in and re-shape;
    // otherwise reuse the layout shaped in `request_layout`. `display_caret` is
    // the caret byte *in display coordinates* (after the composed text), and
    // `preedit_span` is the composed byte range to underline / highlight.
    let mut display_layout = layout.clone();
    let mut display_caret = caret_byte;
    let mut preedit_span: Option<(usize, Option<std::ops::Range<usize>>)> = None;
    if let Some(ref p) = preedit {
        let display = super::layout::compose_display(&committed_text, caret_byte, &p.text);
        match cx.text.shape_document(font, &display) {
            Ok(doc) => {
                display_layout = Rc::new(slate_text::wrap_document(&doc, style.width));
                // `compose_display` floors the caret to a char boundary ≤ caret;
                // recompute the same anchor so the span lines up with the splice,
                // then bind the caret to the IME cursor inside the preedit.
                let at = floor_char_boundary(&committed_text, caret_byte);
                display_caret = crate::ime::caret_display_byte(at, Some(p));
                preedit_span = Some((at, p.selection.clone()));
            }
            Err(e) => {
                log::error!("TextArea: preedit shape_document failed: {e}");
            }
        }
    }
    let layout = &display_layout;
    let has_preedit = preedit_span.is_some();

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

    // 1. Selection highlight — one rect per spanned visual line, behind glyphs.
    //    Suppressed during composition (selection ≡ preedit while composing).
    if !has_preedit && let Some(anchor) = selection_anchor {
        let (lo, hi) = if anchor <= caret_byte {
            (anchor, caret_byte)
        } else {
            (caret_byte, anchor)
        };
        for r in super::layout::selection_rects(layout, lo, hi, style.width) {
            cx.scene.push_rect(RectInstance {
                rect: [
                    Lpx(bounds.origin.x + r.x_lpx),
                    Lpx(bounds.origin.y + r.y_lpx),
                    Lpx(r.width_lpx),
                    Lpx(layout.line_height_lpx),
                ],
                color: style.selection_color,
                corner_radius: Lpx(0.0),
                _pad: [0.0; 3],
            });
        }
    }

    // 2. Glyphs — one rasterize pass per visual line at its own baseline.
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
            cx.glyph_cache,
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

    // 2.5 Preedit overlay — a 1px underline beneath the composed run on every
    //     visual line it covers, plus an optional target-converted highlight
    //     behind the glyphs. Both hug the composed glyphs (no wrap fill).
    if let (Some((preedit_start, sel)), Some(p)) = (preedit_span.as_ref(), preedit.as_ref()) {
        let preedit_end = preedit_start + p.text.len();
        for run in super::layout::preedit_runs(layout, *preedit_start, preedit_end) {
            let vline = &layout.lines[run.line_idx];
            let underline_y = vline.line.y_offset_lpx + vline.line.ascent_lpx + 1.0;
            cx.scene.push_rect(RectInstance {
                rect: [
                    Lpx(bounds.origin.x + run.x_lpx),
                    Lpx(bounds.origin.y + underline_y),
                    Lpx(run.width_lpx),
                    Lpx(1.0),
                ],
                color: style.preedit_underline_color,
                corner_radius: Lpx(0.0),
                _pad: [0.0; 3],
            });
        }
        // Target-converted sub-range highlight (when the IME advertises one),
        // mapped from preedit-local offsets to composed-document bytes.
        if let Some(sel) = sel {
            let sel_start = preedit_start + sel.start.min(p.text.len());
            let sel_end = preedit_start + sel.end.min(p.text.len());
            for run in super::layout::preedit_runs(layout, sel_start, sel_end) {
                let vline = &layout.lines[run.line_idx];
                cx.scene.push_rect(RectInstance {
                    rect: [
                        Lpx(bounds.origin.x + run.x_lpx),
                        Lpx(bounds.origin.y + vline.line.y_offset_lpx),
                        Lpx(run.width_lpx),
                        Lpx(layout.line_height_lpx),
                    ],
                    color: style.preedit_selection_color,
                    corner_radius: Lpx(0.0),
                    _pad: [0.0; 3],
                });
            }
        }
    }

    // 3. Caret — resolve the byte to its line + pixel-x, then advance blink.
    //    Uses `display_caret` so the caret sits inside the composed text.
    // During composition the stored affinity belongs to the committed caret,
    // not the preedit-shifted `display_caret`, so fall back to downstream.
    let effective_affinity = crate::ime::caret_affinity_for_display(has_preedit, caret_affinity);
    let (_, caret_x, caret_y) =
        layout.caret_position_with_affinity(display_caret, effective_affinity);

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

    // 4. Cache the layout + paint origin on the editor state so mouse/key
    //    handlers can map (x, y)↔byte without re-shaping. During composition this
    //    caches the *display* (preedit-shifted) layout, whose byte offsets don't
    //    map back to `state.text` 1:1 — only read it when `preedit.is_none()`
    //    (every mouse/key handler already no-ops while composing).
    if let Some(state_rc) = cx.ime_registry.borrow().get(element_id)
        && let Ok(mut state) = state_rc.try_borrow_mut()
    {
        let scale = cx.scale_factor as f32;
        state.caret_client_rect = Some(slate_platform::PhysicalRect::from_lpx_rect(
            bounds.origin.x + caret_x,
            bounds.origin.y + caret_y,
            1.0,
            layout.line_height_lpx,
            scale,
        ));
        state.last_layout = Some(layout.clone());
        state.paint_origin_x = bounds.origin.x;
        state.paint_origin_y = bounds.origin.y;
    }
}

/// Largest char boundary `<= byte` (clamped to `s.len()`). Mirrors the splice
/// point `compose_display` uses, so the underline span and the caret anchor
/// line up with where the preedit was actually inserted.
fn floor_char_boundary(s: &str, byte: usize) -> usize {
    let mut at = byte.min(s.len());
    while at > 0 && !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}
