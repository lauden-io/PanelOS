"""Boot splash frames from the logo (assets/logo.svg), pixel-exact.

The logo is one SVG path (even-odd): a red disc with the house cut out, so the
white "line" is the background showing through. Every frame is the exact
logo raster (8x supersampled, box-filtered); the draw-on animation only
reveals its white pixels in the order a pen would draw the line (arc length
along the line's centre, round pen tip), so the last frame is the logo itself.

The panel's boot pictures come from tools/splash/storyboard.py, which uses this
module (the logo's layers and the draw-on reveal).

Usage: python render_splash.py <logo.svg> <out_dir> [--preview]
  --preview  writes preview.gif (the whole sequence) and keyframes.png
Always writes the logo-only sequence:
  splash.bin      in the boot splash program's format (buildroot/board/nspanel/src/nspanel-splash.c)
  logo_96.rgba    the finished logo as premultiplied RGBA
"""

import struct

import math
import re
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

SIZE = 480          # panel resolution
DISC = 96           # disc diameter on the panel, px (about 1/5 of the screen height)
SS = 8              # supersampling factor
FPS = 30
DRAW_S = 1.2        # draw-on duration
GHOST = 0.22        # white share of the "ghost" lines before drawing
RED = np.array([255, 0, 0], np.float32)
WHITE = np.array([255, 255, 255], np.float32)
BLACK = np.array([0, 0, 0], np.float32)

# Centre line of the white band in SVG units (viewBox 17631.83), derived from
# the band edges: left wall x=4182, outer-left roof x+y=12620, right roof
# x-y=5866, right wall x=13450, bottom y=12718, inner wall x=6962, inner roof
# x+y=16070 (band width ~982).
LINE = [(4182, 12721), (4182, 8438), (9243, 3377), (13450, 7584), (13450, 12718),
        (6962, 12718), (6962, 9108), (10968, 5102)]
HALF_WIDTH = 491.0
VIEW = 17631.83


def parse_path(svg_text):
    d = re.search(r' d="([^"]+)"', svg_text).group(1)
    toks = re.findall(r"[MmCcLlZz]|-?[0-9.]+", d)
    i, cmd, cur, start = 0, None, (0.0, 0.0), (0.0, 0.0)
    subs, pts = [], []

    def num():
        nonlocal i
        v = float(toks[i])
        i += 1
        return v

    while i < len(toks):
        if toks[i].isalpha():
            cmd = toks[i]
            i += 1
            if cmd in "Zz":
                subs.append(pts)
                pts, cur = [], start
                continue
        if cmd in "Mm":
            x, y = num(), num()
            cur = (cur[0] + x, cur[1] + y) if cmd == "m" else (x, y)
            start, pts = cur, [cur]
            cmd = "l" if cmd == "m" else "L"
        elif cmd in "Ll":
            x, y = num(), num()
            cur = (cur[0] + x, cur[1] + y) if cmd == "l" else (x, y)
            pts.append(cur)
        elif cmd in "Cc":
            c = [num() for _ in range(6)]
            if cmd == "c":
                c = [cur[0] + c[0], cur[1] + c[1], cur[0] + c[2], cur[1] + c[3], cur[0] + c[4], cur[1] + c[5]]
            p0 = cur
            for k in range(1, 33):  # flatten the Bezier finely
                t = k / 32
                mt = 1 - t
                x = mt**3 * p0[0] + 3 * mt * mt * t * c[0] + 3 * mt * t * t * c[2] + t**3 * c[4]
                y = mt**3 * p0[1] + 3 * mt * mt * t * c[1] + 3 * mt * t * t * c[3] + t**3 * c[5]
                pts.append((x, y))
            cur = (c[4], c[5])
    return subs


def logo_layers(svg_text):
    """Coverage (0..1) of the disc and of the white cut-out, panel-sized."""
    subs = parse_path(svg_text)
    big = SIZE * SS
    scale = DISC * SS / VIEW
    off = (big - DISC * SS) / 2

    def poly(sub):
        return [(off + x * scale, off + y * scale) for x, y in sub]

    disc = Image.new("L", (big, big), 0)
    ImageDraw.Draw(disc).polygon(poly(subs[0]), fill=255)
    white = Image.new("L", (big, big), 0)
    dw = ImageDraw.Draw(white)
    dw.polygon(poly(subs[1]), fill=255)   # outer band outline
    dw.polygon(poly(subs[2]), fill=0)     # inner house (red again)
    to = lambda im: np.asarray(im.resize((SIZE, SIZE), Image.BOX), np.float32) / 255.0
    return to(disc), to(white)


class Reveal:
    """How much of each white logo pixel the pen has drawn at time t (0..1).

    Every segment of LINE draws its own straight band (half width HALF_WIDTH,
    anti-aliased edge, round pen tip), so a line looks straight while it is
    drawn. White logo pixels outside every band (the rounded inner corners,
    and where the inner roof joins the right roof) appear when the pen
    reaches that corner; the join near the inner roof's end appears only when
    the inner roof arrives (the right roof stays a straight line without
    roundings until the inner roof joins it from below).
    """

    def __init__(self, white_cov):
        self.white = white_cov
        scale = DISC / VIEW
        off = (SIZE - DISC) / 2
        pts = [(off + x * scale, off + y * scale) for x, y in LINE]
        hw = HALF_WIDTH * scale
        yy, xx = np.mgrid[0:SIZE, 0:SIZE].astype(np.float32) + 0.5
        self.bands = []          # (coverage, arc length at which each pixel is drawn)
        s0 = 0.0
        for k in range(len(pts) - 1):
            (ax, ay), (bx, by) = pts[k], pts[k + 1]
            L = math.dist(pts[k], pts[k + 1])
            ux, uy = (bx - ax) / L, (by - ay) / L
            proj = np.clip((xx - ax) * ux + (yy - ay) * uy, 0, L)
            d = np.hypot(xx - (ax + proj * ux), yy - (ay + proj * uy))
            cov = np.clip(hw - d + 0.5, 0, 1)
            at = s0 + proj - np.sqrt(np.maximum(0, hw * hw - d * d))   # round pen tip
            self.bands.append((cov, at))
            s0 += L
        self.total = s0
        union = np.max([c for c, _ in self.bands], axis=0)
        self.residual = np.clip(white_cov - union, 0, 1)
        # Residual pixels: the nearest corner's time; near the inner roof's end: the very end.
        corner_s, acc = [], 0.0
        for k in range(1, len(pts)):
            acc += math.dist(pts[k - 1], pts[k])
            corner_s.append(acc)
        dists = np.stack([np.hypot(xx - px, yy - py) for px, py in pts[1:]])
        nearest = np.argmin(dists, axis=0)
        self.residual_at = np.array(corner_s, np.float32)[nearest]
        join = np.hypot(xx - pts[-1][0], yy - pts[-1][1]) < 2200 * scale
        self.residual_at = np.where(join, self.total, self.residual_at)

    def shown(self, t):
        T = t * (self.total + 1.0)   # t = 1 draws everything
        drawn = np.max([cov * np.clip(T - at, 0, 1) for cov, at in self.bands], axis=0)
        drawn = drawn + self.residual * np.clip(T - self.residual_at, 0, 1)
        return np.minimum(self.white, drawn)


def compose(disc, white_shown, ghost_share, bg=BLACK, scale=1.0, fade=1.0, ring=None):
    """RGB frame: disc (red) with white where shown, ghost elsewhere."""
    w = np.clip(white_shown, 0, 1)
    face = RED[None, None, :] * (1 - w[..., None]) + WHITE[None, None, :] * w[..., None]
    alpha = disc[..., None] * fade
    img = bg[None, None, :] * (1 - alpha) + face * alpha
    if ring is not None:  # (radius px, alpha)
        rr, ra = ring
        yy, xx = np.mgrid[0:SIZE, 0:SIZE].astype(np.float32) + 0.5
        dist = np.hypot(xx - SIZE / 2, yy - SIZE / 2)
        band = np.clip(1.5 - np.abs(dist - rr), 0, 1) * ra * (dist > DISC / 2)
        img = img * (1 - band[..., None]) + RED[None, None, :] * band[..., None]
    out = Image.fromarray(np.clip(img + 0.5, 0, 255).astype(np.uint8), "RGB")
    if scale != 1.0:
        s = int(SIZE * scale)
        small = out.resize((s, s), Image.LANCZOS)
        out = Image.new("RGB", (SIZE, SIZE), tuple(int(v) for v in bg))
        out.paste(small, ((SIZE - s) // 2, (SIZE - s) // 2))
    return out


def write_panel_data(out_dir, disc, white, reveal, n):
    """splash.bin: header, disc coverage, then the white share of every
    draw-on frame (frame 0 = ghost only), all as u8 maps of the disc's square.

    Header (little endian): magic "NSPS", u16 version 1, u16 size, u16 x0,
    u16 y0, u16 fps, u16 frames, u16 disc diameter, u16 reserved.
    On the panel: R = disc, G = B = disc * white / 255 over black.
    """
    x0 = (SIZE - DISC) // 2
    sl = (slice(x0, x0 + DISC), slice(x0, x0 + DISC))
    q = lambda a: np.clip(a[sl] * 255 + 0.5, 0, 255).astype(np.uint8).tobytes()
    maps = [q(GHOST * white)]
    for f in range(1, n + 1):
        t = f / n
        t = t * t * (3 - 2 * t)
        maps.append(q(GHOST * white + (1 - GHOST) * reveal.shown(t)))
    head = b"NSPS" + struct.pack("<8H", 1, DISC, x0, x0, FPS, len(maps), DISC, 0)
    (out_dir / "splash.bin").write_bytes(head + q(disc) + b"".join(maps))
    # Premultiplied RGBA of the finished logo (same pixels as the last frame).
    d, w = disc[sl], white[sl]
    rgba = np.stack([d, d * w, d * w, d], axis=-1)
    (out_dir / "logo_96.rgba").write_bytes(np.clip(rgba * 255 + 0.5, 0, 255).astype(np.uint8).tobytes())


def main():
    svg, out_dir = Path(sys.argv[1]), Path(sys.argv[2])
    out_dir.mkdir(parents=True, exist_ok=True)
    disc, white = logo_layers(svg.read_text(encoding="utf-8"))
    reveal = Reveal(white)
    frames = []
    # 1. bootloader still: disc + ghost lines
    still = compose(disc, white * GHOST, GHOST)
    frames += [still] * int(0.8 * FPS)
    # 2. draw-on
    n = int(DRAW_S * FPS)
    for f in range(1, n + 1):
        t = f / n
        t = t * t * (3 - 2 * t)                      # ease in-out
        frames.append(compose(disc, GHOST * white + (1 - GHOST) * reveal.shown(t), GHOST))
    final = compose(disc, white, 1)
    frames += [final] * int(0.3 * FPS)
    # 3. waiting for the UI: a thin ring leaves the disc every 1.6 s
    for cycle in range(2):
        for f in range(int(1.6 * FPS)):
            p = f / (1.6 * FPS)
            frames.append(compose(disc, white, 1, ring=(DISC / 2 + 2 + 0.3 * DISC * p, 0.6 * (1 - p))))
    # 4. hand-over to the UI: shrink and fade
    for f in range(1, int(0.5 * FPS) + 1):
        p = f / (0.5 * FPS)
        frames.append(compose(disc, white, 1, scale=1 - 0.2 * p, fade=1 - p))
    write_panel_data(out_dir, disc, white, reveal, n)
    final.save(out_dir / "logo_480.png")
    still.save(out_dir / "bootloader_still.png")
    if "--preview" in sys.argv:
        frames[0].save(out_dir / "preview.gif", save_all=True, append_images=frames[1:],
                       duration=int(1000 / FPS), loop=0, optimize=False)
        keys = [frames[i] for i in (0, int(0.8 * FPS) + n // 4, int(0.8 * FPS) + n // 2,
                                    int(0.8 * FPS) + 3 * n // 4, int(0.8 * FPS) + n + 3,
                                    int(0.8 * FPS) + n + int(0.3 * FPS) + 20)]
        strip = Image.new("RGB", (SIZE * len(keys) + 10 * (len(keys) - 1), SIZE), (60, 60, 60))
        for k, im in enumerate(keys):
            strip.paste(im, (k * (SIZE + 10), 0))
        strip.save(out_dir / "keyframes.png")
    err = np.abs(reveal.shown(1.0) - white).max()
    print(f"{len(frames)} frames, draw-on {n}, last draw-on frame vs logo: max difference {err:.4f}")


if __name__ == "__main__":
    main()
