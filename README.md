# 3gix-biome-compiler

A 3GIX compiler module that gives a frame's ground its measured material as overlay tiles (`matter-format.md` section 12 in the 3GIXHub repository), plus the tool that cuts the source data into tiles. Compilers are allowed to know what things are: this one colours a planet's land from satellite measurements of surface albedo. Nothing it emits says so.

The overlay layer is described in `client-compute.md` section 2.2 and section 10, `matter-format.md` sections 10 to 12, and `rendering.md` section 7 item 6 in the 3GIXHub repository; the design is `docs/build/biome/plan.md` there. This repository holds:

- `crates/biome-tiles`: the tile format, the grid, and sampling. Builds for `wasm32-unknown-unknown`.
- `crates/biome-tiler`: the native CLI that builds the tile pyramid and uploads it to a hub as a source set.
- `crates/biome-module`: the overlay module (module ABI v2) and its native CLI, `biome-module`.
- `tools/tiles-wasm-check`: the determinism digest of `biome-tiles` as a WebAssembly export, run under Node by `scripts/ci.sh`.

- `scripts/modis`: the MODIS download and composite (Python), which `biome-tiler modis-cut` cuts into level 0.

No data is in this repository. Tests use a synthetic world (`biome_tiles::fixture`) and synthetic composites written by the test.

## Grid

An equirectangular grid at 15 arcseconds, 43200 rows by 86400 columns, cell centered: data row `r` (first row northernmost) lies on latitude `90 deg - (r + 1/2) s`, column `c` on longitude `-180 deg + (c + 1/2) s`, `s = 15 arcseconds`.

Tiles hold 240 x 240 samples. Level 0 has the data spacing (a tile is one degree), level `k` has `2^k` times it, levels 0 to 8. Columns past 180 degrees wrap; rows past a pole repeat the last row that touches the globe. A tile's id is `(level << 48) | (row << 24) | col`. Every tile exists at every level.

A coarser level is built in integer arithmetic from the 2 x 2 samples below each sample: land is the rounded mean of the four land values, and each albedo band the rounded land weighted mean `sum(l_i a_i) / sum(l_i)`, 0 when the four land values are 0. Rounding is half up.

## Tile format

One zstd frame (level 9, the zstd crate pinned exactly as the core library pins it) of:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic `3GBA` |
| 4 | 2 | version, `u16` LE, 1 |
| 6 | 1 | level, `u8` |
| 7 | 1 | reserved, 0 |
| 8 | 4 | row, `u32` LE |
| 12 | 4 | col, `u32` LE |
| 16 | 57600 | albedo band 0, `u8` |
| 57616 | 57600 | albedo band 1, `u8` |
| 115216 | 57600 | albedo band 2, `u8` |
| 172816 | 57600 | land fraction, `u8` |

Each block is 240 x 240 values, north row first, west column first. Albedo is reflectance `value / 255`, bands from long to short wavelength as in `matter-format.md`. Land fraction is `value / 255`.

## Sampling

`biome_tiles::sample` reads a level bilinearly over the four nearest sample centers, across tile edges, from tiles the caller supplies (a missing tile is an error). Land is the bilinear land fraction; albedo is land weighted, `sum(w_i l_i a_i) / sum(w_i l_i)`, 0 where there is no land, so ocean samples never tint a coast. `Grid::tiles_for_region` lists the tiles the stencil can read anywhere in a latitude and longitude box. Sampling uses only `+`, `-`, `*`, `/`, and `floor`, so native code and WebAssembly agree bit for bit; `scripts/ci.sh` checks it.

## Commands

```text
biome-tiler finish <dir> [--grid ROWSxCOLS] [--threads N]
biome-tiler synth <dir> [--grid ROWSxCOLS] [--threads N]
biome-tiler modis-cut <composites> <dir> [--grid ROWSxCOLS] [--threads N]
biome-tiler upload <dir>
```

- `finish` builds levels 1 to 8 from the level 0 tiles in `<dir>/tiles/0`, then writes `manifest.bin` (the core manifest format, tile id to the SHA-256 of its bytes) and `summary.json` (tiles and bytes per level, the manifest hash).
- `synth` writes the level 0 tiles of the synthetic world, then runs `finish`.
- `modis-cut` writes the level 0 tiles from the MODIS composites in `<composites>` (see below), then runs `finish`.
- `--grid` uses a smaller grid of the same layout, for tests.
- `upload` sends every tile with `PUT /source/{hash}` unless `HEAD /source/{hash}` says the hub has it, then the manifest. It reads `GX_HUB_URL` and `GX_API_KEY` from the environment only (see `.env.example`).

Output is deterministic (the same input gives the same bytes on any thread count) and resumable (a tile already on disk with the right hash is not rewritten).

## The module

`biome-module` is an overlay layer's module (`client-compute.md` section 2.2 and section 10). It reads one input, a frames slot holding the record of the frame it fills (`matter-format.md` section 11), and fills the frame that record names; `gx_inputs` returns an empty list. An empty record, or a number of records other than one, fails with the reason.

For a surface key `s-{frameId}-{face}-{level}-{x}-{y}` of the record's frame it samples every vertex's WGS84 latitude and longitude (bit for bit the terrain module's) on pyramid level `k(level)`, the coarsest whose equatorial sample spacing is no larger than the tile's vertex spacing there. Coverage is the land fraction, albedo the land albedo, roughness 0.9, each stored as `round(value * 255)`. The parent values at the even vertices are the same at `k(level - 1)`, so they equal the parent tile's own values. A tile with no coverage anywhere is the empty overlay tile. `gx_sources` lists the tiles of both levels the bilinear stencil can read in the tile's latitude and longitude box; a tile holding a pole spans every longitude. Surface keys of other frames get the empty overlay tile, cells the empty section, the registry key the empty registry, and frame keys the empty record.

The layer config `scripts/publish.sh` puts:

```json
{"module": "<module hash>", "sourceManifest": "<manifest hash>", "attachment": [4],
 "overlay": true, "required": true, "exclusive": false, "spanSeconds": 0, "usesOrigin": false,
 "inputs": [{"layer": "system", "frames": [4]}]}
```

```text
biome-module sources KEY
biome-module describe KEY (--fixture | --tiles DIR)
biome-module compile KEY (--fixture | --tiles DIR) --out FILE
biome-module bench RUNS KEY... (--fixture | --tiles DIR)
biome-module conformance DIR
sh scripts/build-module.sh
sh scripts/publish.sh SOURCE_MANIFEST_SHA256
```

- `--tiles DIR` reads `DIR/{level}/{row}/{col}.3gba` (the `tiles` directory `biome-tiler` writes); `--fixture` uses the synthetic world on the real tile ids.
- `conformance DIR` regenerates the vectors in `conformance/module` (`tests/conformance.rs` requires them to match; `scripts/ci.sh` runs them through the built module natively and under Node with the core determinism harness).
- `build-module.sh` builds `target/module/biome.wasm` and prints its size and SHA-256.
- `publish.sh` uploads the module and puts layer `biome`. It reads `GX_HUB_URL`, `GX_API_KEY`, `GX_SPACE_ID`, and `GX_FRAME_ID` (default 4) from the environment only.

Run `sh scripts/ci.sh` (with `CARGO_BUILD_JOBS=4` in small containers) before committing.

## MODIS albedo

Level 0 comes from NASA MODIS `MCD43A3` (white-sky albedo in MODIS bands 1, 4, 3, which become the pyramid's bands 0, 1, 2, with their mandatory quality) and `MCD43A2` (snow flag, land-water type), version 061: one HDF4 file per sinusoidal tile (2400 x 2400 pixels of 463.3 m) per day. HDF4 has no maintained Rust reader, so the composite is Python (`scripts/modis/`); Rust takes over from its output.

### The composite

Per pixel and band, `scripts/modis/composite.py`:

- counts a value when it is not the fill value and its band's quality is 0; with fewer than 3 such values, quality 1 values count too;
- takes the median of the good values on snow-free dates, or of all good values when there is none snow-free (permanent ice stays white, seasonal snow does not count); the median of an even count is the lower middle value;
- takes the land fraction from the most recent land-water type: 255 for land and ephemeral water, 128 for shorelines, 0 otherwise;
- fills a land pixel with no value with the land-weighted mean of the valid pixels in the 3 x 3 window around it, then 9 x 9, 27 x 27, up to the whole tile, rounded half up (none in the tile is reported).

It writes `hHHvVV.bgc`: magic `3GBC`, version `u16` 1, `h` `u8`, `v` `u8`, then planar 2400 x 2400 blocks, row 0 first: three `u16` bands (albedo times 1000, 65535 for none) and the `u8` land fraction, little-endian. The file is written under a temporary name and renamed, so a finished file is whole and means the tile is done.

### The cut

`modis-cut` maps every level 0 sample center `(lat, lon)` to sinusoidal `x = R lon cos(lat)`, `y = R lat`, `R = 6371007.181` m, and takes the values of the MODIS pixel holding it. Albedo `round(value / 1000 * 255)`, half up, clamped to 255; a band with no value or a missing composite gives land 0 and albedo 0, and albedo is 0 wherever land is 0. It reads one row of MODIS tiles at a time (at most 36 composites of 40 MB). Deterministic and resumable like `synth`.

### The full run

1. A Python 3.9 or later virtual environment with the pinned packages:

   ```text
   python3 -m venv .venv
   . .venv/bin/activate
   pip install -r scripts/modis/requirements.txt
   ```

2. A NASA Earthdata login in `~/.netrc` (mode 600), as a `machine urs.earthdata.nasa.gov` entry with your `login` and `password`. Never put it in a file of this repository or on a command line.

3. Download and composite, resumable (run it again after a stop):

   ```text
   sh scripts/modis/modis.sh WORK COMPOSITES [JOBS]
   ```

   It lists the dates (day 1, 9, 17, ... of 2023 and 2024: 92 dates) and the tiles with granules from NASA's public granule search (`WORK/tiles.json`), then composites every tile not yet done, `JOBS` tiles at a time (default 3), printing progress and the bytes downloaded. A date of both products is about 16.6 GB over about 297 tiles, so the 92 dates are about 1.5 TB downloaded. Each tile's files are deleted as soon as they are read; what is kept per tile and date is about 40 MB until the tile's composite is written (about 3.7 GB per tile in progress), and a composite is about 40 MB (about 12 GB in all).

4. Cut level 0 and build the pyramid, then upload and publish:

   ```text
   biome-tiler modis-cut COMPOSITES OUT
   biome-tiler upload OUT
   sh scripts/publish.sh MANIFEST_SHA256
   ```

`python3 -m unittest test_modis` in `scripts/modis` tests the composite; `scripts/ci.sh` runs it when `numpy` and `pyhdf` are installed and prints that it skipped it otherwise.

## Data source

The measured albedo comes from:

Schaaf, C., Wang, Z. (2021). MODIS/Terra+Aqua BRDF/Albedo Daily L3 Global 500m V061. NASA EOSDIS Land Processes DAAC. (`MCD43A3`)

Schaaf, C., Wang, Z. (2021). MODIS/Terra+Aqua BRDF/Albedo Quality Daily L3 Global 500m V061. NASA EOSDIS Land Processes DAAC. (`MCD43A2`)
