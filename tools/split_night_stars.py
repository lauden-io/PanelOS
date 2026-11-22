#!/usr/bin/env python3
"""The stars of the photo weather themes' night skies, split off the photos: the app draws them itself (src/stars.rs),
standing nearly still while the clouds of the photo drift past and hide them.

For each night sky photo with stars (app/ui/wx/<theme>-<condition>-night-sky.jpg, a seamless 792 x 528 tile) it
  - finds the painted stars: small points clearly brighter than the median of their surroundings (an 11 px median of
    the luminance), compact (their light falls off within 2-3 px on every side, unlike the bright edges and streaks of
    clouds or the moon's glow) and well above the texture around them;
  - fills each star from its surroundings (that median, faded in over 1.5-3 px, more for the bright ones) and writes the
    photo back under the same name with the same JPEG quantisation and subsampling;
  - writes the star list next to it (<name>-stars.txt: x, y in tile pixels, the centre of pixel (i, j) being (i, j);
    b, the brightness above the sky, 0..1; k, 1 for a star that twinkles with a glow, 0 for a faint steady point);
  - for a photo with clouds, writes its cloud cover map (<name>-cover.png, 198 x 132 grey, one pixel per 4 x 4 tile
    pixels; 0 clear sky, 255 thick cloud): how far the softened brightness lies above the clear sky of its row.

The stars are taken off the original photos, so the tool refuses a photo that already has its star list unless the
originals are given with --src (e.g. extracted with `git show <commit>:app/ui/wx/<name>.jpg`).

usage: python tools/split_night_stars.py [--src DIR] [--preview DIR] [--check]
  --src DIR      read the original photos from DIR instead of app/ui/wx
  --preview DIR  write before / after crops (4x) and the found stars marked, to look at
  --check        only report what would be found and written (nothing in app/ui/wx changes)
"""
import argparse, collections, io, os, sys
import numpy as np
from PIL import Image, ImageDraw, ImageFilter
from PIL import JpegImagePlugin

HERE = os.path.dirname(os.path.abspath(__file__))
WX = os.path.join(HERE, "..", "app", "ui", "wx")

# (photo, has clouds): the night skies with painted stars (the rainy nights have none)
PHOTOS = [
    ("nature-clear-night", False), ("nature-cloudy-night", True), ("nature-snowy-night", True),
    ("countryside-clear-night", False), ("countryside-cloudy-night", True), ("countryside-snowy-night", True),
    ("city-clear-night", False), ("city-snowy-night", True),
]
W, H = 792, 528
SKY_ROWS = 272          # below this the photos only continue the horizon (the land covers it)
MEDIAN = 11             # the surroundings a star is measured against
MIN_EXCESS = 8.0        # levels above the surroundings a point needs to count as a star
MIN_SNR = 5.0           # ... and times the texture (robust deviation) around it
TWINKLE_MIN_B = 0.055   # the brightest stars twinkle with a glow (as the page's own overlay did), at most
TWINKLE_MAX = 110       # this many of them; the fainter ones stay steady points
B_SCALE = 1 / 1.055     # the overlay's lists measured the brightness a little lower (on average by this, over the 220
                        # stars of the clear nights found in both): kept, so the stars keep their look
COVER_SCALE = 4         # cover map: one pixel per 4 x 4 tile pixels
COVER_FROM, COVER_TO = 2.0, 22.0   # levels above the row's clear sky: from no cover to thick cloud


# a star: its brightest pixel, its excess over the surroundings, its centre of light, the columns of its dash
Star = collections.namedtuple("Star", "x y e0 cx cy x0 x1")


def lum(a):
    return a[..., 0] * 0.299 + a[..., 1] * 0.587 + a[..., 2] * 0.114


def median_rgb(img, size=MEDIAN):
    """The median of every channel (the sky without its points), as floats."""
    return np.asarray(img.filter(ImageFilter.MedianFilter(size))).astype(np.float32)


def blob(mask):
    """The pixels of `mask` connected (sideways or diagonally) to its centre."""
    out = np.zeros_like(mask)
    c = mask.shape[0] // 2
    todo = [(c, c)] if mask[c, c] else []
    while todo:
        y, x = todo.pop()
        if 0 <= y < mask.shape[0] and 0 <= x < mask.shape[1] and mask[y, x] and not out[y, x]:
            out[y, x] = True
            todo += [(y + dy, x + dx) for dy in (-1, 0, 1) for dx in (-1, 0, 1) if dy or dx]
    return out


def close(s, x, y):
    """Star s lies within 2 px of pixel (x, y) (across the tile's seam too)."""
    dx = abs(s.x - x) % W
    return min(dx, W - dx) <= 2 and abs(s.y - y) <= 2


def find_stars(a, bg):
    """The stars of a photo (float RGB) against its median `bg`, brightest first."""
    ex = lum(a) - lum(bg)
    h, w = ex.shape
    p = np.pad(ex, 1, mode="edge")
    neigh = np.stack([p[1 + dy:1 + dy + h, 1 + dx:1 + dx + w] for dy in (-1, 0, 1) for dx in (-1, 0, 1) if dy or dx])
    peaks = (ex >= neigh.max(0)) & (ex >= MIN_EXCESS)
    peaks[SKY_ROWS:] = False
    yy, xx = np.mgrid[-5:6, -5:6]
    d = np.hypot(yy, xx)
    ring3 = (d >= 2.5) & (d < 3.6)
    near = d <= 5.0
    pe = np.pad(ex, 8, mode="wrap")
    found = []
    for y, x in sorted(zip(*np.nonzero(peaks)), key=lambda q: -ex[q]):
        e0 = ex[y, x]
        win = pe[y + 3:y + 14, x + 3:x + 14]
        # compact: a point or a short sideways dash (many of the photos' stars are), little light 3 px away on
        # average, no dark shapes close by (birds; a bright star has a slightly dark JPEG ring of its own)
        half = blob((win > 0.5 * e0) & near)
        rows, cols = np.nonzero(half)
        if half.sum() > 10 or np.ptp(rows) > 2 or np.ptp(cols) > 6 or win[ring3].mean() > 0.15 * e0:
            continue
        if np.percentile(win[near], 10) < -0.4 * e0 - 3 or win[near].min() < -10 - 0.15 * e0:
            continue
        # well above the texture around it (cloud edges are textured, the night sky is not)
        out = pe[y:y + 17, x:x + 17]
        sig = 1.4826 * np.median(np.abs(out - np.median(out)))
        if e0 < MIN_SNR * max(sig, 0.8):
            continue
        # one star per point (a star two pixels big has two peaks; the tile's columns wrap round)
        if any(close(f, x, y) for f in found):
            continue
        c = np.clip(pe[y + 7:y + 10, x + 7:x + 10], 0, None)
        cy, cx = np.mgrid[-1:2, -1:2]
        s = c.sum()
        found.append(Star(int(x), int(y), float(e0), x + float((c * cx).sum() / s), y + float((c * cy).sum() / s),
                          int(x + cols.min() - 5), int(x + cols.max() - 5)))
    return found


def split(a, like):
    """All stars of a photo (float RGB), brightest first, and the photo without them. A star close to a brighter one
    only passes the tests once that one is gone, and a faint one at the edge of the tests may pass them only in the
    written JPEG (`like`'s quantisation), so the search runs again on the filled photo and on it as written until
    neither has a star left."""
    stars, cur = [], a
    for _ in range(10):
        bg = median_rgb(to_image(cur))
        new = find_stars(cur, bg)
        written = Image.open(io.BytesIO(save_jpeg(cur, like, None))).convert("RGB")
        for s in find_stars(np.asarray(written).astype(np.float32), median_rgb(written)):
            if not any(close(n, s.x, s.y) for n in new):
                new.append(s._replace(e0=max(MIN_EXCESS, float(lum(cur[s.y, s.x]) - lum(bg[s.y, s.x])))))
        if not new:
            break
        cur = fill(cur, bg, median_rgb(to_image(cur), 2 * MEDIAN - 1), new)
        stars += new
    return sorted(stars, key=lambda s: -s.e0), cur


def to_image(a):
    return Image.fromarray(np.clip(a + 0.5, 0, 255).astype(np.uint8))


def fill(a, bg, bg_wide, stars):
    """The photo with every star replaced by its surroundings `bg` (within 1.5 px of its dash, faded out by 3 px; a
    bright star by 4 px, the brightest few, with a faint halo, by 6 px from the wider median `bg_wide`)."""
    out = a.copy()
    h, w = a.shape[:2]
    for s in stars:
        r = 3 if s.e0 < 40 else 4 if s.e0 < 80 else 6
        src = bg_wide if r == 6 else bg
        for py in range(max(0, s.y - r), min(h, s.y + r + 1)):
            for px in range(s.x0 - r, s.x1 + r + 1):
                d = np.hypot(px - min(max(px, s.x0), s.x1), py - s.cy)
                k = 1.0 if d <= 1.5 else max(0.0, (r - d) / (r - 1.5))
                if k > 0:
                    qx = px % w
                    out[py, qx] = out[py, qx] * (1 - k) + src[py, qx] * k
    return out


def cover_map(a):
    """0 clear .. 255 thick cloud, one pixel per 4 x 4: the softened luminance above the clear sky of its row."""
    img = to_image(a)
    # soften across the tile's seam too: three tiles side by side
    wide = Image.new("RGB", (W * 3, H))
    for i in range(3):
        wide.paste(img, (i * W, 0))
    y = lum(np.asarray(wide.filter(ImageFilter.GaussianBlur(5))).astype(np.float32))[:, W:2 * W]
    clear = np.percentile(y, 4, axis=1)
    # the clear level of a row changes smoothly down the sky
    k = np.ones(31) / 31
    clear = np.convolve(np.pad(clear, 15, mode="edge"), k, mode="valid")
    c = np.clip((y - clear[:, None] - COVER_FROM) / (COVER_TO - COVER_FROM), 0, 1)
    c = c * c * (3 - 2 * c)
    small = c.reshape(H // COVER_SCALE, COVER_SCALE, W // COVER_SCALE, COVER_SCALE).mean(axis=(1, 3))
    return np.clip(small * 255 + 0.5, 0, 255).astype(np.uint8)


def save_jpeg(arr, like, path):
    """Saves `arr` as a JPEG with the quantisation tables and the subsampling of the original `like`."""
    img = to_image(arr)
    buf = io.BytesIO()
    img.save(buf, "JPEG", qtables=like.quantization, subsampling=JpegImagePlugin.get_sampling(like), optimize=True)
    data = buf.getvalue()
    if path:
        with open(path, "wb") as f:
            f.write(data)
    return data


def twinkles(i, s):
    """The i-th brightest star twinkles with a glow (as the page's own overlay did), or is a steady point."""
    return i < TWINKLE_MAX and s.e0 / 255 * B_SCALE >= TWINKLE_MIN_B


def star_list(name, stars):
    """The star list's text: brightest first; the first ones (up to TWINKLE_MAX, b >= TWINKLE_MIN_B) twinkle."""
    lines = [f"# {name}-sky.jpg: its stars, split off by tools/split_night_stars.py",
             "# x y (tile pixels, the centre of pixel (i, j) is (i, j)), b (brightness above the sky, 0..1), "
             "k (1 twinkles with a glow, 0 a steady point)"]
    for i, s in enumerate(stars):
        b = s.e0 / 255 * B_SCALE
        k = 1 if twinkles(i, s) else 0
        # (a star at the tile's seam: its column on the tile)
        lines.append(f"{round(s.cx, 1) % W:.1f} {s.cy:.1f} {b:.3f} {k}")
    return "\n".join(lines) + "\n"


def preview(name, before, after, stars, cover, out_dir):
    """Before / after crops at 4x around the brightest stars, the whole sky with the stars marked, the cover map."""
    os.makedirs(out_dir, exist_ok=True)
    b8 = to_image(before)
    a8 = to_image(after)
    marked = b8.crop((0, 0, W, SKY_ROWS)).copy()
    d = ImageDraw.Draw(marked)
    for i, s in enumerate(stars):
        d.ellipse((s.cx - 4, s.cy - 4, s.cx + 4, s.cy + 4), outline=(255, 60, 60) if twinkles(i, s) else (60, 255, 60))
    sheet = Image.new("RGB", (W, SKY_ROWS * 3))
    sheet.paste(marked, (0, 0))
    sheet.paste(a8.crop((0, 0, W, SKY_ROWS)), (0, SKY_ROWS))
    # the difference, amplified (what changed: only the stars)
    diff = np.abs(after - before).max(axis=2)
    sheet.paste(Image.fromarray(np.clip(diff * 4, 0, 255).astype(np.uint8)).convert("RGB").crop((0, 0, W, SKY_ROWS)), (0, 2 * SKY_ROWS))
    sheet.save(os.path.join(out_dir, f"{name}-split.png"))
    tiles = []
    for s in stars[:12]:
        box = (s.x - 12, s.y - 12, s.x + 13, s.y + 13)
        tiles.append((b8.crop(box).resize((100, 100), Image.NEAREST), a8.crop(box).resize((100, 100), Image.NEAREST)))
    if tiles:
        zs = Image.new("RGB", (len(tiles) * 104, 204), (60, 0, 0))
        for i, (t0, t1) in enumerate(tiles):
            zs.paste(t0, (i * 104, 0))
            zs.paste(t1, (i * 104, 104))
        zs.save(os.path.join(out_dir, f"{name}-zoom.png"))
    if cover is not None:
        Image.fromarray(cover).resize((W, H), Image.NEAREST).crop((0, 0, W, SKY_ROWS)).save(os.path.join(out_dir, f"{name}-cover.png"))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--src")
    ap.add_argument("--preview")
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    for name, cloudy in PHOTOS:
        photo = os.path.join(WX, f"{name}-sky.jpg")
        stars_path = os.path.join(WX, f"{name}-stars.txt")
        cover_path = os.path.join(WX, f"{name}-cover.png")
        src = os.path.join(args.src, f"{name}-sky.jpg") if args.src else photo
        if not args.src and os.path.exists(stars_path) and not args.check:
            sys.exit(f"{name}: its stars are split off already ({stars_path}); give the original photos with --src")
        orig = Image.open(src)
        rgb = orig.convert("RGB")
        a = np.asarray(rgb).astype(np.float32)
        stars, after = split(a, orig)
        data = save_jpeg(after, orig, None)
        written = np.asarray(Image.open(io.BytesIO(data)).convert("RGB")).astype(np.float32)
        left = find_stars(written, median_rgb(Image.open(io.BytesIO(data)).convert("RGB")))
        cover = cover_map(written) if cloudy else None
        twinkle = sum(1 for i, s in enumerate(stars) if twinkles(i, s))
        change = np.abs(written - a).max(axis=2)
        far = np.ones(change.shape, bool)
        for s in stars:
            far[max(0, s.y - 5):s.y + 6, max(0, s.x0 - 5):s.x1 + 6] = False
        line = (f"{name}: {len(stars)} stars ({twinkle} twinkle), {len(left)} found again in the written photo; away from "
                f"the stars it changed by at most {change[far].max():.0f} levels (mean {change[far].mean():.2f}); "
                f"{os.path.getsize(src)} -> {len(data)} bytes")
        if cover is not None:
            at = [cover[int(s.cy) // COVER_SCALE, int(s.cx) // COVER_SCALE] for s in stars]
            line += f"; cover: clear (< 32) over {100 * (cover < 32).mean():.0f} % of the sky"
            line += f", median {np.median(at):.0f} at its stars" if at else ""
        print(line)
        if args.preview:
            preview(name, a, written, stars, cover, args.preview)
        if args.check:
            continue
        with open(photo, "wb") as f:
            f.write(data)
        with open(stars_path, "w") as f:
            f.write(star_list(name, stars))
        if cover is not None:
            Image.fromarray(cover).save(cover_path, optimize=True)


if __name__ == "__main__":
    main()
