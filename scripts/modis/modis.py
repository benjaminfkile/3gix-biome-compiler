#!/usr/bin/env python3
"""The MODIS download and composite (biome plan decisions 1, 2, 4, and 6).

    modis.py list --year 2023 --year 2024 --every 8 [--out tiles.json]
    modis.py composite --tiles tiles.json --tile hHHvVV --work DIR --out DIR

`list` writes the dates (day 1, 1 + every, ... of each year) and, from
NASA's public granule search (no login), every sinusoidal tile that has
`MCD43A3` or `MCD43A2` granules on those dates, with the download URLs per
tile and date. Each date's search is kept in `<out>.parts/`, so a stopped
run resumes and a finished one repeats nothing.

`composite` downloads one tile's granules date by date over HTTPS with the
Earthdata login in `~/.netrc` (never in this repository or an argument),
keeps per date only the arrays the composite needs (`<work>/<tile>/<date>/`,
memory mapped when the composite reads them a band of rows at a time),
deletes the downloads, and writes `<out>/<tile>.bgc` by the rules of
`composite.py`. A finished `.bgc` means the tile is done; a kept date is
not downloaded again. The bytes downloaded are added to
`<work>/bytes/<tile>`.
"""

import argparse
import datetime
import http.client
import http.cookiejar
import json
import netrc
import os
import re
import shutil
import sys
import time
import urllib.parse
import urllib.request

PRODUCTS = ("MCD43A3", "MCD43A2")
VERSION = "061"
CMR = "https://cmr.earthdata.nasa.gov/search/granules.json"
URS = "urs.earthdata.nasa.gov"
ALBEDO = ("Albedo_WSA_Band1", "Albedo_WSA_Band4", "Albedo_WSA_Band3")
QUALITY = (
    "BRDF_Albedo_Band_Mandatory_Quality_Band1",
    "BRDF_Albedo_Band_Mandatory_Quality_Band4",
    "BRDF_Albedo_Band_Mandatory_Quality_Band3",
)
SNOW = "Snow_BRDF_Albedo"
LANDWATER = "BRDF_Albedo_LandWaterType"
TILE = re.compile(r"\.(h\d\dv\d\d)\.")
CHUNK_ROWS = 100
KEPT = ("values", "quality", "snow", "landwater")


def dates(years, every):
    """`YYYYDDD` strings of day 1, 1 + every, ... of each year."""
    out = []
    for y in sorted(set(years)):
        days = 366 if datetime.date(y, 12, 31).timetuple().tm_yday == 366 else 365
        out.extend(f"{y}{d:03d}" for d in range(1, days + 1, every))
    return out


def search(product, date):
    """Every granule of `product` on `date` from CMR: `{tile: url}`."""
    found = {}
    params = {
        "short_name": product,
        "version": VERSION,
        "readable_granule_name": f"{product}.A{date}.*",
        "options[readable_granule_name][pattern]": "true",
        "page_size": "2000",
    }
    url = CMR + "?" + urllib.parse.urlencode(params)
    after = None
    while True:
        req = urllib.request.Request(url)
        if after:
            req.add_header("CMR-Search-After", after)
        with urllib.request.urlopen(req, timeout=120) as resp:
            body = json.load(resp)
            after = resp.headers.get("CMR-Search-After")
        entries = body.get("feed", {}).get("entry", [])
        for e in entries:
            m = TILE.search(e.get("producer_granule_id") or e.get("title", ""))
            if not m:
                continue
            for link in e.get("links", []):
                href = link.get("href", "")
                if href.startswith("https://") and href.endswith(".hdf") and "data#" in link.get("rel", ""):
                    found[m.group(1)] = href
                    break
        if not entries or not after:
            return found


def cmd_list(args):
    """`modis.py list`."""
    parts = args.out + ".parts"
    os.makedirs(parts, exist_ok=True)
    ds = dates(args.year, args.every)
    tiles = {}
    for i, date in enumerate(ds):
        for product in PRODUCTS:
            part = os.path.join(parts, f"{product}.{date}.json")
            if not os.path.exists(part):
                found = search(product, date)
                with open(part + ".tmp", "w") as f:
                    json.dump(found, f, sort_keys=True)
                os.replace(part + ".tmp", part)
            with open(part) as f:
                found = json.load(f)
            for tile, url in found.items():
                tiles.setdefault(tile, {}).setdefault(date, {})[product] = url
        print(f"list: {i + 1}/{len(ds)} dates, {len(tiles)} tiles", flush=True)
    with open(args.out + ".tmp", "w") as f:
        json.dump({"dates": ds, "tiles": tiles}, f, sort_keys=True, indent=1)
    os.replace(args.out + ".tmp", args.out)
    print(f"list: wrote {args.out}: {len(ds)} dates, {len(tiles)} tiles")


def opener():
    """An HTTPS opener that logs in to Earthdata with `~/.netrc`."""
    auth = netrc.netrc().authenticators(URS)
    if auth is None:
        sys.exit(f"modis.py: no login for {URS} in ~/.netrc")
    pm = urllib.request.HTTPPasswordMgrWithDefaultRealm()
    pm.add_password(None, "https://" + URS, auth[0], auth[2])
    return urllib.request.build_opener(
        urllib.request.HTTPBasicAuthHandler(pm),
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
    )


# Waits between download attempts, seconds. A failed attempt (a timeout, a
# dropped connection, a server error) is retried from the start of the file
# after the next wait; when every attempt fails the error goes to the caller.
RETRY_WAITS = (10, 30, 60, 120, 300)


def download(op, url, path):
    """Downloads `url` to `path`, retrying after RETRY_WAITS. Returns the bytes downloaded."""
    for attempt, wait in enumerate(RETRY_WAITS + (None,)):
        try:
            return download_once(op, url, path)
        except (OSError, http.client.HTTPException) as e:
            if wait is None:
                raise
            print(f"download {os.path.basename(path)}: attempt {attempt + 1} failed ({e}); retrying in {wait} s", flush=True)
            time.sleep(wait)


def download_once(op, url, path):
    """One attempt at downloading `url` to `path`. Returns the bytes downloaded."""
    n = 0
    with op.open(url, timeout=120) as resp, open(path, "wb") as f:
        while True:
            block = resp.read(1 << 20)
            if not block:
                break
            f.write(block)
            n += len(block)
    return n


def read_date(a3, a2):
    """The arrays one date contributes, from its `MCD43A3` and `MCD43A2`
    files: `(values, quality, snow, landwater)`."""
    import numpy as np
    from pyhdf.SD import SD, SDC

    sa3 = SD(a3, SDC.READ)
    sa2 = SD(a2, SDC.READ)
    try:
        values = np.stack([sa3.select(n)[:].astype(np.int16) for n in ALBEDO])
        quality = np.stack([sa3.select(n)[:].astype(np.uint8) for n in QUALITY])
        snow = sa2.select(SNOW)[:].astype(np.uint8)
        landwater = sa2.select(LANDWATER)[:].astype(np.uint8)
    finally:
        sa3.end()
        sa2.end()
    return values, quality, snow, landwater


def add_bytes(work, tile, n):
    """Adds `n` to the tile's downloaded byte count."""
    d = os.path.join(work, "bytes")
    os.makedirs(d, exist_ok=True)
    path = os.path.join(d, tile)
    total = n
    if os.path.exists(path):
        with open(path) as f:
            total += int(f.read().strip() or 0)
    with open(path + ".tmp", "w") as f:
        f.write(f"{total}\n")
    os.replace(path + ".tmp", path)


def cmd_composite(args):
    """`modis.py composite`."""
    import numpy as np

    import composite as c

    m = re.fullmatch(r"h(\d\d)v(\d\d)", args.tile)
    if not m:
        sys.exit(f"modis.py: --tile {args.tile}: not hHHvVV")
    h, v = int(m.group(1)), int(m.group(2))
    os.makedirs(args.out, exist_ok=True)
    final = os.path.join(args.out, args.tile + ".bgc")
    if os.path.exists(final):
        print(f"composite {args.tile}: done already")
        return
    with open(args.tiles) as f:
        listing = json.load(f)
    granules = listing["tiles"].get(args.tile, {})
    tdir = os.path.join(args.work, args.tile)
    os.makedirs(tdir, exist_ok=True)
    op = None
    kept = []
    for date in listing["dates"]:
        urls = granules.get(date, {})
        if not all(p in urls for p in PRODUCTS):
            continue
        keep = os.path.join(tdir, date)
        if not os.path.isdir(keep):
            op = op or opener()
            files = []
            for p in PRODUCTS:
                path = os.path.join(tdir, f"{p}.{date}.hdf")
                n = download(op, urls[p], path)
                add_bytes(args.work, args.tile, n)
                files.append(path)
            arrays = read_date(*files)
            for path in files:
                os.remove(path)
            tmp = keep + ".tmp"
            shutil.rmtree(tmp, ignore_errors=True)
            os.makedirs(tmp)
            for name, a in zip(KEPT, arrays):
                np.save(os.path.join(tmp, name + ".npy"), a)
            os.replace(tmp, keep)
            print(f"composite {args.tile}: {date} kept", flush=True)
        kept.append(keep)
    loaded = [
        {n: np.load(os.path.join(k, n + ".npy"), mmap_mode="r") for n in KEPT} for k in kept
    ]
    bands = np.full((c.BANDS, c.SIZE, c.SIZE), c.NONE, dtype=np.uint16)
    for r0 in range(0, c.SIZE, CHUNK_ROWS):
        r1 = r0 + CHUNK_ROWS
        if loaded:
            bands[:, r0:r1] = c.pixel_values(
                np.stack([z["values"][:, r0:r1] for z in loaded]),
                np.stack([z["quality"][:, r0:r1] for z in loaded]),
                np.stack([z["snow"][r0:r1] for z in loaded]),
            )
    if loaded:
        land = c.land_fraction(np.stack([z["landwater"] for z in loaded]))
    else:
        land = np.zeros((c.SIZE, c.SIZE), dtype=np.uint8)
    left = c.fill_gaps(bands, land)
    for b, n in enumerate(left):
        if n:
            print(f"composite {args.tile}: band {b}: {n} land pixels without a value")
    c.write_bgc(final, h, v, bands, land)
    shutil.rmtree(tdir)
    print(f"composite {args.tile}: {len(kept)} dates, wrote {final}")


def main():
    ap = argparse.ArgumentParser(prog="modis.py")
    sub = ap.add_subparsers(dest="cmd", required=True)
    pl = sub.add_parser("list")
    pl.add_argument("--year", type=int, action="append", required=True)
    pl.add_argument("--every", type=int, default=8)
    pl.add_argument("--out", default="tiles.json")
    pc = sub.add_parser("composite")
    pc.add_argument("--tiles", required=True)
    pc.add_argument("--tile", required=True)
    pc.add_argument("--work", required=True)
    pc.add_argument("--out", required=True)
    args = ap.parse_args()
    if args.cmd == "list":
        cmd_list(args)
    else:
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        cmd_composite(args)


if __name__ == "__main__":
    main()
