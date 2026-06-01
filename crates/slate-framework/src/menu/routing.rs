//! Menu action routing.
//!
//! A [`MenuRegistry`] maps each [`MenuId`] to a handler closure. When a native
//! menu (or a synthetic test) activates an item, the framework looks up the id
//! and invokes its handler on the main thread. Handlers capture and mutate
//! [`Signal`](slate_reactive::Signal)s; the mutation marks the reactive runtime
//! dirty, which schedules the next view rebuild (Strategy A) — the same path
//! `on_click`/key handlers use.
//!
//! ## Re-entrancy
//!
//! The registry is held behind a `RefCell` in `AppState`. Dispatch therefore
//! follows the deferred-op discipline used by overlay dismiss: clone the
//! handler `Rc` out under a short borrow, drop the borrow, *then* invoke — so a
//! handler that rebuilds the menu (mutating the registry) cannot alias a live
//! borrow. See [`AppState::dispatch_menu`](crate::app_state::AppState).

use std::collections::HashMap;
use std::rc::Rc;

use super::model::MenuId;

/// Boxed menu-action handler. `Fn()` (not `FnMut`) so it can be cloned and
/// invoked after its borrow is released; capture an `AppContext`/`Signal`
/// clone for anything the handler needs to touch.
pub type MenuHandler = Rc<dyn Fn()>;

/// Maps [`MenuId`]s to their action handlers.
#[derive(Default)]
pub struct MenuRegistry {
    handlers: HashMap<MenuId, MenuHandler>,
}

impl MenuRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register (or replace) the handler for `id`.
    pub fn register(&mut self, id: MenuId, handler: impl Fn() + 'static) {
        self.handlers.insert(id, Rc::new(handler));
    }

    /// Clone out the handler for `id`, if any. Callers invoke it *after*
    /// releasing any borrow on the registry (re-entrancy discipline).
    pub fn handler(&self, id: MenuId) -> Option<MenuHandler> {
        self.handlers.get(&id).cloned()
    }

    /// Convenience dispatch for non-`RefCell` callers (e.g. unit tests): look
    /// up and invoke the handler for `id`. Returns `true` if a handler ran.
    ///
    /// Do NOT call this through a held `RefCell` borrow of the registry — use
    /// [`handler`](Self::handler) + drop-then-invoke instead.
    pub fn dispatch(&self, id: MenuId) -> bool {
        match self.handler(id) {
            Some(h) => {
                h();
                true
            }
            None => false,
        }
    }

    /// Remove all registered handlers (e.g. before installing a new menu).
    pub fn clear(&mut self) {
        self.handlers.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn dispatch_invokes_only_the_matching_handler() {
        let a = Rc::new(Cell::new(0u32));
        let b = Rc::new(Cell::new(0u32));
        let mut reg = MenuRegistry::new();
        let (ca, cb) = (a.clone(), b.clone());
        reg.register(MenuId(1), move || ca.set(ca.get() + 1));
        reg.register(MenuId(2), move || cb.set(cb.get() + 1));

        assert!(reg.dispatch(MenuId(1)));
        assert_eq!((a.get(), b.get()), (1, 0));
        assert!(reg.dispatch(MenuId(2)));
        assert_eq!((a.get(), b.get()), (1, 1));
    }

    #[test]
    fn dispatch_unknown_id_is_noop() {
        let reg = MenuRegistry::new();
        assert!(!reg.dispatch(MenuId(42)));
    }

    #[test]
    fn re_register_replaces_handler() {
        let hits = Rc::new(Cell::new(0u32));
        let mut reg = MenuRegistry::new();
        let h1 = hits.clone();
        reg.register(MenuId(1), move || h1.set(h1.get() + 10));
        let h2 = hits.clone();
        reg.register(MenuId(1), move || h2.set(h2.get() + 1));
        reg.dispatch(MenuId(1));
        assert_eq!(hits.get(), 1); // second registration won
    }

    #[test]
    fn handler_mutates_a_signal() {
        let runtime = slate_reactive::Runtime::new();
        let count = slate_reactive::Signal::new(runtime, 0u32);
        let mut reg = MenuRegistry::new();
        let c = count.clone();
        reg.register(MenuId(1), move || c.update(|v| *v += 1));

        assert_eq!(count.get(), 0);
        assert!(reg.dispatch(MenuId(1)));
        assert_eq!(count.get(), 1);
    }
}
