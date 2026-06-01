//! Example adopter-side window-geometry store (P6f).
//!
//! The framework owns the [`WindowGeometry`] struct and the save/restore wiring;
//! the adopter owns the storage format + location. This is a tiny plain-text
//! store (`x y w h maximized`, one space-separated line) — no serde, no
//! config-dir dependency pulled into the framework. Mirrors
//! `examples/window-persistence`.

use std::path::PathBuf;

use slate_framework::{PersistenceStore, WindowGeometry};

/// Plain-text geometry store. `x = i32::MIN` encodes an unknown position.
pub(crate) struct FileStore {
    pub(crate) path: PathBuf,
}

impl PersistenceStore for FileStore {
    fn load(&self, _key: &str) -> Option<WindowGeometry> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        let mut it = text.split_whitespace();
        let x: i32 = it.next()?.parse().ok()?;
        let y: i32 = it.next()?.parse().ok()?;
        let w: u32 = it.next()?.parse().ok()?;
        let h: u32 = it.next()?.parse().ok()?;
        let maximized = it.next()? != "0";
        Some(WindowGeometry {
            position: (x != i32::MIN).then_some((x, y)),
            size: (w, h),
            maximized,
        })
    }

    fn save(&self, _key: &str, geometry: WindowGeometry) {
        let (x, y) = geometry.position.unwrap_or((i32::MIN, i32::MIN));
        let line = format!(
            "{x} {y} {} {} {}",
            geometry.size.0, geometry.size.1, geometry.maximized as u8
        );
        if let Err(e) = std::fs::write(&self.path, line) {
            log::warn!("failed to save dashboard window geometry: {e}");
        }
    }
}
