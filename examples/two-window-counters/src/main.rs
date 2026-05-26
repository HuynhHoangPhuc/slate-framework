//! two-window-counters — Phase 5 (D2c) multi-window demo.
//!
//! Two independent counter windows wired up before `App::run`, plus a Ctrl+N
//! keyboard chord that calls `AppContext::create_window` from inside an
//! App-level key handler to open additional counter windows at runtime.
//!
//! Each window owns its own `Signal<i32>`. Clicking the +/- buttons in one
//! window updates only that window's value: the per-window view reads only
//! its own signal so the wake-all reactive policy is invisible to the user.
//!
//! Quit affordance: Ctrl+Q (Win32) / Cmd+Q (macOS, `meta` modifier).
//! `AppContext::request_quit` drives the shutdown path that the platform
//! default does not cover on AppKit. Closing every window quits on Win32
//! by platform default.
//!
//! Why dynamic spawn is wired through an App-level key handler instead of an
//! element `on_click`: element click handlers require `Send + Sync + 'static`
//! (handler maps are designed for future cross-thread access), but
//! `AppContext` holds `Weak<AppState>` which is `!Send`. App-level
//! `on_key_down` takes `FnMut + 'static` without the `Send + Sync` bound, so
//! it is the right anchor for an `AppContext`-capturing closure.
//!
//! Run: `cargo run -p two-window-counters`

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, IntoAny, JustifyContent,
    KeyCode, Text, View, WindowOptions,
};

struct CounterView {
    label: String,
    count: Signal<i32>,
}

impl View for CounterView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let count_text = self.count.clone();
        let inc = self.count.clone();
        let dec = self.count.clone();
        let label = self.label.clone();

        Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK))
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .gap(16.0)
                    .padding_all(24.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new(format!("Counter {label}"))
                    .font_size(20.0)
                    .color(Color::WHITE.into()),
            )
            .child(
                Text::new_reactive(move || format!("{}", count_text.get()))
                    .font_size(48.0)
                    .color(Color::WHITE.into()),
            )
            .child(
                Div::new()
                    .style(|s| s.flex_direction(FlexDirection::Row).gap(12.0))
                    .child(
                        Div::new()
                            .background(Color::from_hex("#3b82f6").unwrap_or(Color::BLUE))
                            .corner_radius(6.0)
                            .style(|s| s.padding_all(12.0))
                            .on_click(move |_, _| inc.update(|n| *n += 1))
                            .child(Text::new("+1").font_size(18.0).color(Color::WHITE.into())),
                    )
                    .child(
                        Div::new()
                            .background(Color::from_hex("#ef4444").unwrap_or(Color::RED))
                            .corner_radius(6.0)
                            .style(|s| s.padding_all(12.0))
                            .on_click(move |_, _| dec.update(|n| *n -= 1))
                            .child(Text::new("-1").font_size(18.0).color(Color::WHITE.into())),
                    ),
            )
            .child(
                Text::new("Ctrl+N — spawn new window   ·   Ctrl+Q — quit")
                    .font_size(12.0)
                    .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    let opts_a = WindowOptions {
        title: "Slate · counter A".into(),
        size: (360, 280),
        min_size: Some((280, 200)),
        resizable: true,
        ..Default::default()
    };
    let opts_b = WindowOptions {
        title: "Slate · counter B".into(),
        size: (360, 280),
        min_size: Some((280, 200)),
        resizable: true,
        position: Some((520, 80)),
        ..Default::default()
    };

    // Shared slot used by the App-level key handler to reach the `AppContext`
    // allocated inside `App::run`. Same pattern as `reactive-counter` uses
    // when an App-level handler needs a framework handle that is only
    // available after the run loop starts.
    let cx_handle: Rc<RefCell<Option<AppContext>>> = Rc::new(RefCell::new(None));
    let cx_for_run = cx_handle.clone();
    let cx_for_key = cx_handle.clone();
    let spawn_count = Rc::new(Cell::new(0u32));

    let mut app = App::new(opts_a);

    // Counter B — registered BEFORE `run`. Its view factory captures its own
    // Signal so the two counters stay independent.
    let _id_b = app.create_window(opts_b, move |cx: &AppContext| {
        let count_b = Signal::new(cx.runtime(), 0i32);
        CounterView {
            label: "B".into(),
            count: count_b,
        }
    });

    app.on_key_down(move |evt, _ctx| {
        // Ctrl+Q / Cmd+Q — quit.
        if evt.code == KeyCode::KeyQ && (evt.modifiers.ctrl || evt.modifiers.meta) {
            if let Some(cx) = cx_for_key.borrow().as_ref() {
                log::info!("Ctrl/Cmd+Q — requesting quit");
                cx.request_quit();
            }
            return;
        }

        // Ctrl+N / Cmd+N — spawn a fresh counter window via the dynamic API.
        if evt.code == KeyCode::KeyN && (evt.modifiers.ctrl || evt.modifiers.meta) {
            let Some(cx) = cx_for_key.borrow().as_ref().cloned() else {
                log::warn!("Ctrl/Cmd+N pressed before AppContext was captured");
                return;
            };
            let id = {
                let n = spawn_count.get() + 1;
                spawn_count.set(n);
                n
            };
            let new_count = Signal::new(cx.runtime(), 0i32);
            let label = format!("Spawn{id}");
            let opts = WindowOptions {
                title: format!("Slate · counter {label}"),
                size: (320, 240),
                min_size: Some((240, 180)),
                resizable: true,
                ..Default::default()
            };
            let result = cx.create_window(opts, move |_inner_cx| CounterView {
                label: label.clone(),
                count: new_count.clone(),
            });
            if result.is_none() {
                log::warn!("AppContext::create_window returned None — platform handle missing");
            }
        }
    })
    .run(move |cx: &AppContext| {
        *cx_for_run.borrow_mut() = Some(cx.clone());
        let count_a = Signal::new(cx.runtime(), 0i32);
        CounterView {
            label: "A".into(),
            count: count_a,
        }
    });
}
