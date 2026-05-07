//! Shared test harness for slate-text visual regression testing.

#![allow(dead_code)]

pub mod golden;
pub mod headless;
pub mod mock;

pub use golden::{assert_alpha_matches_golden, assert_rgba_matches_golden, update_golden_mode};
pub use headless::make_headless_device;
