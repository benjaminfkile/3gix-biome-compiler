# 3gix-biome-compiler

A 3GIX compiler module that gives a frame's ground its measured material as overlay tiles (`matter-format.md` section 12 in the 3GIXHub repository), plus the tool that cuts the source data into tiles. Compilers are allowed to know what things are: this one colours a planet's land from satellite measurements of surface albedo. Nothing it emits says so.

The overlay layer is described in `client-compute.md` section 2.2 and section 10, `matter-format.md` sections 10 to 12, and `rendering.md` section 7 item 6 in the 3GIXHub repository; the design is `docs/build/biome/plan.md` there. This repository holds:

- `crates/biome-tiles`: the tile format, the grid, and sampling. Builds for `wasm32-unknown-unknown`.
- `crates/biome-tiler`: the native CLI that builds the tile pyramid and uploads it to a hub as a source set.
- `crates/biome-module`: the overlay module. An empty placeholder until it is built.
- `tools/tiles-wasm-check`: the determinism digest of `biome-tiles` as a WebAssembly export, run under Node by `scripts/ci.sh`.

No data is in this repository. Tests use a synthetic world (`biome_tiles::fixture`) written by the test.

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
biome-tiler upload <dir>
```

- `finish` builds levels 1 to 8 from the level 0 tiles in `<dir>/tiles/0`, then writes `manifest.bin` (the core manifest format, tile id to the SHA-256 of its bytes) and `summary.json` (tiles and bytes per level, the manifest hash).
- `synth` writes the level 0 tiles of the synthetic world, then runs `finish`.
- `--grid` uses a smaller grid of the same layout, for tests.
- `upload` sends every tile with `PUT /source/{hash}` unless `HEAD /source/{hash}` says the hub has it, then the manifest. It reads `GX_HUB_URL` and `GX_API_KEY` from the environment only (see `.env.example`).

Output is deterministic (the same input gives the same bytes on any thread count) and resumable (a tile already on disk with the right hash is not rewritten).

Run `sh scripts/ci.sh` (with `CARGO_BUILD_JOBS=4` in small containers) before committing.

## Data source

The measured albedo will come from:

Schaaf, C., Wang, Z. (2021). MODIS/Terra+Aqua BRDF/Albedo Daily L3 Global 500m V061. NASA EOSDIS Land Processes DAAC.
