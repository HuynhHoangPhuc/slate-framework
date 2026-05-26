//! Bench: 1k-line TextArea — shaping cache + glyph atlas under tail mutation.

use criterion::{Criterion, criterion_group, criterion_main};
use slate_bench::{headless_app, scenes::textarea_1k_line};
use slate_framework::profiling::{self, CountingAllocator};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

fn bench(c: &mut Criterion) {
    let mut app = headless_app();
    let mut view = textarea_1k_line::build(&app);

    let _ = app.render_view(&mut view).expect("warmup render failed");
    profiling::reset_counters();

    c.bench_function("textarea_1k_line", |b| {
        b.iter(|| {
            view.mutate();
            app.render_view(&mut view).expect("render failed")
        })
    });

    eprintln!(
        "textarea_1k_line counters: paint_cmds={} signal_notify={} effect_reentry={} \
         allocs={}",
        profiling::paint_cmd_count(),
        profiling::signal_notify_count(),
        profiling::effect_reentry_count(),
        profiling::alloc_count(),
    );
}

criterion_group!(benches, bench);
criterion_main!(benches);
