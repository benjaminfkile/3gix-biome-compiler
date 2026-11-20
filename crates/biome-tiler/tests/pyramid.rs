//! The pyramid builder on the synthetic world: the coarser level rule
//! against a direct reading of the level below, determinism across thread
//! counts, resuming, and golden hashes on the full grid.

use std::fs;
use std::path::{Path, PathBuf};

use biome_tiler::pyramid::{finish, read_tile, tile_path, Options, Report, MANIFEST_FILE};
use biome_tiler::synth::synth;
use biome_tiles::grid::coarsen;
use biome_tiles::{Grid, TileKey};
use gx_core::sha256::{sha256, to_hex};

/// 1000 by 2000 samples: four levels, padding rows past the south pole.
fn small() -> Grid {
    Grid::new(1000, 2000).unwrap()
}

fn run(name: &str, grid: Grid, threads: usize) -> (PathBuf, Report, u64, u64) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    let opts = Options {
        threads,
        progress: false,
    };
    let (r, w, s) = synth(&dir, grid, opts).unwrap();
    (dir, r, w, s)
}

/// Land and albedo bytes of lattice sample `(r, c)` of `level`, read from
/// the tile files.
fn read(dir: &Path, grid: &Grid, level: u8, r: u32, c: u32) -> (u8, [u8; 3]) {
    let (key, i) = grid.locate(level, r, c);
    let (t, _) = read_tile(dir, key).unwrap();
    (t.land(i), t.albedo(i))
}

#[test]
fn coarser_levels_follow_the_rule() {
    let g = small();
    let (dir, report, _, _) = run("rule", g, 2);
    assert_eq!(g.top_level(), 3);
    assert_eq!(report.levels.len(), 4);
    for level in 1..=g.top_level() {
        let rows = [0, 1, 7, g.data_rows(level) - 1, g.lattice_rows(level) - 1];
        let cols = [0, 5, 239, 240, g.lattice_cols(level) - 1];
        for &r in &rows {
            for &c in &cols {
                // The four finer samples: rows clamped to the globe,
                // columns wrapped around it.
                let mut land = [0u8; 4];
                let mut albedo = [[0u8; 3]; 4];
                for i in 0..2 {
                    for j in 0..2 {
                        let fr = (2 * r + i).min(g.data_rows(level - 1) - 1);
                        let fc = (2 * c + j) % g.wrap_cols(level - 1);
                        let (l, a) = read(&dir, &g, level - 1, fr, fc);
                        land[(2 * i + j) as usize] = l;
                        albedo[(2 * i + j) as usize] = a;
                    }
                }
                assert_eq!(
                    read(&dir, &g, level, r, c),
                    coarsen(land, albedo),
                    "level {level} sample ({r}, {c})"
                );
            }
        }
    }
    // The synthetic world has ocean, land, and coast on level 1.
    let mut seen = [false; 3];
    for c in 0..g.lattice_cols(1) {
        for r in (0..g.data_rows(1)).step_by(7) {
            let (l, _) = read(&dir, &g, 1, r, c);
            seen[match l {
                0 => 0,
                255 => 1,
                _ => 2,
            }] = true;
        }
    }
    assert_eq!(seen, [true; 3]);
}

#[test]
fn the_same_bytes_on_any_thread_count_and_resumable() {
    let g = small();
    let (a, ra, wa, _) = run("threads-1", g, 1);
    let (b, rb, _, _) = run("threads-3", g, 3);
    assert_eq!(ra.manifest_hash, rb.manifest_hash);
    assert_eq!(ra.summary_json(), rb.summary_json());
    assert_eq!(
        fs::read(a.join(MANIFEST_FILE)).unwrap(),
        fs::read(b.join(MANIFEST_FILE)).unwrap()
    );
    assert_eq!(wa, u64::from(g.tile_rows(0) * g.tile_cols(0)));
    assert_eq!(ra.written + 45, ra.tiles);

    // Running again writes nothing and gives the same manifest.
    let opts = Options {
        threads: 4,
        progress: false,
    };
    let (again, w, s) = synth(&a, g, opts).unwrap();
    assert_eq!((w, again.written), (0, 0));
    assert_eq!(s + again.skipped, again.tiles);
    assert_eq!(again.manifest_hash, ra.manifest_hash);

    // A damaged coarse tile is rewritten by finish.
    let p = tile_path(
        &a,
        TileKey {
            level: 2,
            row: 1,
            col: 2,
        },
    );
    fs::write(&p, b"damaged").unwrap();
    let fixed = finish(&a, g, opts).unwrap();
    assert_eq!(fixed.written, 1);
    assert_eq!(fixed.manifest_hash, ra.manifest_hash);

    // A missing level 0 tile is an error, never a guess.
    let p = tile_path(
        &a,
        TileKey {
            level: 0,
            row: 2,
            col: 4,
        },
    );
    fs::remove_file(&p).unwrap();
    let e = finish(&a, g, opts).unwrap_err();
    assert!(e.to_string().contains("4.3gba"), "{e}");
}

#[test]
fn golden_tiles_on_the_full_grid() {
    let (dir, report, _, _) = run("full", Grid::BIOME, Options::default().threads);
    let golden = [
        (
            (0, 0, 0),
            "ac5150697d448be9eddd534be704035eb95c44eb73b0c3b825626eb81c2df700",
        ),
        (
            (0, 60, 100),
            "7c78529f098a3d88bb22d134972db8fc7cef1c8064af5347213e636980229119",
        ),
        (
            (0, 90, 180),
            "9a45984c560151ec63d4220eb36f06c09a8bfa21da73d6e0482a60f5116c7687",
        ),
        (
            (0, 179, 359),
            "bb7b512c7925645224458cd8f48e99bc985cdbb563ad92f06b1084c9855b7e2a",
        ),
        (
            (8, 0, 0),
            "c2152e3e128d9ff7ac809e83b2b4c6488fd75ca38c43d756e4e0e6b4219c57ce",
        ),
        (
            (8, 0, 1),
            "d61188a935b1a8c76f37c328d370552660ef9bd5f868f4b90009e63b63d346b0",
        ),
    ];
    for ((level, row, col), want) in golden {
        let key = TileKey { level, row, col };
        let bytes = fs::read(tile_path(&dir, key)).unwrap();
        assert_eq!(to_hex(&sha256(&bytes)), want, "{key}");
    }
    assert_eq!(report.tiles, 86459);
    assert_eq!(
        to_hex(&report.manifest_hash),
        "eb8a8567fe6a6e0581394cabf0042dba3e1efa3bb612cd9e8236b4997cfb7244"
    );
    // One level 0 tile is one degree, every tile exists at every level.
    assert_eq!(report.levels[0].tiles, 64800);
    assert_eq!(report.levels[8].tiles, 2);
    fs::remove_dir_all(&dir).unwrap();
}
