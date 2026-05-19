//! Compile-time tests for the public trait surface.
//!
//! Ensures types that should be Send+Sync are, and the API is stable.

use slate_text::{FontHandle, FontMetrics, GlyphBitmap, GlyphMetrics, ShapedGlyph, ShapedLine};

#[test]
fn send_sync_types() {
    fn assert_send_sync<T: Send + Sync>() {}

    // Handle types should be Send+Sync for cross-thread cache sharing
    assert_send_sync::<FontHandle>();

    // Value types should be Send+Sync
    assert_send_sync::<FontMetrics>();
    assert_send_sync::<GlyphMetrics>();
    assert_send_sync::<ShapedGlyph>();

    // Container types should be Send+Sync
    assert_send_sync::<GlyphBitmap>();
    assert_send_sync::<ShapedLine>();
}

#[test]
fn font_handle_hash_eq() {
    use std::collections::HashSet;

    let h1 = FontHandle::from_face_id(0x1000, 16.0, 1.0);
    let h2 = FontHandle::from_face_id(0x1000, 16.0, 1.0);
    let h3 = FontHandle::from_face_id(0x1000, 24.0, 1.0);

    assert_eq!(h1, h2);
    assert_ne!(h1, h3);

    let mut set = HashSet::new();
    set.insert(h1);
    assert!(set.contains(&h2));
    assert!(!set.contains(&h3));
}

#[test]
fn font_handle_size_scale_accessors() {
    let h = FontHandle::from_face_id(0x1000, 16.5, 2.0);
    assert_eq!(h.size_lpx(), 16.5);
    assert_eq!(h.scale(), 2.0);
}
