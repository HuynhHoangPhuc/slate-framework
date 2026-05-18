//! Cross-platform shape-width parity probe.
//!
//! Goal: catch any future unit-of-measure drift between the macOS CoreText
//! backend and the Windows DirectWrite backend. DejaVu Sans is identical
//! across both platforms (we bundle the same TTF), and "Hello, World" at
//! 16 lpx should produce equivalent shaped widths within 1 lpx after the
//! `1 pt = 1 lpx` macOS fix.
//!
//! Pinning a single numeric expectation that both backends must hit means a
//! regression on either platform fails the suite — exactly what we want for
//! the unit-convention contract.

use slate_text::{Font, TEST_FONT, TextBackend};

#[cfg(target_os = "macos")]
use slate_text::CoreTextBackend as BackendImpl;

#[cfg(target_os = "windows")]
use slate_text::DirectWriteBackend as BackendImpl;

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn hello_world_width_matches_cross_platform_pin() {
    let mut backend = BackendImpl::new().unwrap();
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 1.0)
        .expect("failed to load font");

    // Sanity: font reports >0 ascent (catches a stale-metric regression).
    assert!(font.metrics().ascent_lpx > 0.0);

    let line = backend
        .shape_line(&font, "Hello, World")
        .expect("failed to shape");

    // Pinned expected width (lpx). Established on macOS after the
    // `1 pt = 1 lpx` migration; Windows backend should agree within 1 lpx
    // since DejaVu Sans tables are identical and DirectWrite already used
    // the same convention. If Windows reports a materially different value,
    // it's a parity bug worth investigating — do NOT silently retune.
    const EXPECTED_LPX: f32 = 96.58;
    let delta = (line.width_lpx - EXPECTED_LPX).abs();
    assert!(
        delta < 1.0,
        "width_lpx={} drifted {} lpx from pinned cross-platform value ({})",
        line.width_lpx,
        delta,
        EXPECTED_LPX,
    );
}
