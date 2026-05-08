//! PNG diff binary for CI visual regression testing.
//! Usage: diff <actual.png> <expected.png>
//! Exits 0 if images match within tolerance, 1 otherwise.

use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};
use std::env;
use std::path::Path;
use std::process;

const TOLERANCE: u8 = 4;
const THRESHOLD_PERCENT: f64 = 0.1;

struct DiffStats {
    differing_pixels: u64,
    total_pixels: u64,
    max_channel_diff: u8,
}

fn pixel_diff(actual: &DynamicImage, expected: &DynamicImage, tolerance: u8) -> Result<DiffStats, String> {
    if actual.dimensions() != expected.dimensions() {
        return Err(format!(
            "dimension mismatch: actual={:?} expected={:?}",
            actual.dimensions(),
            expected.dimensions()
        ));
    }

    let (width, height) = actual.dimensions();
    let actual_rgba = actual.to_rgba8();
    let expected_rgba = expected.to_rgba8();

    let mut differing_pixels = 0u64;
    let mut max_channel_diff = 0u8;

    for y in 0..height {
        for x in 0..width {
            let a = actual_rgba.get_pixel(x, y);
            let e = expected_rgba.get_pixel(x, y);

            let mut pixel_differs = false;
            for c in 0..4 {
                let d = (a[c] as i16 - e[c] as i16).unsigned_abs() as u8;
                if d > tolerance {
                    pixel_differs = true;
                }
                max_channel_diff = max_channel_diff.max(d);
            }
            if pixel_differs {
                differing_pixels += 1;
            }
        }
    }

    Ok(DiffStats {
        differing_pixels,
        total_pixels: (width as u64) * (height as u64),
        max_channel_diff,
    })
}

fn generate_diff_image(actual: &DynamicImage, expected: &DynamicImage, tolerance: u8) -> RgbaImage {
    let (width, height) = actual.dimensions();
    let actual_rgba = actual.to_rgba8();
    let expected_rgba = expected.to_rgba8();

    let mut diff_img = RgbaImage::new(width, height);

    for y in 0..height {
        for x in 0..width {
            let a = actual_rgba.get_pixel(x, y);
            let e = expected_rgba.get_pixel(x, y);

            let mut max_diff = 0u8;
            for c in 0..4 {
                let d = (a[c] as i16 - e[c] as i16).unsigned_abs() as u8;
                max_diff = max_diff.max(d);
            }

            if max_diff > tolerance {
                diff_img.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            } else {
                let gray = ((a[0] as u16 + a[1] as u16 + a[2] as u16) / 3) as u8;
                diff_img.put_pixel(x, y, Rgba([gray, gray, gray, 128]));
            }
        }
    }

    diff_img
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() != 3 {
        eprintln!("Usage: {} <actual.png> <expected.png>", args[0]);
        eprintln!("Compares two PNG images and exits 0 if they match within tolerance.");
        process::exit(2);
    }

    let actual_path = &args[1];
    let expected_path = &args[2];

    let actual = match image::open(actual_path) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("Failed to open actual image '{}': {}", actual_path, e);
            process::exit(2);
        }
    };

    let expected = match image::open(expected_path) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("Failed to open expected image '{}': {}", expected_path, e);
            process::exit(2);
        }
    };

    let stats = match pixel_diff(&actual, &expected, TOLERANCE) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Diff failed: {}", e);
            process::exit(1);
        }
    };

    let diff_percent = (stats.differing_pixels as f64 / stats.total_pixels as f64) * 100.0;

    println!("PNG Diff Results:");
    println!("  Total pixels: {}", stats.total_pixels);
    println!("  Differing pixels: {} ({:.4}%)", stats.differing_pixels, diff_percent);
    println!("  Max channel diff: {}", stats.max_channel_diff);
    println!("  Tolerance: {}", TOLERANCE);
    println!("  Threshold: {}%", THRESHOLD_PERCENT);

    if diff_percent > THRESHOLD_PERCENT {
        let diff_path = Path::new(actual_path).with_file_name("diff.png");
        let diff_img = generate_diff_image(&actual, &expected, TOLERANCE);
        if let Err(e) = diff_img.save(&diff_path) {
            eprintln!("Warning: failed to save diff image: {}", e);
        } else {
            println!("  Diff image saved to: {}", diff_path.display());
        }

        eprintln!("\nFAILED: Image diff exceeds threshold ({:.4}% > {}%)", diff_percent, THRESHOLD_PERCENT);
        process::exit(1);
    }

    println!("\nPASSED: Images match within tolerance");
}
