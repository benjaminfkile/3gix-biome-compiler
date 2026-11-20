//! The synthetic world as a pyramid: `biome-tiler synth`.
//!
//! Writes every level 0 tile of [`biome_tiles::fixture::level0_tile`]
//! through [`crate::pyramid::write_level0`], then runs
//! [`crate::pyramid::finish`]. Tests build small grids this way, and the
//! full grid gives the timing and size of a whole pyramid. Never real data.

use std::path::Path;

use biome_tiles::fixture::level0_tile;
use biome_tiles::Grid;

use crate::pyramid::{finish, write_level0, Options, PyramidError, Report};

/// Writes the synthetic world's level 0 tiles of `grid` into `out` and
/// builds the rest of the pyramid. Returns [`finish`]'s report and the
/// level 0 tiles written and already present.
pub fn synth(out: &Path, grid: Grid, opts: Options) -> Result<(Report, u64, u64), PyramidError> {
    let (written, skipped) = write_level0(out, grid, opts.threads, |row, col| {
        level0_tile(&grid, row, col)
    })?;
    if opts.progress {
        println!("synth: level 0 written");
    }
    let report = finish(out, grid, opts)?;
    Ok((report, written, skipped))
}
