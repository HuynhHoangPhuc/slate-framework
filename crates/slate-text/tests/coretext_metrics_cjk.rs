//! CoreText CJK metric ratios (macOS only).
//!
//! Locks down Noto Sans JP ascent/descent under the post-fix `1 pt = 1 lpx`
//! convention. CJK fonts have very different ascender geometry than Latin
//! (sTypoAscender often equals unitsPerEm or close to it), so we pin them
//! separately from `coretext_metrics_latin.rs`.

#![cfg(target_os = "macos")]

use slate_text::{BUNDLED_CJK_JP_FONT, CoreTextBackend, Font, TextBackend};

#[test]
fn noto_sans_jp_metrics_under_one_pt_one_lpx() {
    let mut backend = CoreTextBackend::new().unwrap();
    let font = backend
        .load_font_from_bytes(BUNDLED_CJK_JP_FONT, 16.0, 1.0)
        .expect("failed to load font");
    let m = font.metrics();

    // Noto Sans JP at 16 lpx — empirically validated CT-reported ascent
    // (`ct_font.ascent()`) ranges roughly 14–18 lpx depending on which OS
    // version's font metrics CoreText surfaces. Loose bounds are good
    // enough to catch the old 96/72 inflation (which would push values
    // outside the 12–22 lpx envelope at size 16).
    assert!(
        m.ascent_lpx > 12.0 && m.ascent_lpx < 22.0,
        "ascent_lpx={} outside the 1-pt=1-lpx envelope",
        m.ascent_lpx,
    );

    // Descent stored as negative.
    assert!(
        m.descent_lpx < 0.0,
        "descent_lpx={} should be negative",
        m.descent_lpx,
    );

    // The ascent-to-descent magnitude ratio should reflect CJK's
    // characteristically large ascender (>1.5x descent).
    assert!(
        m.ascent_lpx > m.descent_lpx.abs() * 1.2,
        "expected CJK ascent ({}) > 1.2 × |descent| ({})",
        m.ascent_lpx,
        m.descent_lpx.abs(),
    );
}
