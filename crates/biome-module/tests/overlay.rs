//! The overlay tiles against the synthetic world: shared edges, parent
//! values, coverage and albedo, and the frame the record names.

use std::collections::BTreeMap;

use biome_module::overlay::{self, lat_lon, level_for_tile, GRID};
use biome_module::{compile, fixture, Frame};
use biome_tiles::fixture::world;
use biome_tiles::Tile;
use gx_core::key::{ChunkKey, SurfaceKey};
use gx_core::overlay::{decode, Overlay, OverlayTile, PARENT_VERTICES, TILE_VERTICES};

fn tiles_for(keys: &[SurfaceKey]) -> BTreeMap<u64, Tile> {
    let ids: Vec<u64> = keys.iter().flat_map(overlay::sources).collect();
    fixture::tiles(&ids)
}

fn tile(key: &SurfaceKey) -> OverlayTile {
    overlay::tile(key, &tiles_for(&[*key]))
        .unwrap()
        .expect("a tile with land")
}

fn key(face: u8, level: u8, x: u32, y: u32) -> SurfaceKey {
    SurfaceKey::new(fixture::FRAME_ID, face, level, x, y).unwrap()
}

fn at(t: &OverlayTile, i: usize, j: usize) -> (u8, [u8; 3], u8) {
    let v = i + TILE_VERTICES * j;
    (t.coverage[v], t.albedo[v], t.roughness[v])
}

#[test]
fn neighbors_share_their_edge_values() {
    // Inside a face, on the coast.
    let (l, r) = (tile(&key(0, 10, 21, 609)), tile(&key(0, 10, 22, 609)));
    for j in 0..TILE_VERTICES {
        assert_eq!(at(&l, TILE_VERTICES - 1, j), at(&r, 0, j), "row {j}");
    }
    // Across the edge between face 0 (+x) and face 2 (+y): face 0's east
    // edge (s = 1) is face 2's west edge (s = -1), at the same t.
    let (a, b) = (tile(&key(0, 3, 7, 4)), tile(&key(2, 3, 0, 4)));
    for j in 0..TILE_VERTICES {
        assert_eq!(at(&a, TILE_VERTICES - 1, j), at(&b, 0, j), "row {j}");
    }
}

#[test]
fn parent_values_are_the_parents_own() {
    for child in [key(0, 10, 21, 609), key(1, 5, 15, 10), key(4, 3, 3, 3)] {
        let parent = child.parent().unwrap();
        let c = tile(&child);
        let p = tile(&parent);
        let (ox, oy) = (
            (child.x % 2) as usize * (TILE_VERTICES / 2),
            (child.y % 2) as usize * (TILE_VERTICES / 2),
        );
        for m in 0..PARENT_VERTICES {
            for k in 0..PARENT_VERTICES {
                let v = k + PARENT_VERTICES * m;
                assert_eq!(
                    (
                        c.parent_coverage[v],
                        c.parent_albedo[v],
                        c.parent_roughness[v]
                    ),
                    at(&p, ox + k, oy + m),
                    "{child} vertex ({k}, {m})"
                );
            }
        }
    }
}

#[test]
fn coverage_and_albedo_follow_the_world() {
    // Inland: full coverage, albedo the world's within one byte step.
    let k = key(0, 10, 0, 315);
    assert_eq!(level_for_tile(k.level), 0);
    let t = tile(&k);
    for j in (0..TILE_VERTICES).step_by(8) {
        for i in (0..TILE_VERTICES).step_by(8) {
            let (cov, alb, rough) = at(&t, i, j);
            assert_eq!(cov, 255);
            assert_eq!(rough, 230);
            let (lat, lon) = lat_lon(&k, i as i64, j as i64);
            let p = GRID.position(0, lat, lon);
            let (r, c) = (p.y.round() as u32, p.x.round() as u32);
            let (_, w) = world(&GRID, r, c);
            for b in 0..3 {
                assert!(
                    alb[b].abs_diff(w[b]) <= 1,
                    "{i} {j} band {b}: {alb:?} {w:?}"
                );
            }
        }
    }
    // Far out at sea: the empty tile.
    let sea = key(0, 10, 0, 0);
    assert_eq!(overlay::tile(&sea, &tiles_for(&[sea])).unwrap(), None);
    // On the coast: between 0 and 1.
    let coast = tile(&key(0, 10, 21, 609));
    assert!(coast.coverage.iter().all(|&c| c > 0 && c < 255));
}

#[test]
fn the_record_decides_the_frame() {
    let k = key(0, 10, 0, 315);
    let tiles = tiles_for(&[k]);
    let own = compile(&fixture::frame(), &ChunkKey::Surface(k), &tiles).unwrap();
    assert!(matches!(decode(&k, &own), Ok(Overlay::Tile(_))));
    let other = Frame::from_record(&fixture::record_of(5)).unwrap();
    let empty = compile(&other, &ChunkKey::Surface(k), &tiles).unwrap();
    assert_eq!(decode(&k, &empty), Ok(Overlay::Empty(k)));
    // A key of the other frame, with that frame's record, is filled.
    let k5 = SurfaceKey::new(5, 0, 10, 0, 315).unwrap();
    let filled = compile(&other, &ChunkKey::Surface(k5), &tiles).unwrap();
    assert!(matches!(decode(&k5, &filled), Ok(Overlay::Tile(_))));
}
