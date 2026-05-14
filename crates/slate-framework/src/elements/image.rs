//! Image — GPU-rendered image element.
//!
//! Image is a leaf element that renders user-provided RGBA pixel data to the
//! screen via the framework's image atlas.
//!
//! # Pixel format contract
//!
//! The pixel buffer MUST be:
//! - 4 bytes per pixel (RGBA8)
//! - **sRGB-encoded** (NOT linear) — the atlas is `Rgba8UnormSrgb` so the
//!   GPU sampler decodes sRGB→linear automatically.
//! - **Straight** alpha (NOT premultiplied) — the fragment shader
//!   premultiplies at sample time (see `image.wgsl:64`).
//! - Length exactly `width * height * 4` bytes
//!
//! This is exactly what `image::open(...).to_rgba8()` produces from a PNG.
//! No per-pixel conversion required.
//!
//! # Example
//!
//! ```ignore
//! use slate_framework::Image;
//!
//! let img = image::open("logo.png")?.to_rgba8();
//! let (w, h) = img.dimensions();
//! let element = Image::new(w, h, img.into_raw());
//! ```
//!
//! # Size limits
//!
//! `width` and `height` must each be `<= MAX_IMAGE_DIM` (2048). Larger
//! inputs are warn-logged and the constructor returns an empty image.
//! Multi-page atlas support is deferred (see plan §Out of Scope).
//!
//! # Layout
//!
//! Image is a leaf element with intrinsic size equal to pixel dimensions.
//! Use `.style(|s| ...)` to override layout behavior (flex_grow, explicit
//! size, etc.).

use std::hash::{Hash, Hasher};

use taffy::prelude::*;

use slate_renderer::scene::ImageInstance;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{Element, IntoElement, Sealed};
use crate::style::Style;
use crate::types::{
    AccessibilityInfo, AccessibilityRole, Bounds, ElementId, LayoutId, NodeContext,
};

/// Maximum image dimension in pixels. Sourced from `slate_renderer::atlas::PAGE_SIZE`.
/// Images exceeding this size are warn-logged and degraded to empty.
pub const MAX_IMAGE_DIM: u32 = 2048;

/// GPU-rendered image element.
///
/// See [module-level documentation](self) for pixel format contract and usage.
pub struct Image {
    /// Pixel width (0 for empty/invalid image).
    width: u32,
    /// Pixel height (0 for empty/invalid image).
    height: u32,
    /// RGBA8 pixel data (sRGB, straight alpha). Kept on CPU for device-lost recovery.
    pixels: Vec<u8>,
    /// Content hash computed once at construction for paint cache identity.
    content_hash: u64,
    /// Layout style overrides (intrinsic size = pixel dims by default).
    layout_style: Style,
    /// User-provided stability key for dynamic lists.
    user_key: Option<String>,
    /// Stable ElementId allocated during prepaint.
    last_id: Option<ElementId>,
}

/// Layout state for Image — stores Taffy node ID.
pub struct ImageLayoutState {
    node_id: taffy::NodeId,
}

/// Paint state for Image — currently empty (atlas allocation handled by ImageCache).
pub struct ImagePaintState;

impl Image {
    /// Create a new Image from RGBA8 pixel data.
    ///
    /// # Arguments
    ///
    /// - `width`: Pixel width (must be `<= MAX_IMAGE_DIM`)
    /// - `height`: Pixel height (must be `<= MAX_IMAGE_DIM`)
    /// - `pixels`: RGBA8 pixel buffer (sRGB-encoded, straight alpha)
    ///
    /// # Validation
    ///
    /// - If `pixels.len() != width * height * 4`, logs a warning and returns empty image.
    /// - If `width > MAX_IMAGE_DIM` or `height > MAX_IMAGE_DIM`, logs a warning and returns empty image.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // From image crate
    /// let img = image::open("logo.png")?.to_rgba8();
    /// let (w, h) = img.dimensions();
    /// let element = Image::new(w, h, img.into_raw());
    ///
    /// // Procedural generation
    /// let pixels = vec![255u8; 100 * 100 * 4]; // white 100x100
    /// let element = Image::new(100, 100, pixels);
    /// ```
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        let expected_len = (width as usize) * (height as usize) * 4;

        // Validate dimensions
        if width > MAX_IMAGE_DIM || height > MAX_IMAGE_DIM {
            log::warn!(
                "Image::new: dimensions {}x{} exceed MAX_IMAGE_DIM ({}); returning empty image",
                width,
                height,
                MAX_IMAGE_DIM
            );
            return Self::default();
        }

        // Validate pixel buffer length
        if pixels.len() != expected_len {
            log::warn!(
                "Image::new: pixels.len() ({}) != expected ({} = {}x{}x4); returning empty image",
                pixels.len(),
                expected_len,
                width,
                height
            );
            return Self::default();
        }

        // Compute content hash once for paint cache identity
        let content_hash = {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            width.hash(&mut hasher);
            height.hash(&mut hasher);
            pixels.hash(&mut hasher);
            hasher.finish()
        };

        Self {
            width,
            height,
            pixels,
            content_hash,
            layout_style: Style::default(),
            user_key: None,
            last_id: None,
        }
    }

    /// Configure layout style via closure.
    ///
    /// # Example
    ///
    /// ```ignore
    /// Image::new(w, h, pixels)
    ///     .style(|s| s.flex_grow(1.0).width(Length::Px(200.0)))
    /// ```
    pub fn style(mut self, f: impl FnOnce(Style) -> Style) -> Self {
        self.layout_style = f(self.layout_style);
        self
    }

    /// Set a stability key for dynamic lists.
    ///
    /// Use when image order or count changes between frames (e.g., image gallery).
    /// Static trees can omit — tree-position keying handles them automatically.
    pub fn key(mut self, k: impl Into<String>) -> Self {
        self.user_key = Some(k.into());
        self
    }

    /// Returns the pixel width (0 for empty/invalid image).
    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Returns the pixel height (0 for empty/invalid image).
    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Returns the content hash (computed once at construction).
    #[inline]
    pub fn content_hash(&self) -> u64 {
        self.content_hash
    }
}

impl Default for Image {
    /// Returns an empty image (0x0, no pixels).
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            content_hash: 0,
            layout_style: Style::default(),
            user_key: None,
            last_id: None,
        }
    }
}

impl Sealed for Image {}

impl Element for Image {
    type LayoutState = ImageLayoutState;
    type PaintState = ImagePaintState;

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        // Convert unified Style to taffy::Style, then override intrinsic size
        let mut taffy_style = taffy::Style::from(&self.layout_style);

        // Set intrinsic size from pixel dimensions (leaf element)
        if self.width > 0 && self.height > 0 {
            taffy_style.size = taffy::Size {
                width: Dimension::length(self.width as f32),
                height: Dimension::length(self.height as f32),
            };
        }

        let node_id = match cx.taffy.new_leaf(taffy_style) {
            Ok(id) => id,
            Err(e) => {
                log::error!("Image: failed to create Taffy node: {e}; rendering empty");
                match cx.taffy.new_leaf(taffy::Style::default()) {
                    Ok(id) => id,
                    Err(e2) => {
                        log::error!(
                            "Image: Taffy new_leaf also failed ({e2}) — pathological state"
                        );
                        taffy::NodeId::from(u64::MAX)
                    }
                }
            }
        };

        // Set node context (leaf) — non-fatal if fails
        if let Err(e) = cx.taffy.set_node_context(node_id, Some(NodeContext::None)) {
            log::error!("Image: failed to set node context: {e}; layout proceeds without context");
        }

        (LayoutId(node_id), ImageLayoutState { node_id })
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        // Tree-position keying: set user key if provided, allocate stable ID
        if let Some(k) = self.user_key.take() {
            cx.set_next_key(k);
        }
        let element_id = cx.allocate_id::<Image>();
        self.last_id = Some(element_id);

        // Register accessibility node (role = Image)
        if let Some(info) = self.accessibility() {
            cx.prepaint_node_open(element_id, bounds, info);
            cx.prepaint_node_close();
        }

        ImagePaintState
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut Self::LayoutState,
        _paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        // Skip paint for empty images
        if self.width == 0 || self.height == 0 {
            return;
        }

        // Upload to atlas if needed, get allocation
        let alloc = cx.image_cache.upload_if_needed(
            self.content_hash,
            &self.pixels,
            self.width,
            self.height,
            cx.image_atlas,
            cx.queue,
        );

        // Build and push ImageInstance if allocation succeeded
        if let Some(alloc) = alloc {
            let scale = cx.scale_factor as f32;
            cx.scene.push_image(ImageInstance {
                rect: [
                    bounds.origin.x * scale,
                    bounds.origin.y * scale,
                    bounds.size.width * scale,
                    bounds.size.height * scale,
                ],
                uv_rect: alloc.uv_rect,
                tint: [1.0, 1.0, 1.0, 1.0], // premultiplied linear white (no tint)
            });
        }
        // OOM case: warning already logged by upload_if_needed, skip silently
    }

    fn accessibility(&self) -> Option<AccessibilityInfo> {
        Some(AccessibilityInfo {
            role: AccessibilityRole::Image,
            ..Default::default()
        })
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }

    fn paint_input_hash(&self, _bounds: Bounds) -> u64 {
        // Return pre-computed content hash (includes width, height, pixels)
        self.content_hash
    }
}

impl IntoElement for Image {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_valid_image() {
        let pixels = vec![255u8; 10 * 10 * 4];
        let img = Image::new(10, 10, pixels);
        assert_eq!(img.width(), 10);
        assert_eq!(img.height(), 10);
        assert_ne!(img.content_hash(), 0);
    }

    #[test]
    fn new_empty_on_invalid_length() {
        let pixels = vec![255u8; 100]; // wrong length for 10x10
        let img = Image::new(10, 10, pixels);
        assert_eq!(img.width(), 0);
        assert_eq!(img.height(), 0);
    }

    #[test]
    fn new_empty_on_oversized() {
        let pixels = vec![255u8; 3000 * 100 * 4];
        let img = Image::new(3000, 100, pixels); // exceeds MAX_IMAGE_DIM
        assert_eq!(img.width(), 0);
        assert_eq!(img.height(), 0);
    }

    #[test]
    fn default_is_empty() {
        let img = Image::default();
        assert_eq!(img.width(), 0);
        assert_eq!(img.height(), 0);
        assert_eq!(img.content_hash(), 0);
    }

    #[test]
    fn content_hash_deterministic() {
        let pixels = vec![128u8; 5 * 5 * 4];
        let img1 = Image::new(5, 5, pixels.clone());
        let img2 = Image::new(5, 5, pixels);
        assert_eq!(img1.content_hash(), img2.content_hash());
    }

    #[test]
    fn content_hash_differs_for_different_pixels() {
        let pixels1 = vec![0u8; 5 * 5 * 4];
        let pixels2 = vec![255u8; 5 * 5 * 4];
        let img1 = Image::new(5, 5, pixels1);
        let img2 = Image::new(5, 5, pixels2);
        assert_ne!(img1.content_hash(), img2.content_hash());
    }
}
