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
pub mod context;
pub mod element;
pub mod elements;
pub mod executor;
pub mod text_system;
pub mod types;

// Re-export underlying crates
pub use slate_platform;
pub use slate_renderer;
pub use slate_text;

// Re-export core types
pub use context::{LayoutCtx, PaintCtx, PrepaintCtx};
pub use element::{AnyElement, Element, IntoElement};
pub use elements::{Div, Text};
pub use executor::{ForegroundExecutor, RedrawRequester};
pub use text_system::{PlatformFont, TextSystem};
pub use types::{
    AccessibilityInfo, AccessibilityNode, AccessibilityRole, Bounds, Edges, ElementId, HitRegion,
    LayoutId, NodeContext, Point, Size,
};

// Re-export Taffy types commonly used with Elements
pub use taffy::{
    AlignItems, Display, FlexDirection, JustifyContent, Style as TaffyStyle,
    TaffyTree,
};
