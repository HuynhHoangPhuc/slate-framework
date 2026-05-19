//! Stable face identity for CoreText fonts.
//!
//! Hashes the PostScript name of a `CTFont` to produce a `face_id` that is
//! stable across CoreText pointer churn (notably substitute fonts returned by
//! `CTRunGetAttributes` after IME input-source switch).

use crate::FontHandle;
use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_text::CTFont;
use rustc_hash::FxHasher;
use std::hash::Hasher;

/// Compute a stable 64-bit face identifier from a `CTFont`'s PostScript name.
///
/// PSName is the standardized identifier for a logical face. CoreText caches
/// the PSName on the `CTFont`, so this is one CFDictionary lookup per call.
/// FxHasher is deterministic across builds, has no transitive deps, and is
/// used by rustc/serde.
pub(crate) fn ct_font_face_id(font: &CTFont) -> u64 {
    // CTFontCopyPostScriptName returns a +1 CFString (CFRetained owns it).
    let psname: CFRetained<CFString> = unsafe { font.post_script_name() };
    let s = psname.to_string();
    let mut h = FxHasher::default();
    h.write(s.as_bytes());
    h.finish()
}

/// Build a `FontHandle` whose `face_id` is the PSName hash of `font`.
pub(crate) fn font_handle_from_ct_font(font: &CTFont, size_lpx: f32, scale: f32) -> FontHandle {
    FontHandle::from_face_id(ct_font_face_id(font), size_lpx, scale)
}
