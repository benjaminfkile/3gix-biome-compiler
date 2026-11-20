//! Writing level 0 tiles and building the rest of the pyramid from them.
//!
//! The source set of `client-compute.md` section 5.3: tiles in the format
//! of [`biome_tiles::tile`], the manifest of section 5.1 (`manifest.bin`,
//! the core library's encoder), and a `summary.json` with tiles and bytes
//! per level and the manifest hash.
//!
//! # Level 0, then the rest
//!
//! Whatever produces level 0 (the synthetic world of [`crate::synth`], or a
//! step that reads measured data) writes every level 0 tile with
//! [`write_tile`] or [`write_level0`]. [`finish`] then builds levels 1 to
//! the top one row of tiles at a time: it decodes the one or two full width
//! rows of tiles of level `k - 1` a row of level `k` reads, builds that row
//! by the rule of [`biome_tiles::grid::coarsen`] (columns past 180 degrees
//! wrap, rows past the south pole repeat the last row that touches the
//! globe), and writes it. Last it writes the manifest and the summary.
//!
//! # Determinism and resuming
//!
//! Every coarse tile is a pure function of the level 0 tiles, and the
//! manifest and summary are pure functions of the tiles, so every run on
//! every machine writes the same bytes, on any number of threads. Threads
//! only handle tiles of one row side by side; the results are gathered in
//! column order. A tile whose file already exists with the SHA-256 of the
//! bytes it should have is not written again, so an interrupted run is
//! resumed by running it again. Every file is written to a temporary name
//! first and renamed into place, so an interrupted write never leaves a
//! partial file under a tile's name.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use biome_tiles::grid::{coarsen, TILE_SAMPLES, TILE_SIZE};
use biome_tiles::tile::{BANDS, LAND, PLANES};
use biome_tiles::{Grid, Tile, TileError, TileKey};
use gx_core::manifest::{self, Entry, Manifest};
use gx_core::sha256::{sha256, to_hex, DIGEST_LEN};

/// The manifest file in the output directory.
pub const MANIFEST_FILE: &str = "manifest.bin";
/// The summary file in the output directory.
pub const SUMMARY_FILE: &str = "summary.json";
/// The directory of tiles in the output directory.
pub const TILES_DIR: &str = "tiles";

/// The path of a tile's file under the output directory `out`:
/// `tiles/{level}/{row}/{col}.3gba`.
pub fn tile_path(out: &Path, key: TileKey) -> PathBuf {
    out.join(TILES_DIR)
        .join(key.level.to_string())
        .join(key.row.to_string())
        .join(format!("{}.3gba", key.col))
}

/// Why writing or building the pyramid failed.
#[derive(Debug)]
pub enum PyramidError {
    /// A file could not be read or written.
    Io(PathBuf, io::Error),
    /// A tile file did not decode as the tile its path names.
    Tile(PathBuf, TileError),
}

impl fmt::Display for PyramidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PyramidError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            PyramidError::Tile(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for PyramidError {}

/// How to build.
#[derive(Copy, Clone, Debug)]
pub struct Options {
    /// Threads that build and compress tiles. The output does not depend on
    /// it.
    pub threads: usize,
    /// Print a line per finished level.
    pub progress: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
            progress: false,
        }
    }
}

/// Per level counts of a pyramid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelSummary {
    /// The level.
    pub level: u8,
    /// Rows of tiles.
    pub tile_rows: u32,
    /// Columns of tiles.
    pub tile_cols: u32,
    /// Tiles.
    pub tiles: u64,
    /// Bytes of all tiles.
    pub bytes: u64,
}

/// What [`finish`] produced. Everything but `written` and `skipped` is also
/// in `summary.json` and is a pure function of the level 0 tiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The grid.
    pub grid: Grid,
    /// Per level counts, finest first.
    pub levels: Vec<LevelSummary>,
    /// Tiles in all levels.
    pub tiles: u64,
    /// Distinct tile hashes: the most files an upload sends.
    pub distinct: u64,
    /// Bytes of all tiles.
    pub bytes: u64,
    /// Bytes of the manifest.
    pub manifest_bytes: u64,
    /// SHA-256 of the manifest.
    pub manifest_hash: [u8; DIGEST_LEN],
    /// Coarse tiles written by this run.
    pub written: u64,
    /// Coarse tiles already present with the right hash.
    pub skipped: u64,
}

impl Report {
    /// The contents of `summary.json`: every field but `written` and
    /// `skipped`, in a fixed order with a fixed layout.
    pub fn summary_json(&self) -> String {
        let mut s = String::new();
        s.push_str("{\n");
        s.push_str(&format!(
            "  \"grid\": {{ \"rows\": {}, \"cols\": {} }},\n",
            self.grid.rows(),
            self.grid.cols()
        ));
        s.push_str("  \"levels\": [\n");
        for (i, l) in self.levels.iter().enumerate() {
            s.push_str(&format!(
                "    {{ \"level\": {}, \"tileRows\": {}, \"tileCols\": {}, \"tiles\": {}, \"bytes\": {} }}{}\n",
                l.level,
                l.tile_rows,
                l.tile_cols,
                l.tiles,
                l.bytes,
                if i + 1 < self.levels.len() { "," } else { "" }
            ));
        }
        s.push_str("  ],\n");
        s.push_str(&format!("  \"tiles\": {},\n", self.tiles));
        s.push_str(&format!("  \"distinctTiles\": {},\n", self.distinct));
        s.push_str(&format!("  \"tileBytes\": {},\n", self.bytes));
        s.push_str(&format!("  \"manifestBytes\": {},\n", self.manifest_bytes));
        s.push_str(&format!(
            "  \"manifestHash\": \"{}\"\n",
            to_hex(&self.manifest_hash)
        ));
        s.push_str("}\n");
        s
    }
}

/// Encodes `tile` and writes it to its path under `out`, unless the file
/// already holds these bytes. Returns its manifest entry and whether the
/// file was written. This is the writer every level 0 producer goes
/// through.
pub fn write_tile(out: &Path, tile: &Tile) -> Result<(Entry, bool), PyramidError> {
    let bytes = tile.encode();
    let hash = sha256(&bytes);
    let entry = Entry {
        id: tile.id(),
        sha256: hash,
        length: bytes.len() as u64,
    };
    let path = tile_path(out, tile.key());
    if let Ok(existing) = fs::read(&path) {
        if sha256(&existing) == hash {
            return Ok((entry, false));
        }
    }
    write_atomic(&path, &bytes)?;
    Ok((entry, true))
}

/// Writes every level 0 tile of `grid`, `make(row, col)` giving tile
/// `(row, col)`, a row of tiles at a time with `threads` threads side by
/// side. Returns how many files were written and how many already held the
/// right bytes.
pub fn write_level0<F>(
    out: &Path,
    grid: Grid,
    threads: usize,
    make: F,
) -> Result<(u64, u64), PyramidError>
where
    F: Fn(u32, u32) -> Tile + Sync,
{
    let (mut written, mut skipped) = (0, 0);
    for row in 0..grid.tile_rows(0) {
        let results = in_columns(grid.tile_cols(0), threads, |col| {
            write_tile(out, &make(row, col))
        });
        for r in results {
            if r?.1 {
                written += 1;
            } else {
                skipped += 1;
            }
        }
    }
    Ok((written, skipped))
}

/// Runs `f` for every column `0..cols` on up to `threads` threads and
/// returns the results in column order.
fn in_columns<T: Send>(cols: u32, threads: usize, f: impl Fn(u32) -> T + Sync) -> Vec<T> {
    let threads = threads.clamp(1, cols.max(1) as usize);
    let per = (cols as usize).div_ceil(threads) as u32;
    let f = &f;
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads as u32)
            .map(|t| {
                s.spawn(move || {
                    let lo = (t * per).min(cols);
                    let hi = ((t + 1) * per).min(cols);
                    (lo..hi).map(f).collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a tile thread does not panic"))
            .collect()
    })
}

/// Reads and decodes the tile `key` under `out`, checking its header.
/// Returns the tile and its manifest entry.
pub fn read_tile(out: &Path, key: TileKey) -> Result<(Tile, Entry), PyramidError> {
    let path = tile_path(out, key);
    let bytes = fs::read(&path).map_err(|e| PyramidError::Io(path.clone(), e))?;
    let tile = Tile::decode_id(key.id(), &bytes).map_err(|e| PyramidError::Tile(path, e))?;
    let entry = Entry {
        id: key.id(),
        sha256: sha256(&bytes),
        length: bytes.len() as u64,
    };
    Ok((tile, entry))
}

/// Tile `(row, col)` of `level` from the full width rows of tiles of
/// `level - 1` in `finer` (`biome_tiles::grid`, values).
pub fn coarse_tile(grid: &Grid, level: u8, row: u32, col: u32, finer: &FinerRows) -> Tile {
    let n = TILE_SIZE as usize;
    // For each column of the tile, the finer tile column and index inside
    // it of the two columns it reads.
    let cols: Vec<[(usize, usize); 2]> = (0..TILE_SIZE)
        .map(|j| {
            let c = col * TILE_SIZE + j;
            [0, 1].map(|jj| {
                let src = grid.finer_col(level, c, jj);
                ((src / TILE_SIZE) as usize, (src % TILE_SIZE) as usize)
            })
        })
        .collect();
    let mut planes = vec![0u8; PLANES * TILE_SAMPLES];
    for i in 0..TILE_SIZE {
        let r = row * TILE_SIZE + i;
        let rows = [0, 1].map(|ii| {
            let src = grid.finer_row(level, r, ii);
            let tiles = finer
                .get(&(src / TILE_SIZE))
                .expect("a coarse row reads only the rows of tiles loaded for it");
            (tiles, (src % TILE_SIZE) as usize * n)
        });
        for (j, cc) in cols.iter().enumerate() {
            let mut land = [0u8; 4];
            let mut albedo = [[0u8; BANDS]; 4];
            for (a, &(tiles, base)) in rows.iter().enumerate() {
                for (b, &(tc, x)) in cc.iter().enumerate() {
                    let t = &tiles[tc];
                    land[2 * a + b] = t.land(base + x);
                    albedo[2 * a + b] = t.albedo(base + x);
                }
            }
            let (l, al) = coarsen(land, albedo);
            let k = i as usize * n + j;
            for (b, &v) in al.iter().enumerate() {
                planes[b * TILE_SAMPLES + k] = v;
            }
            planes[LAND * TILE_SAMPLES + k] = l;
        }
    }
    Tile::new(TileKey { level, row, col }, planes).expect("a full tile with a valid key")
}

/// Decoded full width rows of tiles of one level, by tile row.
pub type FinerRows = BTreeMap<u32, Vec<Tile>>;

/// Builds levels 1 to the top of `grid` from the level 0 tiles in `out`,
/// then writes `manifest.bin` and `summary.json` (module documentation).
/// Every level 0 tile must be present.
pub fn finish(out: &Path, grid: Grid, opts: Options) -> Result<Report, PyramidError> {
    let mut entries: BTreeMap<u64, Entry> = BTreeMap::new();
    let (mut written, mut skipped) = (0, 0);
    for level in 1..=grid.top_level() {
        let finer_level = level - 1;
        let mut finer: FinerRows = BTreeMap::new();
        for row in 0..grid.tile_rows(level) {
            // The finer rows of tiles this row reads: those holding rows
            // 2r + i, clamped to the globe.
            let first = grid.finer_row(level, row * TILE_SIZE, 0) / TILE_SIZE;
            let last = grid.finer_row(level, row * TILE_SIZE + TILE_SIZE - 1, 1) / TILE_SIZE;
            finer.retain(|&r, _| r >= first);
            for fr in first..=last {
                if finer.contains_key(&fr) {
                    continue;
                }
                let loaded = in_columns(grid.tile_cols(finer_level), opts.threads, |col| {
                    read_tile(
                        out,
                        TileKey {
                            level: finer_level,
                            row: fr,
                            col,
                        },
                    )
                });
                let mut tiles = Vec::with_capacity(loaded.len());
                for r in loaded {
                    let (tile, entry) = r?;
                    entries.insert(entry.id, entry);
                    tiles.push(tile);
                }
                finer.insert(fr, tiles);
            }
            let results = in_columns(grid.tile_cols(level), opts.threads, |col| {
                write_tile(out, &coarse_tile(&grid, level, row, col, &finer))
            });
            for r in results {
                let (entry, wrote) = r?;
                if wrote {
                    written += 1;
                } else {
                    skipped += 1;
                }
                entries.insert(entry.id, entry);
            }
        }
        if opts.progress {
            println!("finish: level {level} built");
        }
    }
    // Every tile exists at every level. A tile no coarser row read (only
    // possible on the top level, or with a single level) is hashed here.
    for level in grid.levels() {
        for row in 0..grid.tile_rows(level) {
            for col in 0..grid.tile_cols(level) {
                let key = TileKey { level, row, col };
                if !entries.contains_key(&key.id()) {
                    let (_, entry) = read_tile(out, key)?;
                    entries.insert(entry.id, entry);
                }
            }
        }
    }
    let mut levels: Vec<LevelSummary> = grid
        .levels()
        .map(|level| LevelSummary {
            level,
            tile_rows: grid.tile_rows(level),
            tile_cols: grid.tile_cols(level),
            tiles: 0,
            bytes: 0,
        })
        .collect();
    for e in entries.values() {
        let level = TileKey::from_id(e.id).expect("a tile id").level as usize;
        levels[level].tiles += 1;
        levels[level].bytes += e.length;
    }
    let mut hashes: Vec<[u8; DIGEST_LEN]> = entries.values().map(|e| e.sha256).collect();
    hashes.sort_unstable();
    hashes.dedup();
    let tiles = entries.len() as u64;
    let bytes = entries.values().map(|e| e.length).sum();
    let m = Manifest::new(entries.into_values().collect())
        .expect("tile ids are unique and fit a u32 count");
    let mbytes = manifest::encode(&m);
    write_atomic(&out.join(MANIFEST_FILE), &mbytes)?;
    let report = Report {
        grid,
        levels,
        tiles,
        distinct: hashes.len() as u64,
        bytes,
        manifest_bytes: mbytes.len() as u64,
        manifest_hash: manifest::manifest_hash(&mbytes),
        written,
        skipped,
    };
    write_atomic(&out.join(SUMMARY_FILE), report.summary_json().as_bytes())?;
    Ok(report)
}

/// Writes `bytes` to `path` through a temporary file in the same directory.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), PyramidError> {
    let io = |e| PyramidError::Io(path.to_path_buf(), e);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes).map_err(io)?;
    fs::rename(&tmp, path).map_err(io)
}
