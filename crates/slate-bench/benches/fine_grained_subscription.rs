//! Bench: 100 reactive counter cards, 1 signal dirty per iter.
//!
//! Measures Strategy A's whole-view rebuild cost on a workload where
//! Strategy B (fine-grained subscription) would skip 99% of view-render
//! work. Pairs with `reactive_counter_100` (all-dirty) for the A1
//! cheap-vs-deep verdict in the prework plan
//! (`plans/260528-0945-a1-fine-grained-reactivity-prework/`).

use criterion::{Criterion, criterion_group, criterion_main};
use slate_bench::{headless_app, scenes::fine_grained_subscription};
use slate_framework::profiling::{self, CountingAllocator};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

fn bench(c: &mut Criterion) {
    let mut app = headless_app();
    let mut view = fine_grained_subscription::build(&app);

    let _ = app.render_view(&mut view).expect("warmup render failed");
    profiling::reset_counters();

    c.bench_function("fine_grained_subscription", |b| {
        b.iter(|| {
            view.bump_one(50);
            app.render_view(&mut view).expect("render failed")
        })
    });

    eprintln!(
        "fine_grained_subscription counters: \
         view_renders={} view_render_ns={} compute_layout_ns={} paint_ns={} present_ns={} \
         paint_cmds={} signal_notify={} effect_reentry={} reentrancy={} allocs={} deallocs={}",
        profiling::view_render_count(),
        profiling::view_render_ns(),
        profiling::compute_layout_ns(),
        profiling::paint_ns(),
        profiling::present_ns(),
        profiling::paint_cmd_count(),
        profiling::signal_notify_count(),
        profiling::effect_reentry_count(),
        profiling::reentrancy_count(),
        profiling::alloc_count(),
        profiling::dealloc_count(),
    );
}

criterion_group!(benches, bench);
criterion_main!(benches);
