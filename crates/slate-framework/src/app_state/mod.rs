//! Shared application state for event handler and resize callback.
//!
//! `AppState<V>` holds all RefCell-wrapped state that was previously captured
//! by the `App::run` closure. Wrapping in `Rc<AppState<V>>` allows both the
//! event handler and the sync resize callback to share the same state.
//!
//! # Borrow Order (ADR-001)
//! Fields are borrowed in a fixed order to avoid deadlock:
//! 1. view  2. layout_tree  3. text_system  4. hit_test_list
//! 5. a11y_nodes  6. scene  7. renderer  8. state_registry  9. text_shaping_cache
//!
//! Each borrow is released before the next begins. Do NOT hold multiple borrows
//! simultaneously unless they are proven non-conflicting.

#![allow(dead_code)] // Some methods unused until callers in later layers wire them

mod dispatch;
mod focus;
mod guards;
mod lifecycle;
mod render;
mod state;
#[cfg(any(test, feature = "test-hooks"))]
mod test_hooks;
mod types;
pub mod window_state;

pub use state::AppState;
pub use types::{AppSignal, DeviceLossReason};
#[cfg(any(test, feature = "test-hooks"))]
pub use types::RecoveryState;
#[allow(unused_imports)]
pub use window_state::WindowState;
