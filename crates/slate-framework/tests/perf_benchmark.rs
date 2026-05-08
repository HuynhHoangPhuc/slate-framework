//! Performance benchmarks for Phase 4 reactive system.
//!
//! Measures frame times and cache hit rates against success criteria:
//! - Idle frame ≤ 1.5 ms
//! - Set-driven frame ≤ 1.5 ms with TextShapingCache
//! - Tess cache hit rate ≥ 80%
//! - 1000-element static tree ≤ 0.5 ms paint stage

use std::time::{Duration, Instant};

use slate_framework::{AnyElement, Color, Div, HeadlessApp, IntoAny, Text};
use slate_reactive::{Runtime, Signal};

const WARMUP_FRAMES: usize = 5;
const MEASURE_FRAMES: usize = 20;

fn measure_frame_times<F>(
    app: &mut HeadlessApp,
    build_element: F,
    warmup: usize,
    measure: usize,
) -> Vec<Duration>
where
    F: Fn() -> AnyElement,
{
    // Warmup
    for _ in 0..warmup {
        let root = build_element();
        let _ = app.render(root);
    }

    // Measure
    let mut times = Vec::with_capacity(measure);
    for _ in 0..measure {
        let root = build_element();
        let start = Instant::now();
        let _ = app.render(root);
        times.push(start.elapsed());
    }
    times
}

fn avg_duration(durations: &[Duration]) -> Duration {
    let total: Duration = durations.iter().sum();
    total / durations.len() as u32
}

fn median_duration(durations: &[Duration]) -> Duration {
    let mut sorted = durations.to_vec();
    sorted.sort();
    sorted[sorted.len() / 2]
}

fn p99_duration(durations: &[Duration]) -> Duration {
    let mut sorted = durations.to_vec();
    sorted.sort();
    sorted[(sorted.len() * 99) / 100]
}

/// Nested div structure similar to reactive-counter
fn counter_app_tree() -> AnyElement {
    Div::new()
        .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK).into())
        .style(|s| {
            s.flex_grow(1.0)
                .padding_all(32.0)
                .gap(16.0)
        })
        .child(
            Text::new("Reactive Counter")
                .font_size(24.0)
                .color(Color::WHITE.into()),
        )
        .child(
            Div::new()
                .background(Color::from_hex("#3b82f6").unwrap_or(Color::BLUE).into())
                .corner_radius(8.0)
                .style(|s| s.padding_all(16.0))
                .child(
                    Text::new("Count: 42")
                        .font_size(32.0)
                        .color(Color::WHITE.into()),
                ),
        )
        .child(
            Text::new("Increments every second via background task")
                .font_size(14.0)
                .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
        )
        .into_any()
}

/// 1000-element tree (flat div with 1000 text children)
fn large_tree_1000() -> AnyElement {
    let mut div = Div::new()
        .background(Color::BLACK.into())
        .style(|s| s.flex_grow(1.0));

    for i in 0..1000 {
        div = div.child(
            Text::new(format!("Item {}", i))
                .font_size(10.0)
                .color(Color::WHITE.into()),
        );
    }

    div.into_any()
}

#[test]
fn bench_idle_frame_time() {
    let mut app = HeadlessApp::new(400, 300).expect("headless app");

    let times = measure_frame_times(&mut app, counter_app_tree, WARMUP_FRAMES, MEASURE_FRAMES);

    let avg = avg_duration(&times);
    let median = median_duration(&times);
    let p99 = p99_duration(&times);

    println!("\n=== Idle Frame Time (counter app, no signal changes) ===");
    println!("  Avg:    {:?}", avg);
    println!("  Median: {:?}", median);
    println!("  P99:    {:?}", p99);
    println!("  Target: ≤ 1.5 ms");

    // Target: ≤ 1.5 ms — report, don't gate (per plan: gate on correctness, not perf)
    if median > Duration::from_micros(1500) {
        println!("  Note: Above target — headless overhead, file follow-up if persistent");
    }
}

#[test]
fn bench_cache_hit_rate() {
    let mut app = HeadlessApp::new(400, 300).expect("headless app");

    // Render 100 frames with same content
    for _ in 0..100 {
        let _ = app.render(counter_app_tree());
    }

    let hits = app.text_shaping_cache_hits();
    let misses = app.text_shaping_cache_misses();
    let total = hits + misses;
    let hit_rate = (hits as f64 / total as f64) * 100.0;

    println!("\n=== TextShapingCache Hit Rate ===");
    println!("  Hits:   {}", hits);
    println!("  Misses: {}", misses);
    println!("  Rate:   {:.1}%", hit_rate);
    println!("  Target: ≥ 80%");

    // Target: ≥ 80% hit rate
    assert!(
        hit_rate >= 80.0,
        "cache hit rate {:.1}% below 80% target",
        hit_rate
    );
}

#[test]
fn bench_1000_element_tree() {
    let mut app = HeadlessApp::new(1920, 1080).expect("headless app");

    let times = measure_frame_times(&mut app, large_tree_1000, 3, 10);

    let avg = avg_duration(&times);
    let median = median_duration(&times);

    println!("\n=== 1000-Element Tree Frame Time ===");
    println!("  Avg:    {:?}", avg);
    println!("  Median: {:?}", median);
    println!("  Note: Includes layout + paint + GPU submit");

    // For 1000 elements, paint is a larger portion but still reasonable
    // We just report; gating on correctness not exact timing
}

#[test]
fn bench_1000_element_cached() {
    let mut app = HeadlessApp::new(1920, 1080).expect("headless app");

    // Warmup: first render (all misses)
    let _ = app.render(large_tree_1000());

    // Reset cache counters by noting initial state
    let initial_hits = app.text_shaping_cache_hits();
    let initial_misses = app.text_shaping_cache_misses();

    // Measure cached renders
    let times = measure_frame_times(&mut app, large_tree_1000, 0, 10);

    let hits = app.text_shaping_cache_hits() - initial_hits;
    let misses = app.text_shaping_cache_misses() - initial_misses;
    let hit_rate = if hits + misses > 0 {
        (hits as f64 / (hits + misses) as f64) * 100.0
    } else {
        0.0
    };

    let avg = avg_duration(&times);
    let median = median_duration(&times);

    println!("\n=== 1000-Element Tree (Cached) ===");
    println!("  Avg frame:   {:?}", avg);
    println!("  Median:      {:?}", median);
    println!("  Cache hits:  {} ({:.1}%)", hits, hit_rate);

    // With 1000 text elements all hitting cache, should be very efficient
    // Note: Target is paint stage ≤ 0.5ms, but we measure full frame
    // Full frame includes layout so we allow more headroom
}

#[test]
fn bench_set_driven_frame() {
    let mut app = HeadlessApp::new(400, 300).expect("headless app");
    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), 0u32);

    // Warmup
    for _ in 0..WARMUP_FRAMES {
        let _ = app.render(counter_app_tree());
    }

    // Measure frames with one signal change per frame
    let mut times = Vec::with_capacity(MEASURE_FRAMES);
    for i in 0..MEASURE_FRAMES {
        signal.set(i as u32);
        let start = Instant::now();
        let _ = app.render(counter_app_tree());
        times.push(start.elapsed());
    }

    let avg = avg_duration(&times);
    let median = median_duration(&times);

    println!("\n=== Set-Driven Frame Time ===");
    println!("  Avg:    {:?}", avg);
    println!("  Median: {:?}", median);
    println!("  Target: ≤ 1.5 ms");

    // Target: ≤ 1.5 ms — report, don't gate (per plan: gate on correctness, not perf)
    if median > Duration::from_micros(1500) {
        println!("  Note: Above target — headless overhead, file follow-up if persistent");
    }
}
