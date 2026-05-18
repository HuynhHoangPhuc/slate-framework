//! Phase 1 tests for the `Lpx` newtype.
//!
//! These tests are written first (TDD) and lock in the contract:
//! - `Lpx` is `#[repr(transparent)]` over `f32` with identical size/alignment.
//! - `bytemuck::cast_slice::<Lpx, f32>` round-trips bit-identically (required
//!   for Phase 2's `*Instance.rect: [Lpx; 4]` Pod casts).
//! - Arithmetic ops compose cleanly without round-trip through raw `f32`.
//! - `Lpx -> f32` is a cheap unwrap; the reverse direction is intentionally
//!   *absent* to prevent the foot-gun that the type exists to fix.

use slate_renderer::Lpx;

#[test]
fn lpx_size_and_alignment_match_f32() {
    assert_eq!(
        core::mem::size_of::<Lpx>(),
        core::mem::size_of::<f32>(),
        "Lpx must be transparent over f32"
    );
    assert_eq!(
        core::mem::align_of::<Lpx>(),
        core::mem::align_of::<f32>(),
        "Lpx must align identically to f32"
    );
}

#[test]
fn lpx_cast_slice_to_f32_is_bit_identical() {
    let lpx = [Lpx(1.5), Lpx(2.5), Lpx(-3.25)];
    let raw: &[f32] = bytemuck::cast_slice(&lpx);
    assert_eq!(raw, &[1.5_f32, 2.5, -3.25]);
}

#[test]
fn lpx_addition() {
    assert_eq!(Lpx(2.0) + Lpx(3.0), Lpx(5.0));
}

#[test]
fn lpx_subtraction() {
    assert_eq!(Lpx(5.0) - Lpx(2.0), Lpx(3.0));
}

#[test]
fn lpx_scalar_multiplication() {
    assert_eq!(Lpx(4.0) * 0.5, Lpx(2.0));
}

#[test]
fn lpx_scalar_division() {
    assert_eq!(Lpx(4.0) / 2.0, Lpx(2.0));
}

#[test]
fn lpx_negation() {
    assert_eq!(-Lpx(1.0), Lpx(-1.0));
}

#[test]
fn lpx_into_f32() {
    let x: f32 = Lpx(7.0).into();
    assert_eq!(x, 7.0);
}

#[test]
fn lpx_zero_constant() {
    assert_eq!(Lpx::ZERO, Lpx(0.0));
}

#[test]
fn lpx_default_is_zero() {
    assert_eq!(Lpx::default(), Lpx(0.0));
}

#[test]
fn lpx_zeroable_byte_pattern() {
    let z: Lpx = bytemuck::Zeroable::zeroed();
    assert_eq!(z, Lpx(0.0));
}

/// `Lpx` deliberately does NOT implement `From<f32>` — that conversion is the
/// foot-gun the type exists to prevent. The corresponding compile-fail test
/// lives as a doc-test on the `Lpx` type itself (see `units.rs`).
#[test]
fn lpx_construction_is_explicit() {
    let _ = Lpx(1.0); // explicit tuple construction is the only path
}
