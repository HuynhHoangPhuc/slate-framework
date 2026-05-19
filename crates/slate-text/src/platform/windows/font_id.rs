//! Stable face identity for DirectWrite fonts.
//!
//! Hashes the PostScript name of an `IDWriteFontFace` to produce a `face_id`
//! that is stable across COM pointer churn. Mirrors `platform/macos/font_id.rs`.

use rustc_hash::FxHasher;
use std::hash::Hasher;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_INFORMATIONAL_STRING_POSTSCRIPT_NAME, IDWriteFontFace, IDWriteFontFace3,
    IDWriteLocalizedStrings,
};
use windows::core::{BOOL, Interface};

/// Compute a stable 64-bit face identifier from a DirectWrite font face's
/// PostScript name. Falls back to the raw COM pointer if the PSName cannot be
/// retrieved (rare for system fonts; bundled fonts always expose PSNAME).
pub(crate) fn idwrite_font_face_id(face: &IDWriteFontFace) -> u64 {
    let psname_hash = unsafe { try_psname_hash(face) };
    psname_hash.unwrap_or_else(|| face.as_raw() as u64)
}

unsafe fn try_psname_hash(face: &IDWriteFontFace) -> Option<u64> {
    let face3: IDWriteFontFace3 = face.cast().ok()?;
    let mut strings: Option<IDWriteLocalizedStrings> = None;
    let mut exists: BOOL = false.into();
    unsafe {
        face3
            .GetInformationalStrings(
                DWRITE_INFORMATIONAL_STRING_POSTSCRIPT_NAME,
                &mut strings,
                &mut exists,
            )
            .ok()?;
    }
    if !exists.as_bool() {
        return None;
    }
    let strings = strings?;
    let count = unsafe { strings.GetCount() };
    if count == 0 {
        return None;
    }
    let len = unsafe { strings.GetStringLength(0).ok()? };
    let mut buf = vec![0u16; (len as usize) + 1];
    unsafe { strings.GetString(0, &mut buf).ok()? };
    buf.truncate(len as usize);
    let s = String::from_utf16(&buf).ok()?;
    let mut h = FxHasher::default();
    h.write(s.as_bytes());
    Some(h.finish())
}
