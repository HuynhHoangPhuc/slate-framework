//! Window geometry persistence.
//!
//! A plain [`WindowGeometry`] value plus a pluggable [`PersistenceStore`] the
//! adopter implements. The framework restores saved geometry onto creation
//! [`WindowOptions`](slate_platform::WindowOptions) before a window is
//! allocated, and saves it back on resize-settle and on close. It never picks a
//! file format or storage location — the adopter's store owns both.
//!
//! Wiring:
//! - Install a store with [`App::persistence`](crate::App::persistence).
//! - Tag each window with a stable
//!   [`persistence_key`](slate_platform::WindowOptions::persistence_key).
//! - The framework loads on create, saves on resize-settle + close.

mod geometry;
mod store;

use std::rc::Rc;

use slate_platform::WindowOptions;

pub use geometry::WindowGeometry;
pub use store::{InMemoryStore, PersistenceStore};

/// Apply stored geometry onto `opts` when a store is present and `opts` carries
/// a [`persistence_key`](slate_platform::WindowOptions::persistence_key).
///
/// Shared restore step for both window-creation paths: the pre-run
/// [`App`](crate::App) path and the dynamic
/// [`AppContext::create_window`](crate::AppContext::create_window) path. No-op
/// when there is no store, no key, or nothing saved under that key.
pub(crate) fn restore_options(store: Option<&Rc<dyn PersistenceStore>>, opts: &mut WindowOptions) {
    let (Some(store), Some(key)) = (store, opts.persistence_key.clone()) else {
        return;
    };
    if let Some(geometry) = store.load(&key) {
        geometry.apply_to(opts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyed_opts() -> WindowOptions {
        WindowOptions {
            size: (800, 600),
            position: None,
            persistence_key: Some("main".into()),
            ..Default::default()
        }
    }

    fn saved() -> WindowGeometry {
        WindowGeometry {
            position: Some((200, 150)),
            size: (1100, 850),
            maximized: true,
        }
    }

    #[test]
    fn no_store_leaves_options_unchanged() {
        let mut opts = keyed_opts();
        restore_options(None, &mut opts);
        assert_eq!(opts.size, (800, 600));
        assert_eq!(opts.position, None);
        assert!(!opts.maximized);
    }

    #[test]
    fn store_without_key_leaves_options_unchanged() {
        let store: Rc<dyn PersistenceStore> = Rc::new(InMemoryStore::new());
        store.save("main", saved());
        let mut opts = WindowOptions {
            size: (800, 600),
            persistence_key: None,
            ..Default::default()
        };
        restore_options(Some(&store), &mut opts);
        assert_eq!(opts.size, (800, 600));
    }

    #[test]
    fn key_present_but_nothing_saved_leaves_options_unchanged() {
        let store: Rc<dyn PersistenceStore> = Rc::new(InMemoryStore::new());
        let mut opts = keyed_opts();
        restore_options(Some(&store), &mut opts);
        assert_eq!(opts.size, (800, 600));
        assert_eq!(opts.position, None);
    }

    #[test]
    fn saved_geometry_is_restored_onto_options() {
        let store: Rc<dyn PersistenceStore> = Rc::new(InMemoryStore::new());
        store.save("main", saved());
        let mut opts = keyed_opts();
        restore_options(Some(&store), &mut opts);
        assert_eq!(opts.position, Some((200, 150)));
        assert_eq!(opts.size, (1100, 850));
        assert!(opts.maximized);
    }
}
