//! Mouse event dispatch.
//!
//! Submodules:
//! - `button`: mouse-down / mouse-up / capture-lost dispatchers.
//! - `motion`: mouse-move / mouse-scrolled / hover diff / coalesced flush.
//! - `helpers`: shared chain-walking primitives (`ancestors`, `bubble_*`,
//!   `fire_hover_transitions`, `button_to_bit`) consumed by both dispatcher
//!   siblings via `use super::helpers::{...}`.

mod button;
mod helpers;
mod motion;
