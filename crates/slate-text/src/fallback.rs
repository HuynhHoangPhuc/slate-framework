//! Font fallback chains for mixed-font text rendering.
//!
//! When the primary font lacks a glyph (glyph_id=0), the fallback system
//! finds an alternative font that supports the codepoint and re-shapes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;

use crate::deferred_font::DeferredFont;
use crate::error::TextError;
use crate::types::{FontDescriptor, FontId, FontStyle, ShapedGlyph, ShapedLine};

/// System for managing font fallback chains.
///
/// Holds a primary font and an ordered list of fallback fonts. When shaping
/// text, missing glyphs (glyph_id=0) are re-shaped with fallback fonts.
///
/// This type is `!Sync` because `resolution_cache` uses `RefCell` (interior
/// mutability without synchronization). Use from a single thread only.
pub struct FontFallbackSystem {
    /// Primary font (always FontId(0)).
    primary: DeferredFont,
    /// Fallback fonts in priority order.
    fallbacks: Vec<DeferredFont>,
    /// Maps PostScript name to FontId for quick lookup.
    font_ids: HashMap<String, FontId>,
    /// Caches fallback resolutions: (primary_font_id, codepoint) -> fallback_font_id.
    resolution_cache: RefCell<HashMap<(FontId, u32), FontId>>,
    /// Opt-out of `Sync`; `RefCell` already makes this `!Sync`, but this
    /// documents the intent explicitly and is robust against future refactors.
    _not_sync: PhantomData<*const ()>,
}

impl FontFallbackSystem {
    /// Create a fallback system from a primary font and system fonts.
    ///
    /// # Arguments
    ///
    /// * `primary` - Primary font descriptor
    /// * `system_fonts` - All available system fonts (from enumerate_system_fonts)
    /// * `fallback_families` - Ordered list of fallback family names to try
    pub fn new(
        primary: FontDescriptor,
        system_fonts: &[FontDescriptor],
        fallback_families: &[&str],
    ) -> Result<Self, TextError> {
        let primary_font = DeferredFont::new(primary)?;
        let mut font_ids = HashMap::new();
        font_ids.insert(
            primary_font.descriptor.postscript_name.clone(),
            FontId::PRIMARY,
        );

        let mut fallbacks = Vec::new();
        let mut next_id = 1u32;

        for family in fallback_families {
            // Find best match for family (prefer regular weight)
            if let Some(desc) = system_fonts.iter().find(|f| {
                f.family.eq_ignore_ascii_case(family)
                    && f.style == FontStyle::Regular
                    && f.weight >= 300
                    && f.weight <= 500
            }) {
                match DeferredFont::new(desc.clone()) {
                    Ok(df) => {
                        font_ids.insert(df.descriptor.postscript_name.clone(), FontId(next_id));
                        fallbacks.push(df);
                        next_id += 1;
                    }
                    Err(_) => continue, // Skip fonts that fail to load
                }
            }
        }

        Ok(Self {
            primary: primary_font,
            fallbacks,
            font_ids,
            resolution_cache: RefCell::new(HashMap::new()),
            _not_sync: PhantomData,
        })
    }

    /// Create with default fallback chain.
    pub fn with_default_fallbacks(
        primary: FontDescriptor,
        system_fonts: &[FontDescriptor],
    ) -> Result<Self, TextError> {
        #[cfg(target_os = "windows")]
        let default_fallbacks = ["Segoe UI", "Arial", "Tahoma", "Verdana", "DejaVu Sans"];

        #[cfg(target_os = "macos")]
        let default_fallbacks = ["Helvetica Neue", "Helvetica", "Arial", "DejaVu Sans"];

        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let default_fallbacks = ["DejaVu Sans", "Liberation Sans", "Noto Sans"];

        Self::new(primary, system_fonts, &default_fallbacks)
    }

    /// Get the primary font.
    pub fn primary(&self) -> &DeferredFont {
        &self.primary
    }

    /// Get a fallback font by index.
    pub fn fallback(&self, index: usize) -> Option<&DeferredFont> {
        self.fallbacks.get(index)
    }

    /// Find a font that supports the given codepoint.
    ///
    /// Returns the primary font if it supports the codepoint, otherwise
    /// searches fallbacks in order. Returns None if no font supports it.
    pub fn find_font_for_codepoint(&self, cp: u32) -> Option<&DeferredFont> {
        if self.primary.has_codepoint(cp) {
            return Some(&self.primary);
        }
        self.fallbacks.iter().find(|f| f.has_codepoint(cp))
    }

    /// Get the FontId for a deferred font.
    pub fn get_font_id(&self, font: &DeferredFont) -> FontId {
        self.font_ids
            .get(&font.descriptor.postscript_name)
            .copied()
            .unwrap_or(FontId::PRIMARY)
    }

    /// Check the resolution cache for a previously resolved fallback.
    pub fn get_cached_fallback(&self, primary_id: FontId, cp: u32) -> Option<FontId> {
        self.resolution_cache
            .borrow()
            .get(&(primary_id, cp))
            .copied()
    }

    /// Cache a fallback resolution.
    pub fn cache_fallback(&self, primary_id: FontId, cp: u32, fallback_id: FontId) {
        self.resolution_cache
            .borrow_mut()
            .insert((primary_id, cp), fallback_id);
    }

    /// Get the number of fallback fonts.
    pub fn fallback_count(&self) -> usize {
        self.fallbacks.len()
    }
}

/// Fix missing glyphs in a shaped line using font fallback.
///
/// Scans for glyphs with glyph_id=0 (missing) and attempts to re-shape them
/// using fallback fonts. Modified glyphs get their font_id updated.
///
/// # Arguments
///
/// * `line` - ShapedLine to fix (modified in place)
/// * `text` - Original text (needed for re-shaping)
/// * `system` - Font fallback system
/// * `shape_fn` - Function to shape a single character with a fallback font
///
/// # Returns
///
/// Number of glyphs that were successfully fixed.
///
/// # Limitations
///
/// Assumes 1:1 glyph-to-char mapping (Phase 2a LTR ASCII scope). Complex scripts
/// with ligatures (Arabic, Devanagari) produce glyph count != char count and will
/// not be handled correctly. BiDi and complex script support is Phase 2c.
pub fn fix_missing_glyphs<F>(
    line: &mut ShapedLine,
    text: &str,
    system: &FontFallbackSystem,
    mut shape_fn: F,
) -> usize
where
    F: FnMut(&DeferredFont, char) -> Option<ShapedGlyph>,
{
    let chars: Vec<char> = text.chars().collect();
    debug_assert_eq!(
        chars.len(),
        line.glyphs.len(),
        "fix_missing_glyphs requires 1:1 glyph-to-char mapping (LTR ASCII only)"
    );
    let mut fixed = 0;

    for (i, glyph) in line.glyphs.iter_mut().enumerate() {
        if glyph.glyph_id != 0 {
            continue; // Glyph exists, skip
        }

        let Some(&c) = chars.get(i) else { continue };
        let cp = c as u32;

        // Check cache first
        if let Some(fb_id) = system.get_cached_fallback(FontId::PRIMARY, cp) {
            // Find the font with this ID and re-shape
            if fb_id == FontId::PRIMARY {
                continue; // Primary already failed
            }
            if let Some(fallback) = system
                .fallbacks
                .iter()
                .find(|f| system.get_font_id(f) == fb_id)
                && let Some(new_glyph) = shape_fn(fallback, c)
            {
                *glyph = ShapedGlyph {
                    font_id: fb_id,
                    ..new_glyph
                };
                fixed += 1;
            }
            continue;
        }

        // Find fallback font that supports this codepoint
        if let Some(fallback) = system.find_font_for_codepoint(cp) {
            let fb_id = system.get_font_id(fallback);
            if fb_id == FontId::PRIMARY {
                // Primary font should have worked, cache and skip
                system.cache_fallback(FontId::PRIMARY, cp, FontId::PRIMARY);
                continue;
            }

            if let Some(new_glyph) = shape_fn(fallback, c) {
                *glyph = ShapedGlyph {
                    font_id: fb_id,
                    ..new_glyph
                };
                system.cache_fallback(FontId::PRIMARY, cp, fb_id);
                fixed += 1;
            }
        }
    }

    fixed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_id_primary_is_zero() {
        assert_eq!(FontId::PRIMARY.0, 0);
    }

    #[test]
    fn fallback_system_has_primary() {
        // This test requires system fonts, skip if unavailable
        // In real tests, we'd mock the system fonts
    }
}
