//! Fails loudly when the suite is run without the `test-hooks` feature.
//!
//! The platform dispatch/redraw suites require `test-hooks` and are otherwise
//! silently skipped — a green `cargo test` then proves nothing about them. This
//! sentinel turns that silent skip into a hard failure so the gated suites
//! cannot rot unnoticed.

#[test]
fn test_hooks_feature_must_be_enabled() {
    // Runtime failure (not a const-block assert) so a feature-less `cargo check`
    // still compiles; only the test run fails when the gate is off.
    if !cfg!(feature = "test-hooks") {
        panic!(
            "the test-hooks feature is off, so the gated platform suites were skipped; \
             re-run with: cargo test -p slate-platform --features test-hooks"
        );
    }
}
