//! Logical-pixel unit type.
//!
//! `Lpx` is a `#[repr(transparent)]` newtype over `f32` that marks a value as
//! a *logical pixel* on the scene wire format. Its purpose is to make
//! "physical pixel" and "logical pixel" non-interchangeable at the type level,
//! so future authors cannot accidentally pre-multiply by `scale_factor` at an
//! element callsite and feed physical pixels into the scene buffer (the
//! pre-existing bug fixed by the surrounding plan).
//!
//! # Layout & bytemuck
//!
//! `Lpx` is `#[repr(transparent)]` over `f32` and derives `Pod + Zeroable`.
//! `bytemuck::cast_slice::<Lpx, f32>` is therefore valid and zero-cost, which
//! is required by `RectInstance.rect: [Lpx; 4]` and friends being uploaded
//! to wgpu instance buffers via `bytemuck::cast_slice`.
//!
//! # Construction & conversion
//!
//! Construction is deliberately explicit: `Lpx(x)`. There is **no**
//! `From<f32> for Lpx` impl — implicit promotion would defeat the foot-gun
//! purpose. The reverse direction is cheap: `f32::from(Lpx(x))` (or
//! `lpx.into()`) unwraps.
//!
//! ```compile_fail
//! # use slate_renderer::Lpx;
//! // This must not compile: implicit f32 -> Lpx is forbidden.
//! let _: Lpx = 1.0_f32.into();
//! ```
//!
//! ```compile_fail
//! # use slate_renderer::Lpx;
//! // Same: explicit `From::from` is also absent.
//! let _ = Lpx::from(1.0_f32);
//! ```
//!
//! # Arithmetic
//!
//! Only the operations actually needed by callsites are implemented:
//! `Add/Sub<Lpx>`, `Mul/Div<f32>`, `Neg`. Mixed arithmetic with bare `f32`
//! (other than scalar multiply/divide) is intentionally not supported.

use std::ops::{Add, Div, Mul, Neg, Sub};

use bytemuck::{Pod, Zeroable};

/// A value in logical pixels (lpx). 1 lpx = 1 AppKit pt = 1 Windows DIP.
///
/// See module-level docs for layout, construction, and arithmetic rules.
#[repr(transparent)]
#[derive(Copy, Clone, Debug, Default, PartialEq, PartialOrd, Pod, Zeroable)]
pub struct Lpx(pub f32);

impl Lpx {
    /// The additive identity: `Lpx(0.0)`.
    pub const ZERO: Lpx = Lpx(0.0);
}

impl Add for Lpx {
    type Output = Lpx;
    #[inline]
    fn add(self, rhs: Lpx) -> Lpx {
        Lpx(self.0 + rhs.0)
    }
}

impl Sub for Lpx {
    type Output = Lpx;
    #[inline]
    fn sub(self, rhs: Lpx) -> Lpx {
        Lpx(self.0 - rhs.0)
    }
}

impl Mul<f32> for Lpx {
    type Output = Lpx;
    #[inline]
    fn mul(self, rhs: f32) -> Lpx {
        Lpx(self.0 * rhs)
    }
}

impl Div<f32> for Lpx {
    type Output = Lpx;
    #[inline]
    fn div(self, rhs: f32) -> Lpx {
        Lpx(self.0 / rhs)
    }
}

impl Neg for Lpx {
    type Output = Lpx;
    #[inline]
    fn neg(self) -> Lpx {
        Lpx(-self.0)
    }
}

impl From<Lpx> for f32 {
    #[inline]
    fn from(v: Lpx) -> f32 {
        v.0
    }
}
