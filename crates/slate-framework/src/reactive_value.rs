//! `Reactive<T>` — a modifier value that is either a fixed value or signal-backed.
//!
//! A `Reactive<T>` lets element modifiers (e.g. `Div::background`) accept either
//! a plain value or a signal/closure whose value is re-read every time the value
//! is resolved. Resolution happens during `View::render` (under the active
//! observer), so a signal-backed value subscribes the view observer exactly like
//! [`crate::Text::new_reactive`] — on `Signal::set` the whole view re-renders and
//! the value is re-resolved. This is the Strategy-A whole-view rebuild model;
//! there is no sub-rebuild granularity here.

use std::sync::Arc;

use crate::color::Color;
use slate_reactive::Signal;

/// A modifier value that is either static or dynamically resolved.
///
/// Construct via `From` (`value.into()`, `signal.into()`) or
/// [`Reactive::dynamic`] for an arbitrary closure.
pub enum Reactive<T> {
    /// A fixed value, resolved once with no subscription.
    Static(T),
    /// A value re-computed on each `resolve()`; subscribes the active observer.
    Dynamic(Arc<dyn Fn() -> T + Send + Sync>),
}

impl<T> Reactive<T> {
    /// Build a dynamic `Reactive` from a closure re-run on every `resolve()`.
    pub fn dynamic(f: impl Fn() -> T + Send + Sync + 'static) -> Self {
        Reactive::Dynamic(Arc::new(f))
    }
}

impl<T: Clone> Reactive<T> {
    /// Resolve the current value. For the dynamic variant this runs the closure,
    /// which — when called under an observer scope — subscribes that observer.
    pub fn resolve(&self) -> T {
        match self {
            Reactive::Static(v) => v.clone(),
            Reactive::Dynamic(f) => f(),
        }
    }
}

impl<T: Clone> Clone for Reactive<T> {
    fn clone(&self) -> Self {
        match self {
            Reactive::Static(v) => Reactive::Static(v.clone()),
            Reactive::Dynamic(f) => Reactive::Dynamic(f.clone()),
        }
    }
}

impl<T> From<T> for Reactive<T> {
    fn from(value: T) -> Self {
        Reactive::Static(value)
    }
}

impl From<[f32; 4]> for Reactive<Color> {
    fn from(arr: [f32; 4]) -> Self {
        Reactive::Static(Color(arr))
    }
}

impl<T: Clone + Send + Sync + 'static> From<Signal<T>> for Reactive<T> {
    fn from(signal: Signal<T>) -> Self {
        Reactive::dynamic(move || signal.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slate_reactive::Runtime;

    #[test]
    fn static_resolves_to_value() {
        let r: Reactive<f32> = 8.0.into();
        assert_eq!(r.resolve(), 8.0);
    }

    #[test]
    fn from_color_is_static() {
        let r: Reactive<Color> = Color::RED.into();
        assert_eq!(r.resolve(), Color::RED);
    }

    #[test]
    fn from_array_becomes_color() {
        let r: Reactive<Color> = [1.0, 0.0, 0.0, 1.0].into();
        assert_eq!(r.resolve(), Color([1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn dynamic_reflects_signal_after_set() {
        let rt = Runtime::new();
        let signal = Signal::new(rt, Color::RED);
        let r: Reactive<Color> = signal.clone().into();
        assert_eq!(r.resolve(), Color::RED);

        signal.set(Color::BLUE);
        assert_eq!(r.resolve(), Color::BLUE);
    }

    #[test]
    fn dynamic_closure_reresolves() {
        let r = Reactive::dynamic(|| 3.0_f32);
        assert_eq!(r.resolve(), 3.0);
    }
}
