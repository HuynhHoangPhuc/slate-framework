use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// A scope that owns reactive primitives and disposes them on drop.
///
/// When a `ReactiveOwner` is dropped, all primitives registered with it
/// (Memo, Effect) are disposed, breaking subscription cycles.
///
/// Owners form a stack via thread-local storage. The current owner is
/// available via `ReactiveOwner::current()`.
pub struct ReactiveOwner {
    inner: Rc<OwnerInner>,
}

struct OwnerInner {
    children: RefCell<Vec<OwnedChild>>,
}

#[allow(dead_code)] // Fields hold ownership, not read
enum OwnedChild {
    Rc(Box<dyn Any>),
    Arc(Box<dyn Any + Send + Sync>),
}

impl ReactiveOwner {
    /// Creates a new root-level owner.
    pub fn root() -> Self {
        Self {
            inner: Rc::new(OwnerInner {
                children: RefCell::new(Vec::new()),
            }),
        }
    }

    /// Enters this owner's scope, making it the current owner.
    /// Returns a guard that exits the scope on drop.
    pub fn enter(&self) -> OwnerGuard {
        OWNER_STACK.with(|s| s.borrow_mut().push(self.inner.clone()));
        OwnerGuard { _private: () }
    }

    /// Returns the current owner, if any.
    pub fn current() -> Option<ReactiveOwner> {
        OWNER_STACK.with(|s| {
            s.borrow().last().map(|inner| ReactiveOwner {
                inner: inner.clone(),
            })
        })
    }

    /// Registers an Rc child primitive with this owner.
    /// The child will be dropped when this owner is dropped.
    #[allow(dead_code)]
    pub(crate) fn own<T: 'static>(&self, child: Rc<T>) {
        self.inner
            .children
            .borrow_mut()
            .push(OwnedChild::Rc(Box::new(child)));
    }

    /// Registers an Arc child primitive with this owner.
    /// The child will be dropped when this owner is dropped.
    pub(crate) fn own_arc<T: Send + Sync + 'static>(&self, child: Arc<T>) {
        self.inner
            .children
            .borrow_mut()
            .push(OwnedChild::Arc(Box::new(child)));
    }

    /// Returns the number of owned children (for testing).
    #[cfg(test)]
    pub fn child_count(&self) -> usize {
        self.inner.children.borrow().len()
    }
}

impl Drop for ReactiveOwner {
    fn drop(&mut self) {
        self.inner.children.borrow_mut().clear();
    }
}

/// RAII guard that exits the owner scope on drop.
pub struct OwnerGuard {
    _private: (),
}

impl Drop for OwnerGuard {
    fn drop(&mut self) {
        OWNER_STACK.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

thread_local! {
    static OWNER_STACK: RefCell<Vec<Rc<OwnerInner>>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_scope_entry_exit() {
        assert!(ReactiveOwner::current().is_none());

        let owner = ReactiveOwner::root();
        {
            let _guard = owner.enter();
            assert!(ReactiveOwner::current().is_some());
        }

        assert!(ReactiveOwner::current().is_none());
    }

    #[test]
    fn nested_owners() {
        let outer = ReactiveOwner::root();
        let inner = ReactiveOwner::root();

        let _g1 = outer.enter();
        assert!(ReactiveOwner::current().is_some());

        {
            let _g2 = inner.enter();
            assert!(ReactiveOwner::current().is_some());
        }

        assert!(ReactiveOwner::current().is_some());
    }

    #[test]
    fn panic_safety() {
        use std::panic::AssertUnwindSafe;

        let owner = ReactiveOwner::root();

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = owner.enter();
            panic!("intentional panic");
        }));

        assert!(result.is_err());
        assert!(ReactiveOwner::current().is_none());
    }
}
