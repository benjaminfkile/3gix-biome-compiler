//! Fixture tiles: B1's synthetic world on the real tile grid, for the tests,
//! the conformance vectors, and `bench`.
//!
//! The module asks for tiles of the pyramid ([`GRID`]) by their real ids.
//! The fixtures fill any of those tiles, at any level, from
//! [`biome_tiles::fixture::world_at`] at each lattice sample's center, with
//! the lattice rules of [`biome_tiles::grid`] (columns past 180 degrees wrap,
//! rows past the south pole repeat the last row). On level 0 that is exactly
//! [`biome_tiles::fixture::level0_tile`]; coarser levels are the same world
//! sampled coarser rather than built by the pyramid's averaging, which is
//! all the module's tests need.
//!
//! [`record`] is the frame record the system layer answers for the frame
//! the module fills; the tests and the conformance vectors pass it as the
//! layer's frames slot. Nothing here is on the module's output path.

use std::collections::BTreeMap;

use biome_tiles::fixture::world_at;
use biome_tiles::tile::{BANDS, LAND};
use biome_tiles::{Tile, TileKey};
use gx_core::frame_record::FrameRecord;
use gx_core::units::{Kilograms, Meters, Vec3};

use crate::frame::Frame;
use crate::overlay::GRID;

/// The frame id the system module gives the frame the fixture describes:
/// test data, not a module constant.
pub const FRAME_ID: u64 = 4;

/// The record of frame [`FRAME_ID`] as the system module answers it
/// (`matter-format.md` section 11).
pub fn record() -> FrameRecord {
    record_of(FRAME_ID)
}

/// The same record naming frame `frame_id`.
pub fn record_of(frame_id: u64) -> FrameRecord {
    FrameRecord {
        empty: false,
        frame_id,
        parent_frame_id: 0,
        root_extent: Meters::new(5.096808e7),
        max_depth: 6,
        mass: Kilograms::new(5.972168399787584e24),
        j2: 0.00108263,
        reference_radius: Meters::new(6378137.0),
        principal_moments: Vec3::new(
            7.992584813748298e37,
            7.992584813748298e37,
            8.018887480921225e37,
        ),
    }
}

/// The [`Frame`] of [`record`].
pub fn frame() -> Frame {
    Frame::from_record(&record()).expect("the system module's record of the frame")
}

/// The fixture tile with id `id`. Panics on an id that is not a tile of
/// [`GRID`].
pub fn tile(id: u64) -> Tile {
    let key = TileKey::from_id(id).expect("a tile id");
    let level = key.level;
    assert!(level <= GRID.top_level() && key.row < GRID.tile_rows(level));
    assert!(key.col < GRID.tile_cols(level));
    let n = GRID.tile_size();
    let samples = GRID.tile_samples();
    let rows = f64::from(GRID.data_rows(level));
    let cols = f64::from(GRID.wrap_cols(level));
    let mut planes = vec![0u8; (LAND + 1) * samples];
    for i in 0..n {
        let r = GRID.lattice_row(level, i64::from(key.row * n + i));
        let v = ((f64::from(r) + 0.5) / rows).min(1.0);
        for j in 0..n {
            let c = GRID.lattice_col(level, i64::from(key.col * n + j)) % GRID.wrap_cols(level);
            let u = (f64::from(c) + 0.5) / cols;
            let (l, a) = world_at(u, v);
            let k = (i * n + j) as usize;
            for (b, &x) in a.iter().enumerate().take(BANDS) {
                planes[b * samples + k] = x;
            }
            planes[LAND * samples + k] = l;
        }
    }
    Tile::new(key, planes).expect("a full tile")
}

/// The fixture tiles with these ids.
pub fn tiles(ids: &[u64]) -> BTreeMap<u64, Tile> {
    ids.iter().map(|&id| (id, tile(id))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level0_is_the_synthetic_world() {
        let id = TileKey {
            level: 0,
            row: 40,
            col: 100,
        }
        .id();
        assert_eq!(tile(id), biome_tiles::fixture::level0_tile(&GRID, 40, 100));
    }
}
