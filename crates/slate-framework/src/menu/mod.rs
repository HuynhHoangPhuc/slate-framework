//! Declarative native-menu model and action routing.
//!
//! This module is the framework-side, platform-agnostic half of native menus
//! (menu bar + OS context menu). It is **distinct** from the in-canvas
//! [`MenuList`](crate::MenuList)/[`ContextMenu`](crate::ContextMenu) overlay
//! widgets: those render inside the GPU canvas for in-canvas affordances, while
//! this module describes *OS-native* menus that the per-platform backends
//! (P6b macOS `NSMenu`, P6c Windows `HMENU`) build from a lowered
//! [`PlatformMenu`](slate_platform::PlatformMenu). One canonical path per use —
//! do not route native menus through the in-canvas widgets or vice-versa.
//!
//! # Shape
//!
//! - [`Menu`] / [`MenuItem`] / [`MenuAction`] / [`SubMenu`]: the closure-free,
//!   `Clone` model (see [`model`]).
//! - [`MenuId`] + [`Accelerator`]: stable routing id and neutral key equivalent.
//! - [`MenuRegistry`]: maps ids to action handlers (see [`routing`]).
//!
//! Build a [`Menu`], register handlers by [`MenuId`], install it via
//! [`AppContext::set_menu`](crate::AppContext::set_menu) /
//! [`AppContext::on_menu_action`](crate::AppContext::on_menu_action).

mod lower;
pub mod model;
pub mod routing;

pub use model::{Accelerator, Menu, MenuAction, MenuId, MenuItem, SubMenu};
pub use routing::{MenuHandler, MenuRegistry};
