//! Bench: dense image gallery — atlas eviction + scroll-back per frame.

use criterion::{Criterion, criterion_group, criterion_main};
use slate_bench::{headless_app, scenes::dense_image_gallery};
use slate_framework::profiling::{self, CountingAllocator};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

fn bench(c: &mut Criterion) {
    let mut app = headless_app();
    let mut view = dense_image_gallery::build(&app);

    // Warm up cache state, then measure cold deltas.
    let _ = app.render_view(&mut view).expect("warmup render failed");
    profiling::reset_counters();

    c.bench_function("dense_image_gallery", |b| {
        b.iter(|| {
            view.advance();
            app.render_view(&mut view).expect("render failed")
        })
    });

    eprintln!(
        "dense_image_gallery counters: paint_cmds={} signal_notify={} effect_reentry={} \
         reentrancy={} allocs={}",
        profiling::paint_cmd_count(),
        profiling::signal_notify_count(),
        profiling::effect_reentry_count(),
        profiling::reentrancy_count(),
        profiling::alloc_count(),
    );
}

criterion_group!(benches, bench);
criterion_main!(benches);
