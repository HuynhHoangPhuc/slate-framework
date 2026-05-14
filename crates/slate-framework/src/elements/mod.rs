//! Built-in element implementations.
//!
//! Phase 2 provides:
//! - `Div` — flexbox container element
//! - `Text` — text label element
//! - `Image` — GPU-rendered image element

pub mod div;
pub mod image;
pub mod text;

pub use div::Div;
pub use image::{Image, MAX_IMAGE_DIM};
pub use text::{Text, TextAlign, TextWrap};
