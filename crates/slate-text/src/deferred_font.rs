//! Deferred font loading for fallback chains.
//!
//! Implements Ghostty's `DeferredFace` pattern: store font metadata only at startup,
//! upgrade to full font on first glyph request. Reduces startup time and memory.

use crate::error::TextError;
use crate::types::FontDescriptor;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Character set for O(1) codepoint lookup.
///
/// Uses a bitmap for BMP (U+0000-U+FFFF) and a sparse HashSet for SMP (U+10000+).
/// Memory: ~8KB for BMP bitmap + variable for SMP.
pub struct CharacterSet {
    /// Bitmap for Basic Multilingual Plane (64K codepoints = 8KB).
    bmp: [u64; 1024],
    /// Sparse set for Supplementary Multilingual Plane.
    smp: HashSet<u32>,
}

impl CharacterSet {
    /// Create an empty character set.
    pub fn new() -> Self {
        Self {
            bmp: [0; 1024],
            smp: HashSet::new(),
        }
    }

    /// Insert a codepoint into the set.
    pub fn insert(&mut self, cp: u32) {
        if cp < 0x10000 {
            let idx = (cp / 64) as usize;
            let bit = cp % 64;
            self.bmp[idx] |= 1u64 << bit;
        } else {
            self.smp.insert(cp);
        }
    }

    /// Check if a codepoint is in the set.
    #[inline]
    pub fn contains(&self, cp: u32) -> bool {
        if cp < 0x10000 {
            let idx = (cp / 64) as usize;
            let bit = cp % 64;
            (self.bmp[idx] >> bit) & 1 != 0
        } else {
            self.smp.contains(&cp)
        }
    }

    /// Returns the number of codepoints in the set.
    pub fn len(&self) -> usize {
        let bmp_count: usize = self.bmp.iter().map(|w| w.count_ones() as usize).sum();
        bmp_count + self.smp.len()
    }

    /// Returns true if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.bmp.iter().all(|&w| w == 0) && self.smp.is_empty()
    }
}

impl Default for CharacterSet {
    fn default() -> Self {
        Self::new()
    }
}

/// A font that is loaded lazily on first use.
///
/// Stores metadata (descriptor, charset, path) without loading the full font file.
/// Call `load()` to get the actual font when needed for shaping/rasterization.
pub struct DeferredFont {
    /// Font metadata from system enumeration.
    pub descriptor: FontDescriptor,
    /// Codepoints supported by this font.
    charset: CharacterSet,
    /// Resolved path to font file.
    font_path: PathBuf,
}

impl DeferredFont {
    /// Create a deferred font from a descriptor.
    ///
    /// Resolves font path and extracts charset from cmap table.
    /// This is fast (~1ms) as it only parses the cmap table.
    pub fn new(descriptor: FontDescriptor) -> Result<Self, TextError> {
        let font_path = resolve_font_path(&descriptor)?;
        let charset = extract_charset(&font_path)?;

        Ok(Self {
            descriptor,
            charset,
            font_path,
        })
    }

    /// Create from a descriptor with a known path.
    ///
    /// Skips path resolution when descriptor already has a path.
    pub fn from_descriptor_with_path(descriptor: FontDescriptor) -> Result<Self, TextError> {
        let font_path = descriptor
            .path
            .clone()
            .ok_or_else(|| TextError::FontNotFound {
                family: descriptor.family.clone(),
            })?;
        let charset = extract_charset(&font_path)?;

        Ok(Self {
            descriptor,
            charset,
            font_path,
        })
    }

    /// Check if this font supports a codepoint.
    #[inline]
    pub fn has_codepoint(&self, cp: u32) -> bool {
        self.charset.contains(cp)
    }

    /// Check if this font supports a character.
    #[inline]
    pub fn has_char(&self, c: char) -> bool {
        self.charset.contains(c as u32)
    }

    /// Get the path to the font file.
    pub fn path(&self) -> &Path {
        &self.font_path
    }

    /// Read the font file bytes.
    pub fn read_bytes(&self) -> Result<Vec<u8>, TextError> {
        std::fs::read(&self.font_path).map_err(|e| TextError::FontFileLoad(e.to_string()))
    }

    /// Get the number of supported codepoints.
    pub fn charset_size(&self) -> usize {
        self.charset.len()
    }
}

/// Maximum font file size (50 MB) before we refuse eager load.
///
/// Stopgap guard against OOM from pathologically large font files (e.g. massive CJK collections).
/// When mmap support is added this limit can be relaxed.
const MAX_FONT_FILE_BYTES: u64 = 50_000_000;

/// Extract character set from a font file's cmap table.
fn extract_charset(path: &Path) -> Result<CharacterSet, TextError> {
    // Size guard: refuse to read files larger than MAX_FONT_FILE_BYTES.
    let metadata = std::fs::metadata(path)
        .map_err(|e| TextError::FontFileLoad(format!("{}: {}", path.display(), e)))?;
    if metadata.len() > MAX_FONT_FILE_BYTES {
        return Err(TextError::FontFileLoad(
            "font file too large for eager load".into(),
        ));
    }

    let data = std::fs::read(path)
        .map_err(|e| TextError::FontFileLoad(format!("{}: {}", path.display(), e)))?;

    let face = ttf_parser::Face::parse(&data, 0)
        .map_err(|e| TextError::FontFileLoad(format!("parse {}: {}", path.display(), e)))?;

    let mut charset = CharacterSet::new();

    // Iterate all cmap subtables and collect codepoints
    if let Some(cmap) = face.tables().cmap {
        for subtable in cmap.subtables {
            if !subtable.is_unicode() {
                continue;
            }
            subtable.codepoints(|cp| {
                charset.insert(cp);
            });
        }
    }

    Ok(charset)
}

/// Resolve font path from descriptor.
#[cfg(target_os = "windows")]
fn resolve_font_path(desc: &FontDescriptor) -> Result<PathBuf, TextError> {
    // If descriptor has a path, use it
    if let Some(ref path) = desc.path
        && path.exists()
    {
        return Ok(path.clone());
    }

    // Search common Windows font directories
    let font_dirs = [
        std::env::var("WINDIR")
            .map(|w| PathBuf::from(w).join("Fonts"))
            .ok(),
        std::env::var("LOCALAPPDATA")
            .map(|l| PathBuf::from(l).join("Microsoft\\Windows\\Fonts"))
            .ok(),
    ];

    // Try common font file patterns
    let family_clean = desc.family.replace(' ', "");
    let patterns = [
        format!("{}.ttf", family_clean),
        format!("{}.otf", family_clean),
        format!("{}.ttc", family_clean),
        format!("{}Regular.ttf", family_clean),
        format!("{}-Regular.ttf", family_clean),
    ];

    for dir in font_dirs.iter().flatten() {
        for pattern in &patterns {
            let candidate = dir.join(pattern);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    Err(TextError::FontNotFound {
        family: desc.family.clone(),
    })
}

#[cfg(target_os = "macos")]
fn resolve_font_path(desc: &FontDescriptor) -> Result<PathBuf, TextError> {
    // If descriptor has a path, use it
    if let Some(ref path) = desc.path {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    // Search common macOS font directories
    let font_dirs = [
        PathBuf::from("/System/Library/Fonts"),
        PathBuf::from("/Library/Fonts"),
        dirs::home_dir()
            .map(|h| h.join("Library/Fonts"))
            .unwrap_or_default(),
    ];

    // Try common font file patterns
    let family_clean = desc.family.replace(' ', "");
    let patterns = [
        format!("{}.ttf", family_clean),
        format!("{}.otf", family_clean),
        format!("{}.ttc", family_clean),
        format!("{}.dfont", family_clean),
    ];

    for dir in &font_dirs {
        for pattern in &patterns {
            let candidate = dir.join(pattern);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    Err(TextError::FontNotFound {
        family: desc.family.clone(),
    })
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn resolve_font_path(desc: &FontDescriptor) -> Result<PathBuf, TextError> {
    // If descriptor has a path, use it
    if let Some(ref path) = desc.path {
        if path.exists() {
            return Ok(path.clone());
        }
    }
    Err(TextError::FontNotFound {
        family: desc.family.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charset_bmp_insert_and_check() {
        let mut cs = CharacterSet::new();
        cs.insert('A' as u32);
        cs.insert('z' as u32);
        cs.insert(0x4E00); // CJK unified ideograph

        assert!(cs.contains('A' as u32));
        assert!(cs.contains('z' as u32));
        assert!(cs.contains(0x4E00));
        assert!(!cs.contains('B' as u32));
    }

    #[test]
    fn charset_smp_insert_and_check() {
        let mut cs = CharacterSet::new();
        cs.insert(0x1F600); // Grinning face emoji
        cs.insert(0x1F4A9); // Pile of poo emoji

        assert!(cs.contains(0x1F600));
        assert!(cs.contains(0x1F4A9));
        assert!(!cs.contains(0x1F601));
    }

    #[test]
    fn charset_len_counts_correctly() {
        let mut cs = CharacterSet::new();
        assert_eq!(cs.len(), 0);
        assert!(cs.is_empty());

        cs.insert('A' as u32);
        cs.insert('B' as u32);
        cs.insert(0x1F600);

        assert_eq!(cs.len(), 3);
        assert!(!cs.is_empty());
    }
}
