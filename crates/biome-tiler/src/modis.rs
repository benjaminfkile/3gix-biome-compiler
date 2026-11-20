//! The level 0 cut from MODIS composites: `biome-tiler modis-cut`.
//!
//! Fills the pyramid's level 0 (biome plan decisions 1, 2, 4, and 6,
//! 3GIXHub `docs/build/biome/plan.md`) from the per tile composites
//! `scripts/modis/modis.py` writes. The layer is the overlay of
//! `client-compute.md` section 2.2 and section 10; its source set is made as
//! section 5.3 says.
//!
//! # Composites
//!
//! One `.bgc` file per MODIS sinusoidal tile `(h, v)`, named `hHHvVV.bgc`:
//! magic `3GBC`, version `u16` 1, `h` `u8`, `v` `u8`, then planar
//! [`PIXELS`] x [`PIXELS`] blocks, row 0 (north) first: three `u16` albedo
//! bands (MODIS bands 1, 4, 3, white-sky albedo times 1000, [`NONE`] for
//! none) and one `u8` land fraction. Little-endian.
//!
//! # The sinusoidal grid
//!
//! GCTP sinusoidal on a sphere of radius [`R`]. Tile `(h, v)` has its upper
//! left corner at `x = X0 + h T`, `y = Y0 - v T` ([`X0`], [`Y0`],
//! [`TILE_M`] = `T`), and pixel `(r, c)` is centered at
//! `x0 + (c + 1/2) s`, `y0 - (r + 1/2) s`, `s` = [`PIXEL_M`]. Latitude and
//! longitude are WGS84 geodetic, as MODIS products treat them.
//!
//! # The cut
//!
//! Every sample center `(lat, lon)` of a level 0 tile maps to
//! `x = R lon cos(lat)`, `y = R lat` and takes the values of the MODIS pixel
//! holding that point (nearest pixel). Albedo `u8 = round(value / 1000 *
//! 255)` half up, clamped to 255. A band at [`NONE`] or a missing
//! composite file gives land 0 and albedo 0; the land fraction is copied
//! otherwise, and where it is 0 the albedo is 0 (as in every tile of the
//! pyramid, so oceans compress well; sampling never reads albedo without
//! land). Composites are read one row of MODIS tiles (`v`) at a time, only
//! the rows the current row of level 0 tiles needs, so memory stays near
//! 36 composites (about 1.5 GB).
//!
//! # Determinism
//!
//! The cosine is `gx_core::detmath::cos` (`+`, `-`, `*`, `/` only) and the
//! rest is correctly rounded IEEE 754 arithmetic, so every machine picks
//! the same pixels. Tiles go through [`write_level0`]'s writer, so the cut
//! is resumable as `synth` is.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use biome_tiles::grid::{TILE_SAMPLES, TILE_SIZE};
use biome_tiles::tile::{BANDS, LAND, PLANES};
use biome_tiles::{Grid, Tile, TileKey};
use gx_core::detmath;

#[cfg(doc)]
use crate::pyramid::write_level0;
use crate::pyramid::{finish, in_columns, write_tile, Options, PyramidError, Report};

/// Radius of the sphere of the MODIS sinusoidal grid, meters.
pub const R: f64 = 6_371_007.181;
/// Width and height of a MODIS tile, meters.
pub const TILE_M: f64 = 1_111_950.519_667;
/// Pixels per side of a MODIS tile.
pub const PIXELS: usize = 2400;
/// Width and height of a MODIS pixel, meters.
pub const PIXEL_M: f64 = TILE_M / PIXELS as f64;
/// `x` of the west edge of tile column 0, meters.
pub const X0: f64 = -20_015_109.354;
/// `y` of the north edge of tile row 0, meters.
pub const Y0: f64 = 10_007_554.677;
/// Tile columns (`h`).
pub const H_TILES: u32 = 36;
/// Tile rows (`v`).
pub const V_TILES: u32 = 18;
/// A composite band with no value.
pub const NONE: u16 = 65535;
/// Magic of a composite file.
pub const MAGIC: [u8; 4] = *b"3GBC";
/// Version of the composite file format.
pub const VERSION: u16 = 1;
/// Header length of a composite file.
pub const HEADER_LEN: usize = 8;
/// Length of a composite file.
pub const FILE_LEN: usize = HEADER_LEN + (BANDS * 2 + 1) * PIXELS * PIXELS;

/// One MODIS tile's composite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composite {
    /// Tile column.
    pub h: u8,
    /// Tile row.
    pub v: u8,
    /// Three planar bands of `PIXELS * PIXELS` values, albedo times 1000 or
    /// [`NONE`].
    pub bands: Vec<u16>,
    /// The land fraction, `value / 255`.
    pub land: Vec<u8>,
}

impl Composite {
    /// The file bytes of this composite.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(FILE_LEN);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.push(self.h);
        out.push(self.v);
        for b in &self.bands {
            out.extend_from_slice(&b.to_le_bytes());
        }
        out.extend_from_slice(&self.land);
        out
    }

    /// Parses composite file bytes.
    pub fn parse(bytes: &[u8]) -> Result<Composite, String> {
        if bytes.len() != FILE_LEN || bytes[..4] != MAGIC {
            return Err("not a composite file".into());
        }
        if u16::from_le_bytes([bytes[4], bytes[5]]) != VERSION {
            return Err("unknown composite version".into());
        }
        let n = PIXELS * PIXELS;
        let body = &bytes[HEADER_LEN..];
        let bands = body[..BANDS * 2 * n]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect();
        Ok(Composite {
            h: bytes[6],
            v: bytes[7],
            bands,
            land: body[BANDS * 2 * n..].to_vec(),
        })
    }

    /// The land byte and albedo bytes of pixel `(row, col)` as the cut
    /// stores them (module documentation, the cut).
    pub fn sample(&self, row: usize, col: usize) -> (u8, [u8; BANDS]) {
        let i = row * PIXELS + col;
        let mut albedo = [0; BANDS];
        for (b, a) in albedo.iter_mut().enumerate() {
            let v = self.bands[b * PIXELS * PIXELS + i];
            if v == NONE {
                return (0, [0; BANDS]);
            }
            *a = albedo_byte(v);
        }
        let land = self.land[i];
        if land == 0 {
            return (0, [0; BANDS]);
        }
        (land, albedo)
    }
}

/// `round(value / 1000 * 255)`, half up, clamped to 255, in integers.
pub fn albedo_byte(value: u16) -> u8 {
    ((u32::from(value) * 255 + 500) / 1000).min(255) as u8
}

/// The path of the composite of tile `(h, v)` under `dir`.
pub fn composite_path(dir: &Path, h: u32, v: u32) -> PathBuf {
    dir.join(format!("h{h:02}v{v:02}.bgc"))
}

/// The MODIS tile row `v` and pixel row holding geodetic latitude `lat`
/// (radians).
pub fn modis_row(lat: f64) -> (u32, usize) {
    let y = R * lat;
    let v = ((Y0 - y) / TILE_M)
        .floor()
        .clamp(0.0, f64::from(V_TILES - 1));
    let y0 = Y0 - v * TILE_M;
    let r = ((y0 - y) / PIXEL_M).floor().clamp(0.0, (PIXELS - 1) as f64);
    (v as u32, r as usize)
}

/// The MODIS tile column `h` and pixel column holding geodetic latitude
/// `lat` and longitude `lon` (radians, in `[-pi, pi]`).
pub fn modis_col(lat: f64, lon: f64) -> (u32, usize) {
    let x = R * lon * detmath::cos(lat);
    let h = ((x - X0) / TILE_M)
        .floor()
        .clamp(0.0, f64::from(H_TILES - 1));
    let x0 = X0 + h * TILE_M;
    let c = ((x - x0) / PIXEL_M).floor().clamp(0.0, (PIXELS - 1) as f64);
    (h as u32, c as usize)
}

/// Why the cut failed.
#[derive(Debug)]
pub enum CutError {
    /// A composite could not be read.
    Io(PathBuf, io::Error),
    /// A composite file is not one, or names another tile.
    Composite(PathBuf, String),
    /// Writing the pyramid failed.
    Pyramid(PyramidError),
}

impl fmt::Display for CutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CutError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            CutError::Composite(p, e) => write!(f, "{}: {e}", p.display()),
            CutError::Pyramid(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CutError {}

impl From<PyramidError> for CutError {
    fn from(e: PyramidError) -> Self {
        CutError::Pyramid(e)
    }
}

/// Reads the composite of `(h, v)` under `dir`; `None` when there is no
/// file.
pub fn read_composite(dir: &Path, h: u32, v: u32) -> Result<Option<Composite>, CutError> {
    let path = composite_path(dir, h, v);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(CutError::Io(path, e)),
    };
    let c = Composite::parse(&bytes).map_err(|e| CutError::Composite(path.clone(), e))?;
    if (u32::from(c.h), u32::from(c.v)) != (h, v) {
        return Err(CutError::Composite(path, "names another tile".into()));
    }
    Ok(Some(c))
}

/// The loaded rows of composites, by `v`, then `h`.
type Rows = BTreeMap<u32, Vec<Option<Composite>>>;

/// Level 0 tile `(row, col)` of `grid` from the loaded composite rows.
fn cut_tile(grid: &Grid, rows: &Rows, row: u32, col: u32) -> Tile {
    let n = TILE_SIZE as usize;
    let mut planes = vec![0u8; PLANES * TILE_SAMPLES];
    for i in 0..n {
        let r = grid.data_row(row * TILE_SIZE + i as u32);
        let (lat, _) = grid.center(0, r, 0);
        let (v, pr) = modis_row(lat);
        let tiles = &rows[&v];
        for j in 0..n {
            let c = grid.data_col(col * TILE_SIZE + j as u32);
            let (_, lon) = grid.center(0, 0, c);
            let (h, pc) = modis_col(lat, lon);
            let Some(comp) = &tiles[h as usize] else {
                continue;
            };
            let (land, albedo) = comp.sample(pr, pc);
            let k = i * n + j;
            for (b, a) in albedo.iter().enumerate() {
                planes[b * TILE_SAMPLES + k] = *a;
            }
            planes[LAND * TILE_SAMPLES + k] = land;
        }
    }
    Tile::new(TileKey { level: 0, row, col }, planes)
        .expect("a level 0 tile has a valid key and size")
}

/// Writes the level 0 tiles of `grid` into `out` from the composites in
/// `composites`, then builds the rest of the pyramid with [`finish`].
/// Returns [`finish`]'s report and the level 0 tiles written and already
/// present.
pub fn modis_cut(
    composites: &Path,
    out: &Path,
    grid: Grid,
    opts: Options,
) -> Result<(Report, u64, u64), CutError> {
    let (mut written, mut skipped) = (0, 0);
    let mut rows: Rows = BTreeMap::new();
    for row in 0..grid.tile_rows(0) {
        let mut need: Vec<u32> = (0..TILE_SIZE)
            .map(|i| {
                let r = grid.data_row(row * TILE_SIZE + i);
                modis_row(grid.center(0, r, 0).0).0
            })
            .collect();
        need.dedup();
        rows.retain(|v, _| need.contains(v));
        for &v in &need {
            if let std::collections::btree_map::Entry::Vacant(e) = rows.entry(v) {
                let mut line = Vec::with_capacity(H_TILES as usize);
                for h in 0..H_TILES {
                    line.push(read_composite(composites, h, v)?);
                }
                e.insert(line);
            }
        }
        let rows = &rows;
        let results = in_columns(grid.tile_cols(0), opts.threads, |col| {
            write_tile(out, &cut_tile(&grid, rows, row, col))
        });
        for r in results {
            if r?.1 {
                written += 1;
            } else {
                skipped += 1;
            }
        }
        if opts.progress && (row + 1) % 10 == 0 {
            println!(
                "modis-cut: {}/{} rows of level 0",
                row + 1,
                grid.tile_rows(0)
            );
        }
    }
    if opts.progress {
        println!("modis-cut: level 0 written");
    }
    let report = finish(out, grid, opts)?;
    Ok((report, written, skipped))
}
