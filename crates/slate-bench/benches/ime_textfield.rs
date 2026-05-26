//! Bench: IME textfield — single-line shaping under simulated keystrokes.

use criterion::{Criterion, criterion_group, criterion_main};
use slate_bench::{headless_app, scenes::ime_textfield};
use slate_framework::profiling;

fn bench(c: &mut Criterion) {
    let mut app = headless_app();
    let mut view = ime_textfield::build(&app);

    let _ = app.render_view(&mut view).expect("warmup render failed");
    profiling::reset_counters();

    c.bench_function("ime_textfield", |b| {
        b.iter(|| {
            view.type_char();
            app.render_view(&mut view).expect("render failed")
        })
    });

    eprintln!(
        "ime_textfield counters: paint_cmds={} signal_notify={} effect_reentry={} \
         allocs={}",
        profiling::paint_cmd_count(),
        profiling::signal_notify_count(),
        profiling::effect_reentry_count(),
        profiling::alloc_count(),
    );
}

criterion_group!(benches, bench);
criterion_main!(benches);
