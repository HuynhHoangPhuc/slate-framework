//! Reactive primitives for Slate UI framework.
//!
//! This crate provides the core reactive system: `Signal`, `Memo`, `Effect`,
//! and the `Runtime` that coordinates them. It has no UI dependencies.
//!
//! # Core Primitives
//!
//! - `Signal<T>`: reactive value container (Send + Sync)
//! - `Memo<T>`: cached derived value with lazy recomputation
//! - `Effect`: UI-thread side effect (!Send)
//! - `ReactiveOwner`: scope-based cleanup for Memo/Effect
//!
//! # Runtime
//!
//! - `Runtime`: dirty bit, redraw hook, ID allocation, observer dispatch
//! - `with_observer` / `current_observer`: thread-local observer scope

mod effect;
mod ids;
mod memo;
mod observer;
mod owner;
mod runtime;
mod signal;

pub use effect::Effect;
pub use ids::{EffectId, ObserverId, SignalId};
pub use memo::Memo;
pub use observer::{current_observer, with_observer};
pub use owner::{OwnerGuard, ReactiveOwner};
pub use runtime::Runtime;
pub use signal::Signal;

#[allow(unused_imports)] // Used internally by memo module
pub(crate) use observer::observer_stack_depth;
#[allow(unused_imports)] // Re-exported for internal use
pub(crate) use runtime::{MemoNotify, ObserverKind};
