//! The reference ellipsoid and a deterministic inverse tangent, copied
//! from the terrain compiler's module so a vertex's latitude and longitude
//! are bit for bit the terrain module's.
//!
//! The biome data is laid on WGS84 (`client-compute.md` section 10: an
//! overlay colours the frame's surface tiles, `matter-format.md` section
//! 10.1), semi-major axis [`A`] = 6378137 m and flattening [`F`] =
//! 1 / 298.257223563 (NIMA TR8350.2, third edition, 2000).
//!
//! # Determinism
//!
//! `gx_core::detmath` has no inverse tangent, so [`atan`] and [`atan2`] are
//! written out with `+`, `-`, `*`, `/`, and `sqrt` only: two exact
//! half-angle reductions and a fixed odd polynomial. Every operation is
//! correctly rounded in IEEE 754 and none is fused, so the bits are the
//! same natively and in WebAssembly.

use core::f64::consts::{FRAC_PI_2, PI};

/// WGS84 semi-major (equatorial) axis, meters.
pub const A: f64 = 6_378_137.0;

/// WGS84 flattening.
pub const F: f64 = 1.0 / 298.257_223_563;

/// WGS84 semi-minor (polar) axis, meters: `A (1 - F)`.
pub const B: f64 = A * (1.0 - F);

/// First eccentricity squared: `F (2 - F)`.
pub const E2: f64 = F * (2.0 - F);

/// The inverse tangent, radians, from `+`, `-`, `*`, `/`, and `sqrt` only.
/// Odd; within a few units in the last place of the true value.
pub fn atan(x: f64) -> f64 {
    let a = x.abs();
    let r = if a <= 1.0 {
        atan_unit(a)
    } else {
        FRAC_PI_2 - atan_unit(1.0 / a)
    };
    if x < 0.0 {
        -r
    } else {
        r
    }
}

/// The angle of the vector `(x, y)` from the positive `x` axis, radians,
/// `-pi` to `pi`, from `+`, `-`, `*`, `/`, and `sqrt` only. `(0, 0)` gives
/// 0. A negative zero `y` is treated as positive.
pub fn atan2(y: f64, x: f64) -> f64 {
    let (ax, ay) = (x.abs(), y.abs());
    if ax == 0.0 && ay == 0.0 {
        return 0.0;
    }
    let a = if ay <= ax {
        atan_unit(ay / ax)
    } else {
        FRAC_PI_2 - atan_unit(ax / ay)
    };
    let a = if x < 0.0 { PI - a } else { a };
    if y < 0.0 {
        -a
    } else {
        a
    }
}

/// `atan(t)` for `t` from 0 to 1.
///
/// Two half-angle steps, `atan(t) = 2 atan(t / (1 + sqrt(1 + t^2)))`, bring
/// the argument below `tan(pi/16)` (about 0.199). There the Taylor series
/// to the `t^27` term is exact to below a part in `10^17`.
fn atan_unit(t: f64) -> f64 {
    let t = t / (1.0 + (1.0 + t * t).sqrt());
    let t = t / (1.0 + (1.0 + t * t).sqrt());
    let t2 = t * t;
    // Horner over the coefficients (-1)^k / (2k + 1), k = 13 down to 0.
    let mut s = 0.0f64;
    for k in (0..14u32).rev() {
        let c = 1.0 / f64::from(2 * k + 1);
        let c = if k % 2 == 1 { -c } else { c };
        s = c + t2 * s;
    }
    4.0 * (t * s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atan_matches_the_platform_closely() {
        let mut worst = 0.0f64;
        for i in -4000..=4000 {
            let x = f64::from(i) * 0.0037;
            let d = (atan(x) - x.atan()).abs();
            worst = worst.max(d / x.atan().abs().max(1e-300));
        }
        assert!(worst < 1e-15, "{worst}");
        for &x in &[1e-300, 1e-8, 0.5, 1.0, 2.0, 1e8, 1e300, f64::INFINITY] {
            assert!((atan(x) - x.atan()).abs() <= 4.0 * f64::EPSILON * x.atan().abs());
        }
        assert_eq!(atan(0.0), 0.0);
    }

    #[test]
    fn atan2_quadrants() {
        for i in 0..360 {
            let a = (f64::from(i) + 0.5) * PI / 180.0 - PI;
            let (y, x) = (a.sin() * 3.0, a.cos() * 3.0);
            assert!((atan2(y, x) - y.atan2(x)).abs() < 1e-15, "{i}");
        }
        assert_eq!(atan2(0.0, 0.0), 0.0);
        assert_eq!(atan2(0.0, -1.0), PI);
        assert_eq!(atan2(1.0, 0.0), FRAC_PI_2);
        assert_eq!(atan2(-1.0, 0.0), -FRAC_PI_2);
    }
}
