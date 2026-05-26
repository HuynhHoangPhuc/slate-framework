//! Smoke test for every bench scene. Each scene must:
//!   1. Build without panicking.
//!   2. Render one frame through `HeadlessApp::render_view`.
//!   3. Emit at least one paint command (proves the pipeline ran end-to-end).
//!
//! Counters are process-global atomics; the test takes a per-process mutex
//! so parallel `cargo test` threads do not interleave reads/writes.

use std::sync::{Mutex, OnceLock};

use slate_bench::{headless_app, scenes};
use slate_framework::profiling;

fn lock() -> std::sync::MutexGuard<'static, ()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn dense_image_gallery_renders() {
    let _g = lock();
    let mut app = headless_app();
    let mut view = scenes::dense_image_gallery::build(&app);
    profiling::reset_counters();
    let _ = app.render_view(&mut view).expect("render failed");
    assert!(
        profiling::paint_cmd_count() > 0,
        "dense_image_gallery: paint_cmd_count must be > 0"
    );
}

#[test]
fn textarea_1k_line_renders() {
    let _g = lock();
    let mut app = headless_app();
    let mut view = scenes::textarea_1k_line::build(&app);
    profiling::reset_counters();
    let _ = app.render_view(&mut view).expect("render failed");
    assert!(
        profiling::paint_cmd_count() > 0,
        "textarea_1k_line: paint_cmd_count must be > 0"
    );
}

#[test]
fn reactive_counter_100_renders() {
    let _g = lock();
    let mut app = headless_app();
    let mut view = scenes::reactive_counter_100::build(&app);
    profiling::reset_counters();
    let _ = app.render_view(&mut view).expect("render failed");
    assert!(
        profiling::paint_cmd_count() > 0,
        "reactive_counter_100: paint_cmd_count must be > 0"
    );
}

#[test]
fn ime_textfield_renders() {
    let _g = lock();
    let mut app = headless_app();
    let mut view = scenes::ime_textfield::build(&app);
    profiling::reset_counters();
    let _ = app.render_view(&mut view).expect("render failed");
    assert!(
        profiling::paint_cmd_count() > 0,
        "ime_textfield: paint_cmd_count must be > 0"
    );
}
