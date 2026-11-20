//! The biome module's command line: list the sources of a key, compile and
//! describe a key natively, time it, and regenerate the conformance
//! vectors. It calls the same functions the module runs, with the system
//! module's record of frame 4 ([`fixture::record`]) as the layer's frame
//! record.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use biome_module::{compile, conformance, decode_tiles, fixture, sources};
use biome_tiles::{Tile, TileKey};
use gx_core::key::ChunkKey;
use gx_core::overlay::{self, Overlay};
use gx_core::sha256::{sha256, to_hex};

const USAGE: &str = "usage:
  biome-module sources KEY
  biome-module describe KEY (--fixture | --tiles DIR)
  biome-module compile KEY (--fixture | --tiles DIR) --out FILE
  biome-module bench RUNS KEY... (--fixture | --tiles DIR)
  biome-module conformance DIR

--tiles DIR reads DIR/{level}/{row}/{col}.3gba, the layout biome-tiler
writes under out/tiles. --fixture uses the synthetic fixture tiles.

bench times what gx_compile does for each key (decode the tile bytes,
compile, encode), RUNS times, and prints the median, minimum, and maximum
in milliseconds. The tiles are read or made once, before timing.

Every command uses the system module's record of frame 4 as the frame
record.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let cmd = args.first().ok_or(USAGE)?;
    match cmd.as_str() {
        "sources" => {
            let key = parse_key(args.get(1))?;
            for id in sources(&fixture::frame(), &key) {
                let k = TileKey::from_id(id).ok_or("bad id")?;
                println!("{id} {}/{}/{}", k.level, k.row, k.col);
            }
            Ok(())
        }
        "compile" | "describe" => {
            let key = parse_key(args.get(1))?;
            let tiles = load_tiles(&key, &args[2..])?;
            let bytes = compile(&fixture::frame(), &key, &tiles)?;
            eprintln!("{} bytes sha256 {}", bytes.len(), to_hex(&sha256(&bytes)));
            if cmd == "describe" {
                describe(&key, &bytes)
            } else {
                let out = flag(&args[2..], "--out").ok_or(USAGE)?;
                std::fs::write(&out, &bytes).map_err(|e| format!("{out}: {e}"))
            }
        }
        "bench" => bench(&args[1..]),
        "conformance" => {
            let dir = PathBuf::from(args.get(1).ok_or(USAGE)?);
            conformance::write(&dir)
        }
        _ => Err(USAGE.to_string()),
    }
}

/// `bench RUNS KEY... (--fixture | --tiles DIR)`: see [`USAGE`].
fn bench(args: &[String]) -> Result<(), String> {
    let runs: usize = args
        .first()
        .ok_or(USAGE)?
        .parse()
        .map_err(|e| format!("runs: {e}"))?;
    let opts = args
        .iter()
        .position(|a| a.starts_with("--"))
        .unwrap_or(args.len());
    for k in &args[1..opts] {
        let key = parse_key(Some(k))?;
        let tiles = load_tiles(&key, &args[opts..])?;
        let encoded: Vec<(u64, Vec<u8>)> = tiles.iter().map(|(&id, t)| (id, t.encode())).collect();
        let mut times = Vec::with_capacity(runs);
        let mut out = Vec::new();
        for _ in 0..runs.max(1) {
            let start = std::time::Instant::now();
            let decoded = decode_tiles(encoded.iter().map(|(id, b)| (*id, b.as_slice())))?;
            out = compile(&fixture::frame(), &key, &decoded)?;
            times.push(start.elapsed().as_secs_f64() * 1e3);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "{key} tiles {} bytes {} sha256 {} median {:.2} ms min {:.2} max {:.2}",
            encoded.len(),
            out.len(),
            &to_hex(&sha256(&out))[..16],
            times[times.len() / 2],
            times[0],
            times[times.len() - 1]
        );
    }
    Ok(())
}

fn parse_key(s: Option<&String>) -> Result<ChunkKey, String> {
    s.ok_or(USAGE)?
        .parse::<ChunkKey>()
        .map_err(|e| format!("key: {e}"))
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn load_tiles(key: &ChunkKey, args: &[String]) -> Result<BTreeMap<u64, Tile>, String> {
    let ids = sources(&fixture::frame(), key);
    if args.iter().any(|a| a == "--fixture") {
        return Ok(fixture::tiles(&ids));
    }
    let dir = flag(args, "--tiles").ok_or(USAGE)?;
    let mut out = BTreeMap::new();
    for id in ids {
        let k = TileKey::from_id(id).ok_or("bad id")?;
        let path = Path::new(&dir)
            .join(k.level.to_string())
            .join(k.row.to_string())
            .join(format!("{}.3gba", k.col));
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        out.insert(id, Tile::decode_id(id, &bytes).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn describe(key: &ChunkKey, bytes: &[u8]) -> Result<(), String> {
    let ChunkKey::Surface(s) = key else {
        println!("{{\"key\":\"{key}\",\"bytes\":{}}}", bytes.len());
        return Ok(());
    };
    match overlay::decode(s, bytes).map_err(|e| e.to_string())? {
        Overlay::Empty(_) => println!("{{\"key\":\"{key}\",\"overlay\":\"empty\"}}"),
        Overlay::Tile(t) => {
            let full = t.coverage.iter().filter(|&&c| c == 255).count();
            let none = t.coverage.iter().filter(|&&c| c == 0).count();
            let partial = t.coverage.len() - full - none;
            println!(
                "{{\"key\":\"{key}\",\"full\":{full},\"partial\":{partial},\"none\":{none},\"albedo0\":{:?}}}",
                t.albedo[0]
            );
        }
    }
    Ok(())
}
