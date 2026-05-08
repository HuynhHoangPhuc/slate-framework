use std::cell::RefCell;

use crate::ids::ObserverId;

thread_local! {
    static OBSERVER_STACK: RefCell<Vec<ObserverId>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard that pops the observer from the stack on drop.
/// Ensures panic-safe cleanup.
struct ObserverGuard;

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        OBSERVER_STACK.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

/// Executes a closure within an observer scope.
///
/// Pushes `id` onto the thread-local observer stack before running `f`,
/// and pops it after `f` completes (even on panic).
///
/// Signals read via `.get()` during this scope will auto-subscribe
/// the observer identified by `id`.
#[inline]
pub fn with_observer<R>(id: ObserverId, f: impl FnOnce() -> R) -> R {
    OBSERVER_STACK.with(|s| s.borrow_mut().push(id));
    let _guard = ObserverGuard;
    f()
}

/// Returns the currently active observer, if any.
///
/// Returns `None` if called outside any `with_observer` scope.
#[inline]
pub fn current_observer() -> Option<ObserverId> {
    OBSERVER_STACK.with(|s| s.borrow().last().copied())
}

/// Returns the current depth of the observer stack.
/// Used for recursion guards in Memo (phase 3).
#[inline]
#[allow(dead_code)] // Used in phase 3
pub(crate) fn observer_stack_depth() -> usize {
    OBSERVER_STACK.with(|s| s.borrow().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_observers() {
        let a = ObserverId::new(1);
        let b = ObserverId::new(2);

        assert_eq!(current_observer(), None);

        with_observer(a, || {
            assert_eq!(current_observer(), Some(a));

            with_observer(b, || {
                assert_eq!(current_observer(), Some(b));
            });

            assert_eq!(current_observer(), Some(a));
        });

        assert_eq!(current_observer(), None);
    }

    #[test]
    fn panic_safety() {
        let a = ObserverId::new(1);

        let result = std::panic::catch_unwind(|| {
            with_observer(a, || {
                assert_eq!(current_observer(), Some(a));
                panic!("intentional panic");
            });
        });

        assert!(result.is_err());
        assert_eq!(current_observer(), None);
    }

    #[test]
    fn stack_depth() {
        let a = ObserverId::new(1);
        let b = ObserverId::new(2);

        assert_eq!(observer_stack_depth(), 0);

        with_observer(a, || {
            assert_eq!(observer_stack_depth(), 1);

            with_observer(b, || {
                assert_eq!(observer_stack_depth(), 2);
            });

            assert_eq!(observer_stack_depth(), 1);
        });

        assert_eq!(observer_stack_depth(), 0);
    }
}
