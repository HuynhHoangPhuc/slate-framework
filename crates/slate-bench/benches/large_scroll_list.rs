//! Bench: large virtualized scroll list — window slides one step per frame.
//!
//! The headline number is `paint_cmds`: with virtualization it stays in the
//! tens even though the list holds 100k rows. A non-virtualized list would push
//! paint commands proportional to the full count.

use criterion::{Criterion, criterion_group, criterion_main};
use slate_bench::{headless_app, scenes::large_scroll_list};
use slate_framework::profiling::{self, CountingAllocator};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

fn bench(c: &mut Criterion) {
    let mut app = headless_app();
    let mut view = large_scroll_list::build(&app);

    // Warm up (font/atlas/shaping caches), then measure cold deltas.
    let _ = app.render_view(&mut view).expect("warmup render failed");
    profiling::reset_counters();

    c.bench_function("large_scroll_list", |b| {
        b.iter(|| {
            view.advance();
            app.render_view(&mut view).expect("render failed")
        })
    });

    eprintln!(
        "large_scroll_list counters: paint_cmds={} signal_notify={} effect_reentry={} \
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
