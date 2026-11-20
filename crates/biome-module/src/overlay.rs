//! Overlay tiles of the frame (`matter-format.md` section 12,
//! `client-compute.md` section 10, `rendering.md` section 7 item 6).
//!
//! Vertex `(i, j)` of a surface tile (`matter-format.md` section 10.1) is
//! placed on the WGS84 ellipsoid by the core library; its geodetic latitude
//! and longitude come from the ellipsoid's normal there, exactly as the
//! terrain module computes them ([`lat_lon`]). Its coverage is the land
//! fraction and its albedo the land albedo of one pyramid level
//! ([`level_for_tile`]), sampled bilinearly by [`biome_tiles::sample`]; its
//! roughness is [`ROUGHNESS`]. The parent values are the same computation at
//! the parent tile's level and at the vertex's own latitude and longitude,
//! so they are bit for bit the parent tile's own values there.

use std::collections::BTreeMap;
use std::f64::consts::{FRAC_PI_2, PI};

use biome_tiles::{sample, Grid, Tile, TileError};
use gx_core::key::SurfaceKey;
use gx_core::matter::Compression;
use gx_core::overlay::{self, OverlayTile, PARENT_VERTICES, TILE_VERTICES};
use gx_core::surface::{ellipsoid_point, vertex_direction, TILE_QUADS};

use crate::geodesy::{atan2, A, B};

/// The pyramid's grid: 15 arcseconds, levels 0 to 8.
pub const GRID: Grid = Grid::BIOME;

/// Roughness of the land, 0 to 1.
pub const ROUGHNESS: f64 = 0.9;

/// Widening of a tile's latitude and longitude box, radians, against
/// rounding in the vertex positions (about 6 mm on the ground).
pub const ANGLE_MARGIN: f64 = 1e-9;

/// The pyramid level a tile of `level` reads: the coarsest whose sample
/// spacing on the equator is no larger than the tile's vertex spacing
/// there (a quarter of the equator over `TILE_QUADS * 2^level`).
pub fn level_for_tile(level: u8) -> u8 {
    let spacing = FRAC_PI_2 * A / (TILE_QUADS as f64 * (1u64 << level.min(63)) as f64);
    let base = A * GRID.spacing(0);
    let mut k = 0u8;
    while k < GRID.top_level() && base * (2u64 << k) as f64 <= spacing {
        k += 1;
    }
    k
}

/// Geodetic latitude and longitude (radians) of the tile vertex `(i, j)`.
pub fn lat_lon(key: &SurfaceKey, i: i64, j: i64) -> (f64, f64) {
    let (_, n) = ellipsoid_point(vertex_direction(key, i, j), A, B);
    let lat = atan2(n[2], (n[0] * n[0] + n[1] * n[1]).sqrt());
    (lat, atan2(n[1], n[0]))
}

/// Whether the tile holds a pole: a polar face whose tile covers the
/// face's center.
pub fn holds_pole(key: &SurfaceKey) -> bool {
    if key.face < 4 {
        return false;
    }
    let half = 1u32 << key.level >> 1;
    let covers = |t: u32| key.level == 0 || t == half || t + 1 == half;
    covers(key.x) && covers(key.y)
}

/// The latitude and longitude box of the tile's vertices (radians,
/// longitudes unwrapped from the first vertex's, so a box across 180
/// degrees reaches past it). A tile holding a pole spans every longitude.
pub fn bounds(key: &SurfaceKey) -> (f64, f64, f64, f64) {
    let last = TILE_QUADS as i64;
    let (lat0, lon0) = lat_lon(key, 0, 0);
    let (mut lat_lo, mut lat_hi, mut lon_lo, mut lon_hi) = (lat0, lat0, lon0, lon0);
    for j in 0..=last {
        for i in 0..=last {
            let (lat, mut lon) = lat_lon(key, i, j);
            if lon - lon0 > PI {
                lon -= 2.0 * PI;
            } else if lon - lon0 < -PI {
                lon += 2.0 * PI;
            }
            lat_lo = lat_lo.min(lat);
            lat_hi = lat_hi.max(lat);
            lon_lo = lon_lo.min(lon);
            lon_hi = lon_hi.max(lon);
        }
    }
    if holds_pole(key) {
        let (south, north) = if key.face == 4 {
            (lat_lo - ANGLE_MARGIN, FRAC_PI_2)
        } else {
            (-FRAC_PI_2, lat_hi + ANGLE_MARGIN)
        };
        return (south, north, -PI - 1.0, PI + 1.0);
    }
    (
        lat_lo - ANGLE_MARGIN,
        lat_hi + ANGLE_MARGIN,
        lon_lo - ANGLE_MARGIN,
        lon_hi + ANGLE_MARGIN,
    )
}

/// The source tiles of a surface key: those of its pyramid level and of
/// its parent's that the bilinear stencil can read anywhere in the tile's
/// box, ascending and unique. The caller checks the frame.
pub fn sources(key: &SurfaceKey) -> Vec<u64> {
    let (la0, la1, lo0, lo1) = bounds(key);
    let mut ids = GRID.tiles_for_region(level_for_tile(key.level), la0, la1, lo0, lo1);
    if let Some(p) = key.parent() {
        ids.extend(GRID.tiles_for_region(level_for_tile(p.level), la0, la1, lo0, lo1));
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// `value` in 0 to 1 as a stored byte: `round(value * 255)`, half up,
/// clamped to 0 to 255.
pub fn byte(value: f64) -> u8 {
    (value * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8
}

/// Coverage, albedo, and roughness bytes at latitude `lat` and longitude
/// `lon` (radians) on pyramid `level`.
pub fn material(
    tiles: &BTreeMap<u64, Tile>,
    level: u8,
    lat: f64,
    lon: f64,
) -> Result<(u8, [u8; 3], u8), TileError> {
    let s = sample(&GRID, tiles, level, lat, lon)?;
    Ok((byte(s.land), s.albedo.map(byte), byte(ROUGHNESS)))
}

/// The overlay tile of `key` from `tiles` (those [`sources`] lists), or
/// `None` when every coverage and every parent coverage is 0. The caller
/// checks the frame.
pub fn tile(
    key: &SurfaceKey,
    tiles: &BTreeMap<u64, Tile>,
) -> Result<Option<OverlayTile>, TileError> {
    let own_level = level_for_tile(key.level);
    let parent_level = key.parent().map_or(own_level, |p| level_for_tile(p.level));
    let mut own = Vec::with_capacity(TILE_VERTICES * TILE_VERTICES);
    let mut lat_lons = Vec::with_capacity(TILE_VERTICES * TILE_VERTICES);
    for j in 0..TILE_VERTICES as i64 {
        for i in 0..TILE_VERTICES as i64 {
            let (lat, lon) = lat_lon(key, i, j);
            lat_lons.push((lat, lon));
            own.push(material(tiles, own_level, lat, lon)?);
        }
    }
    let mut parent = Vec::with_capacity(PARENT_VERTICES * PARENT_VERTICES);
    for m in 0..PARENT_VERTICES {
        for k in 0..PARENT_VERTICES {
            let v = 2 * k + TILE_VERTICES * 2 * m;
            if parent_level == own_level {
                parent.push(own[v]);
            } else {
                let (lat, lon) = lat_lons[v];
                parent.push(material(tiles, parent_level, lat, lon)?);
            }
        }
    }
    if own.iter().chain(parent.iter()).all(|v| v.0 == 0) {
        return Ok(None);
    }
    Ok(Some(OverlayTile {
        key: *key,
        coverage: own.iter().map(|v| v.0).collect(),
        albedo: own.iter().map(|v| v.1).collect(),
        roughness: own.iter().map(|v| v.2).collect(),
        parent_coverage: parent.iter().map(|v| v.0).collect(),
        parent_albedo: parent.iter().map(|v| v.1).collect(),
        parent_roughness: parent.iter().map(|v| v.2).collect(),
    }))
}

/// The overlay tile bytes of `key`: [`tile`] encoded with zstd, or the
/// empty overlay tile.
pub fn encode(key: &SurfaceKey, tiles: &BTreeMap<u64, Tile>) -> Result<Vec<u8>, TileError> {
    Ok(match tile(key, tiles)? {
        Some(t) => overlay::encode(&t, Compression::Zstd),
        None => overlay::encode_empty(key),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_get_finer_with_the_tile_level() {
        let levels: Vec<u8> = (0..=20).map(level_for_tile).collect();
        assert_eq!(levels[0], GRID.top_level());
        assert_eq!(levels[20], 0);
        assert!(levels.windows(2).all(|w| w[1] <= w[0]));
    }

    #[test]
    fn bytes_round_half_up_and_clamp() {
        assert_eq!(byte(0.0), 0);
        assert_eq!(byte(1.0), 255);
        assert_eq!(byte(-0.5), 0);
        assert_eq!(byte(2.0), 255);
        assert_eq!(byte(0.5 / 255.0), 1);
        assert_eq!(byte(ROUGHNESS), 230);
    }
}
