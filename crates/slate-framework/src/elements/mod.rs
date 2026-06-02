//! Built-in element implementations.
//!
//! - `Div` — flexbox container element
//! - `Text` — text label element
//! - `Image` — GPU-rendered image element
//! - `Overlay` — content painted above the tree, anchored to a target rect
//! - `ScrollView` — clipped, vertically scrollable container
//! - `TextField` — single-line editable text input with IME support

/// The P5e DataGrid widget (2-D, virtualized, screen-reader-usable). Registers
/// a11y nodes for all logical cells while shaping glyph content only for the
/// visible window; verified by `tests/widget_a11y_roles.rs` +
/// `tests/grid_a11y_tree_shape.rs`.
pub mod data_grid;
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
pub mod list;
mod roving;
pub mod slider;
pub mod switch;
pub mod tree;

// Layout-container widget set (P5b dashboard shell). Thin Div compositions +
// the resizable Splitter.
pub mod panel;
pub mod splitter;
pub mod status_bar;
pub mod toolbar;

// Overlay-based widget set (P5c). All consume the Overlay layer + the shared
// MenuList core.
pub mod context_menu;
pub mod menu;
pub mod select;
pub mod tooltip;

pub use button::{Button, IconButton};
pub use checkbox::Checkbox;
pub use context_menu::ContextMenu;
pub use data_grid::{Column, DataGrid};
pub use div::{Border, Div};
pub use list::{List, ListEntry, VirtualList};
pub use menu::{MenuEntry, MenuList};
pub use select::Select;
pub use tooltip::Tooltip;
pub use panel::Panel;
pub use splitter::{SplitAxis, Splitter};
pub use status_bar::StatusBar;
pub use toolbar::Toolbar;
pub use tree::{Tree, TreeNode};
pub use image::{Image, MAX_IMAGE_DIM};
pub use overlay::{Align, Overlay, Placement, Side};
pub use scroll_view::ScrollView;
pub use slider::Slider;
pub use switch::Switch;
pub use text::{Text, TextAlign, TextWrap};
pub use text_area::{TextArea, TextAreaStyle};
pub use text_field::{TextField, TextFieldStyle};
