"""Tests of `composite.py` on hand-made stacks (plain `unittest`)."""

import os
import tempfile
import unittest

import numpy as np

import composite as c

FILL = c.ALBEDO_FILL


def stack(per_date, quality=None, snow=None, h=1, w=1):
    """A `(D, 3, h, w)` stack whose every band and pixel holds the date's
    value, with per date qualities and snow flags (default 0)."""
    d = len(per_date)
    values = np.empty((d, 3, h, w), dtype=np.int16)
    q = np.zeros((d, 3, h, w), dtype=np.uint8)
    s = np.zeros((d, h, w), dtype=np.uint8)
    for i, val in enumerate(per_date):
        values[i] = val
        if quality is not None:
            q[i] = quality[i]
        if snow is not None:
            s[i] = snow[i]
    return values, q, s


def one(values, q, s):
    return int(c.pixel_values(values, q, s)[0, 0, 0])


class PixelValues(unittest.TestCase):
    def test_quality0_preferred(self):
        # Three quality 0 values: the quality 1 values do not count.
        v = stack([100, 200, 300, 900, 950], quality=[0, 0, 0, 1, 1])
        self.assertEqual(one(*v), 200)

    def test_quality1_fallback(self):
        # Two quality 0 values: quality 1 values count too.
        v = stack([100, 200, 900, 950], quality=[0, 0, 1, 1])
        self.assertEqual(one(*v), 200)
        v = stack([100, 900, 950], quality=[0, 1, 1])
        self.assertEqual(one(*v), 900)

    def test_fill_and_bad_quality_ignored(self):
        v = stack([FILL, 400, 500, 600], quality=[0, 255, 0, 0])
        self.assertEqual(one(*v), 500)
        v = stack([FILL, 400], quality=[0, 255])
        self.assertEqual(one(*v), c.NONE)

    def test_snow_free_first(self):
        v = stack([800, 900, 100, 120, 140], snow=[1, 1, 0, 0, 0])
        self.assertEqual(one(*v), 120)

    def test_all_dates_fallback(self):
        # Never snow-free: the median of all good values.
        v = stack([800, 900, 850], snow=[1, 1, 255])
        self.assertEqual(one(*v), 850)

    def test_lower_median_of_even_count(self):
        v = stack([40, 10, 30, 20])
        self.assertEqual(one(*v), 20)
        v = stack([7, 3])
        self.assertEqual(one(*v), 3)

    def test_bands_independent(self):
        values, q, s = stack([100, 200, 300])
        values[:, 1] = [[[5]], [[6]], [[7]]]
        q[:, 2] = 255
        out = c.pixel_values(values, q, s)
        self.assertEqual(out[:, 0, 0].tolist(), [200, 6, c.NONE])


class Land(unittest.TestCase):
    def test_per_type(self):
        types = np.arange(9, dtype=np.uint8)
        types[8] = 255
        land = c.land_fraction(types[None, None, :])
        self.assertEqual(land[0].tolist(), [0, 255, 128, 0, 255, 0, 0, 0, 0])

    def test_most_recent_date(self):
        lw = np.array([[[1, 1, 7]], [[7, 255, 255]]], dtype=np.uint8)
        self.assertEqual(c.land_fraction(lw)[0].tolist(), [0, 255, 0])


class Gaps(unittest.TestCase):
    def grid(self, n):
        bands = np.full((3, n, n), c.NONE, dtype=np.uint16)
        land = np.full((n, n), 255, dtype=np.uint8)
        return bands, land

    def test_3x3(self):
        bands, land = self.grid(30)
        bands[:, 10, 11] = 100
        bands[:, 11, 10] = 201
        land[11, 10] = 128
        bands[:, 10, 13] = 999  # outside the 3 x 3 window of (10, 10)
        c.fill_gaps(bands, land)
        # (255 * 100 + 128 * 201) / 383 = 133.76, rounds to 134.
        self.assertEqual(int(bands[0, 10, 10]), 134)

    def test_rounds_half_up(self):
        bands, land = self.grid(5)
        bands[:, 0, 1] = 1
        bands[:, 1, 0] = 2
        c.fill_gaps(bands, land)
        self.assertEqual(int(bands[0, 0, 0]), 2)

    def test_9x9(self):
        bands, land = self.grid(30)
        bands[:, 10, 14] = 300
        bands[:, 10, 15] = 999  # outside 9 x 9
        c.fill_gaps(bands, land)
        self.assertEqual(int(bands[0, 10, 10]), 300)

    def test_27x27(self):
        bands, land = self.grid(60)
        bands[:, 10, 23] = 400
        bands[:, 10, 24] = 999  # outside 27 x 27
        c.fill_gaps(bands, land)
        self.assertEqual(int(bands[0, 10, 10]), 400)
        self.assertEqual(int(bands[0, 10, 22]), 400)

    def test_81x81(self):
        bands, land = self.grid(100)
        bands[:, 0, 50] = 500
        bands[:, 0, 91] = 999  # outside 81 x 81 of (0, 10)
        c.fill_gaps(bands, land)
        self.assertEqual(int(bands[0, 0, 10]), 500)

    def test_whole_tile(self):
        bands, land = self.grid(150)
        bands[:, 149, 149] = 600
        bands[:, 0, 149] = 0
        land[0, 149] = 0  # no land: not a valid pixel
        c.fill_gaps(bands, land)
        self.assertEqual(int(bands[0, 0, 0]), 600)

    def test_not_filled_without_land_or_values(self):
        bands, land = self.grid(10)
        land[0, 0] = 0
        bands[:, 5, 5] = 50
        left = c.fill_gaps(bands, land)
        self.assertEqual(int(bands[0, 0, 0]), c.NONE)
        self.assertEqual(left, [0, 0, 0])
        bands, land = self.grid(10)
        left = c.fill_gaps(bands, land)
        self.assertEqual(left, [100, 100, 100])
        self.assertTrue((bands == c.NONE).all())

    def test_composite(self):
        values, q, s = stack([100, 200, FILL], h=2, w=2)
        values[:, :, 1, 1] = FILL
        lw = np.ones((3, 2, 2), dtype=np.uint8)
        bands, land, left = c.composite(values, q, s, lw)
        self.assertEqual(bands[0].tolist(), [[100, 100], [100, 100]])
        self.assertEqual(land.tolist(), [[255, 255], [255, 255]])
        self.assertEqual(left, [0, 0, 0])


class Bgc(unittest.TestCase):
    def test_round_trip(self):
        rng = np.random.default_rng(7)
        bands = rng.integers(0, 65536, size=(3, c.SIZE, c.SIZE), dtype=np.uint16)
        land = rng.integers(0, 256, size=(c.SIZE, c.SIZE), dtype=np.uint8)
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "h09v04.bgc")
            c.write_bgc(path, 9, 4, bands, land)
            self.assertEqual(os.listdir(d), ["h09v04.bgc"])
            with open(path, "rb") as f:
                head = f.read(8)
            self.assertEqual(head, b"3GBC\x01\x00\x09\x04")
            self.assertEqual(os.path.getsize(path), 8 + 7 * c.SIZE * c.SIZE)
            h, v, b2, l2 = c.read_bgc(path)
        self.assertEqual((h, v), (9, 4))
        self.assertTrue((b2 == bands).all())
        self.assertTrue((l2 == land).all())


if __name__ == "__main__":
    unittest.main()
