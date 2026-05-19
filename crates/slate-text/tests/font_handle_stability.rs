//! Regression tests for `FontHandle` identity stability.
//!
//! Pins the invariant that two distinct platform face objects representing the
//! same logical face produce equal `FontHandle`s. This is the contract the
//! macOS shaping path relies on after `260519-0701-font-handle-stable-identity`.

use slate_text::FontHandle;

/// Platform-independent invariant: equal `face_id`s built via the
/// public constructor must yield equal `FontHandle`s; differing `face_id`s
/// must yield unequal handles. Mirrors the macOS substitute-font path where
/// two `CTFont` instances with the same PSName produce identical PSName
/// hashes regardless of pointer address.
#[test]
fn face_id_equality_independent_of_construction_path() {
    let face_id = 0xDEAD_BEEF_CAFE_F00D_u64;
    let h_from_call_a = FontHandle::from_face_id(face_id, 16.0, 2.0);
    let h_from_call_b = FontHandle::from_face_id(face_id, 16.0, 2.0);
    assert_eq!(
        h_from_call_a, h_from_call_b,
        "equal face_id must produce equal FontHandle"
    );

    let h_different_id = FontHandle::from_face_id(0x1234_5678_9ABC_DEF0, 16.0, 2.0);
    assert_ne!(
        h_from_call_a, h_different_id,
        "different face_id must produce unequal FontHandle"
    );

    let h_different_size = FontHandle::from_face_id(face_id, 24.0, 2.0);
    assert_ne!(
        h_from_call_a, h_different_size,
        "same face_id but different size_lpx must produce unequal FontHandle"
    );

    let h_different_scale = FontHandle::from_face_id(face_id, 16.0, 1.0);
    assert_ne!(
        h_from_call_a, h_different_scale,
        "same face_id but different scale must produce unequal FontHandle"
    );
}

/// macOS shaping invariant: repeated `shape_line` calls on the same CJK input
/// must produce byte-equal `font_handle` values at each glyph position. Pins
/// the IME-switch substitute-font regression — substitute `CTFont`s with
/// fresh pointer addresses but identical PSName must hash to the same
/// `face_id` and thus the same `FontHandle`.
#[cfg(target_os = "macos")]
#[test]
fn macos_repeated_shape_produces_stable_handles() {
    use slate_text::{BUNDLED_CJK_JP_FONT, CoreTextBackend, TextBackend};

    let mut backend = CoreTextBackend::new().expect("CoreText backend init");
    let font = backend
        .load_font_from_bytes(BUNDLED_CJK_JP_FONT, 16.0, 2.0)
        .expect("load bundled JP font");

    let text = "こんにちは世界";
    let line_a = backend.shape_line(&font, text).expect("shape A");
    let line_b = backend.shape_line(&font, text).expect("shape B");

    assert_eq!(
        line_a.glyphs.len(),
        line_b.glyphs.len(),
        "glyph count must be stable across shape calls"
    );

    for (i, (a, b)) in line_a.glyphs.iter().zip(line_b.glyphs.iter()).enumerate() {
        assert_eq!(
            a.font_handle, b.font_handle,
            "glyph {i}: font_handle must be byte-equal across shape calls"
        );
    }
}
