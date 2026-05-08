//! Built-in element implementations.
//!
//! Phase 2 provides:
//! - `Div` — flexbox container element
//! - `Text` — text label element

pub mod div;
pub mod text;

pub use div::Div;
pub use text::{Text, TextAlign, TextWrap};
