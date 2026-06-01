//! [`PersistenceStore`] — the pluggable backend the adopter implements.

use std::cell::RefCell;
use std::collections::HashMap;

use super::geometry::WindowGeometry;

/// Adopter-supplied storage for window geometry.
///
/// The framework owns [`WindowGeometry`] and the save/restore *wiring*; the
/// adopter owns *where* and *how* it is stored (a JSON file, the registry, a
/// preferences database). The `key` is the per-window
/// [`persistence_key`](slate_platform::WindowOptions::persistence_key).
///
/// Stores are held as `Rc<dyn PersistenceStore>` and invoked on the UI thread,
/// so impls need not be `Send`/`Sync`.
pub trait PersistenceStore {
    /// Load saved geometry for `key`, or `None` if nothing is stored.
    fn load(&self, key: &str) -> Option<WindowGeometry>;
    /// Save (or overwrite) the geometry stored under `key`.
    fn save(&self, key: &str, geometry: WindowGeometry);
}

/// In-memory [`PersistenceStore`]: geometry survives within one process run but
/// not across restart.
///
/// Serves as the basis for unit tests and as a zero-dependency placeholder for
/// adopters who have not yet wired durable storage. A real adopter store writes
/// `WindowGeometry` to disk in a format of its choosing.
#[derive(Default)]
pub struct InMemoryStore {
    map: RefCell<HashMap<String, WindowGeometry>>,
}

impl InMemoryStore {
    /// Create an empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl PersistenceStore for InMemoryStore {
    fn load(&self, key: &str) -> Option<WindowGeometry> {
        self.map.borrow().get(key).copied()
    }

    fn save(&self, key: &str, geometry: WindowGeometry) {
        self.map.borrow_mut().insert(key.to_string(), geometry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom() -> WindowGeometry {
        WindowGeometry {
            position: Some((120, 80)),
            size: (1024, 768),
            maximized: false,
        }
    }

    #[test]
    fn load_missing_key_is_none() {
        let store = InMemoryStore::new();
        assert_eq!(store.load("absent"), None);
    }

    #[test]
    fn save_then_load_round_trips() {
        let store = InMemoryStore::new();
        store.save("main", geom());
        assert_eq!(store.load("main"), Some(geom()));
    }

    #[test]
    fn save_overwrites_existing_key() {
        let store = InMemoryStore::new();
        store.save("main", geom());
        let updated = WindowGeometry {
            maximized: true,
            ..geom()
        };
        store.save("main", updated);
        assert_eq!(store.load("main"), Some(updated));
    }

    #[test]
    fn keys_are_independent() {
        let store = InMemoryStore::new();
        store.save("a", geom());
        assert_eq!(store.load("b"), None);
        assert_eq!(store.load("a"), Some(geom()));
    }
}
