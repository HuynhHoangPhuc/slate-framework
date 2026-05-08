//! slate-framework — GPU-accelerated Rust UI framework.
//!
//! This crate provides the Element-based UI framework built on top of
//! `slate-platform` (windowing), `slate-renderer` (GPU rendering), and
//! `slate-text` (text shaping/rasterization).
//!
//! # Phase 3 Capabilities
//!
//! - Three-phase Element lifecycle (request_layout/prepaint/paint)
//! - Taffy-based Flexbox/Grid layout
//! - Built-in elements: `Div` (container), `Text` (label)
//! - Type-erased element trees via `AnyElement`
//! - Platform-dispatching `TextSystem`
//!
//! # Example
//!
//! ```ignore
//! use slate_framework::{Div, Text, Edges};
//!
//! let ui = Div::new()
//!     .background([0.1, 0.1, 0.1, 1.0])
//!     .padding(Edges::all(16.0))
//!     .child(Text::new("Hello, Slate!").color([1.0, 1.0, 1.0, 1.0]));
//! ```

// Core modules
pub mod app;
pub mod color;
pub mod context;
pub mod element;
pub mod elements;
pub mod executor;
pub mod headless;
pub mod hit_test;
pub mod layout;
pub(crate) mod reactive_state;
pub mod style;
pub mod text_system;
pub mod types;
pub mod view;

// Re-export underlying crates
pub use slate_platform;
pub use slate_platform::WindowOptions;
pub use slate_renderer;
pub use slate_text;

// Re-export core types
pub use app::App;
pub use headless::{HeadlessApp, HeadlessError};
pub use color::Color;
pub use context::{LayoutCtx, PaintCtx, PrepaintCtx};
pub use element::{AnyElement, Element, IntoElement};
pub use elements::{Div, Text, TextAlign, TextWrap};
pub use view::{IntoAny, View};
pub use executor::{BackgroundExecutor, Executor, ForegroundExecutor, RedrawRequester};
pub use hit_test::{CursorStyle, HitRegion, HitTestList, HitTestResult};
pub use layout::{compute_layout, resolve_bounds, resolve_child_bounds, LayoutTree};
pub use style::{DisplayMode, Length, Overflow, Position, SizeConstraint, Style};
pub use text_system::{PlatformFont, TextSystem};
pub use types::{
    AccessibilityInfo, AccessibilityNode, AccessibilityRole, Bounds, Edges, ElementId, LayoutId,
    LiveRegion, NodeContext, Point, Size,
};

// Note: TaffyTree is NOT re-exported — internal modules import taffy::TaffyTree directly.
// This hides the layout tree implementation from external consumers.

/// Curated re-exports of Taffy layout enums used by Slate styles.
///
/// These are the public layout primitives — `TaffyTree` itself is hidden.
/// TODO(v1.1): mirror as Slate-owned enums + From<TaffyEnum> impls to enable backend swap.
pub mod layout_types {
    pub use taffy::{AlignItems, FlexDirection, FlexWrap, JustifyContent};
}

// Re-export layout enums at crate root for convenience
pub use layout_types::{AlignItems, FlexDirection, FlexWrap, JustifyContent};
