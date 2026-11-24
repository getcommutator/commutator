# The wallpaper Dither effect from crates/ui/src/new_thread_background_effects.rs (dither_color +
# 4x4 Bayer), with a bigger dot so the pattern survives at icon size.
#   python3 dither.py drawing.png out.png [dot=8] [bg=#0d0c0b]
import sys
from PIL import Image
import numpy as np

BAYER = np.array([[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]])
src, dst = sys.argv[1], sys.argv[2]
dot = int(sys.argv[3]) if len(sys.argv) > 3 else 8
bg = sys.argv[4] if len(sys.argv) > 4 else "#0d0c0b"

art = Image.open(src).convert("RGBA")
base = Image.new("RGBA", art.size, bg)
base.alpha_composite(art)
img = np.asarray(base.convert("RGB")).astype(float)
h, w, _ = img.shape
# one sample per dot, at its centre (as the Rust code does per 2x2 cell)
small = img[dot // 2::dot, dot // 2::dot]
peak = small.max(axis=2, keepdims=True)
gy, gx = np.indices(small.shape[:2])
threshold = BAYER[gy % 4, gx % 4][..., None]
bright = peak / 255 > (threshold + 0.5) / 16
gain = np.where(bright, 255 / np.maximum(peak, 1), 0.08)
out = (small * gain).round().clip(0, 255).astype("uint8")
Image.fromarray(out).resize((w, h), Image.NEAREST).save(dst)
