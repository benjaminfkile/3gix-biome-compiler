//! A fixed digest over everything this crate computes.
//!
//! [`digest`] encodes a set of tiles with fixed contents, decodes them back,
//! samples them at fixed points on two levels, lists the tiles of fixed
//! regions, and hashes every byte, every `f64` bit pattern, and every id
//! that comes out. The native test pins the digest, and `scripts/ci.sh` runs
//! the same function compiled to WebAssembly under Node and requires the
//! same digest (`space-model.md` section 4 item 6, determinism checks). It
//! needs no input, so the check has nothing to disagree about but the code.

use std::collections::BTreeMap;

use gx_core::sha256::{Sha256, DIGEST_LEN};

use crate::grid::{Grid, TILE_SAMPLES, TILE_SIZE};
use crate::sample::sample;
use crate::tile::{Tile, TileKey, PLANES};

/// The grid the digest uses: 480 by 960 samples, levels 0 and 1.
pub const GRID: Grid = match Grid::new(480, 960) {
    Some(g) => g,
    None => panic!("the check grid is valid"),
};

/// The value of block `plane` at lattice sample `(r, c)` in the check's
/// tiles: a fixed integer pattern with every byte value, and runs of 0 and
/// 255 in the land block.
pub fn pattern(level: u8, plane: usize, r: u32, c: u32) -> u8 {
    let a = (r as i64 * 7919 + c as i64 * 104_729 + level as i64 * 31 + plane as i64 * 17) % 256;
    if plane == PLANES - 1 {
        match (r / 3 + c / 5) % 4 {
            0 => 0,
            1 => 255,
            _ => a as u8,
        }
    } else {
        a as u8
    }
}

/// Every tile of both levels of [`GRID`], filled with [`pattern`].
pub fn tiles() -> BTreeMap<u64, Tile> {
    let mut out = BTreeMap::new();
    for level in GRID.levels() {
        for row in 0..GRID.tile_rows(level) {
            for col in 0..GRID.tile_cols(level) {
                let mut s = Vec::with_capacity(PLANES * TILE_SAMPLES);
                for p in 0..PLANES {
                    for i in 0..TILE_SIZE {
                        for j in 0..TILE_SIZE {
                            s.push(pattern(level, p, row * TILE_SIZE + i, col * TILE_SIZE + j));
                        }
                    }
                }
                let t = Tile::new(TileKey { level, row, col }, s).expect("a full tile");
                out.insert(t.id(), t);
            }
        }
    }
    out
}

/// The digest: SHA-256 over every tile's encoded bytes in id order, then
/// over the bit patterns of [`sample`] at 1024 fixed points per level, then
/// over the ids [`Grid::tiles_for_region`] lists for fixed boxes. Panics if
/// a tile does not decode to itself.
pub fn digest() -> [u8; DIGEST_LEN] {
    let tiles = tiles();
    let mut h = Sha256::new();
    let mut decoded = BTreeMap::new();
    for (&id, tile) in &tiles {
        let bytes = tile.encode();
        h.update(&id.to_le_bytes());
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
        let back = Tile::decode_id(id, &bytes).expect("the tile decodes");
        assert_eq!(&back, tile, "the tile round trips");
        decoded.insert(id, back);
    }
    let pi = core::f64::consts::PI;
    for level in GRID.levels() {
        for i in 0..32u32 {
            for j in 0..32u32 {
                // Latitudes from pole to pole and longitudes past both ends.
                let lat = (f64::from(i) / 31.0 - 0.5) * pi;
                let lon = (f64::from(j) / 31.0 - 0.5) * 2.2 * pi + 0.001;
                let s = sample(&GRID, &decoded, level, lat, lon).expect("every tile");
                h.update(&s.land.to_bits().to_le_bytes());
                for a in s.albedo {
                    h.update(&a.to_bits().to_le_bytes());
                }
            }
        }
        for i in 0..16u32 {
            let lat = (f64::from(i) / 15.0 - 0.5) * pi;
            let lon = (f64::from(i) / 15.0 - 0.5) * 2.4 * pi;
            let span = f64::from(i) * 0.05;
            let ids = GRID.tiles_for_region(level, lat - span, lat, lon, lon + 2.0 * span);
            h.update(&(ids.len() as u64).to_le_bytes());
            for id in ids {
                h.update(&id.to_le_bytes());
            }
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_pinned() {
        let want = include_str!("../../../conformance/tiles-digest.sha256").trim();
        assert_eq!(gx_core::sha256::to_hex(&digest()), want);
    }
}
