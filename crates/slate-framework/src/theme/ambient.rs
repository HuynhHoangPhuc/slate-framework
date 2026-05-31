//! Ambient theme access.
//!
//! The active theme is published into a thread-local for the duration of a
//! window's redraw (render → layout → paint). Widgets read it through the free
//! function [`theme`] instead of having a `Theme` prop-drilled to them.
//!
//! Reactivity is Strategy-A: [`theme`] reads the [`ThemeMode`] signal via
//! `Signal::get`, which subscribes the view observer while `render` is running
//! inside `with_observer`. Flipping the mode signal therefore marks the view
//! dirty and re-renders the whole tree with the new palette — no per-token
//! signal wiring required.

use std::cell::RefCell;

use slate_reactive::Signal;

use super::{Theme, ThemeMode, ThemeSet};

/// Handle to the app-global theme published for the current redraw.
#[derive(Clone)]
pub(crate) struct ActiveTheme {
    /// The light/dark palettes installed on the app.
    pub set: ThemeSet,
    /// The signal selecting between them.
    pub mode: Signal<ThemeMode>,
}

thread_local! {
    /// Active theme for the in-flight redraw on this thread, or `None` outside
    /// a render pass (unit tests, pre-`run`).
    static ACTIVE: RefCell<Option<ActiveTheme>> = const { RefCell::new(None) };
}

/// RAII guard that publishes an [`ActiveTheme`] for the lifetime of a redraw
/// and clears it on drop, so a panic mid-render cannot leave a stale theme
/// installed for the next frame.
pub(crate) struct ThemeScope {
    _private: (),
}

impl ThemeScope {
    /// Publish `active` as the ambient theme for this thread.
    pub(crate) fn install(active: ActiveTheme) -> Self {
        ACTIVE.with(|c| *c.borrow_mut() = Some(active));
        ThemeScope { _private: () }
    }
}

impl Drop for ThemeScope {
    fn drop(&mut self) {
        ACTIVE.with(|c| *c.borrow_mut() = None);
    }
}

/// Resolve the current theme.
///
/// Call inside `View::render` (or any widget builder reached from it) to read
/// semantic tokens, e.g. `Div::new().background(theme().surface)`. Reading the
/// mode signal here subscribes the view, so flipping
/// [`AppContext::theme_mode`](crate::AppContext::theme_mode) re-renders with
/// the new palette.
///
/// Outside a redraw (no ambient theme installed) this returns
/// [`Theme::light`] so the function is always safe to call.
pub fn theme() -> Theme {
    // Clone the handle out before touching the signal so we never hold the
    // thread-local borrow across `Signal::get`.
    let active = ACTIVE.with(|c| c.borrow().clone());
    match active {
        Some(a) => a.set.resolve(a.mode.get()),
        None => {
            // No ambient theme installed: outside a redraw, or a widget builder
            // called from an event handler. Returns light without subscribing —
            // log so accidental out-of-render calls are diagnosable.
            log::debug!(target: "slate::theme", "theme() called with no active theme; defaulting to light");
            Theme::light()
        }
    }
}
