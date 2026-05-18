//! Font handle for cache keying.
//!
//! `FontHandle` uniquely identifies a font instance by combining the platform
//! font pointer address with size and scale. Pointer alone is insufficient
//! because CoreText/DirectWrite may recycle pointers across reloads at
//! different sizes.

/// Unique identifier for a font instance, used as cache key.
///
/// # Safety Invariant
///
/// `FontHandle` is only valid while the originating `Font` value is alive.
/// Platform font object pointers (CTFont/IDWriteFontFace) are unique per-instance
/// and stable while held, preventing address reuse.
///
/// # Cache Key Uniqueness
///
/// `ptr_addr` is a raw pointer address, NOT a hash. Uniqueness comes from the
/// `(ptr_addr, size_lpx_bits, scale_bits)` triple. Two fonts at different sizes
/// from the same byte slice will have distinct handles even if they share the
/// same underlying pointer.
#[derive(Copy, Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct FontHandle {
    /// Raw pointer address of the platform font object.
    pub ptr_addr: u64,
    /// Font size in logical pixels, bit-encoded via `f32::to_bits()`.
    pub size_lpx_bits: u32,
    /// Display scale factor, bit-encoded via `f32::to_bits()`.
    pub scale_bits: u32,
}

impl FontHandle {
    /// Build from the underlying platform font pointer + size + scale.
    ///
    /// # Arguments
    ///
    /// * `ptr` - Platform font object pointer (CTFont*, IDWriteFontFace*, etc.)
    /// * `size_lpx` - Font size in logical pixels (1 lpx = 1 DIP at scale=1.0)
    /// * `scale` - Display scale factor (e.g., 2.0 for Retina)
    pub fn from_ptr_size_scale<T>(ptr: *const T, size_lpx: f32, scale: f32) -> Self {
        Self {
            ptr_addr: ptr as u64,
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
