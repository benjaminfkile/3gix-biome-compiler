"""The MODIS albedo composite as pure functions over numpy arrays.

Implements decisions 1, 2, and 4 of the biome plan (3GIXHub
`docs/build/biome/plan.md`): one static map per MODIS sinusoidal tile of
white-sky albedo in MODIS bands 1, 4, and 3 (the pyramid's bands 0, 1, 2)
and a land fraction, from `MCD43A3` and `MCD43A2` version 061. No files and
no network here, so tests drive it directly; `modis.py` does the reading.

Rules, per pixel:

- Good values: a band value counts when it is not the fill value and its
  band's quality is 0. When a pixel has fewer than 3 such values in a band,
  quality 1 values count too.
- Snow-free first: the value per band is the median of the good values on
  snow-free dates (snow flag 0); a pixel with no good snow-free value uses
  the median of all its good values.
- The median of an even count is the lower of the two middle values.
- Land fraction from the land-water type of the most recent date that has
  one: 255 for types 1 and 4, 128 for type 2, 0 otherwise and for fill.
- Gaps: a pixel with land above 0 and no value in a band takes the
  land-weighted mean of the valid pixels in the 3 x 3 window around it,
  then 9 x 9, 27 x 27, up to the whole tile, rounded half up. None in the
  whole tile leaves the band at its fill value and is reported.

The `.bgc` file: magic `3GBC`, version `u16` 1, `h` `u8`, `v` `u8`, then
planar blocks of `SIZE x SIZE`, row 0 first: three `u16` bands (albedo
times 1000, 65535 for none) and one `u8` land fraction. Little-endian.
"""

import os

import numpy as np

SIZE = 2400
"""Pixels per side of a MODIS sinusoidal tile at 500 m."""

BANDS = 3
"""Albedo bands kept: MODIS bands 1, 4, 3, in that order."""

ALBEDO_FILL = 32767
"""`Albedo_WSA_Band*` fill value; valid values are 0 to 32766."""

NONE = 65535
"""A composite band with no value."""

MIN_QUALITY0 = 3
"""Below this many quality 0 values, quality 1 values count too."""

MAGIC = b"3GBC"
VERSION = 1


def pixel_values(values, quality, snow):
    """The per pixel, per band median of the good values.

    `values` is `(D, 3, H, W)` int16 (`Albedo_WSA_Band1`, `_Band4`,
    `_Band3` per date), `quality` `(D, 3, H, W)` uint8 (the mandatory
    quality of each band), `snow` `(D, H, W)` uint8 (`Snow_BRDF_Albedo`).
    Returns `(3, H, W)` uint16, `NONE` where a band has no good value.
    """
    values = np.asarray(values)
    quality = np.asarray(quality)
    snow = np.asarray(snow)
    d = values.shape[0]
    out = np.full(values.shape[1:], NONE, dtype=np.uint16)
    if d == 0:
        return out
    snow_free = snow == 0
    for b in range(values.shape[1]):
        v = values[:, b].astype(np.int32)
        q = quality[:, b]
        valid = (v >= 0) & (v != ALBEDO_FILL)
        q0 = valid & (q == 0)
        q1 = valid & (q == 1)
        n0 = q0.sum(axis=0)
        good = q0 | (q1 & (n0 < MIN_QUALITY0)[None])
        sf = good & snow_free
        use = np.where((sf.sum(axis=0) > 0)[None], sf, good)
        n = use.sum(axis=0)
        stacked = np.where(use, v, ALBEDO_FILL)
        stacked.sort(axis=0)
        idx = np.maximum(n - 1, 0) // 2
        med = np.take_along_axis(stacked, idx[None], axis=0)[0]
        out[b] = np.where(n > 0, med, NONE).astype(np.uint16)
    return out


def land_fraction(landwater):
    """The land fraction `(H, W)` uint8 from `BRDF_Albedo_LandWaterType`
    per date, `(D, H, W)` uint8 in date order: the type of the most recent
    date that has one (not 255) gives 255 for types 1 and 4, 128 for type
    2, and 0 for every other type and for fill."""
    landwater = np.asarray(landwater)
    latest = np.full(landwater.shape[1:], 255, dtype=np.uint8)
    for lw in landwater:
        latest = np.where(lw != 255, lw, latest)
    land = np.zeros(latest.shape, dtype=np.uint8)
    land[(latest == 1) | (latest == 4)] = 255
    land[latest == 2] = 128
    return land


def _window_sums(table, rows, cols, half):
    """Sums of the summed-area `table` over the windows of half size `half`
    around pixels `(rows, cols)`, clipped to the tile."""
    h = table.shape[0] - 1
    w = table.shape[1] - 1
    r0 = np.maximum(rows - half, 0)
    r1 = np.minimum(rows + half + 1, h)
    c0 = np.maximum(cols - half, 0)
    c1 = np.minimum(cols + half + 1, w)
    return table[r1, c1] - table[r0, c1] - table[r1, c0] + table[r0, c0]


def _summed_area(a):
    """The summed-area table of `a` (int64), with a leading row and column
    of zeros."""
    t = np.zeros((a.shape[0] + 1, a.shape[1] + 1), dtype=np.int64)
    t[1:, 1:] = a.astype(np.int64).cumsum(axis=0).cumsum(axis=1)
    return t


def fill_gaps(bands, land):
    """Fills the gaps of `bands` (`(3, H, W)` uint16) in place where `land`
    (`(H, W)` uint8) is above 0: the land-weighted mean of the valid pixels
    in the 3 x 3 window, then 9 x 9, 27 x 27, up to the whole tile, rounded
    half up. Valid pixels are those that had a value before filling.
    Returns per band the number of gap pixels left at `NONE`."""
    left = []
    h, w = land.shape
    lw = land.astype(np.int64)
    for b in range(bands.shape[0]):
        band = bands[b]
        has = band != NONE
        gaps = (~has) & (land > 0)
        rows, cols = np.nonzero(gaps)
        weight = _summed_area(np.where(has, lw, 0))
        total = _summed_area(np.where(has, lw * band.astype(np.int64), 0))
        half = 1
        while rows.size > 0:
            ws = _window_sums(weight, rows, cols, half)
            ok = ws > 0
            if ok.any():
                ts = _window_sums(total, rows[ok], cols[ok], half)
                band[rows[ok], cols[ok]] = ((2 * ts + ws[ok]) // (2 * ws[ok])).astype(
                    np.uint16
                )
                rows = rows[~ok]
                cols = cols[~ok]
            if half >= max(h, w):
                break
            half = 3 * half + 1
        left.append(int(rows.size))
    return left


def composite(values, quality, snow, landwater):
    """The whole composite of one tile from its per date stacks (see
    [`pixel_values`] and [`land_fraction`]). Returns `(bands, land, left)`:
    `(3, H, W)` uint16, `(H, W)` uint8, and per band the number of land
    pixels left without a value."""
    bands = pixel_values(values, quality, snow)
    land = land_fraction(landwater)
    left = fill_gaps(bands, land)
    return bands, land, left


def bgc_bytes(h, v, bands, land):
    """The bytes of a `.bgc` file of tile `(h, v)`."""
    bands = np.asarray(bands, dtype="<u2")
    land = np.asarray(land, dtype=np.uint8)
    if bands.shape != (BANDS, SIZE, SIZE) or land.shape != (SIZE, SIZE):
        raise ValueError("a composite is 3 bands and a land fraction of 2400 x 2400")
    head = MAGIC + VERSION.to_bytes(2, "little") + bytes([h, v])
    return head + bands.tobytes() + land.tobytes()


def write_bgc(path, h, v, bands, land):
    """Writes a `.bgc` file to a temporary name next to `path`, then
    renames it, so a file under `path` is always whole."""
    tmp = path + ".tmp"
    with open(tmp, "wb") as f:
        f.write(bgc_bytes(h, v, bands, land))
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


def read_bgc(path):
    """Reads a `.bgc` file. Returns `(h, v, bands, land)`."""
    with open(path, "rb") as f:
        data = f.read()
    n = SIZE * SIZE
    if len(data) != 8 + BANDS * 2 * n + n or data[:4] != MAGIC:
        raise ValueError(f"{path}: not a composite")
    if int.from_bytes(data[4:6], "little") != VERSION:
        raise ValueError(f"{path}: unknown version")
    bands = np.frombuffer(data, dtype="<u2", count=BANDS * n, offset=8)
    land = np.frombuffer(data, dtype=np.uint8, count=n, offset=8 + BANDS * 2 * n)
    return data[6], data[7], bands.reshape(BANDS, SIZE, SIZE), land.reshape(SIZE, SIZE)
