//! Shared editing core for the single-line `TextField` and the multi-line
//! `TextArea`.
//!
//! These submodules hold the editing primitives that are identical regardless
//! of layout: grapheme-aware caret motion, the undo/redo coalesce stack,
//! selection-aware editing ops over [`ImeState`](crate::ime::ImeState),
//! clipboard copy/cut/paste (parameterized by a `multiline` flag), and the
//! caret-blink advance. `TextField` consumes these directly; `TextArea` reuses
//! the same primitives and layers multi-line specialization on top.
//!
//! The split is behavior-preserving: every function here was moved verbatim
//! out of `text_field/` and is exercised by the same test suite.

pub mod undo;

pub(crate) mod blink;
pub(crate) mod clipboard;
pub(crate) mod grapheme;
pub(crate) mod ops;
pub(crate) mod shortcuts;
