//! The module's conformance vectors: fixed requests, the fixture tiles each
//! needs, and the SHA-256 of the answer each must produce.
//!
//! Implements the determinism check of `space-model.md` section 4 item 6
//! for this module. `conformance/` holds, per vector, `NAME.request.bin`
//! (the request bytes of `client-compute.md` section 4), `NAME.frames.bin`
//! (the frame bytes passed to `gx_inputs` and `gx_sources`, section 4.1:
//! the system module's record of the frame, [`crate::fixture::record`]),
//! `NAME.input-list.bin` (the input list `gx_inputs` returns: always
//! empty), `NAME.tiles.bin` (the tile bytes passed to `gx_compile`: the
//! fixture tiles of [`crate::fixture`] for exactly the ids `gx_sources`
//! returns), `NAME.inputs.bin` (the input bytes passed to `gx_compile`: the
//! frame record alone), and `NAME.sha256` (lowercase hex of the expected
//! answer), plus `index.json` saying what each covers. The core
//! determinism harness runs the built module over them through module ABI
//! v2 natively and under Node and requires these hashes from both.
//! [`write`] regenerates the directory with the module's own entry points
//! ([`gx_module_guest::run_inputs`], [`gx_module_guest::run_sources_v2`],
//! and [`gx_module_guest::run_compile_v2`]).
//!
//! Requests carry origin 0 and span 0, as the hub sends them to a layer
//! with `usesOrigin` false and `spanSeconds` 0.

use std::fmt::Write as _;
use std::path::Path;

use biome_tiles::TileKey;
use gx_core::frame_record;
use gx_core::module_io::{
    decode_source_ids, encode_inputs, encode_request, encode_tiles, ModuleRequest, Tile,
};
use gx_core::sha256::{sha256, to_hex};
use gx_module_guest::{run_compile_v2, run_inputs, run_sources_v2};

use crate::{fixture, BiomeModule};

/// One conformance vector.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Vector {
    /// File name stem.
    pub name: &'static str,
    /// The chunk key.
    pub key: &'static str,
    /// What the vector covers.
    pub why: &'static str,
}

/// The vectors.
pub const VECTORS: [Vector; 10] = [
    v(
        "01-level0",
        "s-4-0-0-0-0",
        "a level 0 tile: face 0, read from pyramid level 8",
    ),
    v(
        "02-coast",
        "s-4-0-10-21-609",
        "a deep tile on a coast: partial coverage",
    ),
    v(
        "03-inland",
        "s-4-0-10-0-315",
        "an inland tile: full coverage",
    ),
    v(
        "04-ocean",
        "s-4-0-10-0-0",
        "an all ocean tile: the empty overlay tile",
    ),
    v("05-polar", "s-4-4-3-3-3", "a tile holding the north pole"),
    v(
        "06-antimeridian",
        "s-4-1-5-15-10",
        "a tile on the 180 degree line, reading both sides of it",
    ),
    v("07-cell", "4-3-4-4-4", "a cell key: the empty section"),
    v("08-registry", "registry", "the empty registry"),
    v("09-frame", "frame-4", "a frame key: the empty record"),
    v(
        "10-other-frame",
        "s-5-0-2-1-1",
        "a surface key of another frame: the empty overlay tile",
    ),
];

const fn v(name: &'static str, key: &'static str, why: &'static str) -> Vector {
    Vector { name, key, why }
}

impl Vector {
    /// The request bytes.
    pub fn request(&self) -> Vec<u8> {
        encode_request(&ModuleRequest {
            origin_utc_ms: 0,
            span_seconds: 0,
            span_index: 0,
            key: self.key.parse().expect("conformance keys parse"),
        })
    }

    /// The frame bytes passed to `gx_inputs` and `gx_sources`: one frames
    /// slot holding the system module's record of the frame, none for a
    /// frame key.
    pub fn frames(&self) -> Vec<u8> {
        if self.key.starts_with("frame-") {
            // A frame key is passed no records (`client-compute.md`
            // section 4.1).
            return encode_inputs(&[]).expect("no records");
        }
        let record = frame_record::encode(&fixture::record());
        encode_inputs(&[&record]).expect("one record")
    }

    /// The input list the module returns, through its own `gx_inputs`
    /// logic.
    pub fn input_list(&self) -> Result<Vec<u8>, String> {
        run_inputs::<BiomeModule>(&self.request(), &self.frames())
    }

    /// The input bytes passed to `gx_compile`: the frame record alone, as
    /// the module reads no layer slot.
    pub fn inputs(&self) -> Vec<u8> {
        self.frames()
    }

    /// The tile ids the module asks for, through its own `gx_sources`
    /// logic.
    pub fn sources(&self) -> Result<Vec<u64>, String> {
        let ids = run_sources_v2::<BiomeModule>(&self.request(), &self.frames())?;
        decode_source_ids(&ids).map_err(|e| e.to_string())
    }

    /// The tile bytes: the fixture tiles of [`Vector::sources`].
    pub fn tiles(&self) -> Result<Vec<u8>, String> {
        let ids = self.sources()?;
        let encoded: Vec<(u64, Vec<u8>)> = ids
            .iter()
            .map(|&id| (id, fixture::tile(id).encode()))
            .collect();
        let tiles: Vec<Tile> = encoded
            .iter()
            .map(|(id, bytes)| Tile { id: *id, bytes })
            .collect();
        encode_tiles(&tiles).map_err(|e| e.to_string())
    }

    /// The answer the module returns, computed natively.
    pub fn answer(&self) -> Result<Vec<u8>, String> {
        run_compile_v2::<BiomeModule>(&self.request(), &self.tiles()?, &self.inputs())
    }
}

/// Every file of the conformance directory, `(file name, bytes)`, in name
/// order.
pub fn files() -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut index = String::from("{\n  \"cases\": {\n");
    for (i, vec) in VECTORS.iter().enumerate() {
        let tiles = vec.tiles()?;
        let hash = to_hex(&sha256(&run_compile_v2::<BiomeModule>(
            &vec.request(),
            &tiles,
            &vec.inputs(),
        )?));
        out.push((format!("{}.frames.bin", vec.name), vec.frames()));
        out.push((format!("{}.input-list.bin", vec.name), vec.input_list()?));
        out.push((format!("{}.inputs.bin", vec.name), vec.inputs()));
        out.push((format!("{}.request.bin", vec.name), vec.request()));
        out.push((
            format!("{}.sha256", vec.name),
            format!("{hash}\n").into_bytes(),
        ));
        out.push((format!("{}.tiles.bin", vec.name), tiles));
        let sources: Vec<String> = vec
            .sources()?
            .iter()
            .map(|&id| {
                let k = TileKey::from_id(id).expect("module ids are tile ids");
                format!("\"{}/{}/{}\"", k.level, k.row, k.col)
            })
            .collect();
        let comma = if i + 1 < VECTORS.len() { "," } else { "" };
        let _ = write!(
            index,
            "    \"{}\": {{\n      \"key\": \"{}\",\n      \"origin_utc_ms\": 0,\n      \"sha256\": \"{}\",\n      \"sources\": [{}],\n      \"why\": \"{}\"\n    }}{comma}\n",
            vec.name,
            vec.key,
            hash,
            sources.join(", "),
            vec.why
        );
    }
    index.push_str("  }\n}\n");
    out.push(("index.json".to_string(), index.into_bytes()));
    out.sort();
    Ok(out)
}

/// Writes every file of [`files`] into `dir`, creating it if needed.
pub fn write(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, bytes) in files()? {
        let p = dir.join(name);
        std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(())
}
