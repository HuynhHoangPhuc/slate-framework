//! [`WindowGeometry`] — the plain value persisted across launches.

use slate_platform::{Window, WindowOptions, WindowPlacement};

/// A window's saved geometry: where it sits, how big it is, and whether it was
/// maximized.
///
/// Deliberately plain (`Copy`, no `serde`): the framework owns the *shape* of
/// the data, while the adopter's [`PersistenceStore`](super::PersistenceStore)
/// owns the serialization format and storage location. This keeps the framework
/// free of any config-dir / format dependency (charter "no format lock-in").
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WindowGeometry {
    /// Top-left position in the platform's screen coordinate space, or `None`
    /// when the platform could not report it. See [`Window::position`].
    pub position: Option<(i32, i32)>,
    /// Logical content size in points.
    pub size: (u32, u32),
    /// Whether the window was maximized when captured.
    pub maximized: bool,
}

impl WindowGeometry {
    /// Capture the current geometry of a live platform window.
    ///
    /// `maximized` is the authoritative flag for restore. The `position`/`size`
    /// captured while maximized differ by platform: Windows reports the restored
    /// `rcNormalPosition` (so un-maximizing later returns to sane bounds), while
    /// macOS reports the *zoomed* frame — restoring a macOS window that was saved
    /// maximized re-zooms it correctly, but its un-zoom target becomes the zoomed
    /// size until the user resizes. Capturing while not maximized is exact on
    /// both platforms.
    pub fn capture(window: &dyn Window) -> Self {
        Self {
            position: window.position(),
            size: window.logical_size(),
            maximized: window.placement() == WindowPlacement::Maximized,
        }
    }

    /// Apply this geometry onto creation [`WindowOptions`] (restore-on-create).
    ///
    /// Overrides position (when known), size, and the maximized flag; leaves
    /// every other option untouched so adopter choices (title, min size,
    /// resizable, persistence key) survive.
    pub fn apply_to(&self, opts: &mut WindowOptions) {
        if let Some(pos) = self.position {
            opts.position = Some(pos);
        }
        opts.size = self.size;
        opts.maximized = self.maximized;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_overrides_position_size_and_maximized() {
        let mut opts = WindowOptions {
            size: (800, 600),
            position: None,
            maximized: false,
            ..Default::default()
        };
        let geom = WindowGeometry {
            position: Some((42, 17)),
            size: (1280, 720),
            maximized: true,
        };
        geom.apply_to(&mut opts);
        assert_eq!(opts.position, Some((42, 17)));
        assert_eq!(opts.size, (1280, 720));
        assert!(opts.maximized);
    }

    #[test]
    fn apply_with_unknown_position_keeps_existing_position() {
        let mut opts = WindowOptions {
            position: Some((10, 10)),
            ..Default::default()
        };
        let geom = WindowGeometry {
            position: None,
            size: (640, 480),
            maximized: false,
        };
        geom.apply_to(&mut opts);
        // position untouched when the saved geometry has none; size applied.
        assert_eq!(opts.position, Some((10, 10)));
        assert_eq!(opts.size, (640, 480));
    }

    #[test]
    fn apply_preserves_unrelated_options() {
        let mut opts = WindowOptions {
            title: "Keep Me".into(),
            min_size: Some((320, 240)),
            resizable: true,
            persistence_key: Some("main".into()),
            ..Default::default()
        };
        WindowGeometry {
            position: Some((0, 0)),
            size: (900, 700),
            maximized: false,
        }
        .apply_to(&mut opts);
        assert_eq!(opts.title, "Keep Me");
        assert_eq!(opts.min_size, Some((320, 240)));
        assert!(opts.resizable);
        assert_eq!(opts.persistence_key.as_deref(), Some("main"));
    }
}
