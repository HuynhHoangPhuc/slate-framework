//! reactive-counter — Phase 9b demo: focusable cards with per-element keys.
//!
//! Three focusable cards, each with its own counter Signal. Click a card or
//! Tab/Shift+Tab between them. Once a card is focused:
//!   - Space / Enter / `+` → increment that card's counter
//!   - `-` / Backspace → decrement
//!
//! Middle card opts out of the focus ring via `Div::focus_ring(false)` to
//! prove the opt-out works while still receiving keyboard events.
//!
//! Background task increments card 1 every second (reactive smoke test).
//! App-level handler still records the last key + modifiers for diagnostics.
//!
//! Run: `cargo run -p reactive-counter`

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, IntoAny, JustifyContent,
    Key, Modifiers, NamedKey, Text, Timer, View, WindowOptions,
};

struct DemoView {
    counts: [Signal<i32>; 3],
    last_key: Signal<String>,
    mods: Signal<String>,
}

fn format_mods(m: Modifiers) -> String {
    let mut parts: Vec<&str> = vec!["modifiers:"];
    if m.shift {
        parts.push("\u{21E7}");
    }
    if m.ctrl {
        parts.push("\u{2303}");
    }
    if m.alt {
        parts.push("\u{2325}");
    }
    if m.meta {
        parts.push("\u{2318}");
    }
    if parts.len() == 1 {
        parts.push("-");
    }
    parts.join(" ")
}

fn card(label: &'static str, counter: Signal<i32>, ring: bool) -> Div {
    let c_text = counter.clone();
    let c_handler = counter;
    Div::new()
        .focusable(true)
        .tab_index(0)
        .focus_ring(ring)
        .background(Color::from_hex("#3b82f6").unwrap_or(Color::BLUE).into())
        .corner_radius(8.0)
        .style(|s| {
            s.flex_direction(FlexDirection::Column)
                .align_items(AlignItems::Center)
                .justify_content(JustifyContent::Center)
                .gap(6.0)
                .padding_all(16.0)
                .width(120.0)
                .height(120.0)
        })
        .on_key_down(move |ev, _cx| match &ev.key {
            Key::Named(NamedKey::Space | NamedKey::Enter) => c_handler.update(|n| *n += 1),
            Key::Named(NamedKey::Backspace) => c_handler.update(|n| *n -= 1),
            Key::Character(s) if s == "+" => c_handler.update(|n| *n += 1),
            Key::Character(s) if s == "-" => c_handler.update(|n| *n -= 1),
            _ => {}
        })
        .child(
            Text::new(label)
                .font_size(13.0)
                .color(Color::WHITE.into()),
        )
        .child(
            Text::new_reactive(move || c_text.get().to_string())
                .font_size(28.0)
                .color(Color::WHITE.into()),
        )
}

impl View for DemoView {
    fn render(&mut self) -> AnyElement {
        let lk = self.last_key.clone();
        let m = self.mods.clone();
        let [c0, c1, c2] = self.counts.clone();

        Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK).into())
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .gap(16.0)
                    .padding_all(24.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new("Focusable Cards · Tab / click / +/- / Space")
                    .font_size(18.0)
                    .color(Color::WHITE.into()),
            )
            .child(
                Div::new()
                    .style(|s| s.flex_direction(FlexDirection::Row).gap(16.0))
                    .child(card("Card 1 (ring)", c0, true))
                    .child(card("Card 2 (no ring)", c1, false))
                    .child(card("Card 3 (ring)", c2, true)),
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
                .font_size(12.0)
                .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .child(
                Text::new_reactive(move || m.get())
                    .font_size(12.0)
                    .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    let opts = WindowOptions {
        title: "Slate · reactive-counter — Tab through focusable cards".into(),
        size: (560, 380),
        min_size: Some((400, 280)),
        resizable: true,
        ..Default::default()
    };

    struct Signals {
        last_key: Signal<String>,
        mods: Signal<String>,
    }

    let signals: Rc<RefCell<Option<Signals>>> = Rc::new(RefCell::new(None));
    let key_signals = signals.clone();
    let factory_signals = signals;

    App::new(opts)
        .on_key_down(move |event, _cx| {
            if let Some(s) = key_signals.borrow().as_ref() {
                s.last_key
                    .set(format!("{:?} → {:?}", event.code, event.key));
                s.mods.set(format_mods(event.modifiers));
            }
        })
        .run(move |cx: &AppContext| {
            let counts = [
                Signal::new(cx.runtime(), 0i32),
                Signal::new(cx.runtime(), 0i32),
                Signal::new(cx.runtime(), 0i32),
            ];
            let last_key = Signal::new(cx.runtime(), String::new());
            let mods = Signal::new(cx.runtime(), String::from("modifiers: -"));

            *factory_signals.borrow_mut() = Some(Signals {
                last_key: last_key.clone(),
                mods: mods.clone(),
            });

            let bg = cx.background_executor();
            let counter0 = counts[0].clone();
            bg.spawn(async move {
                loop {
                    Timer::after(Duration::from_secs(1)).await;
                    counter0.update(|n| *n += 1);
                }
            })
            .detach();

            DemoView {
                counts,
                last_key,
                mods,
            }
        });
}
