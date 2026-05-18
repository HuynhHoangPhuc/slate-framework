//! CoreText Latin metric ratios (macOS only).
//!
//! Pins the post-fix `1 pt = 1 lpx` convention: with the old 96/72 inflation,
//! a 16-lpx DejaVu Sans cap-height would land at ~8.6 lpx (75% of the
//! correct value). After the fix it should be ~11.5 lpx.

#![cfg(target_os = "macos")]

use slate_text::{CoreTextBackend, Font, TEST_FONT, TextBackend};

#[test]
fn dejavu_sans_metrics_match_lpx_convention() {
    let mut backend = CoreTextBackend::new().unwrap();
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 1.0)
        .expect("failed to load font");
    let m = font.metrics();

    // DejaVu Sans cap-height ratio ≈ 0.71875 → ~11.5 lpx at size 16.
    // Pre-fix value was ~8.6 lpx; assertion fails the old convention.
    assert!(
        m.cap_height_lpx > 11.0 && m.cap_height_lpx < 13.0,
        "cap_height_lpx={} should be ~11.5 (was ~8.6 under old 96/72 convention)",
        m.cap_height_lpx,
    );

    // DejaVu Sans x-height ratio ≈ 0.5 → ~8 lpx.
    assert!(
        m.x_height_lpx > 7.0 && m.x_height_lpx < 9.0,
        "x_height_lpx={} should be ~8.0",
        m.x_height_lpx,
    );

    // Ascent ratio ≈ 0.928 → ~14.8 lpx; allow ~14-16.5.
    assert!(
        m.ascent_lpx > 14.0 && m.ascent_lpx < 16.5,
        "ascent_lpx={} should be ~14.8",
        m.ascent_lpx,
    );

    // Descent stored as negative.
    assert!(m.descent_lpx < 0.0, "descent_lpx should be negative");
}
