//! `biome-tiler`: builds the biome tile pyramid and uploads it to a hub
//! (`client-compute.md` section 5.3).
//!
//! ```text
//! biome-tiler finish <dir> [--grid ROWSxCOLS] [--threads N]
//! biome-tiler synth <dir> [--grid ROWSxCOLS] [--threads N]
//! biome-tiler modis-cut <composites> <dir> [--grid ROWSxCOLS] [--threads N]
//! biome-tiler upload <dir>
//! ```
//!
//! `finish` builds levels 1 to 8 from the level 0 tiles in `<dir>`, then
//! `manifest.bin` and `summary.json`. `synth` writes the level 0 tiles of
//! the synthetic world, then runs `finish`. `modis-cut` writes the level 0
//! tiles from the MODIS composites in `<composites>`, then runs `finish`. Both use the 43200 by 86400
//! grid unless `--grid` names a smaller one of the same layout. `upload`
//! reads `GX_HUB_URL` and `GX_API_KEY` from the environment.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use biome_tiler::modis::modis_cut;
use biome_tiler::pyramid::{finish, Options, Report};
use biome_tiler::synth::synth;
use biome_tiler::upload::{upload, Hub};
use biome_tiles::Grid;
use gx_core::sha256::to_hex;

const USAGE: &str = "usage:
  biome-tiler finish <dir> [--grid ROWSxCOLS] [--threads N]
  biome-tiler synth <dir> [--grid ROWSxCOLS] [--threads N]
  biome-tiler modis-cut <composites> <dir> [--grid ROWSxCOLS] [--threads N]
  biome-tiler upload <dir>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("biome-tiler: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("finish") => {
            let (dir, grid, opts) = parse(&args[1..])?;
            let start = Instant::now();
            let r = finish(&dir, grid, opts).map_err(|e| e.to_string())?;
            print_report("finish", &r, start);
            Ok(())
        }
        Some("synth") => {
            let (dir, grid, opts) = parse(&args[1..])?;
            let start = Instant::now();
            let (r, written, skipped) = synth(&dir, grid, opts).map_err(|e| e.to_string())?;
            println!("synth: level 0: {written} tiles written, {skipped} already present");
            print_report("synth", &r, start);
            Ok(())
        }
        Some("modis-cut") => {
            let [composites, rest @ ..] = &args[1..] else {
                return Err(USAGE.into());
            };
            let (dir, grid, opts) = parse(rest)?;
            let start = Instant::now();
            let (r, written, skipped) = modis_cut(&PathBuf::from(composites), &dir, grid, opts)
                .map_err(|e| e.to_string())?;
            println!("modis-cut: level 0: {written} tiles written, {skipped} already present");
            print_report("modis-cut", &r, start);
            Ok(())
        }
        Some("upload") => {
            let [dir] = &args[1..] else {
                return Err(USAGE.into());
            };
            let hub = Hub::from_env().map_err(|e| e.to_string())?;
            let r = upload(&PathBuf::from(dir), hub, true).map_err(|e| e.to_string())?;
            println!(
                "upload: {} tiles, {} sent, {} skipped, {} bytes sent",
                r.tiles, r.sent, r.skipped, r.bytes_sent
            );
            println!("manifest {}", to_hex(&r.manifest_hash));
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn print_report(what: &str, r: &Report, start: Instant) {
    println!(
        "{what}: {} tiles ({} coarse written, {} already present), {} tile bytes, {:.2} s",
        r.tiles,
        r.written,
        r.skipped,
        r.bytes,
        start.elapsed().as_secs_f64()
    );
    println!("manifest {}", to_hex(&r.manifest_hash));
}

fn parse(args: &[String]) -> Result<(PathBuf, Grid, Options), String> {
    let mut positional = Vec::new();
    let mut grid = Grid::BIOME;
    let mut opts = Options {
        progress: true,
        ..Options::default()
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--grid" => {
                let v = it.next().ok_or(USAGE)?;
                let (r, c) = v.split_once('x').ok_or_else(|| format!("--grid: {v}"))?;
                let parse = |s: &str| s.parse::<u32>().map_err(|_| format!("--grid: {v}"));
                grid = Grid::new(parse(r)?, parse(c)?)
                    .ok_or_else(|| format!("--grid: {v} is not a pyramid grid"))?;
            }
            "--threads" => {
                let v = it.next().ok_or(USAGE)?;
                opts.threads = v.parse().map_err(|_| format!("--threads: {v}"))?;
            }
            _ => positional.push(a),
        }
    }
    let [dir] = positional[..] else {
        return Err(USAGE.into());
    };
    Ok((PathBuf::from(dir), grid, opts))
}
