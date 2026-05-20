//! TextArea — multi-line wrapped text element.
//!
//! # v1 scope (this phase)
//!
//! Display + caret only: text bound to a `Signal<String>` is wrapped at a fixed
//! `style.width`, hard `\n` breaks force new lines, the element auto-grows in
//! height (floored to `min_lines`), and a blinking caret renders on the correct
//! visual line when focused. Vertical caret nav, editing keys, and selection
//! land in later phases.
//!
//! # Design lock: direct Element impl
//!
//! Like [`TextField`](crate::elements::TextField), TextArea implements `Element`
//! directly rather than wrapping a `Div` tree, because the caret overlay must be
//! emitted in the same paint pass as the glyphs after layout. The byte-aware
//! [`MultilineLayout`](slate_text::MultilineLayout) is built once in
//! `request_layout` (to size the Taffy leaf) and reused in `paint`.

mod handlers;
mod layout;
mod mouse;
mod nav;
mod paint;

use std::rc::Rc;

use slate_reactive::Signal;
use slate_text::MultilineLayout;
use taffy::prelude::*;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{Element, IntoElement, Sealed};
use crate::event::{ImeHandlers, KeyHandlers, MouseHandlers};
use crate::focus::FocusableEntry;
use crate::hit_test::{CursorStyle, HitRegion};
use crate::text_system::PlatformFont;
use crate::types::{Bounds, ElementId, LayoutId};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Visual style for `TextArea`.
#[derive(Clone, Debug)]
pub struct TextAreaStyle {
    /// Font size in logical pixels.
    pub font_size: f32,
    /// Text color (linear, premultiplied RGBA).
    pub color: [f32; 4],
    /// Optional background fill.
    pub background: Option<[f32; 4]>,
    /// Caret color (linear, premultiplied RGBA).
    pub caret_color: [f32; 4],
    /// Selection highlight color (translucent accent; ~30% alpha). Unused until
    /// selection lands in a later phase; kept here so the style is stable.
    pub selection_color: [f32; 4],
    /// Color of the 1px underline drawn beneath the active IME composition.
    pub preedit_underline_color: [f32; 4],
    /// Highlight color for the IME target-converted sub-range, drawn behind the
    /// composed glyphs (translucent accent; ~30% alpha).
    pub preedit_selection_color: [f32; 4],
    /// Fixed content width in logical pixels — text wraps to this width.
    pub width: f32,
    /// Minimum height in whole text rows. The element never shrinks below this
    /// many line-heights even when the text occupies fewer lines.
    pub min_lines: usize,
}

impl Default for TextAreaStyle {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            color: [1.0, 1.0, 1.0, 1.0],
            background: None,
            caret_color: [1.0, 1.0, 1.0, 1.0],
            selection_color: [0.4, 0.6, 1.0, 0.3],
            preedit_underline_color: [1.0, 1.0, 1.0, 1.0],
            preedit_selection_color: [0.4, 0.6, 1.0, 0.3],
            width: 300.0,
            min_lines: 1,
        }
    }
}

/// Multi-line text element backed by a `Signal<String>`.
pub struct TextArea {
    value: Signal<String>,
    style: TextAreaStyle,
    font: Option<PlatformFont>,
    /// Stable ElementId allocated during prepaint (available after prepaint).
    last_id: Option<ElementId>,
}

impl TextArea {
    /// Create a new TextArea bound to `value`.
    pub fn new(value: Signal<String>) -> Self {
        Self {
            value,
            style: TextAreaStyle::default(),
            font: None,
            last_id: None,
        }
    }

    /// Override the visual style.
    pub fn style(mut self, s: TextAreaStyle) -> Self {
        self.style = s;
        self
    }
}

// ---------------------------------------------------------------------------
// Per-phase state types
// ---------------------------------------------------------------------------

/// State produced during `request_layout`, carrying the shaped layout forward
/// to `paint` so the document is shaped exactly once per frame.
pub struct TextAreaLayoutState {
    layout: Rc<MultilineLayout>,
}

/// State produced during `prepaint`, consumed by `paint`.
pub struct TextAreaPaintState {
    element_id: ElementId,
    /// Whether this element was focused at prepaint time (gates caret visibility).
    focused: bool,
}

// ---------------------------------------------------------------------------
// Element impl
// ---------------------------------------------------------------------------

impl Sealed for TextArea {}

impl Element for TextArea {
    type LayoutState = TextAreaLayoutState;
    type PaintState = TextAreaPaintState;

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        let scale = cx.scale_factor as f32;

        // Load bundled font once (mirrors TextField::request_layout).
        if self.font.is_none() {
            match cx
                .text
                .load_font_from_bytes(slate_text::TEST_FONT, self.style.font_size, scale)
            {
                Ok(f) => self.font = Some(f),
                Err(e) => {
                    log::error!("TextArea: font load failed: {e}; rendering zero-size");
                    let node_id = cx
                        .taffy
                        .new_leaf(taffy::Style::default())
                        .unwrap_or_else(|_| taffy::NodeId::from(u64::MAX));
                    return (
                        LayoutId(node_id),
                        TextAreaLayoutState {
                            layout: Rc::new(empty_layout()),
                        },
                    );
                }
            }
        }

        let font = self.font.as_ref().unwrap();
        let current = self.value.get_untracked();

        let (layout, height) = match layout::build_layout(
            cx.text,
            font,
            &current,
            self.style.width,
            self.style.min_lines,
        ) {
            Ok(pair) => pair,
            Err(e) => {
                log::error!("TextArea: layout build failed: {e}; rendering zero-size");
                (Rc::new(empty_layout()), 0.0)
            }
        };

        let node_id = match cx.taffy.new_leaf(taffy::Style {
            size: taffy::Size {
                width: Dimension::length(self.style.width),
                height: Dimension::length(height),
            },
            ..Default::default()
        }) {
            Ok(id) => id,
            Err(e) => {
                log::error!("TextArea: Taffy new_leaf failed: {e}");
                taffy::NodeId::from(u64::MAX)
            }
        };

        (LayoutId(node_id), TextAreaLayoutState { layout })
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        let element_id = cx.allocate_id::<TextArea>();
        self.last_id = Some(element_id);

        // I-beam hit region for pointer hit testing.
        cx.register_hit_region(
            HitRegion::new(element_id, bounds, 0).with_cursor(CursorStyle::Text),
        );

        // Opt into keyboard focus (tab_index 0 = default Tab cycle order).
        cx.register_focusable(
            FocusableEntry {
                id: element_id,
                tab_index: 0,
                focus_ring: true,
            },
            bounds,
            0.0,
        );

        // Ensure ImeState exists; seed text from the signal on first frame, then
        // seed the undo baseline once (reuses the Phase 1 single-line helpers —
        // the buffer + undo contract is shared across both editable elements).
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
            state.seed_undo_baseline();
        }

        let focused = cx.focused_element() == Some(element_id);

        // Register multi-line key + IME handlers (mirrors TextField; the key
        // handler additionally consumes Enter → '\n' and handles ↑/↓/Home/End).
        cx.register_key_handlers(
            element_id,
            KeyHandlers {
                on_key_down: Some(handlers::build_key_down_handler(self.value.clone())),
                on_key_up: None,
                on_text_input: Some(handlers::build_text_input_handler(self.value.clone())),
            },
        );

        cx.register_ime_handlers(
            element_id,
            ImeHandlers {
                on_ime_preedit: Some(handlers::build_ime_preedit_handler()),
                on_ime_commit: Some(handlers::build_ime_commit_handler(self.value.clone())),
                on_ime_enabled: None,
                on_ime_disabled: None,
            },
        );

        // Click-to-place + drag-select across visual lines. Handlers read the
        // cached MultilineLayout + paint origin written by paint() last frame.
        cx.register_mouse_handlers(
            element_id,
            MouseHandlers {
                on_mouse_down: Some(mouse::build_mouse_down_handler()),
                on_mouse_move: Some(mouse::build_mouse_move_handler()),
                on_mouse_up: Some(mouse::build_mouse_up_handler()),
            },
        );

        TextAreaPaintState {
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
        let font = match &self.font {
            Some(f) => f,
            None => return,
        };
        // Display-only invariant (this phase): the layout is built from the
        // signal in `request_layout` and the caret byte indexes the seeded
        // `ImeState.text`, which the prepaint seed keeps equal to the signal.
        // When editing lands, build the layout from the same buffer the caret
        // indexes so glyph runs and caret never diverge.
        paint::paint(
            &self.style,
            font,
            &layout_state.layout,
            paint_state.element_id,
            paint_state.focused,
            bounds,
            cx,
        );
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

/// Expose the production `on_key_down` handler so headless tests can drive the
/// real closure (its borrow pattern, ↑/↓/Home/End/Enter arms) through
/// `AppState::dispatch_key_down`, rather than re-implementing it. Test-only.
#[cfg(feature = "test-hooks")]
pub fn build_key_down_handler_for_test(
    value: Signal<String>,
) -> crate::event::ElementKeyHandler {
    handlers::build_key_down_handler(value)
}

/// Expose the production mouse handlers (down/move/up) so headless tests can
/// drive the real click-count machine + word/line-select branch through
/// `AppState::dispatch_mouse_*`, rather than re-implementing the closures.
/// Test-only.
#[cfg(feature = "test-hooks")]
pub fn build_mouse_handlers_for_test() -> MouseHandlers {
    MouseHandlers {
        on_mouse_down: Some(mouse::build_mouse_down_handler()),
        on_mouse_move: Some(mouse::build_mouse_move_handler()),
        on_mouse_up: Some(mouse::build_mouse_up_handler()),
    }
}

/// Expose the production `on_text_input` handler (typed-character insertion) so
/// end-to-end tests can dispatch text input through the real closure. Test-only.
#[cfg(feature = "test-hooks")]
pub fn build_text_input_handler_for_test(
    value: Signal<String>,
) -> crate::event::ElementTextInputHandler {
    handlers::build_text_input_handler(value)
}

/// Expose the production IME handlers (preedit + commit) so end-to-end tests can
/// drive composition through the real closures. Test-only.
#[cfg(feature = "test-hooks")]
pub fn build_ime_handlers_for_test(value: Signal<String>) -> ImeHandlers {
    ImeHandlers {
        on_ime_preedit: Some(handlers::build_ime_preedit_handler()),
        on_ime_commit: Some(handlers::build_ime_commit_handler(value)),
        on_ime_enabled: None,
        on_ime_disabled: None,
    }
}

/// A degenerate single-empty-line layout used when font load or shaping fails,
/// so `paint` always has a valid (zero-glyph) layout to read.
fn empty_layout() -> MultilineLayout {
    MultilineLayout {
        lines: Vec::new(),
        total_height_lpx: 0.0,
        line_height_lpx: 0.0,
    }
}

// ---------------------------------------------------------------------------
// IntoElement
// ---------------------------------------------------------------------------

impl IntoElement for TextArea {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
