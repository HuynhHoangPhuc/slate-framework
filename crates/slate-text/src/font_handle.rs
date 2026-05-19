//! Font handle for cache keying.
//!
//! `FontHandle` uniquely identifies a font instance by combining a stable
//! `face_id` (hash of the platform face's PostScript name) with size and scale.
//! Platform face pointers are NOT used: CoreText returns fresh `CTFont`
//! addresses for substitute faces across shape calls (notably after IME
//! input-source switches), so pointer-as-identity causes glyph-cache thrash.

/// Unique identifier for a font instance, used as cache key.
///
/// # Cache Key Uniqueness
///
/// `face_id` is a 64-bit FxHash of the face's PostScript name (UTF-8 bytes).
/// Uniqueness comes from the `(face_id, size_lpx_bits, scale_bits)` triple.
/// Two fonts at different sizes from the same face will have distinct handles
/// even though they share `face_id`.
///
/// Two distinct platform face objects (e.g. substitute `CTFont` instances with
/// fresh pointer addresses) that represent the same logical face (same PSName)
/// produce equal `FontHandle`s — by design.
#[derive(Copy, Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct FontHandle {
    /// 64-bit FxHash of the face's PostScript name. Stable across pointer churn.
    pub face_id: u64,
    /// Font size in logical pixels, bit-encoded via `f32::to_bits()`.
    pub size_lpx_bits: u32,
    /// Display scale factor, bit-encoded via `f32::to_bits()`.
    pub scale_bits: u32,
}

impl FontHandle {
    /// Build from a precomputed `face_id` + size + scale.
    ///
    /// Platform-specific code derives `face_id` via PSName hashing (see
    /// `platform/macos/font_id.rs`). Tests may pass synthetic ids directly.
    pub fn from_face_id(face_id: u64, size_lpx: f32, scale: f32) -> Self {
        Self {
            face_id,
            size_lpx_bits: size_lpx.to_bits(),
            scale_bits: scale.to_bits(),
        }
    }

    /// Returns the font size in logical pixels.
    #[inline]
    pub fn size_lpx(&self) -> f32 {
        f32::from_bits(self.size_lpx_bits)
    }

    /// Returns the display scale factor.
    #[inline]
    pub fn scale(&self) -> f32 {
        f32::from_bits(self.scale_bits)
    }
}
