//! Tests for system font enumeration and deferred font loading.

#[cfg(target_os = "windows")]
use slate_text::DirectWriteBackend;

#[cfg(target_os = "windows")]
use slate_text::TextBackend;

#[cfg(target_os = "windows")]
use slate_text::DeferredFont;

#[cfg(target_os = "windows")]
#[test]
fn enumerate_system_fonts_returns_fonts() {
    let backend = DirectWriteBackend::new().expect("Failed to create DirectWrite backend");
    let fonts = backend
        .enumerate_system_fonts()
        .expect("Failed to enumerate system fonts");

    // Windows always has some system fonts
    assert!(!fonts.is_empty(), "Should find at least one system font");

    // Check that we find common Windows fonts
    let has_arial = fonts.iter().any(|f| f.family.eq_ignore_ascii_case("Arial"));
    let has_segoe = fonts.iter().any(|f| f.family.contains("Segoe"));
    assert!(has_arial || has_segoe, "Should find Arial or Segoe UI");
}

#[cfg(target_os = "windows")]
#[test]
fn enumerated_fonts_have_valid_metadata() {
    let backend = DirectWriteBackend::new().expect("Failed to create DirectWrite backend");
    let fonts = backend
        .enumerate_system_fonts()
        .expect("Failed to enumerate system fonts");

    for font in fonts.iter().take(10) {
        // Family name should not be empty
        assert!(!font.family.is_empty(), "Family name should not be empty");

        // PostScript name should not be empty
        assert!(
            !font.postscript_name.is_empty(),
            "PostScript name should not be empty"
        );

        // Weight should be in valid range (100-900)
        assert!(
            font.weight >= 100 && font.weight <= 900,
            "Weight {} out of range for {}",
            font.weight,
            font.family
        );
    }
}

#[cfg(target_os = "macos")]
use slate_text::CoreTextBackend;

#[cfg(target_os = "macos")]
use slate_text::TextBackend;

#[cfg(target_os = "macos")]
#[test]
fn enumerate_system_fonts_returns_fonts() {
    let backend = CoreTextBackend::new();
    let fonts = backend
        .enumerate_system_fonts()
        .expect("Failed to enumerate system fonts");

    // macOS always has some system fonts
    assert!(!fonts.is_empty(), "Should find at least one system font");

    // Check that we find common macOS fonts
    let has_helvetica = fonts.iter().any(|f| f.family.contains("Helvetica"));
    let has_sf = fonts.iter().any(|f| f.family.contains("SF"));
    assert!(has_helvetica || has_sf, "Should find Helvetica or SF fonts");
}

#[cfg(target_os = "macos")]
#[test]
fn enumerated_fonts_have_valid_metadata() {
    let backend = CoreTextBackend::new();
    let fonts = backend
        .enumerate_system_fonts()
        .expect("Failed to enumerate system fonts");

    for font in fonts.iter().take(10) {
        // Family name should not be empty
        assert!(!font.family.is_empty(), "Family name should not be empty");

        // PostScript name should not be empty
        assert!(
            !font.postscript_name.is_empty(),
            "PostScript name should not be empty"
        );

        // Weight should be in valid range (100-900)
        assert!(
            font.weight >= 100 && font.weight <= 900,
            "Weight {} out of range for {}",
            font.weight,
            font.family
        );
    }
}

// ============================================================================
// Deferred Font Loading Tests
// ============================================================================

#[cfg(target_os = "windows")]
#[test]
fn deferred_font_extracts_charset_from_arial() {
    let backend = DirectWriteBackend::new().expect("Failed to create DirectWrite backend");
    let fonts = backend
        .enumerate_system_fonts()
        .expect("Failed to enumerate system fonts");

    // Find Arial (should always exist on Windows)
    let arial = fonts
        .into_iter()
        .find(|f| f.family.eq_ignore_ascii_case("Arial"))
        .expect("Arial not found");

    // Use DeferredFont::new which resolves font path automatically
    let deferred = DeferredFont::new(arial).expect("Failed to create DeferredFont for Arial");

    // Arial should support ASCII
    assert!(deferred.has_char('A'), "Arial should support 'A'");
    assert!(deferred.has_char('z'), "Arial should support 'z'");
    assert!(deferred.has_char('0'), "Arial should support '0'");
    assert!(deferred.has_char(' '), "Arial should support space");

    // Charset should have a reasonable size
    assert!(
        deferred.charset_size() > 100,
        "Arial should have many glyphs"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn deferred_font_can_read_bytes() {
    let backend = DirectWriteBackend::new().expect("Failed to create DirectWrite backend");
    let fonts = backend
        .enumerate_system_fonts()
        .expect("Failed to enumerate system fonts");

    // Find Arial
    let arial = fonts
        .into_iter()
        .find(|f| f.family.eq_ignore_ascii_case("Arial"))
        .expect("Arial not found");

    // Use DeferredFont::new which resolves font path automatically
    let deferred = DeferredFont::new(arial).expect("Failed to create DeferredFont for Arial");
    let bytes = deferred.read_bytes().expect("Failed to read font bytes");

    // Font file should be non-empty and start with valid signature
    assert!(!bytes.is_empty(), "Font file should not be empty");
    // TrueType: 0x00010000 or 'true' or 'typ1'
    // OpenType: 'OTTO'
    // TTC: 'ttcf'
    let sig = &bytes[0..4];
    let is_valid = sig == [0x00, 0x01, 0x00, 0x00]
        || sig == b"true"
        || sig == b"typ1"
        || sig == b"OTTO"
        || sig == b"ttcf";
    assert!(is_valid, "Invalid font signature: {:?}", sig);
}
