//! The biome overlay module: gives the land of one frame its measured
//! albedo from the biome tile pyramid.
//!
//! A compiler module in the sense of `client-compute.md` section 2.2 and
//! section 10: a WebAssembly binary with no imports, built with
//! `gx-module-guest` to module ABI v2 (`client-compute.md` section 4.1),
//! for a layer with `overlay` true. Its output is overlay tiles
//! (`matter-format.md` section 12) that the renderer composites over the
//! frame's surface (`rendering.md` section 7 item 6).
//!
//! - [`BiomeModule`]: the module. Its layer reads one input, a frames slot
//!   with the record of the frame it fills (`matter-format.md` section 11),
//!   which the layer config chooses: `"inputs": [{"layer": "system",
//!   "frames": [4]}]`. `gx_inputs` returns an empty list; `gx_sources` lists
//!   the tiles a surface key reads ([`overlay::sources`]); `gx_compile`
//!   answers every key ([`compile`]).
//! - [`frame`]: which frame the module fills, from the frame record.
//! - [`geodesy`]: the WGS84 ellipsoid and a deterministic inverse tangent.
//! - [`overlay`](mod@overlay): one surface tile's coverage, albedo, and
//!   roughness, and the tiles it reads.
//! - `fixture` and `conformance` (native only): the synthetic tiles and the
//!   conformance vectors.
//!
//! What it answers (`client-compute.md` section 2.2): a surface key of the
//! record's frame gets an overlay tile, or the empty overlay tile where it
//! holds no land; a surface key of any other frame the empty overlay tile;
//! a cell key the empty section; the registry key the empty registry; a
//! frame key the empty record. The layer runs with `usesOrigin` false and
//! `spanSeconds` 0, so every output is a function of the key, the frame
//! record, and the tiles alone, and identical inputs give identical bytes
//! natively and in WebAssembly (`space-model.md` section 1 rule 7).

#![warn(missing_docs)]

#[cfg(not(target_arch = "wasm32"))]
pub mod conformance;
#[cfg(not(target_arch = "wasm32"))]
pub mod fixture;
pub mod frame;
pub mod geodesy;
pub mod overlay;

use std::collections::BTreeMap;

use biome_tiles::Tile;
use gx_core::frame_record::{self as record, FrameRecord};
use gx_core::key::ChunkKey;
use gx_core::matter::{self, Compression, Section};
use gx_core::module_io::InputEntry;
use gx_core::registry::{self, Registry};
use gx_core::units::Meters;
use gx_module_guest::{gx_module_v2, FramesSlot, Input, ModuleV2, Request, Tiles};

pub use frame::{Frame, FrameError};

/// The biome overlay module (`client-compute.md` sections 4.1 and 10).
///
/// The request's origin and span are ignored. Every key but a frame key
/// needs the one non-empty frame record ([`Frame::from_records`]);
/// otherwise `gx_sources` lists no tiles and `gx_compile` fails with the
/// reason.
pub struct BiomeModule;

/// The tiles `key` reads in `frame`: the surface keys of the frame only.
pub fn sources(frame: &Frame, key: &ChunkKey) -> Vec<u64> {
    match key {
        ChunkKey::Surface(s) if s.frame_id == frame.frame_id => overlay::sources(s),
        _ => Vec::new(),
    }
}

/// Decodes the tiles passed to `gx_compile`, keyed by id.
pub fn decode_tiles<'a>(
    tiles: impl Iterator<Item = (u64, &'a [u8])>,
) -> Result<BTreeMap<u64, Tile>, String> {
    let mut out = BTreeMap::new();
    for (id, bytes) in tiles {
        out.insert(id, Tile::decode_id(id, bytes).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// The answer to `key` in `frame` from `tiles` (see the crate
/// documentation).
pub fn compile(
    frame: &Frame,
    key: &ChunkKey,
    tiles: &BTreeMap<u64, Tile>,
) -> Result<Vec<u8>, String> {
    match key {
        ChunkKey::Registry => Ok(registry::encode(&Registry::empty())),
        ChunkKey::Frame(id) => Ok(record::encode(&FrameRecord::empty(*id))),
        ChunkKey::Cell(cell) => {
            let g = cell.geometry(Meters::new(frame.root_extent));
            let s = Section::empty(*cell, g.origin, g.edge).map_err(|e| e.to_string())?;
            Ok(matter::encode(&s, Compression::None))
        }
        ChunkKey::Surface(s) if s.frame_id == frame.frame_id => {
            overlay::encode(s, tiles).map_err(|e| e.to_string())
        }
        ChunkKey::Surface(s) => Ok(gx_core::overlay::encode_empty(s)),
    }
}

impl ModuleV2 for BiomeModule {
    /// Slot 0 lists one frame, the one the layer fills.
    const FRAMES_SLOTS: &'static [FramesSlot] = &[FramesSlot { slot: 0, frames: 1 }];

    /// The layer has no layer slots: always empty.
    fn inputs(_req: &Request, _frames: &[FrameRecord]) -> Vec<InputEntry> {
        Vec::new()
    }

    /// The tiles the request's surface key reads; none for every other key
    /// and for records that do not describe the frame.
    fn sources(req: &Request, frames: &[FrameRecord]) -> Vec<u64> {
        match Frame::from_records(frames) {
            Ok(f) => sources(&f, &req.key),
            Err(_) => Vec::new(),
        }
    }

    /// Decodes the tiles and answers the request's key.
    fn compile(
        req: &Request,
        tiles: &Tiles,
        frames: &[FrameRecord],
        _inputs: &[Input],
    ) -> Result<Vec<u8>, String> {
        if let ChunkKey::Frame(id) = req.key {
            return Ok(record::encode(&FrameRecord::empty(id)));
        }
        let frame = Frame::from_records(frames).map_err(|e| format!("{}: {e}", req.key))?;
        let decoded = decode_tiles(tiles.as_slice().iter().map(|t| (t.id, t.bytes)))
            .map_err(|e| format!("{}: {e}", req.key))?;
        compile(&frame, &req.key, &decoded).map_err(|e| format!("{}: {e}", req.key))
    }
}

gx_module_v2!(BiomeModule);
