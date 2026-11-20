//! Reading land fraction and albedo off a level: bilinear, land weighted,
//! across tile edges.
//!
//! The biome module reads its source tiles through [`sample`]
//! (`client-compute.md` section 4, `gx_compile` receives the tiles
//! `gx_sources` asked for, and section 10, overlay layers). The caller
//! supplies the tiles, normally the ones [`Grid::tiles_for_region`] or
//! [`Grid::bilinear_tiles`] listed; a tile the stencil needs and the caller
//! did not supply is an error, never a guess. Stencils that reach past 180
//! degrees wrap and stencils that reach past a pole are clamped, as
//! [`Grid::lattice_col`] and [`Grid::lattice_row`] describe.
//!
//! The arithmetic is `+`, `-`, `*`, `/`, and `floor` on `f64`, written as
//! separate operations (Rust never fuses them), so the result is bitwise the
//! same natively and in WebAssembly.

use std::collections::BTreeMap;

use crate::grid::Grid;
use crate::tile::{Tile, TileError, BANDS};

/// Where a sampler gets tiles from.
pub trait TileSource {
    /// The tile with this id, or `None` if the caller did not supply it.
    fn tile(&self, id: u64) -> Option<&Tile>;
}

impl TileSource for BTreeMap<u64, Tile> {
    fn tile(&self, id: u64) -> Option<&Tile> {
        self.get(&id)
    }
}

impl TileSource for [Tile] {
    fn tile(&self, id: u64) -> Option<&Tile> {
        self.iter().find(|t| t.id() == id)
    }
}

impl TileSource for Vec<Tile> {
    fn tile(&self, id: u64) -> Option<&Tile> {
        self.as_slice().tile(id)
    }
}

/// Land fraction and albedo at one point.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Sample {
    /// Land fraction, 0 (ocean) to 1 (land).
    pub land: f64,
    /// Albedo of the land part, reflectance 0 to 1, per band from long to
    /// short wavelength. 0 where `land` is 0.
    pub albedo: [f64; BANDS],
}

/// The land byte and albedo bytes of lattice sample index `(r, c)` on
/// `level`, after wrapping and clamping.
pub fn sample_at<S: TileSource + ?Sized>(
    grid: &Grid,
    tiles: &S,
    level: u8,
    r: i64,
    c: i64,
) -> Result<(u8, [u8; BANDS]), TileError> {
    let (key, index) = grid.locate(
        level,
        grid.lattice_row(level, r),
        grid.lattice_col(level, c),
    );
    let id = key.id();
    let tile = tiles.tile(id).ok_or(TileError::Missing(id))?;
    Ok((tile.land(index), tile.albedo(index)))
}

/// Land fraction and albedo at latitude `lat` and longitude `lon` (radians)
/// on `level`, bilinear over the four nearest sample centers.
///
/// With weights `w_i` and land fractions `l_i` of the four samples, land is
/// `sum(w_i l_i)` and each albedo band `sum(w_i l_i a_i) / sum(w_i l_i)`, 0
/// when `sum(w_i l_i)` is 0, so ocean samples never tint a coast.
pub fn sample<S: TileSource + ?Sized>(
    grid: &Grid,
    tiles: &S,
    level: u8,
    lat: f64,
    lon: f64,
) -> Result<Sample, TileError> {
    let p = grid.position(level, lat, lon);
    let (fy, fx) = (p.y.floor(), p.x.floor());
    let (ty, tx) = (p.y - fy, p.x - fx);
    let (r0, c0) = (fy as i64, fx as i64);
    let wy = [1.0 - ty, ty];
    let wx = [1.0 - tx, tx];
    let mut land = 0.0f64;
    let mut weighted = [0.0f64; BANDS];
    for (i, &y) in wy.iter().enumerate() {
        for (j, &x) in wx.iter().enumerate() {
            let (l, a) = sample_at(grid, tiles, level, r0 + i as i64, c0 + j as i64)?;
            let wl = y * x * (f64::from(l) / 255.0);
            land += wl;
            for (acc, &v) in weighted.iter_mut().zip(a.iter()) {
                *acc += wl * (f64::from(v) / 255.0);
            }
        }
    }
    let mut albedo = [0.0f64; BANDS];
    if land > 0.0 {
        for (out, &acc) in albedo.iter_mut().zip(weighted.iter()) {
            *out = acc / land;
        }
    }
    Ok(Sample { land, albedo })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{TILE_SAMPLES, TILE_SIZE};
    use crate::tile::{TileKey, LAND};
    use core::f64::consts::PI;

    /// A 240 x 480 grid: one tile row, two tile columns on level 0.
    fn grid() -> Grid {
        Grid::new(240, 480).unwrap()
    }

    /// Land and band 0 values of sample `(r, c)`: a pattern with ocean and
    /// partial land.
    fn value(r: u32, c: u32) -> (u8, u8) {
        let land = match (r * 7 + c * 3) % 5 {
            0 => 0,
            1 => 255,
            k => (k * 60) as u8,
        };
        (land, ((r * 13 + c * 29) % 256) as u8)
    }

    fn tiles() -> Vec<Tile> {
        (0..2)
            .map(|col| {
                let mut p = vec![0u8; 4 * TILE_SAMPLES];
                for i in 0..TILE_SIZE {
                    for j in 0..TILE_SIZE {
                        let (l, a) = value(i, col * TILE_SIZE + j);
                        let k = (i * TILE_SIZE + j) as usize;
                        p[k] = a;
                        p[TILE_SAMPLES + k] = 255 - a;
                        p[2 * TILE_SAMPLES + k] = a / 2;
                        p[LAND * TILE_SAMPLES + k] = l;
                    }
                }
                Tile::new(
                    TileKey {
                        level: 0,
                        row: 0,
                        col,
                    },
                    p,
                )
                .unwrap()
            })
            .collect()
    }

    /// The analytic bilinear value at lattice position `(y, x)`, with the
    /// stencil rows `r0, r0 + 1` and columns `c0, c0 + 1` as given.
    fn analytic(y: f64, x: f64, rows: [u32; 2], cols: [u32; 2]) -> (f64, f64) {
        let (ty, tx) = (y - y.floor(), x - x.floor());
        let mut land = 0.0;
        let mut alb = 0.0;
        for (i, &r) in rows.iter().enumerate() {
            for (j, &c) in cols.iter().enumerate() {
                let w = if i == 0 { 1.0 - ty } else { ty } * if j == 0 { 1.0 - tx } else { tx };
                let (l, a) = value(r, c);
                land += w * f64::from(l) / 255.0;
                alb += w * f64::from(l) / 255.0 * f64::from(a) / 255.0;
            }
        }
        (land, if land > 0.0 { alb / land } else { 0.0 })
    }

    fn check(lat_deg: f64, lon_deg: f64, rows: [u32; 2], cols: [u32; 2]) {
        let g = grid();
        let (lat, lon) = (lat_deg * PI / 180.0, lon_deg * PI / 180.0);
        let s = sample(&g, &tiles(), 0, lat, lon).unwrap();
        let p = g.position(0, lat, lon);
        let (land, alb) = analytic(p.y, p.x, rows, cols);
        assert!((s.land - land).abs() < 1e-12, "{} {}", s.land, land);
        assert!((s.albedo[0] - alb).abs() < 1e-12, "{} {}", s.albedo[0], alb);
    }

    #[test]
    fn interior_matches_analytic_bilinear() {
        // Spacing 0.75 degrees. Row 40 center 59.625, column 100 center
        // -104.625.
        check(59.4, -104.3, [40, 41], [100, 101]);
        // Exactly on a sample center: that sample's values.
        let g = grid();
        let (lat, lon) = g.center(0, 40, 100);
        let s = sample(&g, &tiles(), 0, lat, lon).unwrap();
        let (l, a) = value(40, 100);
        assert!((s.land - f64::from(l) / 255.0).abs() < 1e-9);
        if l > 0 {
            assert!((s.albedo[0] - f64::from(a) / 255.0).abs() < 1e-9);
        }
    }

    #[test]
    fn across_a_tile_edge() {
        // Columns 239 (tile 0) and 240 (tile 1), around longitude 0.
        check(10.1, 0.1, [106, 107], [239, 240]);
    }

    #[test]
    fn across_the_antimeridian() {
        // Columns 479 (the last) and 0, from both sides.
        check(-20.2, 179.9, [146, 147], [479, 0]);
        check(-20.2, -179.9, [146, 147], [479, 0]);
    }

    #[test]
    fn ocean_never_tints_and_missing_tiles_are_errors() {
        let g = grid();
        let mut t = tiles();
        // All ocean: albedo 0 whatever the albedo bytes are.
        for tile in &mut t {
            let mut p = tile.planes().to_vec();
            p[LAND * TILE_SAMPLES..].fill(0);
            *tile = Tile::new(tile.key(), p).unwrap();
        }
        let s = sample(&g, &t, 0, 0.3, 0.3).unwrap();
        assert_eq!(
            s,
            Sample {
                land: 0.0,
                albedo: [0.0; 3]
            }
        );
        // One land sample in the stencil gives its own albedo.
        let mut p = t[0].planes().to_vec();
        let (r, c) = (40u32, 100u32);
        p[LAND * TILE_SAMPLES + (r * TILE_SIZE + c) as usize] = 255;
        let one = vec![Tile::new(t[0].key(), p).unwrap(), t[1].clone()];
        let (lat, lon) = g.center(0, r, c);
        let s = sample(&g, &one, 0, lat - 0.001, lon + 0.002).unwrap();
        let a = t[0].albedo((r * TILE_SIZE + c) as usize);
        assert!(s.land > 0.0 && s.land < 1.0);
        assert!((s.albedo[1] - f64::from(a[1]) / 255.0).abs() < 1e-12);
        // Without tile 1, a stencil across the edge fails.
        let e = sample(&g, &t[..1], 0, 0.0, 0.0).unwrap_err();
        assert_eq!(
            e,
            TileError::Missing(
                TileKey {
                    level: 0,
                    row: 0,
                    col: 1
                }
                .id()
            )
        );
    }

    /// A fixed pseudo random sequence in `[0, 1)`.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    #[test]
    fn region_tiles_cover_every_stencil_read() {
        let g = crate::check::GRID;
        let all = crate::check::tiles();
        let d = PI / 180.0;
        let mut rng = Lcg(7);
        // Boxes: random ones, one at each pole, one across 180 degrees, one
        // past -180 degrees, and the whole globe.
        let mut boxes = vec![
            (85.0, 90.0, -30.0, 40.0),
            (-90.0, -88.0, 100.0, 200.0),
            (-5.0, 5.0, 170.0, 190.0),
            (10.0, 12.0, -185.0, -175.0),
            (-90.0, 90.0, -180.0, 180.0),
        ];
        for _ in 0..40 {
            let lat = rng.next() * 180.0 - 90.0;
            let lon = rng.next() * 400.0 - 200.0;
            let (h, w) = (rng.next() * 20.0, rng.next() * 30.0);
            boxes.push((lat, (lat + h).min(90.0), lon, lon + w));
        }
        for level in g.levels() {
            for &(la0, la1, lo0, lo1) in &boxes {
                let ids = g.tiles_for_region(level, la0 * d, la1 * d, lo0 * d, lo1 * d);
                let only: BTreeMap<u64, Tile> =
                    ids.iter().map(|id| (*id, all[id].clone())).collect();
                let mut points = vec![(la0, lo0), (la1, lo1), (la0, lo1), (la1, lo0)];
                for _ in 0..200 {
                    points.push((
                        la0 + (la1 - la0) * rng.next(),
                        lo0 + (lo1 - lo0) * rng.next(),
                    ));
                }
                for (lat, lon) in points {
                    let (lat, lon) = (lat * d, lon * d);
                    sample(&g, &only, level, lat, lon).unwrap_or_else(|e| {
                        panic!("level {level} box {la0} {la1} {lo0} {lo1}: {e}")
                    });
                    for id in g.bilinear_tiles(level, lat, lon) {
                        assert!(ids.contains(&id));
                    }
                }
            }
        }
        // The full grid, level 0, a box at the north pole across 180
        // degrees: every stencil tile listed.
        let g = Grid::BIOME;
        for _ in 0..2000 {
            let lat = 89.0 + rng.next();
            let lon = 179.5 + rng.next();
            let ids = g.tiles_for_region(0, 89.0 * d, 90.0 * d, 179.5 * d, 180.5 * d);
            for id in g.bilinear_tiles(0, lat * d, lon * d) {
                assert!(ids.contains(&id), "{lat} {lon}");
            }
        }
    }
}
