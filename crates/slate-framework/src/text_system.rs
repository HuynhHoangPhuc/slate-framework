//! Platform-dispatching text system wrapper.
//!
//! `TextSystem` hides the platform-specific `TextBackend` behind `#[cfg]`
//! so Element, Context, and tree types remain non-generic.

use std::marker::PhantomData;

use slate_renderer::atlas::Atlas;
use slate_renderer::scene::GlyphInstance;
use slate_text::backend::Font;
use slate_text::run_builder::TextRunBuilder;
use slate_text::types::ShapedLine;
use slate_text::{GlyphCache, TextError};

#[cfg(target_os = "macos")]
use slate_text::CoreTextBackend;
#[cfg(target_os = "windows")]
use slate_text::DirectWriteBackend;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
compile_error!(
    "slate-framework currently supports macOS and Windows only. Linux support is planned for a future release."
);

/// Concrete text system wrapping the platform backend.
///
/// This struct hides the platform-specific `TextBackend` behind `#[cfg]`,
/// keeping all Element and Context types non-generic.
///
/// # Thread Safety
///
/// `TextSystem` is explicitly `!Send` via `PhantomData<*const ()>`.
/// DirectWrite is apartment-threaded; we keep parity across platforms.
///
/// # Multi-window note
///
/// The glyph cache used to live on `TextSystem` but does NOT anymore: its
/// values are `AllocId`s scoped to a specific `Atlas`, and each window owns
/// its own renderer/atlas pair. A process-shared cache would hand window B
/// an `AllocId` allocated in window A's atlas, producing token-collision
/// glyph corruption on cross-window paints. Each `WindowState` now owns
/// its own `GlyphCache` and passes it through `PaintCtx`.
pub struct TextSystem {
    #[cfg(target_os = "macos")]
    backend: CoreTextBackend,
    #[cfg(target_os = "windows")]
    backend: DirectWriteBackend,

    // Red-team Finding 5: Explicit !Send marker.
    // CoreText is thread-safe and windows-rs IDWriteFactory is Send,
    // so we cannot rely on inherited !Send. Pin the type to one thread
    // to keep async/spawn contracts honest.
    _not_send: PhantomData<*const ()>,
}

// !Send is enforced by PhantomData<*const ()> in the struct.
// Compile-fail test for verification: tests/compile-fail/text_system_send.rs

impl TextSystem {
    /// Create a new TextSystem for the current platform.
    pub fn new() -> Result<Self, TextError> {
        Ok(Self {
            #[cfg(target_os = "macos")]
            backend: CoreTextBackend::new()?,
            #[cfg(target_os = "windows")]
            backend: DirectWriteBackend::new()?,
            _not_send: PhantomData,
        })
    }

    /// Load a font by family name from system fonts.
    ///
    /// # Arguments
    ///
    /// * `family` - Font family name (e.g., "Helvetica", "Segoe UI")
    /// * `size_lpx` - Font size in logical pixels
    /// * `scale` - Display scale factor (e.g., 2.0 for Retina)
    pub fn load_font(
        &mut self,
        family: &str,
        size_lpx: f32,
        scale: f32,
    ) -> Result<PlatformFont, TextError> {
        use slate_text::TextBackend;
        let inner = self.backend.load_font(family, size_lpx, scale)?;
        Ok(PlatformFont {
            inner,
            _not_send: PhantomData,
        })
    }

    /// Load a font from raw TTF/OTF bytes.
    ///
    /// This is the primary path for bundled fonts.
    pub fn load_font_from_bytes(
        &mut self,
        bytes: &'static [u8],
        size_lpx: f32,
        scale: f32,
    ) -> Result<PlatformFont, TextError> {
        use slate_text::TextBackend;
        let inner = self.backend.load_font_from_bytes(bytes, size_lpx, scale)?;
        Ok(PlatformFont {
            inner,
            _not_send: PhantomData,
        })
    }

    /// Shape a line of text into positioned glyphs.
    pub fn shape_line(&self, font: &PlatformFont, text: &str) -> Result<ShapedLine, TextError> {
        use slate_text::TextBackend;
        self.backend.shape_line(&font.inner, text)
    }

    /// Shape a single line through the bidi segment + reorder pipeline,
    /// producing a run-bearing [`ShapedLine`].
    ///
    /// Use this for editable single-line text (`TextField`) so the line carries
    /// UAX #9 level-runs and the run-aware caret / hit-test math works on mixed
    /// and RTL input. Pure-LTR / CJK output is byte-identical to [`Self::shape_line`]
    /// (empty `runs` → original LTR pen walk); the trade-off is the loss of
    /// whole-line cross-word kerning, consistent with the rest of the stack.
    pub fn shape_line_bidi(
        &self,
        font: &PlatformFont,
        text: &str,
    ) -> Result<ShapedLine, TextError> {
        slate_text::shape_line_bidi(&self.backend, &font.inner, text)
    }

    /// Shape each whitespace-delimited word once for cached wrapping.
    ///
    /// Returns the pre-shaped words plus the inter-word space advance. Fit them
    /// to any width with [`slate_text::wrap_shaped_words`] — re-fitting on a
    /// resize then costs zero shaping calls.
    pub fn shape_words(
        &self,
        font: &PlatformFont,
        text: &str,
    ) -> Result<(Vec<slate_text::ShapedWord>, f32), TextError> {
        slate_text::shape_words(&self.backend, &font.inner, text)
    }

    /// Shape `text` into a byte-aware multi-line document (split on hard `\n`,
    /// each word shaped once). Fit to any width with [`slate_text::wrap_document`]
    /// at zero further shaping cost — the multi-line analogue of [`Self::shape_words`].
    pub fn shape_document(
        &self,
        font: &PlatformFont,
        text: &str,
    ) -> Result<slate_text::ShapedDocument, TextError> {
        slate_text::shape_document(&self.backend, &font.inner, text)
    }

    /// Measure text dimensions without rasterizing.
    ///
    /// Returns (width, height) in logical pixels.
    pub fn measure_text(&self, font: &PlatformFont, text: &str) -> Result<(f32, f32), TextError> {
        let shaped = self.shape_line(font, text)?;
        Ok((shaped.width_lpx, shaped.ascent_lpx - shaped.descent_lpx))
    }

    /// Rasterize a shaped text run to GPU glyph instances.
    ///
    /// `glyph_cache` and `atlas` MUST belong to the same window: the cache stores
    /// `AllocId`s scoped to `atlas`. Mixing a cache from window A with window B's
    /// atlas yields token-collision corruption.
    ///
    /// # Arguments
    ///
    /// * `font` - Font used for rasterization
    /// * `shaped` - Pre-shaped line of text
    /// * `baseline_lpx` - Baseline position `[x, y]` in logical pixels
    /// * `color` - Text color as premultiplied RGBA
    /// * `glyph_cache` - Per-window glyph cache paired with `atlas`
    /// * `atlas` - Per-window GPU atlas for glyph storage
    /// * `queue` - GPU queue for upload commands
    #[allow(clippy::too_many_arguments)]
    pub fn rasterize_text_run(
        &self,
        font: &PlatformFont,
        shaped: &ShapedLine,
        baseline_lpx: [f32; 2],
        color: [f32; 4],
        glyph_cache: &mut GlyphCache,
        atlas: &mut Atlas,
        queue: &wgpu::Queue,
    ) -> Result<Vec<GlyphInstance>, TextError> {
        let builder = TextRunBuilder {
            backend: &self.backend,
            font: &font.inner,
            baseline_lpx,
            color,
        };
        builder.build(shaped, glyph_cache, atlas, queue)
    }
}

/// Platform-erased font handle.
///
/// Wraps the platform-specific font type behind `#[cfg]`.
pub struct PlatformFont {
    #[cfg(target_os = "macos")]
    pub(crate) inner: <CoreTextBackend as slate_text::TextBackend>::Font,
    #[cfg(target_os = "windows")]
    pub(crate) inner: <DirectWriteBackend as slate_text::TextBackend>::Font,

    _not_send: PhantomData<*const ()>,
}

impl PlatformFont {
    /// Get font metrics (ascent, descent, etc.).
    pub fn metrics(&self) -> slate_text::types::FontMetrics {
        self.inner.metrics()
    }

    /// Get the font size in logical pixels.
    pub fn size_lpx(&self) -> f32 {
        self.inner.size_lpx()
    }

    /// Get the display scale factor.
    pub fn scale(&self) -> f32 {
        self.inner.scale()
    }
}

// Note: !Send verification via compile-fail test; see tests/compile-fail/text_system_send.rs.
// PhantomData<*const ()> marker enforces !Send at compile time.

// GlyphCache CPU-state invalidation on device-lost is handled inline by the
// recovery path against the affected window's per-window `GlyphCache`, so no
// `RendererObserver` is required from the text-system side. See
// `crate::app_state::render::recovery` for the call site.
