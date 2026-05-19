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
//! - Bidi visual↔logical reorder.

mod grapheme;
mod handlers;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Caret blink half-period. Matches macOS `NSTextInsertionPointBlinkPeriod`.
const CARET_BLINK_PERIOD: Duration = Duration::from_millis(530);

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
// Undo / redo — Apple Notes coalesce (Phase 10b.3)
// ---------------------------------------------------------------------------

const UNDO_STACK_CAP: usize = 100;
const COALESCE_TIME_GAP: Duration = Duration::from_millis(500);

/// Snapshot of TextField state. Stored as **post-edit** snapshots — the state
/// the field was in immediately after the edit that produced it. `entries[0]`
/// is the baseline (initial value) so undo can always walk back to "before the
/// first edit".
#[derive(Clone, Debug)]
pub(super) struct EditSnapshot {
    pub(super) text: String,
    pub(super) caret: usize,
    pub(super) anchor: Option<usize>,
}

/// Classification of an edit for coalesce decisions. `Insert` and `Backspace`
/// are coalescible bursts; `Discrete` (paste, cut, selection-delete, IME
/// commit) always pushes a fresh undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditOp {
    Insert,
    Backspace,
    Discrete,
}

/// Apple Notes-style coalesce: typing within 500 ms of the previous typing
/// keystroke folds into the same undo step; anything else (motion, time gap,
/// op-type change, discrete op) opens a new step.
#[derive(Debug)]
pub(super) struct UndoStack {
    /// History of post-edit states. Always non-empty; `entries[0]` is the
    /// baseline. `entries[cursor]` reflects the live document.
    entries: VecDeque<EditSnapshot>,
    cursor: usize,
    /// Op that produced the head entry. `None` after undo/redo so the next
    /// edit pushes fresh rather than coalescing into a state the user already
    /// walked back through.
    head_op: Option<EditOp>,
    head_edit_t: Option<Instant>,
    /// Set by non-typing caret motion (arrow / Home / End / click / drag).
    /// Forces the next edit to open a fresh undo step.
    motion_since_last_edit: bool,
}

impl UndoStack {
    /// Construct an undo stack seeded with the field's initial state.
    pub(super) fn with_baseline(text: String, caret: usize) -> Self {
        let mut entries = VecDeque::with_capacity(UNDO_STACK_CAP);
        entries.push_back(EditSnapshot {
            text,
            caret,
            anchor: None,
        });
        Self {
            entries,
            cursor: 0,
            head_op: None,
            head_edit_t: None,
            motion_since_last_edit: false,
        }
    }

    /// Record a completed edit. `post` is the state *after* the mutation. The
    /// 4-trigger policy decides whether to push a new entry or overwrite the
    /// head entry in place.
    pub(super) fn record_edit(&mut self, op: EditOp, post: EditSnapshot) {
        // New edits truncate the redo tail.
        if self.cursor + 1 < self.entries.len() {
            self.entries.drain(self.cursor + 1..);
        }

        if self.should_push(op) {
            if self.entries.len() == UNDO_STACK_CAP {
                self.entries.pop_front();
                self.cursor = self.cursor.saturating_sub(1);
            }
            self.entries.push_back(post);
            self.cursor = self.entries.len() - 1;
        } else if let Some(slot) = self.entries.get_mut(self.cursor) {
            *slot = post;
        }

        self.head_op = Some(op);
        self.head_edit_t = Some(Instant::now());
        self.motion_since_last_edit = false;
    }

    /// Mark that the caret moved without an edit. Forces the next edit to
    /// open a fresh undo step.
    pub(super) fn mark_motion(&mut self) {
        self.motion_since_last_edit = true;
    }

    /// Pop one undo step. Returns the state to restore (and the field's live
    /// state should be assigned from it). `None` when nothing left to undo.
    pub(super) fn undo(&mut self) -> Option<EditSnapshot> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        // After undo, the head op is no longer the live op — the next edit
        // must open a new step, never coalesce with what the user walked back.
        self.head_op = None;
        self.head_edit_t = None;
        self.motion_since_last_edit = false;
        Some(self.entries[self.cursor].clone())
    }

    /// Step forward through redo. Returns the state to restore, or `None` if
    /// nothing to redo.
    pub(super) fn redo(&mut self) -> Option<EditSnapshot> {
        if self.cursor + 1 >= self.entries.len() {
            return None;
        }
        self.cursor += 1;
        self.head_op = None;
        self.head_edit_t = None;
        self.motion_since_last_edit = false;
        Some(self.entries[self.cursor].clone())
    }

    fn should_push(&self, op: EditOp) -> bool {
        let Some(prev_op) = self.head_op else {
            return true;
        };
        // Trigger 4: discrete ops never coalesce (in either direction).
        if op == EditOp::Discrete || prev_op == EditOp::Discrete {
            return true;
        }
        // Trigger 3: op-type change.
        if prev_op != op {
            return true;
        }
        // Trigger 1: time gap > 500 ms since last edit.
        if let Some(t) = self.head_edit_t
            && t.elapsed() > COALESCE_TIME_GAP
        {
            return true;
        }
        // Trigger 2: caret moved by non-typing action since last edit.
        if self.motion_since_last_edit {
            return true;
        }
        false
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(super) fn cursor(&self) -> usize {
        self.cursor
    }
}

/// Shared, thread-safe handle to a TextField's undo history.
///
/// Lock-order invariant: when both are held in the same handler, the
/// `RefCell<ImeState>` borrow comes **first**, then `Mutex<UndoStack>`.
/// Reversing the order would deadlock against the RefCell panic on
/// re-entry and is forbidden — all current call sites snapshot inside
/// the live `RefMut<ImeState>` to enforce this naturally.
pub(super) type UndoStackHandle = Arc<Mutex<UndoStack>>;

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
    /// Per-field undo history. Handlers push a pre-mutation snapshot before
    /// every text edit (10a.7 scaffolding; bindings in 10b.3).
    undo: UndoStackHandle,
}

impl TextField {
    /// Create a new TextField bound to `value`.
    pub fn new(value: Signal<String>) -> Self {
        let initial = value.get_untracked();
        let caret = initial.len();
        let undo = Arc::new(Mutex::new(UndoStack::with_baseline(initial, caret)));
        Self {
            value,
            style: TextFieldStyle::default(),
            font: None,
            last_id: None,
            undo,
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
                on_key_down: Some(build_key_down_handler(
                    self.value.clone(),
                    Arc::clone(&self.undo),
                )),
                on_key_up: None,
                on_text_input: Some(build_text_input_handler(
                    self.value.clone(),
                    Arc::clone(&self.undo),
                )),
            },
        );

        cx.register_ime_handlers(
            element_id,
            ImeHandlers {
                on_ime_preedit: Some(build_ime_preedit_handler()),
                on_ime_commit: Some(build_ime_commit_handler(
                    self.value.clone(),
                    Arc::clone(&self.undo),
                )),
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
                on_mouse_down: Some(build_mouse_down_handler(Arc::clone(&self.undo))),
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
        let (committed_text, caret_byte, preedit_snapshot, selection_anchor) = {
            match cx.ime_registry.borrow().get(element_id) {
                Some(rc) => {
                    let s = rc.borrow();
                    (
                        s.text.clone(),
                        s.caret,
                        s.preedit.clone(),
                        s.selection_anchor,
                    )
                }
                None => (self.value.get_untracked(), 0, None, None),
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
        let display_caret = caret_safe; // byte offset into display_string

        // Cluster-aware byte→pixel-x. Snaps to grapheme boundaries, so CJK
        // ligatures, Indic clusters, and emoji ZWJ sequences all behave.
        let pixel_x =
            |byte: usize| -> f32 { slate_text::pixel_x_at_byte(&shaped, &display_string, byte) };
        let caret_pixel_x = pixel_x(display_caret);

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
                let lo_px = pixel_x(lo);
                let hi_px = pixel_x(hi);
                let w = (hi_px - lo_px).max(0.0);
                if w > 0.0 {
                    cx.scene.push_rect(RectInstance {
                        rect: [
                            Lpx(bounds.origin.x + lo_px),
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
            if paint_state.focused {
                let now = Instant::now();
                match state.blink.next {
                    None => {
                        state.blink.visible = true;
                        state.blink.next = Some(now + CARET_BLINK_PERIOD);
                    }
                    Some(t) if now >= t => {
                        state.blink.visible = !state.blink.visible;
                        state.blink.next = Some(now + CARET_BLINK_PERIOD);
                    }
                    Some(_) => {}
                }
                caret_visible = state.blink.visible;
                if let Some(deadline) = state.blink.next {
                    cx.schedule_redraw_at(deadline);
                }
            } else {
                // Unfocused: drop any in-flight blink so the next focus gain
                // starts a fresh cycle from visible=true.
                state.blink.visible = true;
                state.blink.next = None;
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

// ---------------------------------------------------------------------------
// Unit tests — UndoStack coalesce policy (Phase 10b.3)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod undo_tests {
    use super::*;

    fn snap(text: &str, caret: usize) -> EditSnapshot {
        EditSnapshot {
            text: text.to_string(),
            caret,
            anchor: None,
        }
    }

    fn fresh() -> UndoStack {
        UndoStack::with_baseline(String::new(), 0)
    }

    #[test]
    fn baseline_is_present_and_undo_returns_none() {
        let mut s = fresh();
        assert_eq!(s.len(), 1);
        assert_eq!(s.cursor(), 0);
        assert!(s.undo().is_none());
    }

    #[test]
    fn typing_burst_coalesces_into_single_step() {
        let mut s = fresh();
        s.record_edit(EditOp::Insert, snap("a", 1));
        s.record_edit(EditOp::Insert, snap("ab", 2));
        s.record_edit(EditOp::Insert, snap("abc", 3));
        // Baseline + one coalesced typing entry.
        assert_eq!(s.len(), 2);
        let restored = s.undo().expect("undo should restore baseline");
        assert_eq!(restored.text, "");
    }

    #[test]
    fn op_type_change_opens_new_step() {
        let mut s = fresh();
        s.record_edit(EditOp::Insert, snap("a", 1));
        s.record_edit(EditOp::Backspace, snap("", 0));
        // Baseline + insert + backspace = 3.
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn motion_marks_split_typing_into_two_steps() {
        let mut s = fresh();
        s.record_edit(EditOp::Insert, snap("a", 1));
        s.mark_motion();
        s.record_edit(EditOp::Insert, snap("ab", 2));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn discrete_op_never_coalesces() {
        let mut s = fresh();
        s.record_edit(EditOp::Discrete, snap("p", 1));
        s.record_edit(EditOp::Discrete, snap("pp", 2));
        // Two discrete ops in a row both push.
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn typing_after_discrete_op_does_not_coalesce_backward() {
        let mut s = fresh();
        s.record_edit(EditOp::Discrete, snap("pasted", 6));
        s.record_edit(EditOp::Insert, snap("pastedX", 7));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn undo_then_edit_truncates_redo_tail() {
        let mut s = fresh();
        s.record_edit(EditOp::Insert, snap("a", 1));
        s.record_edit(EditOp::Backspace, snap("", 0));
        s.undo();
        // cursor now at the insert entry; new edit truncates redo tail.
        s.record_edit(EditOp::Insert, snap("aX", 2));
        assert_eq!(s.len(), 3);
        assert_eq!(s.cursor(), 2);
        assert!(s.redo().is_none(), "redo tail must be empty after new edit");
    }

    #[test]
    fn redo_walks_forward_after_undo() {
        let mut s = fresh();
        s.record_edit(EditOp::Insert, snap("a", 1));
        s.record_edit(EditOp::Backspace, snap("", 0));
        let _ = s.undo();
        let redone = s.redo().expect("redo available");
        assert_eq!(redone.text, "");
    }

    #[test]
    fn capacity_overflow_drops_oldest_and_clamps_cursor() {
        let mut s = fresh();
        // Force a fresh push every step (motion between Inserts).
        for i in 1..=(UNDO_STACK_CAP + 5) {
            s.record_edit(EditOp::Insert, snap(&"x".repeat(i), i));
            s.mark_motion();
        }
        assert_eq!(s.len(), UNDO_STACK_CAP);
        // cursor points at the latest entry — never out of range.
        assert!(s.cursor() < s.len());
    }

    #[test]
    fn undo_after_typing_burst_restores_pre_burst_state() {
        let mut s = fresh();
        s.record_edit(EditOp::Insert, snap("h", 1));
        s.record_edit(EditOp::Insert, snap("he", 2));
        s.record_edit(EditOp::Insert, snap("hel", 3));
        let restored = s.undo().expect("undo");
        assert_eq!(restored.text, "", "Apple Notes: one undo erases the burst");
    }
}
