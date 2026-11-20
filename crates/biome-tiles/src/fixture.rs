//! The synthetic world: land fraction and albedo for every data sample of a
//! grid, never real data.
//!
//! Tests of this repository and of the biome module, and the timing of
//! `biome-tiler synth`, use it. Land is where a smooth function of latitude
//! and longitude is positive, with partial land along the edge where it
//! crosses zero; albedo changes with latitude and longitude. The world is a
//! function of the sample's place as a fraction of the grid, so a smaller
//! grid of the same layout shows the same world, coarser.
//!
//! The arithmetic is `+`, `-`, `*`, `/`, and `floor` on `f64`, so the world
//! is the same on every machine and natively and in WebAssembly.

use crate::grid::{Grid, TILE_SAMPLES, TILE_SIZE};
use crate::tile::{Tile, TileKey, BANDS, LAND};

/// Width of the partial land edge, in units of the land function.
const EDGE: f64 = 0.004;

/// A smooth periodic bump: `16 t^2 (1 - t)^2` of the fractional part of
/// `t`, 0 at whole numbers, 1 halfway between, with a continuous slope.
fn bump(t: f64) -> f64 {
    let f = t - t.floor();
    let g = f * (1.0 - f);
    16.0 * g * g
}

/// `x` to a byte, `x` in `[0, 1]` scaled by 255 and rounded half up.
fn byte(x: f64) -> u8 {
    let x = x.clamp(0.0, 1.0);
    (x * 255.0 + 0.5).floor() as u8
}

/// The land function at `u` (longitude, 0 at 180 degrees west to 1) and
/// `v` (colatitude, 0 at the north pole to 1 at the south pole): positive
/// on land.
pub fn land_function(u: f64, v: f64) -> f64 {
    let p = 2.0 * v - 1.0;
    let p2 = p * p;
    let p4 = p2 * p2;
    0.5 * bump(2.0 * u + 0.1) + 0.4 * bump(3.0 * u + 2.0 * v + 0.3) + 0.6 * p4 * p4 - 0.45
}

/// The land byte and albedo bytes of data sample `(r, c)` of `grid`
/// (counted from the north and from 180 degrees west). Where the land byte
/// is 0 the albedo bytes are 0.
pub fn world(grid: &Grid, r: u32, c: u32) -> (u8, [u8; BANDS]) {
    let u = (f64::from(c) + 0.5) / f64::from(grid.cols());
    let v = (f64::from(r) + 0.5) / f64::from(grid.rows());
    world_at(u, v)
}

/// The land byte and albedo bytes of the world at `u` (longitude, 0 at 180
/// degrees west to 1) and `v` (colatitude, 0 at the north pole to 1 at the
/// south pole). [`world`] is this at a sample's center. Where the land byte
/// is 0 the albedo bytes are 0.
pub fn world_at(u: f64, v: f64) -> (u8, [u8; BANDS]) {
    let land = byte(land_function(u, v) / EDGE + 0.5);
    if land == 0 {
        return (0, [0; BANDS]);
    }
    let p = 2.0 * v - 1.0;
    let albedo = [
        byte(0.10 + 0.25 * bump(4.0 * u + v) + 0.1 * v),
        byte(0.08 + 0.2 * bump(5.0 * u - 3.0 * v)),
        byte(0.05 + 0.3 * p * p),
    ];
    (land, albedo)
}

/// The level 0 tile `(row, col)` of `grid` filled with [`world`], with the
/// lattice rules of [`crate::grid`]: columns past 180 degrees wrap and rows
/// past the south pole repeat the last data row.
pub fn level0_tile(grid: &Grid, row: u32, col: u32) -> Tile {
    let mut planes = vec![0u8; LAND * TILE_SAMPLES + TILE_SAMPLES];
    for i in 0..TILE_SIZE {
        let r = grid.data_row(row * TILE_SIZE + i);
        for j in 0..TILE_SIZE {
            let c = grid.data_col(col * TILE_SIZE + j);
            let (l, a) = world(grid, r, c);
            let k = (i * TILE_SIZE + j) as usize;
            for (b, &v) in a.iter().enumerate() {
                planes[b * TILE_SAMPLES + k] = v;
            }
            planes[LAND * TILE_SAMPLES + k] = l;
        }
    }
    Tile::new(TileKey { level: 0, row, col }, planes).expect("a full tile with a valid key")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_ocean_land_and_coast() {
        let g = Grid::new(480, 960).unwrap();
        let (mut ocean, mut land, mut coast) = (0, 0, 0);
        let mut bands = [u8::MAX, 0];
        for r in 0..g.rows() {
            for c in 0..g.cols() {
                let (l, a) = world(&g, r, c);
                match l {
                    0 => ocean += 1,
                    255 => land += 1,
                    _ => coast += 1,
                }
                if l > 0 {
                    bands = [bands[0].min(a[0]), bands[1].max(a[0])];
                }
            }
        }
        assert!(ocean > 50_000 && land > 50_000 && coast > 1000);
        assert!(bands[1] - bands[0] > 40, "{bands:?}");
    }
}
