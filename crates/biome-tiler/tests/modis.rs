//! `modis-cut` on synthetic composites written by the test: samples are
//! checked against the sinusoidal mapping worked out by hand.

use std::fs;
use std::path::{Path, PathBuf};

use biome_tiler::modis::{
    albedo_byte, composite_path, modis_cut, read_composite, Composite, NONE, PIXELS,
};
use biome_tiler::pyramid::{read_tile, Options};
use biome_tiles::Grid;

const R: f64 = 6_371_007.181;
const T: f64 = 1_111_950.519_667;

/// 1000 by 2000 samples, 0.18 degrees apart.
fn small() -> Grid {
    Grid::new(1000, 2000).unwrap()
}

/// The synthetic composite of tile `(h, v)`: every pixel's values are a
/// function of the tile and the pixel.
fn composite(h: u32, v: u32) -> Composite {
    let n = PIXELS * PIXELS;
    let mut bands = vec![0u16; 3 * n];
    let mut land = vec![0u8; n];
    for r in 0..PIXELS {
        for c in 0..PIXELS {
            let i = r * PIXELS + c;
            let (pr, pc) = (r as u32, c as u32);
            bands[i] = ((pr * 31 + pc * 17) % 1000) as u16;
            bands[n + i] = (h * 20 + v) as u16;
            bands[2 * n + i] = ((pr + pc) % 1001) as u16;
            land[i] = ((pr * 3 + pc) % 254 + 1) as u8;
        }
    }
    Composite {
        h: h as u8,
        v: v as u8,
        bands,
        land,
    }
}

/// What the composite of `(h, v)` holds at pixel `(pr, pc)`, as tile bytes.
fn expected(h: u32, v: u32, pr: u32, pc: u32) -> (u8, [u8; 3]) {
    let b = |x: u32| ((f64::from(x) / 1000.0 * 255.0) + 0.5).floor() as u8;
    (
        ((pr * 3 + pc) % 254 + 1) as u8,
        [
            b((pr * 31 + pc * 17) % 1000),
            b(h * 20 + v),
            b((pr + pc) % 1001),
        ],
    )
}

/// The MODIS tile and pixel of lattice sample `(r, c)` of `small()`, by
/// the formulas of the brief: `(h, v, pixel row, pixel col)`.
fn by_hand(r: u32, c: u32) -> (u32, u32, u32, u32) {
    let s = 0.18_f64.to_radians();
    let lat = 90_f64.to_radians() - (f64::from(r) + 0.5) * s;
    let lon = -180_f64.to_radians() + (f64::from(c) + 0.5) * s;
    let (x, y) = (R * lon * lat.cos(), R * lat);
    let size = T / 2400.0;
    let h = ((x + 20_015_109.354) / T).floor();
    let v = ((10_007_554.677 - y) / T).floor();
    let pc = ((x - (-20_015_109.354 + h * T)) / size).floor();
    let pr = ((10_007_554.677 - v * T - y) / size).floor();
    (h as u32, v as u32, pr as u32, pc as u32)
}

fn dirs(name: &str) -> (PathBuf, PathBuf) {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&base);
    let (c, o) = (base.join("composites"), base.join("out"));
    fs::create_dir_all(&c).unwrap();
    (c, o)
}

fn read(dir: &Path, grid: &Grid, r: u32, c: u32) -> (u8, [u8; 3]) {
    let (key, i) = grid.locate(0, r, c);
    let (t, _) = read_tile(dir, key).unwrap();
    (t.land(i), t.albedo(i))
}

#[test]
fn samples_follow_the_sinusoidal_mapping() {
    let grid = small();
    let (comp, out) = dirs("modis-cut");
    for (h, v) in [(19, 8), (20, 8), (18, 2), (0, 8), (35, 8)] {
        fs::write(composite_path(&comp, h, v), composite(h, v).to_bytes()).unwrap();
    }
    let opts = Options {
        threads: 3,
        progress: false,
    };
    let (report, written, skipped) = modis_cut(&comp, &out, grid, opts).unwrap();
    assert_eq!((written, skipped), (45, 0));

    // (row, col, h, v) worked out by hand: latitude 90 - (r + 1/2) 0.18
    // degrees, longitude -180 + (c + 1/2) 0.18 degrees, and since
    // T = R pi / 18, h = 18 + lon cos(lat) / 10 degrees and
    // v = (90 - lat) / 10 degrees, rounded down.
    let cases = [
        // The equator: 0.09 N, 15.03 E.
        (499, 1083, 19, 8),
        // 60 degrees north: 60.03 N, 15.03 E, x as at 7.51 E on the equator.
        (166, 1083, 18, 2),
        // Across a MODIS tile edge: 19.89 E and 20.07 E at 0.09 N.
        (499, 1110, 19, 8),
        (499, 1111, 20, 8),
        // Across the 180 degree line: 179.91 W and 179.91 E.
        (499, 0, 0, 8),
        (499, 1999, 35, 8),
    ];
    for (r, c, h, v) in cases {
        let (bh, bv, pr, pc) = by_hand(r, c);
        assert_eq!((bh, bv), (h, v), "sample ({r}, {c})");
        assert_eq!(
            read(&out, &grid, r, c),
            expected(h, v, pr, pc),
            "sample ({r}, {c})"
        );
    }
    // The equator sample by hand: y = R 0.09 deg = 10007.55 m, pixel row
    // (T - y) / s = 2378.4.
    assert_eq!(by_hand(499, 1083).2, 2378);
    // The 180 degree line: x = -R 179.91 deg cos(0.09 deg), as far east of
    // the west edge as 0.0902 deg on the equator, 0.0902 / 10 * 2400 = 21.6
    // pixels.
    assert_eq!(by_hand(499, 0).3, 21);

    // No composite (89.91 W on the equator, tile (9, 8)): land 0.
    assert_eq!(by_hand(499, 500).0, 9);
    assert_eq!(read(&out, &grid, 499, 500), (0, [0; 3]));

    // Running again writes nothing and gives the same manifest.
    let (again, written, skipped) = modis_cut(&comp, &out, grid, opts).unwrap();
    assert_eq!((written, skipped), (0, 45));
    assert_eq!(again.manifest_hash, report.manifest_hash);
}

#[test]
fn values_and_files() {
    assert_eq!(albedo_byte(0), 0);
    assert_eq!(albedo_byte(2), 1); // 0.51 rounds up
    assert_eq!(albedo_byte(1000), 255);
    assert_eq!(albedo_byte(32766), 255);

    let mut c = composite(3, 4);
    c.bands[PIXELS * PIXELS + 5] = NONE;
    c.land[6] = 0;
    assert_eq!(c.sample(0, 5), (0, [0; 3]));
    assert_eq!(c.sample(0, 6), (0, [0; 3]));
    assert_eq!(c.sample(0, 7), expected(3, 4, 0, 7));

    let (dir, _) = dirs("modis-files");
    fs::write(composite_path(&dir, 3, 4), c.to_bytes()).unwrap();
    assert_eq!(read_composite(&dir, 3, 4).unwrap(), Some(c));
    assert_eq!(read_composite(&dir, 3, 5).unwrap(), None);
    fs::copy(composite_path(&dir, 3, 4), composite_path(&dir, 4, 4)).unwrap();
    assert!(read_composite(&dir, 4, 4).is_err());
}
