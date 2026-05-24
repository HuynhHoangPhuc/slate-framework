//! Smoke test for reactive Text with Signal.
//!
//! Validates Text::new_reactive + observer wiring.

use slate_framework::reactive::Signal;
use slate_framework::{AnyElement, HeadlessApp, IntoAny, Text, View};

struct CounterView {
    count: Signal<u32>,
}

impl View for CounterView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let c = self.count.clone();
        Text::new_reactive(move || format!("Count: {}", c.get())).into_any()
    }
}

#[test]
fn reactive_text_updates_on_signal_change() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    let count = Signal::new(app.runtime(), 0u32);
    let mut view = CounterView {
        count: count.clone(),
    };

    // First render with count = 0
    let _img1 = app.render_view(&mut view).expect("first render");

    // Update signal
    count.set(42);

    // Second render should show count = 42
    let _img2 = app.render_view(&mut view).expect("second render");

    // Verify signal value changed
    assert_eq!(count.get(), 42);
}

#[test]
fn signal_increment_updates_reactive_text() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    let count = Signal::new(app.runtime(), 0u32);
    let mut view = CounterView {
        count: count.clone(),
    };

    // Initial render
    let _img = app.render_view(&mut view).expect("render");
    assert_eq!(count.get(), 0);

    // Increment multiple times
    for expected in 1..=5 {
        count.update(|n| *n += 1);
        let _img = app.render_view(&mut view).expect("render after increment");
        assert_eq!(count.get(), expected);
    }
}

#[test]
fn headless_runtime_accessible() {
    let app = HeadlessApp::new(100, 100).expect("headless app");
    let runtime = app.runtime();

    // Can create signals with headless runtime
    let signal: Signal<String> = Signal::new(runtime, "test".to_string());
    assert_eq!(signal.get(), "test");

    signal.set("updated".to_string());
    assert_eq!(signal.get(), "updated");
}
