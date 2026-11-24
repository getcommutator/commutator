# Dithers a drawing for the icon.
#   python3 dither.py drawing.png out.png [dot=4] [bg=#ffffff] [style=atkinson]
# atkinson: the classic Mac error diffusion (only 6/8 of the error is passed on, so light faces stay
#           clean and dark ones go solid), black and white. The icon uses this.
# bayer:    the wallpaper Dither effect from crates/ui/src/new_thread_background_effects.rs
#           (dither_color + 4x4 Bayer), colour-preserving.
# `dot` is the size of one dither pixel, so the pattern survives at icon size.
import sys
from PIL import Image
import numpy as np

src, dst = sys.argv[1], sys.argv[2]
dot = int(sys.argv[3]) if len(sys.argv) > 3 else 4
bg = sys.argv[4] if len(sys.argv) > 4 else "#ffffff"
style = sys.argv[5] if len(sys.argv) > 5 else "atkinson"

art = Image.open(src).convert("RGBA")
base = Image.new("RGBA", art.size, bg)
base.alpha_composite(art)
w, h = base.size

if style == "atkinson":
    g = np.asarray(base.convert("L").resize((w // dot, h // dot), Image.BOX)).astype(float) / 255
    gh, gw = g.shape
    out = np.zeros_like(g)
    for y in range(gh):
        for x in range(gw):
            new = 0.0 if g[y, x] < 0.5 else 1.0
            out[y, x] = new
            err = (g[y, x] - new) / 8
            for dx, dy in ((1, 0), (2, 0), (-1, 1), (0, 1), (1, 1), (0, 2)):
                if 0 <= x + dx < gw and y + dy < gh:
                    g[y + dy, x + dx] += err
    img = Image.fromarray((out * 255).astype("uint8")).convert("RGB")
else:
    BAYER = np.array([[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]])
    full = np.asarray(base.convert("RGB")).astype(float)
    small = full[dot // 2::dot, dot // 2::dot]            # one sample per dot, at its centre
    peak = small.max(axis=2, keepdims=True)
    gy, gx = np.indices(small.shape[:2])
    threshold = BAYER[gy % 4, gx % 4][..., None]
    bright = peak / 255 > (threshold + 0.5) / 16
    gain = np.where(bright, 255 / np.maximum(peak, 1), 0.08)
    img = Image.fromarray((small * gain).round().clip(0, 255).astype("uint8"))
img.resize((w, h), Image.NEAREST).save(dst)
