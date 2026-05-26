//! Bench: 100 reactive counter cards — observer-dispatch hot path.

use criterion::{Criterion, criterion_group, criterion_main};
use slate_bench::{headless_app, scenes::reactive_counter_100};
use slate_framework::profiling;

fn bench(c: &mut Criterion) {
    let mut app = headless_app();
    let mut view = reactive_counter_100::build(&app);

    let _ = app.render_view(&mut view).expect("warmup render failed");
    profiling::reset_counters();

    c.bench_function("reactive_counter_100", |b| {
        b.iter(|| {
            view.bump_all();
            app.render_view(&mut view).expect("render failed")
        })
    });

    eprintln!(
        "reactive_counter_100 counters: paint_cmds={} signal_notify={} effect_reentry={} \
         allocs={}",
        profiling::paint_cmd_count(),
        profiling::signal_notify_count(),
        profiling::effect_reentry_count(),
        profiling::alloc_count(),
    );
}

criterion_group!(benches, bench);
criterion_main!(benches);
