//! Built-in element implementations.
//!
//! - `Div` — flexbox container element
//! - `Text` — text label element
//! - `Image` — GPU-rendered image element
//! - `Overlay` — content painted above the tree, anchored to a target rect
//! - `ScrollView` — clipped, vertically scrollable container
//! - `TextField` — single-line editable text input with IME support

/// **Throwaway** — the S0 DataGrid accessibility spike element. Deliberately
/// reachable (so the spike example can drive it) but NOT re-exported at
/// `elements::` or the crate root: it is not part of the public widget set and
/// is deleted once the real `DataGrid` adopts its verified role/nav pattern.
pub mod data_grid_a11y_spike;
pub mod div;
pub mod image;
pub mod overlay;
pub mod scroll_view;
pub mod text;
pub mod text_area;
pub mod text_edit;
pub mod text_field;

// Form-control widget set (P5a foundation). Pure interaction-state + theme
// consumers; no overlay/scroll dependency.
pub mod button;
pub mod checkbox;
mod control_paint;
pub mod slider;
pub mod switch;

pub use button::{Button, IconButton};
pub use checkbox::Checkbox;
pub use div::Div;
pub use image::{Image, MAX_IMAGE_DIM};
pub use overlay::{Align, Overlay, Placement, Side};
pub use scroll_view::ScrollView;
pub use slider::Slider;
pub use switch::Switch;
pub use text::{Text, TextAlign, TextWrap};
pub use text_area::{TextArea, TextAreaStyle};
pub use text_field::{TextField, TextFieldStyle};
