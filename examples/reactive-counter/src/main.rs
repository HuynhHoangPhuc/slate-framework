//! reactive-counter — Phase 5 demo + Phase 9a keyboard smoke harness.
//!
//! Demonstrates reactive UI with Signal-driven Text that updates automatically
//! when a background task increments the counter. Phase 9a extends it to also
//! showcase keyboard handling:
//!   - Space / Enter → increment counter by 1
//!   - Backspace → reset counter to 0
//!   - Last key + modifier flags + last typed text shown live below the count
//!
//! Run: `cargo run -p reactive-counter`

use std::time::Duration;

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, IntoAny, JustifyContent,
    KeyCode, Modifiers, Text, Timer, View, WindowOptions,
};

struct CounterView {
    count: Signal<u32>,
    last_key: Signal<String>,
    mods: Signal<String>,
    last_text: Signal<String>,
}

fn format_mods(m: Modifiers) -> String {
    let mut parts: Vec<&str> = vec!["modifiers:"];
    if m.shift {
        parts.push("\u{21E7}"); // ⇧
    }
    if m.ctrl {
        parts.push("\u{2303}"); // ⌃
    }
    if m.alt {
        parts.push("\u{2325}"); // ⌥
    }
    if m.meta {
        parts.push("\u{2318}"); // ⌘
    }
    if parts.len() == 1 {
        parts.push("-");
    }
    parts.join(" ")
}

impl View for CounterView {
    fn render(&mut self) -> AnyElement {
        let c = self.count.clone();
        let lk = self.last_key.clone();
        let m = self.mods.clone();
        let lt = self.last_text.clone();

        Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK).into())
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .gap(12.0)
                    .padding_all(32.0)
                    .flex_grow(1.0)
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
                        Text::new_reactive(move || format!("Count: {}", c.get()))
                            .font_size(32.0)
                            .color(Color::WHITE.into()),
                    ),
            )
            .child(
                Text::new_reactive(move || {
                    let s = lk.get();
                    if s.is_empty() {
                        "last key: -".to_string()
                    } else {
                        format!("last key: {s}")
                    }
                })
                .font_size(13.0)
                .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .child(
                Text::new_reactive(move || m.get())
                    .font_size(13.0)
                    .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .child(
                Text::new_reactive(move || {
                    let s = lt.get();
                    if s.is_empty() {
                        "last typed: -".to_string()
                    } else {
                        format!("last typed: {s:?}")
                    }
                })
                .font_size(13.0)
                .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .child(
                Text::new("Space/Enter +1 · Backspace resets · hold Shift/Ctrl/Alt/Meta")
                    .font_size(12.0)
                    .color(Color::from_hex("#64748b").unwrap_or(Color::WHITE).into()),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    let opts = WindowOptions {
        title: "Slate · reactive-counter — Press Space/Enter/Backspace".into(),
        size: (480, 360),
        min_size: Some((320, 240)),
        resizable: true,
        ..Default::default()
    };

    // Signals constructed up-front so the same instances feed both the keyboard
    // handlers (registered before run) and the view factory closure. Runtime
    // is read from a temporary AppContext-less Signal::new in the factory; we
    // can't access cx.runtime() outside .run(), so we delay Signal creation
    // until the factory closure runs. Workaround: build a Cell of OnceLock-ish
    // state via Rc<RefCell<Option<Signals>>> initialized inside the factory,
    // and clone references into both handlers and view.
    //
    // Simpler: use the view factory to build all signals, then build a shared
    // Rc holding Signal handles. Handlers reference them via clone. The
    // App::new builder chain runs BEFORE .run(view_fn), but the closures
    // passed to on_key_down/etc. only fire AFTER the factory has run, so
    // capturing the same Signal clones works as long as we construct them
    // before registering handlers... which we can't, since they need runtime.
    //
    // Resolution: stash signals in a Rc<RefCell<Option<Signals>>>; populate
    // inside the factory; handlers borrow at fire time. Lazy init, single
    // assignment.
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Signals {
        count: Signal<u32>,
        last_key: Signal<String>,
        mods: Signal<String>,
        last_text: Signal<String>,
    }

    let signals: Rc<RefCell<Option<Signals>>> = Rc::new(RefCell::new(None));

    let key_down_signals = signals.clone();
    let text_signals = signals.clone();
    let factory_signals = signals.clone();

    App::new(opts)
        .on_key_down(move |event| {
            if let Some(s) = key_down_signals.borrow().as_ref() {
                s.last_key
                    .set(format!("{:?} → {:?}", event.code, event.key));
                s.mods.set(format_mods(event.modifiers));
                match event.code {
                    KeyCode::Space | KeyCode::Enter => s.count.update(|n| *n += 1),
                    KeyCode::Backspace => s.count.set(0),
                    _ => {}
                }
            }
        })
        .on_text_input(move |e| {
            if let Some(s) = text_signals.borrow().as_ref() {
                s.last_text.set(e.text.clone());
            }
        })
        .run(move |cx: &AppContext| {
            let count = Signal::new(cx.runtime(), 0u32);
            let last_key = Signal::new(cx.runtime(), String::new());
            let mods = Signal::new(cx.runtime(), String::from("modifiers: -"));
            let last_text = Signal::new(cx.runtime(), String::new());

            *factory_signals.borrow_mut() = Some(Signals {
                count: count.clone(),
                last_key: last_key.clone(),
                mods: mods.clone(),
                last_text: last_text.clone(),
            });

            let bg = cx.background_executor();
            let counter = count.clone();
            bg.spawn(async move {
                loop {
                    Timer::after(Duration::from_secs(1)).await;
                    counter.update(|n| *n += 1);
                    log::info!("Counter incremented to {}", counter.get());
                }
            })
            .detach();

            CounterView {
                count,
                last_key,
                mods,
                last_text,
            }
        });
}
