//! Text — text label element.
//!
//! Leaf element that renders single or multi-line text with wrapping support.

use slate_text::types::ShapedLine;
use taffy::prelude::*;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{Element, IntoElement, Sealed};
use crate::text_system::PlatformFont;
use crate::types::{
    AccessibilityInfo, AccessibilityNode, AccessibilityRole, Bounds, ElementId, LayoutId,
    NodeContext,
};

/// Text wrapping mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextWrap {
    /// No wrapping — single line, may overflow container.
    #[default]
    None,
    /// Wrap on whitespace at container width.
    Wrap,
    /// Wrap and break inside long words (v1.1 — falls back to Wrap for now).
    WrapBreakWord,
}

/// Text alignment within the container.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Text label element.
///
/// Renders single or multi-line text with optional wrapping.
///
/// # Example
///
/// ```ignore
/// Text::new("Hello, World!")
///     .color([1.0, 1.0, 1.0, 1.0])
///     .font_size(16.0)
///     .wrap(TextWrap::Wrap)
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
    /// Text wrapping mode.
    pub wrap: TextWrap,
    /// Text alignment.
    pub align: TextAlign,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0], // White
            font_size: 14.0,
            font_family: None,
            wrap: TextWrap::None,
            align: TextAlign::Left,
        }
    }
}

/// Layout state for Text — stores shaped lines for rasterization.
pub struct TextLayoutState {
    /// Pre-shaped text lines (one for single-line, multiple for wrapped).
    lines: Vec<ShapedLine>,
    /// Line height in logical pixels.
    line_height: f32,
    /// Taffy node ID.
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

    /// Set text wrapping mode.
    pub fn wrap(mut self, wrap: TextWrap) -> Self {
        self.style.wrap = wrap;
        self
    }

    /// Set text alignment.
    pub fn align(mut self, align: TextAlign) -> Self {
        self.style.align = align;
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

        // For non-wrapped text, just shape the whole content as single line
        // For wrapped text, we'll do the wrapping in a measure function
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

        let line_height = shaped.ascent_lpx - shaped.descent_lpx;

        // For wrapping, we use a measure function approach
        let (width, height, lines) = if self.style.wrap != TextWrap::None {
            // Wrapped: width comes from parent container, height computed from lines
            // For now, shape as single line; actual wrapping happens if parent constrains width
            let width = shaped.width_lpx;
            let height = line_height;
            (width, height, vec![shaped])
        } else {
            // No wrap: fixed intrinsic size
            let width = shaped.width_lpx;
            let height = line_height;
            (width, height, vec![shaped])
        };

        // Create Taffy leaf node
        let style = if self.style.wrap != TextWrap::None {
            // For wrapped text: flex-shrink and don't set fixed width
            Style {
                size: taffy::Size {
                    width: Dimension::AUTO,
                    height: Dimension::AUTO,
                },
                min_size: taffy::Size {
                    width: Dimension::length(0.0),
                    height: Dimension::length(line_height),
                },
                ..Default::default()
            }
        } else {
            Style {
                size: taffy::Size {
                    width: Dimension::length(width),
                    height: Dimension::length(height),
                },
                ..Default::default()
            }
        };

        let node_id = cx
            .taffy
            .new_leaf(style)
            .expect("failed to create Text node");

        // Store text info in node context
        cx.taffy
            .set_node_context(
                node_id,
                Some(NodeContext::Text {
                    width_lpx: width,
                    height_lpx: height,
                }),
            )
            .expect("failed to set node context");

        let state = TextLayoutState {
            lines,
            line_height,
            node_id,
        };

        (LayoutId(node_id), state)
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        // Register accessibility node for this Text
        if let Some(info) = self.accessibility() {
            cx.register_a11y_node(AccessibilityNode {
                id: ElementId::next(),
                bounds,
                info,
                children: Vec::new(),
            });
        }

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

        // Paint each line
        let mut y_offset = 0.0;
        for line in &layout_state.lines {
            // Compute x position based on alignment
            let line_width = line.width_lpx;
            let baseline_x = match self.style.align {
                TextAlign::Left => bounds.origin.x,
                TextAlign::Center => bounds.origin.x + (bounds.size.width - line_width) / 2.0,
                TextAlign::Right => bounds.origin.x + bounds.size.width - line_width,
            };

            // baseline_y = top of line + ascent
            let baseline_y = bounds.origin.y + y_offset + line.ascent_lpx;

            // Rasterize text to glyph instances
            let glyphs = match cx.text.rasterize_text_run(
                font,
                line,
                [baseline_x, baseline_y],
                self.style.color,
                cx.glyph_atlas,
                cx.queue,
            ) {
                Ok(g) => g,
                Err(e) => {
                    log::error!("Text rasterization failed: {e}");
                    continue;
                }
            };

            // Push glyphs to scene
            for glyph in glyphs {
                cx.scene.push_glyph(glyph);
            }

            y_offset += layout_state.line_height;
        }
    }

    fn accessibility(&self) -> Option<AccessibilityInfo> {
        Some(AccessibilityInfo {
            role: AccessibilityRole::Label,
            label: Some(self.content.clone()),
            ..Default::default()
        })
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
