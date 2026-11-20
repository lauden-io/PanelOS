#!/usr/bin/env python3
"""Pictures for the looks' menus, at their exact display size (the renderer scales without filtering):
- scanlines.png (448 x 420): the arcade screen's scanlines, 1 px of 26 % black every 3 px, transparent between; the
  screen shows the part it covers (the largest menu is 448 x 420).
usage: python menu_textures.py <out dir>   (app/ui/menu)"""
import os, sys
from PIL import Image

W, H = 448, 420


def scanlines(path):
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    px = img.load()
    for y in range(0, H, 3):
        for x in range(W):
            px[x, y] = (0, 0, 0, round(0.26 * 255))
    img.save(path, optimize=True)


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "."
    os.makedirs(out, exist_ok=True)
    scanlines(os.path.join(out, "scanlines.png"))
