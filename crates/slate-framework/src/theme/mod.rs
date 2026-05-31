//! Semantic design tokens with ambient, light/dark-switchable access.
//!
//! Widgets read a [`Theme`] through the free [`theme`] accessor instead of
//! hard-coding colours and sizes. The app holds a [`ThemeSet`] (light + dark
//! palettes) and a `Signal<ThemeMode>`; flipping the signal re-renders the
//! whole view with the other palette (Strategy-A reactivity).
//!
//! See [`crate::App::theme`] to install custom palettes and
//! [`crate::AppContext::theme_mode`] to toggle light/dark at runtime.

mod ambient;
mod defaults;
mod tokens;

pub(crate) use ambient::{ActiveTheme, ThemeScope};
pub use ambient::theme;
pub use tokens::{Radii, Spacing, Theme, TypeStyle, Typography};

/// Which palette of a [`ThemeSet`] is active.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// Light palette (the default).
    #[default]
    Light,
    /// Dark palette.
    Dark,
}

/// The light and dark palettes installed on an app.
///
/// The default set is [`Theme::light`] + [`Theme::dark`]; adopters override
/// via [`crate::App::theme`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ThemeSet {
    /// Palette used when the mode is [`ThemeMode::Light`].
    pub light: Theme,
    /// Palette used when the mode is [`ThemeMode::Dark`].
    pub dark: Theme,
}

impl ThemeSet {
    /// Build a set from explicit light and dark palettes.
    pub fn new(light: Theme, dark: Theme) -> Self {
        ThemeSet { light, dark }
    }

    /// Return the palette for `mode`.
    pub fn resolve(&self, mode: ThemeMode) -> Theme {
        match mode {
            ThemeMode::Light => self.light,
            ThemeMode::Dark => self.dark,
        }
    }
}

impl Default for ThemeSet {
    fn default() -> Self {
        ThemeSet {
            light: Theme::light(),
            dark: Theme::dark(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slate_reactive::{Effect, Runtime, Signal};

    #[test]
    fn light_and_dark_tokens_differ() {
        let light = Theme::light();
        let dark = Theme::dark();
        assert_ne!(light.bg, dark.bg);
        assert_ne!(light.fg, dark.fg);
        assert_ne!(light.surface, dark.surface);
        // Non-colour scales are shared across modes.
        assert_eq!(light.spacing, dark.spacing);
        assert_eq!(light.radius, dark.radius);
    }

    #[test]
    fn theme_set_resolves_by_mode() {
        let set = ThemeSet::default();
        assert_eq!(set.resolve(ThemeMode::Light), Theme::light());
        assert_eq!(set.resolve(ThemeMode::Dark), Theme::dark());
    }

    #[test]
    fn theme_returns_default_light_without_ambient() {
        // No ThemeScope installed → safe default.
        assert_eq!(theme(), Theme::light());
    }

    #[test]
    fn ambient_theme_tracks_mode_signal() {
        let rt = Runtime::new();
        let mode = Signal::new(rt.clone(), ThemeMode::Light);
        let _scope = ThemeScope::install(ActiveTheme {
            set: ThemeSet::default(),
            mode: mode.clone(),
        });

        assert_eq!(theme().bg, Theme::light().bg);
        mode.set(ThemeMode::Dark);
        assert_eq!(theme().bg, Theme::dark().bg);
    }

    #[test]
    fn flipping_mode_re_runs_observer() {
        // Proves Strategy-A: a closure that reads `theme()` is re-run when the
        // mode signal flips — the mechanism that re-renders the view.
        let rt = Runtime::new();
        let mode = Signal::new(rt.clone(), ThemeMode::Light);
        let _scope = ThemeScope::install(ActiveTheme {
            set: ThemeSet::default(),
            mode: mode.clone(),
        });

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<ThemeMode>::new()));
        let seen_eff = seen.clone();
        let _eff = Effect::new(rt.clone(), move || {
            // Reading through the ambient accessor must subscribe the observer.
            let _ = theme();
            seen_eff.lock().unwrap().push(if theme().bg == Theme::dark().bg {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            });
        });

        mode.set(ThemeMode::Dark);
        rt.drain_dirty();
        rt.drain_effects();

        let log = seen.lock().unwrap();
        assert_eq!(log.first(), Some(&ThemeMode::Light));
        assert_eq!(log.last(), Some(&ThemeMode::Dark));
    }

    #[test]
    fn adopter_override_resolves_custom_palette() {
        // Adopters build a custom theme by cloning a default and tweaking.
        let mut custom_light = Theme::light();
        custom_light.accent = crate::color::Color::from_hex("#ff00ff").unwrap();
        let set = ThemeSet::new(custom_light, Theme::dark());

        let rt = Runtime::new();
        let mode = Signal::new(rt.clone(), ThemeMode::Light);
        let _scope = ThemeScope::install(ActiveTheme {
            set,
            mode: mode.clone(),
        });

        assert_eq!(theme().accent, custom_light.accent);
        assert_ne!(theme().accent, Theme::light().accent);
    }
}
