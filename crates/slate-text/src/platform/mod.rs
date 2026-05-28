//! Platform-specific text backend implementations.
//!
//! These submodules are crate-internal; backend types are re-exported at the
//! `slate_text` crate root (`DirectWriteBackend`, `CoreTextBackend`).

#[cfg(target_os = "macos")]
pub(crate) mod macos;

#[cfg(target_os = "windows")]
pub(crate) mod windows;
