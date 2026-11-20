//! The biome tiler: builds the tile pyramid of [`biome_tiles`] and uploads
//! it to a hub as a source set.
//!
//! This is the producing side of `client-compute.md` section 5 (source
//! data): section 5.3 says a source set is made by a tool in the compiler's
//! repository that reads the raw dataset from local disk, writes tiles,
//! uploads each with `PUT /source/{hash}`, then uploads the manifest of
//! section 5.1. The layer it feeds is the overlay of section 2.2 and section
//! 10.
//!
//! - [`pyramid`]: the level 0 writer and the deterministic, resumable
//!   builder of the coarser levels, the manifest, and the summary.
//! - [`synth`]: the synthetic world as a pyramid, for tests and timing.
//! - [`upload`]: the uploader.

#![warn(missing_docs)]

pub mod pyramid;
pub mod synth;
pub mod upload;
