//! Integration test for Image element rendering via HeadlessApp.
//!
//! Uses a distinctive 4x4 pixel pattern (quadrants: red, green, blue, white)
//! and verifies the rendered output via pixel readback.

use slate_framework::{Div, HeadlessApp, Image, IntoAny};

/// Generate a 4x4 RGBA8 test pattern with distinct quadrant colors:
/// - Top-left: Red (255, 0, 0, 255)
/// - Top-right: Green (0, 255, 0, 255)
/// - Bottom-left: Blue (0, 0, 255, 255)
/// - Bottom-right: White (255, 255, 255, 255)
fn generate_quadrant_pattern() -> Vec<u8> {
    let mut pixels = Vec::with_capacity(4 * 4 * 4);

    for y in 0..4 {
        for x in 0..4 {
            let color = match (x < 2, y < 2) {
                (true, true) => [255, 0, 0, 255],       // top-left: red
                (false, true) => [0, 255, 0, 255],      // top-right: green
                (true, false) => [0, 0, 255, 255],      // bottom-left: blue
                (false, false) => [255, 255, 255, 255], // bottom-right: white
            };
            pixels.extend_from_slice(&color);
        }
    }

    pixels
}

/// Sample a pixel from an RgbaImage at (x, y).
fn sample_pixel(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 4] {
    let p = img.get_pixel(x, y);
    [p[0], p[1], p[2], p[3]]
}

/// Check if two colors are within sRGB tolerance (±8 per channel).
/// Tolerance accounts for atlas filtering + sRGB roundtrip.
fn colors_match(a: [u8; 4], b: [u8; 4], tolerance: u8) -> bool {
    a.iter()
        .zip(b.iter())
        .all(|(av, bv)| (*av as i16 - *bv as i16).unsigned_abs() <= tolerance as u16)
}

#[test]
fn image_element_renders_to_headless() {
    let _ = env_logger::builder().is_test(true).try_init();

    // Create a 64x64 headless app (small but enough for a 4x4 image)
    let mut app = HeadlessApp::new(64, 64).expect("HeadlessApp creation failed");

    let pattern = generate_quadrant_pattern();
    let img = Image::new(4, 4, pattern);

    // Wrap image in a Div to center it
    let ui = Div::new().child(img).into_any();

    let output = app.render(ui).expect("render failed");

    // Verify the image was rendered by checking the top-left area.
    // The image starts at (0, 0) by default layout (no centering style applied).
    // We sample at (0, 0) which should be red, (2, 0) which should be green,
    // (0, 2) which should be blue, (2, 2) which should be white.

    // Note: sRGB tolerance of 8 accounts for GPU filtering and roundtrip.
    let tolerance = 8;

    let top_left = sample_pixel(&output, 0, 0);
    let top_right = sample_pixel(&output, 2, 0);
    let bottom_left = sample_pixel(&output, 0, 2);
    let bottom_right = sample_pixel(&output, 2, 2);

    assert!(
        colors_match(top_left, [255, 0, 0, 255], tolerance),
        "top-left expected red, got {:?}",
        top_left
    );
    assert!(
        colors_match(top_right, [0, 255, 0, 255], tolerance),
        "top-right expected green, got {:?}",
        top_right
    );
    assert!(
        colors_match(bottom_left, [0, 0, 255, 255], tolerance),
        "bottom-left expected blue, got {:?}",
        bottom_left
    );
    assert!(
        colors_match(bottom_right, [255, 255, 255, 255], tolerance),
        "bottom-right expected white, got {:?}",
        bottom_right
    );
}

#[test]
fn image_element_with_style_override() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut app = HeadlessApp::new(64, 64).expect("HeadlessApp creation failed");

    // Create a 2x2 solid red image
    let pixels = vec![
        255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
    ];
    let img = Image::new(2, 2, pixels);

    let ui = Div::new().child(img).into_any();
    let output = app.render(ui).expect("render failed");

    // Image renders at its intrinsic 2x2 size
    let p = sample_pixel(&output, 0, 0);
    assert!(
        colors_match(p, [255, 0, 0, 255], 8),
        "expected red at origin, got {:?}",
        p
    );
}

#[test]
fn empty_image_does_not_crash() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut app = HeadlessApp::new(32, 32).expect("HeadlessApp creation failed");

    // Empty image (default)
    let img = Image::default();
    let ui = Div::new().child(img).into_any();

    // Should not panic
    let output = app.render(ui);
    assert!(output.is_ok(), "empty image render should not fail");
}

#[test]
fn oversized_image_degrades_gracefully() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut app = HeadlessApp::new(32, 32).expect("HeadlessApp creation failed");

    // Oversized image (exceeds MAX_IMAGE_DIM) — should degrade to empty
    let pixels = vec![255u8; 3000 * 100 * 4];
    let img = Image::new(3000, 100, pixels);

    assert_eq!(img.width(), 0, "oversized image should have width 0");
    assert_eq!(img.height(), 0, "oversized image should have height 0");

    let ui = Div::new().child(img).into_any();
    let output = app.render(ui);
    assert!(output.is_ok(), "oversized image render should not fail");
}
