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
//! # Out of scope (Phase 10)
//!
//! - Text selection beyond the preedit overlay
//! - Clipboard cut/copy/paste
//! - Undo/redo
//! - Click-to-position caret
//! - Blinking caret
//!
//! See `plans/260517-0930-slate-phase-9c-ime-composition/phase-05-minimal-textfield-element.md`.

mod grapheme;
mod handlers;

use slate_reactive::Signal;
use slate_renderer::scene::RectInstance;
use taffy::prelude::*;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{Element, IntoElement, Sealed};
use crate::event::{ImeHandlers, KeyHandlers};
use crate::focus::FocusableEntry;
use crate::hit_test::{CursorStyle, HitRegion};
use crate::text_system::PlatformFont;
use crate::types::{Bounds, ElementId, LayoutId};

use handlers::{
    build_ime_commit_handler, build_ime_preedit_handler, build_key_down_handler,
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
                    });
            (self.style.width, shaped.ascent_lpx - shaped.descent_lpx)
        } else {
            match cx.text.shape_line(font, &current) {
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

        // Optional background
        if let Some(bg) = self.style.background {
            cx.scene.push_rect(RectInstance {
                rect: [
                    bounds.origin.x,
                    bounds.origin.y,
                    bounds.size.width,
                    line_height,
                ],
                color: bg,
                corner_radius: 0.0,
                _pad: [0.0; 3],
            });
        }

        let font = match &self.font {
            Some(f) => f,
            None => return,
        };

        // Snapshot ImeState to avoid holding a RefCell borrow across shaping calls
        let (committed_text, caret_byte, preedit_snapshot) = {
            match cx.ime_registry.borrow().get(element_id) {
                Some(rc) => {
                    let s = rc.borrow();
                    (s.text.clone(), s.caret, s.preedit.clone())
                }
                None => (self.value.get_untracked(), 0, None),
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

        // Shape display string
        let shaped = match cx.text.shape_line(font, &display_string) {
            Ok(s) => s,
            Err(e) => {
                log::error!("TextField: shape_line failed: {e}");
                return;
            }
        };

        let baseline_x = bounds.origin.x;
        let baseline_y = bounds.origin.y + shaped.ascent_lpx;

        // Build glyph advance table (index i = x-offset before glyph i).
        // MVP approximation: one glyph ≈ one character. Breaks for multi-glyph clusters
        // (e.g. certain Indic scripts). Full glyph↔byte mapping deferred to Phase 10.
        let mut advances: Vec<f32> = Vec::with_capacity(shaped.glyphs.len() + 1);
        {
            let mut x = 0.0f32;
            for g in &shaped.glyphs {
                advances.push(x);
                x += g.x_advance_lpx;
            }
            advances.push(x); // sentinel: pixel position after last glyph
        }

        let display_caret = caret_safe; // byte offset into display_string
        let caret_char_idx = display_string[..display_caret].chars().count();
        let preedit_start_char = caret_char_idx;
        let preedit_char_count = preedit_snapshot
            .as_ref()
            .map(|p| p.text.chars().count())
            .unwrap_or(0);

        let get_advance = |char_idx: usize| -> f32 {
            advances
                .get(char_idx)
                .copied()
                .unwrap_or_else(|| advances.last().copied().unwrap_or(0.0))
        };

        let caret_pixel_x = get_advance(caret_char_idx);
        let preedit_start_px = get_advance(preedit_start_char);
        let preedit_end_px = get_advance(preedit_start_char + preedit_char_count);

        // Rasterize and push glyphs
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

        // Caret — 1px vertical line, visible only when focused
        if paint_state.focused {
            cx.scene.push_rect(RectInstance {
                rect: [
                    bounds.origin.x + caret_pixel_x,
                    bounds.origin.y,
                    1.0,
                    line_height,
                ],
                color: self.style.caret_color,
                corner_radius: 0.0,
                _pad: [0.0; 3],
            });
        }

        // Preedit underline + optional selection highlight
        if let Some(ref preedit) = preedit_snapshot {
            let preedit_width = (preedit_end_px - preedit_start_px).max(0.0);

            // 1px underline beneath baseline
            if preedit_width > 0.0 {
                let underline_y = bounds.origin.y + shaped.ascent_lpx + 1.0;
                cx.scene.push_rect(RectInstance {
                    rect: [
                        bounds.origin.x + preedit_start_px,
                        underline_y,
                        preedit_width,
                        1.0,
                    ],
                    color: self.style.color,
                    corner_radius: 0.0,
                    _pad: [0.0; 3],
                });
            }

            // Translucent selection rect for IME target-converted range
            if let Some(ref sel) = preedit.selection {
                let sel_sc = preedit
                    .text
                    .get(..sel.start.min(preedit.text.len()))
                    .map(|s| s.chars().count())
                    .unwrap_or(0);
                let sel_ec = preedit
                    .text
                    .get(..sel.end.min(preedit.text.len()))
                    .map(|s| s.chars().count())
                    .unwrap_or(0);

                let sel_start_px = get_advance(preedit_start_char + sel_sc);
                let sel_end_px = get_advance(preedit_start_char + sel_ec);
                let sel_w = (sel_end_px - sel_start_px).max(0.0);

                if sel_w > 0.0 {
                    cx.scene.push_rect(RectInstance {
                        rect: [
                            bounds.origin.x + sel_start_px,
                            bounds.origin.y,
                            sel_w,
                            line_height,
                        ],
                        color: self.style.preedit_selection_color,
                        corner_radius: 0.0,
                        _pad: [0.0; 3],
                    });
                }
            }
        }

        // Update ImeState.caret_screen_rect for OS IME query channel.
        // `try_borrow_mut` skips silently on re-entrancy (handler re-entering paint).
        let caret_phys_x = ((bounds.origin.x + caret_pixel_x) * scale).round() as i32;
        let caret_phys_y = (bounds.origin.y * scale).round() as i32;
        let caret_phys_w = (1.0_f32 * scale).max(1.0) as u32;
        let caret_phys_h = (line_height * scale).round() as u32;

        if let Some(state_rc) = cx.ime_registry.borrow().get(element_id)
            && let Ok(mut state) = state_rc.try_borrow_mut()
        {
            state.caret_screen_rect = slate_platform::PhysicalRect::new(
                caret_phys_x,
                caret_phys_y,
                caret_phys_w,
                caret_phys_h,
            );
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
