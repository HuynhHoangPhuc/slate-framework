#![deny(missing_docs)]

//! slate-framework — GPU-accelerated Rust UI framework.
//!
//! This crate provides the Element-based UI framework built on top of
//! `slate-platform` (windowing), `slate-renderer` (GPU rendering), and
//! `slate-text` (text shaping/rasterization).
//!
//! # Capabilities
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
pub mod a11y_accesskit;
pub mod app;
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub mod app_state;
#[cfg(not(any(test, feature = "test-hooks")))]
pub(crate) mod app_state;

// Re-export device-loss reason classification so integration tests can
// match on it without enabling the `test-hooks` feature.
pub use app_state::DeviceLossReason;
pub mod color;
pub mod context;
pub mod element;
pub mod elements;
pub mod erased_view;
pub mod event;
pub mod executor;
pub mod focus;
pub(crate) mod focus_ring;
pub mod headless;
pub mod hit_test;
pub(crate) mod image_cache;
pub mod ime;
pub mod interaction_state;
pub mod layout;
pub(crate) mod paint_cache;
#[cfg(feature = "profiling")]
pub mod profiling;
pub(crate) mod reactive_state;
pub mod reactive_value;
pub mod render_cx;
pub mod style;
pub mod theme;
#[cfg(all(target_os = "windows", feature = "test-hooks"))]
#[doc(hidden)]
pub mod test_support;
pub mod text_system;
pub mod types;
pub mod view;

// Re-export underlying crates
pub use slate_platform;
pub use slate_platform::{Key, KeyCode, Modifiers, MouseButton, NamedKey, WindowOptions};
pub use slate_renderer;
pub use slate_text;

// Re-export core types
pub use app::{App, AppContext};
pub use color::Color;
pub use event::{
    EventCtx, KeyEvent, KeyHandler, MouseEvent, PointerEvent, PointerEventKind, ScrollEvent,
    TextInputEvent, TextInputHandler,
};
pub use headless::{HeadlessApp, HeadlessError};
pub use interaction_state::StateStyle;
pub use reactive_value::Reactive;
pub use render_cx::RenderCx;

/// Reactive primitives for building reactive UIs.
///
/// # Example
///
/// ```ignore
/// use slate_framework::reactive::Signal;
///
/// let count = Signal::new(cx.runtime(), 0u32);
/// let c = count.clone();
/// Text::new_reactive(move || format!("Count: {}", c.get()))
/// ```
pub mod reactive {
    pub use slate_reactive::{Effect, Memo, ReactiveOwner, Signal};
}

// Re-export smol::Timer for async timing.
pub use context::{LayoutCtx, PaintCtx, PrepaintCtx};
pub use element::{AnyElement, Element, IntoElement};
/// Axis-aligned clip rectangle (logical px); appears in [`PaintCtx::push_clip`].
pub use slate_renderer::ClipRect;
pub use elements::{
    Align, Div, Image, MAX_IMAGE_DIM, Overlay, Placement, ScrollView, Side, Text, TextAlign,
    TextArea, TextAreaStyle, TextField, TextFieldStyle, TextWrap,
};
pub use executor::{BackgroundExecutor, Executor, ForegroundExecutor, RedrawRequester};
pub use focus::{FocusRegistry, FocusableEntry};
pub use hit_test::{CursorStyle, HitRegion, HitTestList, HitTestResult};
pub use ime::{CachedImeQuery, ImeRegistry, ImeState, PendingImeOp, Preedit};
pub use layout::{LayoutTree, compute_layout, resolve_bounds, resolve_child_bounds};
pub use smol::Timer;
pub use style::{DisplayMode, Length, Overflow, Position, SizeConstraint, Style};
pub use theme::{Radii, Spacing, Theme, ThemeMode, ThemeSet, TypeStyle, Typography, theme};
pub use text_system::{PlatformFont, TextSystem};
pub use types::{
    AccessibilityAction, AccessibilityInfo, AccessibilityNode, AccessibilityRelationships,
    AccessibilityRole, Bounds, Edges, ElementId, LayoutId, LiveRegion, NodeContext, Point, Size,
};
pub use view::{IntoAny, View};

// Note: TaffyTree is NOT re-exported — internal modules import taffy::TaffyTree directly.
// This hides the layout tree implementation from external consumers.

/// Curated re-exports of Taffy layout enums used by Slate styles.
///
/// These are the public layout primitives — `TaffyTree` itself is hidden.
/// TODO(v1.1): mirror as Slate-owned enums + `From<TaffyEnum>` impls to enable backend swap.
pub mod layout_types {
    pub use taffy::{AlignItems, FlexDirection, FlexWrap, JustifyContent};
}

// Re-export layout enums at crate root for convenience
pub use layout_types::{AlignItems, FlexDirection, FlexWrap, JustifyContent};
