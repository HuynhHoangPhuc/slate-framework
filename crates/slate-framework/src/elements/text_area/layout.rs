//! TextArea layout construction.
//!
//! Shapes the document once and fits it to a fixed content width, then floors
//! the element height to `min_lines` rows so an empty or short field still
//! reserves space. The expensive shaping happens here; `paint` reuses the
//! returned [`MultilineLayout`] (carried on the layout state) for glyph
//! placement and caret math — no re-shaping per frame.

use std::rc::Rc;

use slate_text::{MultilineLayout, TextError};

use crate::text_system::{PlatformFont, TextSystem};

/// Build the wrapped multi-line layout for `text` at `width_lpx`, returning the
/// shared layout plus the element height floored to `min_lines` rows.
///
/// `min_lines` of 0 is treated as 1 so the floor never collapses the field to
/// zero height (a layout always has ≥ 1 visual line).
pub(crate) fn build_layout(
    text_system: &TextSystem,
    font: &PlatformFont,
    text: &str,
    width_lpx: f32,
    min_lines: usize,
) -> Result<(Rc<MultilineLayout>, f32), TextError> {
    let doc = text_system.shape_document(font, text)?;
    let layout = slate_text::wrap_document(&doc, width_lpx);

    let floor_rows = min_lines.max(1) as f32;
    let height = layout.total_height_lpx.max(floor_rows * layout.line_height_lpx);

    Ok((Rc::new(layout), height))
}
