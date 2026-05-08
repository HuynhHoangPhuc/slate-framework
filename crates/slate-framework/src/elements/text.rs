//! Text — text label element.
//!
//! Leaf element that renders a single line of text.

use slate_text::types::ShapedLine;
use taffy::prelude::*;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{Element, IntoElement, Sealed};
use crate::text_system::PlatformFont;
use crate::types::{Bounds, LayoutId, NodeContext};

/// Text label element.
///
/// Renders a single line of text with the specified font and color.
///
/// # Example
///
/// ```ignore
/// Text::new("Hello, World!")
///     .color([1.0, 1.0, 1.0, 1.0])
///     .font_size(16.0)
/// ```
pub struct Text {
    content: String,
    style: TextStyle,
    font: Option<PlatformFont>,
}

/// Visual styling for Text.
#[derive(Clone, Debug)]
pub struct TextStyle {
    /// Text color (linear, premultiplied RGBA).
    pub color: [f32; 4],
    /// Font size in logical pixels.
    pub font_size: f32,
    /// Font family name (None = bundled DejaVu Sans).
    pub font_family: Option<String>,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0], // White
            font_size: 14.0,
            font_family: None,
        }
    }
}

/// Layout state for Text — stores shaped line for rasterization.
pub struct TextLayoutState {
    /// Pre-shaped text line.
    shaped: ShapedLine,
    /// Taffy node ID (reserved for future use).
    #[allow(dead_code)]
    node_id: taffy::NodeId,
}

/// Paint state for Text — currently empty.
pub struct TextPaintState;

impl Text {
    /// Create a new Text element with the given content.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            style: TextStyle::default(),
            font: None,
        }
    }

    /// Set text color (linear, premultiplied RGBA).
    pub fn color(mut self, color: [f32; 4]) -> Self {
        self.style.color = color;
        self
    }

    /// Set font size in logical pixels.
    pub fn font_size(mut self, size: f32) -> Self {
        self.style.font_size = size;
        self
    }

    /// Set font family name.
    pub fn font_family(mut self, family: impl Into<String>) -> Self {
        self.style.font_family = Some(family.into());
        self
    }
}

impl Sealed for Text {}

impl Element for Text {
    type LayoutState = TextLayoutState;
    type PaintState = TextPaintState;

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        let scale = cx.scale_factor as f32;

        // Load font if not cached
        if self.font.is_none() {
            let font = if let Some(ref family) = self.style.font_family {
                cx.text
                    .load_font(family, self.style.font_size, scale)
                    .unwrap_or_else(|_| {
                        // Fall back to bundled font
                        cx.text
                            .load_font_from_bytes(
                                slate_text::TEST_FONT,
                                self.style.font_size,
                                scale,
                            )
                            .expect("failed to load bundled font")
                    })
            } else {
                cx.text
                    .load_font_from_bytes(slate_text::TEST_FONT, self.style.font_size, scale)
                    .expect("failed to load bundled font")
            };
            self.font = Some(font);
        }

        let font = self.font.as_ref().unwrap();

        // Shape the text
        let shaped = match cx.text.shape_line(font, &self.content) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Text shaping failed: {e}; using zero-size layout");
                slate_text::types::ShapedLine {
                    glyphs: Vec::new(),
                    width_lpx: 0.0,
                    ascent_lpx: 0.0,
                    descent_lpx: 0.0,
                    y_offset_lpx: 0.0,
                }
            }
        };

        let width = shaped.width_lpx;
        let height = shaped.ascent_lpx - shaped.descent_lpx;

        // Create Taffy leaf node with fixed size
        let style = Style {
            size: taffy::Size {
                width: Dimension::length(width),
                height: Dimension::length(height),
            },
            ..Default::default()
        };

        let node_id = cx
            .taffy
            .new_leaf(style)
            .expect("failed to create Text node");

        // Store text dimensions in node context for measure
        cx.taffy
            .set_node_context(
                node_id,
                Some(NodeContext::Text {
                    width_lpx: width,
                    height_lpx: height,
                }),
            )
            .expect("failed to set node context");

        let state = TextLayoutState { shaped, node_id };

        (LayoutId(node_id), state)
    }

    fn prepaint(
        &mut self,
        _bounds: Bounds,
        _layout_state: &mut Self::LayoutState,
        _cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        // Text has no interactive regions (yet)
        TextPaintState
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        _paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        let font = match &self.font {
            Some(f) => f,
            None => return, // No font loaded, skip painting
        };

        // Compute baseline position
        // baseline_y = bounds.origin.y + ascent (text hangs from baseline)
        let baseline_x = bounds.origin.x;
        let baseline_y = bounds.origin.y + layout_state.shaped.ascent_lpx;

        // Rasterize text to glyph instances
        let glyphs = match cx.text.rasterize_text_run(
            font,
            &layout_state.shaped,
            [baseline_x, baseline_y],
            self.style.color,
            cx.glyph_atlas,
            cx.queue,
        ) {
            Ok(g) => g,
            Err(e) => {
                log::error!("Text rasterization failed: {e}");
                return;
            }
        };

        // Push glyphs to scene
        for glyph in glyphs {
            cx.scene.push_glyph(glyph);
        }
    }
}

impl IntoElement for Text {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

// IntoElement for &'static str
impl Sealed for &'static str {}

impl IntoElement for &'static str {
    type Element = Text;
    fn into_element(self) -> Text {
        Text::new(self.to_string())
    }
}

// IntoElement for String
impl Sealed for String {}

impl IntoElement for String {
    type Element = Text;
    fn into_element(self) -> Text {
        Text::new(self)
    }
}
