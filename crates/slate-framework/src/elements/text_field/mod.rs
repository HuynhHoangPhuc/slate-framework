//! TextField — single-line editable text element.
//!
//! # v1 scope
//!
//! Single-line, `Signal<String>` value, byte-offset caret, grapheme-aware
//! navigation (←/→/Home/End/Backspace), IME preedit overlay with underline,
//! non-IME ASCII insertion via `TextInput` events.
//!
//! # Design lock: direct Element impl (option B)
//!
//! TextField implements `Element` directly rather than wrapping a `Div` tree.
//! Rationale: caret and preedit-underline overlays must be emitted in the same
//! paint pass as the text glyphs, after glyph-advance accumulation. Routing
//! through Div's child-tree model would require Div to grow paint-extension hooks
//! that no other element needs. This lock is documented here so future devs do
//! not re-litigate the Div-wrapper alternative.
//!
//! # Out of scope
//!
//! - Multi-line editing, wrap, vertical nav, scroll viewport — future `TextArea`.
//! - Rich text / styled spans.
//!
//! # Bidi
//!
//! The display string is shaped through the bidi segment + reorder pipeline
//! (`shape_line_bidi`), so mixed / RTL text reorders into visual order and the
//! caret / hit-test geometry walks per level-run. Direction-boundary caret
//! affinity (which side of an LTR↔RTL seam the caret favours) is a later refinement.

mod handlers;

use std::time::Instant;

use slate_reactive::Signal;
use slate_renderer::Lpx;
use slate_renderer::scene::RectInstance;
use taffy::prelude::*;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{Element, IntoElement, Sealed};
use crate::event::{ImeHandlers, KeyHandlers, MouseHandlers};
use crate::focus::FocusableEntry;
use crate::hit_test::{CursorStyle, HitRegion};
use crate::text_system::PlatformFont;
use crate::types::{Bounds, ElementId, LayoutId};

use handlers::{
    build_ime_commit_handler, build_ime_preedit_handler, build_key_down_handler,
    build_mouse_down_handler, build_mouse_move_handler, build_mouse_up_handler,
    build_text_input_handler,
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Visual style for `TextField`.
#[derive(Clone, Debug)]
pub struct TextFieldStyle {
    /// Font size in logical pixels.
    pub font_size: f32,
    /// Text color (linear, premultiplied RGBA).
    pub color: [f32; 4],
    /// Optional background fill.
    pub background: Option<[f32; 4]>,
    /// Caret color (linear, premultiplied RGBA).
    pub caret_color: [f32; 4],
    /// Preedit selection highlight color (translucent accent; ~30% alpha).
    pub preedit_selection_color: [f32; 4],
    /// Intrinsic width hint in logical pixels (used when value is empty).
    pub width: f32,
}

impl Default for TextFieldStyle {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            color: [1.0, 1.0, 1.0, 1.0],
            background: None,
            caret_color: [1.0, 1.0, 1.0, 1.0],
            preedit_selection_color: [0.4, 0.6, 1.0, 0.3],
            width: 200.0,
        }
    }
}

/// Single-line editable text input backed by a `Signal<String>`.
pub struct TextField {
    value: Signal<String>,
    style: TextFieldStyle,
    font: Option<PlatformFont>,
    /// Stable ElementId allocated during prepaint (available after prepaint).
    last_id: Option<ElementId>,
}

impl TextField {
    /// Create a new TextField bound to `value`.
    pub fn new(value: Signal<String>) -> Self {
        Self {
            value,
            style: TextFieldStyle::default(),
            font: None,
            last_id: None,
        }
    }

    /// Override the visual style.
    pub fn style(mut self, s: TextFieldStyle) -> Self {
        self.style = s;
        self
    }
}

// ---------------------------------------------------------------------------
// Per-phase state types
// ---------------------------------------------------------------------------

/// State produced during `request_layout`.
pub struct TextFieldLayoutState {
    line_height: f32,
}

/// State produced during `prepaint`, consumed by `paint`.
pub struct TextFieldPaintState {
    element_id: ElementId,
    /// Whether this element was focused at prepaint time (determines caret visibility).
    focused: bool,
}

// ---------------------------------------------------------------------------
// Element impl
// ---------------------------------------------------------------------------

impl Sealed for TextField {}

impl Element for TextField {
    type LayoutState = TextFieldLayoutState;
    type PaintState = TextFieldPaintState;

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        let scale = cx.scale_factor as f32;

        // Load bundled font (mirrors Text::request_layout)
        if self.font.is_none() {
            match cx
                .text
                .load_font_from_bytes(slate_text::TEST_FONT, self.style.font_size, scale)
            {
                Ok(f) => self.font = Some(f),
                Err(e) => {
                    log::error!("TextField: font load failed: {e}; rendering zero-size");
                    let node_id = cx
                        .taffy
                        .new_leaf(taffy::Style::default())
                        .unwrap_or_else(|_| taffy::NodeId::from(u64::MAX));
                    return (LayoutId(node_id), TextFieldLayoutState { line_height: 0.0 });
                }
            }
        }

        let font = self.font.as_ref().unwrap();

        // Measure current value for intrinsic width; fall back to style.width when empty
        let current = self.value.get_untracked();
        let (intrinsic_w, line_height) = if current.is_empty() {
            let shaped =
                cx.text
                    .shape_line(font, "M")
                    .unwrap_or_else(|_| slate_text::types::ShapedLine {
                        glyphs: Vec::new(),
                        width_lpx: 0.0,
                        ascent_lpx: self.style.font_size,
                        descent_lpx: 0.0,
                        y_offset_lpx: 0.0,
                        base_direction: slate_text::Direction::Ltr,
                        runs: Vec::new(),
                    });
            (self.style.width, shaped.ascent_lpx - shaped.descent_lpx)
        } else {
            // Bidi pipeline so intrinsic width matches the run-bearing line the
            // paint pass shapes (and caret geometry consumes).
            match cx.text.shape_line_bidi(font, &current) {
                Ok(shaped) => (
                    shaped.width_lpx.max(self.style.width),
                    shaped.ascent_lpx - shaped.descent_lpx,
                ),
                Err(_) => (self.style.width, self.style.font_size),
            }
        };

        let node_id = match cx.taffy.new_leaf(taffy::Style {
            size: taffy::Size {
                width: Dimension::length(intrinsic_w),
                height: Dimension::length(line_height),
            },
            ..Default::default()
        }) {
            Ok(id) => id,
            Err(e) => {
                log::error!("TextField: Taffy new_leaf failed: {e}");
                taffy::NodeId::from(u64::MAX)
            }
        };

        (LayoutId(node_id), TextFieldLayoutState { line_height })
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        let element_id = cx.allocate_id::<TextField>();
        self.last_id = Some(element_id);

        // I-beam hit region for pointer hit testing
        cx.register_hit_region(
            HitRegion::new(element_id, bounds, 0).with_cursor(CursorStyle::Text),
        );

        // Opt into keyboard focus (tab_index 0 = default Tab cycle order)
        cx.register_focusable(
            FocusableEntry {
                id: element_id,
                tab_index: 0,
                focus_ring: true,
            },
            bounds,
            0.0,
        );

        // Ensure ImeState entry exists; seed text from signal on first frame
        let state_rc = cx.register_ime_state(element_id);
        {
            let mut state = state_rc.borrow_mut();
            if state.text.is_empty() {
                let v = self.value.get_untracked();
                if !v.is_empty() {
                    state.caret = v.len();
                    state.text = v;
                }
            }
            // Seed the undo baseline once, after the text seed, so it matches
            // whatever initial value was seeded (empty or pre-filled). The
            // guard makes later frames a no-op — undo lives on the
            // render-surviving ImeState, so re-seeding would discard history.
            state.seed_undo_baseline();
        }

        // Capture focus snapshot before building handlers
        let focused = cx.focused_element() == Some(element_id);

        // Build and register key + IME handlers
        cx.register_key_handlers(
            element_id,
            KeyHandlers {
                on_key_down: Some(build_key_down_handler(self.value.clone())),
                on_key_up: None,
                on_text_input: Some(build_text_input_handler(self.value.clone())),
            },
        );

        cx.register_ime_handlers(
            element_id,
            ImeHandlers {
                on_ime_preedit: Some(build_ime_preedit_handler()),
                on_ime_commit: Some(build_ime_commit_handler(self.value.clone())),
                on_ime_enabled: None,
                on_ime_disabled: None,
            },
        );

        // Click-to-position + drag-select. Builders read the cached ShapedLine
        // and paint_origin_x written by paint() on the previous frame; the
        // first-frame race (no cached shape yet) resolves itself on the next
        // paint.
        cx.register_mouse_handlers(
            element_id,
            MouseHandlers {
                on_mouse_down: Some(build_mouse_down_handler()),
                on_mouse_move: Some(build_mouse_move_handler()),
                on_mouse_up: Some(build_mouse_up_handler()),
            },
        );

        TextFieldPaintState {
            element_id,
            focused,
        }
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        let element_id = paint_state.element_id;
        let line_height = layout_state.line_height;
        let scale = cx.scale_factor as f32;

        // Optional background. Scene wire format is in logical pixels.
        if let Some(bg) = self.style.background {
            cx.scene.push_rect(RectInstance {
                rect: [
                    Lpx(bounds.origin.x),
                    Lpx(bounds.origin.y),
                    Lpx(bounds.size.width),
                    Lpx(line_height),
                ],
                color: bg,
                corner_radius: Lpx(0.0),
                _pad: [0.0; 3],
            });
        }

        let font = match &self.font {
            Some(f) => f,
            None => return,
        };

        // Snapshot ImeState into owned locals. The borrow is released before
        // `shape_line` (which is fallible and may log) and before the
        // `try_borrow_mut` at the end of paint() that updates
        // `caret_screen_rect` — keeping the registry borrow scope narrow.
        let (committed_text, caret_byte, caret_affinity, preedit_snapshot, selection_anchor) = {
            match cx.ime_registry.borrow().get(element_id) {
                Some(rc) => {
                    let s = rc.borrow();
                    (
                        s.text.clone(),
                        s.caret,
                        s.caret_affinity,
                        s.preedit.clone(),
                        s.selection_anchor,
                    )
                }
                None => (
                    self.value.get_untracked(),
                    0,
                    slate_text::Affinity::Downstream,
                    None,
                    None,
                ),
            }
        };

        // Build display string: committed[..caret] + preedit.text + committed[caret..]
        let caret_safe = caret_byte.min(committed_text.len());
        let display_string = if let Some(ref p) = preedit_snapshot {
            format!(
                "{}{}{}",
                &committed_text[..caret_safe],
                &p.text,
                &committed_text[caret_safe..]
            )
        } else {
            committed_text.clone()
        };

        // Shape display string through the bidi pipeline so the line carries
        // UAX #9 level-runs; the caret / hit-test math (`pixel_x_at_byte`,
        // `byte_at_pixel_x`) switches to its run-aware path for mixed / RTL
        // text and stays byte-identical for pure-LTR / CJK (empty `runs`).
        let shaped = match cx.text.shape_line_bidi(font, &display_string) {
            Ok(s) => s,
            Err(e) => {
                log::error!("TextField: shape_line_bidi failed: {e}");
                return;
            }
        };

        let baseline_x = bounds.origin.x;
        let baseline_y = bounds.origin.y + shaped.ascent_lpx;
        let display_caret = caret_safe; // byte offset into display_string

        // Cluster-aware byte→pixel-x. Snaps to grapheme boundaries, so CJK
        // ligatures, Indic clusters, and emoji ZWJ sequences all behave.
        let pixel_x =
            |byte: usize| -> f32 { slate_text::pixel_x_at_byte(&shaped, &display_string, byte) };
        // The caret itself binds to a direction-boundary edge via its stored
        // affinity; selection/preedit edges use the default-downstream `pixel_x`.
        // During composition the stored affinity belongs to the committed caret,
        // not the preedit-shifted `display_caret`, so fall back to downstream.
        let effective_affinity =
            crate::ime::caret_affinity_for_display(preedit_snapshot.is_some(), caret_affinity);
        let caret_pixel_x = if shaped.runs.is_empty() {
            pixel_x(display_caret)
        } else {
            slate_text::run_caret_x_at_affinity(&shaped, display_caret, effective_affinity)
        };

        // 1. User selection rect (gated). Rendered behind glyphs so glyphs
        //    sit on top. Suppressed while a composition is active —
        //    selection vs. preedit interaction is settled in 10a.5.
        if preedit_snapshot.is_none()
            && let Some(anchor) = selection_anchor
        {
            // Editor paths should keep `selection_anchor` on a char boundary;
            // `pixel_x_at_byte` snap-floors mid-codepoint values silently, so
            // this assertion exists to catch upstream bugs before the visual
            // boundary moves.
            debug_assert!(
                committed_text.is_char_boundary(anchor.min(committed_text.len())),
                "selection_anchor must land on a char boundary"
            );
            let anchor_safe = anchor.min(committed_text.len());
            let (lo, hi) = if anchor_safe <= caret_safe {
                (anchor_safe, caret_safe)
            } else {
                (caret_safe, anchor_safe)
            };
            if lo < hi {
                // Collect (x_start, width) spans: one per level-run on a mixed /
                // RTL line (the logical range is not a single contiguous x-span
                // there), or a single span on the pure-LTR / CJK fast path.
                let spans: Vec<(f32, f32)> = if !shaped.runs.is_empty() {
                    slate_text::run_selection_rects(&shaped, lo, hi)
                } else {
                    let lo_px = pixel_x(lo);
                    let hi_px = pixel_x(hi);
                    let w = (hi_px - lo_px).max(0.0);
                    if w > 0.0 { vec![(lo_px, w)] } else { Vec::new() }
                };
                for (x_start, w) in spans {
                    cx.scene.push_rect(RectInstance {
                        rect: [
                            Lpx(bounds.origin.x + x_start),
                            Lpx(bounds.origin.y),
                            Lpx(w),
                            Lpx(line_height),
                        ],
                        color: self.style.preedit_selection_color,
                        corner_radius: Lpx(0.0),
                        _pad: [0.0; 3],
                    });
                }
            }
        }

        // 2. Glyphs
        match cx.text.rasterize_text_run(
            font,
            &shaped,
            [baseline_x, baseline_y],
            self.style.color,
            cx.glyph_atlas,
            cx.queue,
        ) {
            Ok(glyphs) => {
                for glyph in glyphs {
                    cx.scene.push_glyph(glyph);
                }
            }
            Err(e) => {
                log::error!("TextField: rasterize_text_run failed: {e}");
                return;
            }
        }

        // 3. Preedit underline + IME target-converted overlay
        if let Some(ref preedit) = preedit_snapshot {
            let preedit_end_byte = display_caret + preedit.text.len();
            let preedit_start_px = pixel_x(display_caret);
            let preedit_end_px = pixel_x(preedit_end_byte);
            let preedit_width = (preedit_end_px - preedit_start_px).max(0.0);

            // 1px underline beneath baseline
            if preedit_width > 0.0 {
                let underline_y = bounds.origin.y + shaped.ascent_lpx + 1.0;
                cx.scene.push_rect(RectInstance {
                    rect: [
                        Lpx(bounds.origin.x + preedit_start_px),
                        Lpx(underline_y),
                        Lpx(preedit_width),
                        Lpx(1.0),
                    ],
                    color: self.style.color,
                    corner_radius: Lpx(0.0),
                    _pad: [0.0; 3],
                });
            }

            // Translucent overlay for the OS target-converted sub-range.
            if let Some(ref sel) = preedit.selection {
                let sel_start_byte = display_caret + sel.start.min(preedit.text.len());
                let sel_end_byte = display_caret + sel.end.min(preedit.text.len());
                let sel_start_px = pixel_x(sel_start_byte);
                let sel_end_px = pixel_x(sel_end_byte);
                let sel_w = (sel_end_px - sel_start_px).max(0.0);

                if sel_w > 0.0 {
                    cx.scene.push_rect(RectInstance {
                        rect: [
                            Lpx(bounds.origin.x + sel_start_px),
                            Lpx(bounds.origin.y),
                            Lpx(sel_w),
                            Lpx(line_height),
                        ],
                        color: self.style.preedit_selection_color,
                        corner_radius: Lpx(0.0),
                        _pad: [0.0; 3],
                    });
                }
            }
        }

        // 4. Caret — 1px vertical line, visible only when focused and inside
        //    the "on" half of the blink cycle. Drawn last so it sits on top
        //    of selection / preedit overlays.
        //
        //    Blink: 530 ms half-period (matches macOS NSTextInsertionPointBlinkPeriod).
        //    On every focused paint we advance the cycle: if the deadline has
        //    passed we flip `visible` and arm the next deadline; either way we
        //    ask the platform to redraw at the deadline so the cycle continues
        //    without input. Unfocused: blink state stays at its defaults; no
        //    timer scheduled — zero CPU.
        let mut caret_visible = false;
        if let Some(state_rc) = cx.ime_registry.borrow().get(element_id)
            && let Ok(mut state) = state_rc.try_borrow_mut()
        {
            let (visible, next_deadline) = crate::elements::text_edit::blink::advance_blink(
                &mut state.blink,
                paint_state.focused,
                Instant::now(),
            );
            caret_visible = visible;
            if let Some(deadline) = next_deadline {
                cx.schedule_redraw_at(deadline);
            }
        }
        if paint_state.focused && caret_visible {
            cx.scene.push_rect(RectInstance {
                rect: [
                    Lpx(bounds.origin.x + caret_pixel_x),
                    Lpx(bounds.origin.y),
                    Lpx(1.0),
                    Lpx(line_height),
                ],
                color: self.style.caret_color,
                corner_radius: Lpx(0.0),
                _pad: [0.0; 3],
            });
        }

        // Update ImeState.caret_screen_rect for OS IME query channel AND cache
        // the just-shaped line + paint origin so the mouse handlers can compute
        // click-to-byte on the next frame. Folding both writes into a single
        // borrow keeps the registry borrow scope narrow and matches the
        // try_borrow_mut re-entrancy guard already in place.
        // `try_borrow_mut` skips silently on re-entrancy (handler re-entering paint).
        // Scene-space coordinates are lpx; convert to physical here since IME
        // delegates report screen-space physical pixels to the OS.
        if let Some(state_rc) = cx.ime_registry.borrow().get(element_id)
            && let Ok(mut state) = state_rc.try_borrow_mut()
        {
            state.caret_screen_rect = slate_platform::PhysicalRect::from_lpx_rect(
                bounds.origin.x + caret_pixel_x,
                bounds.origin.y,
                1.0,
                line_height,
                scale,
            );
            state.last_shaped = Some(std::rc::Rc::new(shaped));
            state.paint_origin_x = bounds.origin.x;
        }
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

// ---------------------------------------------------------------------------
// IntoElement
// ---------------------------------------------------------------------------

impl IntoElement for TextField {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
