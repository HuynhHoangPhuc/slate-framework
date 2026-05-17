//! Built-in element implementations.
//!
//! Phase 2 provides:
//! - `Div` — flexbox container element
//! - `Text` — text label element
//! - `Image` — GPU-rendered image element
//!
//! Phase 5 adds:
//! - `TextField` — single-line editable text input with IME support

pub mod div;
pub mod image;
pub mod text;
pub mod text_field;

pub use div::Div;
pub use image::{Image, MAX_IMAGE_DIM};
pub use text::{Text, TextAlign, TextWrap};
pub use text_field::{TextField, TextFieldStyle};
