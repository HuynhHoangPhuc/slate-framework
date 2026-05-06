//! Visual regression golden image helpers.
//!
//! Provides SSIM-based image comparison with per-OS golden files and
//! automatic diff generation on mismatch.

use image::{ImageBuffer, Rgba, RgbaImage};
use std::env;
use std::fs;
use std::path::PathBuf;

/// Returns `true` if `UPDATE_GOLDENS` env is set to "1" or "true".
pub fn update_golden_mode() -> bool {
    env::var("UPDATE_GOLDENS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Returns the SSIM threshold (default 0.98, overridable via `GOLDEN_TOLERANCE_OVERRIDE`).
fn golden_tolerance() -> f64 {
    env::var("GOLDEN_TOLERANCE_OVERRIDE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.98)
}

/// Returns "mac" or "win" based on the target OS.
fn os_prefix() -> &'static str {
    if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(target_os = "windows") {
        "win"
    } else {
        panic!("Unsupported OS for golden tests; expected macOS or Windows")
    }
}

/// Resolves the golden file path for the given test name.
pub fn golden_path(name: &str) -> PathBuf {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    PathBuf::from(manifest_dir)
        .join("tests")
        .join("goldens")
        .join(format!("{}-{}.png", os_prefix(), name))
}

/// Resolves the diff output path for the given test name.
fn diff_path(name: &str) -> PathBuf {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    PathBuf::from(manifest_dir)
        .join("..")
        .join("..")
        .join("target")
        .join("vis-regress-diffs")
        .join(format!("{}-{}.diff.png", os_prefix(), name))
}

/// Lift R8 alpha pixels to RGBA grayscale (R=G=B=alpha, A=255).
fn alpha_to_rgba(pixels: &[u8], width: u32, height: u32) -> RgbaImage {
    let mut img = ImageBuffer::new(width, height);
    for (i, &alpha) in pixels.iter().enumerate() {
        let x = (i as u32) % width;
        let y = (i as u32) / width;
        img.put_pixel(x, y, Rgba([alpha, alpha, alpha, 255]));
    }
    img
}

/// Save an R8 alpha buffer as a grayscale PNG.
pub fn save_png_alpha(path: &std::path::Path, pixels: &[u8], width: u32, height: u32) {
    let img = alpha_to_rgba(pixels, width, height);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("Failed to create golden directory");
    }
    img.save(path).expect("Failed to save alpha PNG");
}

/// Save an RGBA buffer as a PNG.
pub fn save_png_rgba(path: &std::path::Path, pixels: &[u8], width: u32, height: u32) {
    let img: RgbaImage = ImageBuffer::from_raw(width, height, pixels.to_vec())
        .expect("Invalid RGBA buffer dimensions");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("Failed to create golden directory");
    }
    img.save(path).expect("Failed to save RGBA PNG");
}

/// Generate a 3-up diff image: expected | actual | per-pixel diff.
fn generate_diff_image(expected: &RgbaImage, actual: &RgbaImage) -> RgbaImage {
    let width = expected.width().max(actual.width());
    let height = expected.height().max(actual.height());

    let mut diff = ImageBuffer::new(width * 3, height);

    for y in 0..height {
        for x in 0..width {
            let exp_pixel = if x < expected.width() && y < expected.height() {
                *expected.get_pixel(x, y)
            } else {
                Rgba([0, 0, 0, 255])
            };
            let act_pixel = if x < actual.width() && y < actual.height() {
                *actual.get_pixel(x, y)
            } else {
                Rgba([0, 0, 0, 255])
            };

            let diff_pixel = Rgba([
                exp_pixel[0].abs_diff(act_pixel[0]),
                exp_pixel[1].abs_diff(act_pixel[1]),
                exp_pixel[2].abs_diff(act_pixel[2]),
                255,
            ]);

            diff.put_pixel(x, y, exp_pixel);
            diff.put_pixel(x + width, y, act_pixel);
            diff.put_pixel(x + width * 2, y, diff_pixel);
        }
    }

    diff
}

/// Assert that an R8 alpha bitmap matches its golden file (SSIM >= threshold).
///
/// In update mode (`UPDATE_GOLDENS=1`), writes the golden and returns.
/// On mismatch, writes a 3-up diff PNG and panics with details.
#[track_caller]
pub fn assert_alpha_matches_golden(pixels: &[u8], width: u32, height: u32, name: &str) {
    let actual = alpha_to_rgba(pixels, width, height);
    assert_image_matches_golden(&actual, name);
}

/// Assert that an RGBA buffer matches its golden file (SSIM >= threshold).
///
/// In update mode (`UPDATE_GOLDENS=1`), writes the golden and returns.
/// On mismatch, writes a 3-up diff PNG and panics with details.
#[track_caller]
pub fn assert_rgba_matches_golden(pixels: &[u8], width: u32, height: u32, name: &str) {
    let actual: RgbaImage = ImageBuffer::from_raw(width, height, pixels.to_vec())
        .expect("Invalid RGBA buffer dimensions");
    assert_image_matches_golden(&actual, name);
}

#[track_caller]
fn assert_image_matches_golden(actual: &RgbaImage, name: &str) {
    let path = golden_path(name);

    if update_golden_mode() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("Failed to create golden directory");
        }
        actual.save(&path).expect("Failed to save golden");
        return;
    }

    if !path.exists() {
        panic!(
            "Golden file missing: {}. Run with UPDATE_GOLDENS=1 to create it.",
            path.display()
        );
    }

    let expected = image::open(&path)
        .expect("Failed to open golden file")
        .to_rgba8();

    let result =
        image_compare::rgba_hybrid_compare(actual, &expected).expect("Failed to compare images");

    let threshold = golden_tolerance();
    if result.score < threshold {
        let diff_out = diff_path(name);
        if let Some(parent) = diff_out.parent() {
            fs::create_dir_all(parent).expect("Failed to create diff directory");
        }
        let diff_img = generate_diff_image(&expected, actual);
        diff_img.save(&diff_out).expect("Failed to save diff PNG");

        panic!(
            "SSIM {:.4} below {:.2} for {} ({}); diff written to {}",
            result.score,
            threshold,
            name,
            os_prefix(),
            diff_out.display()
        );
    }
}
