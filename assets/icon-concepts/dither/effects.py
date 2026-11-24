# Black-and-white takes on the drawing in other styles: the app's wallpaper effects (Halftone, Ascii,
# Scanlines from crates/ui/src/new_thread_background_effects.rs, here pure, without the 60% source
# blend) and classic dithers. Each writes ink-<style>.png: black ink on white, 1024².
#   python3 effects.py [scheme-centered.png]
import sys
from PIL import Image
import numpy as np

src = sys.argv[1] if len(sys.argv) > 1 else "scheme-centered.png"
art = Image.open(src).convert("RGBA")
paper = Image.new("RGBA", art.size, "white")
paper.alpha_composite(art)
gray = np.asarray(paper.convert("L")).astype(float)       # 0 black .. 255 white
N = gray.shape[0]


def save(name, ink):          # ink: bool array, True = black
    Image.fromarray(np.where(ink, 0, 255).astype("uint8")).convert("RGB").save(f"ink-{name}.png")


def blocks(scale):            # the image at 1/scale, so a dither's dots are `scale` px wide
    return np.asarray(Image.fromarray(gray.astype("uint8")).resize((N // scale, N // scale), Image.BOX)).astype(float)


def grow(ink, scale):
    return np.kron(ink, np.ones((scale, scale), bool))


def bayer(n):
    m = np.array([[0]])
    while m.shape[0] < n:
        m = np.block([[4 * m, 4 * m + 2], [4 * m + 3, 4 * m + 1]])
    return m


def ordered(n, scale):
    g = blocks(scale)
    m = bayer(n)
    t = (m[np.indices(g.shape)[0] % n, np.indices(g.shape)[1] % n] + 0.5) / (n * n)
    return grow(g / 255 < t, scale)


def diffuse(scale, taps, divisor):
    g = blocks(scale) / 255
    h, w = g.shape
    out = np.zeros_like(g, bool)
    for y in range(h):
        for x in range(w):
            old = g[y, x]
            new = 0.0 if old < 0.5 else 1.0
            out[y, x] = new == 0.0
            err = (old - new) / divisor
            for dx, dy, k in taps:
                if 0 <= x + dx < w and y + dy < h:
                    g[y + dy, x + dx] += err * k
    return grow(out, scale)


save("bayer4", ordered(4, 4))
save("bayer8", ordered(8, 4))
save("atkinson", diffuse(4, [(1, 0, 1), (2, 0, 1), (-1, 1, 1), (0, 1, 1), (1, 1, 1), (0, 2, 1)], 8))
save("floyd", diffuse(4, [(1, 0, 7), (-1, 1, 3), (0, 1, 5), (1, 1, 1)], 16))

# Halftone: one dot per cell, its radius from the cell's darkness (the app's sqrt curve)
C = 12
g = blocks(C)
yy, xx = np.indices((C, C))
dist = np.hypot(yy - (C - 1) / 2, xx - (C - 1) / 2)
ink = np.zeros((N, N), bool)
for cy in range(g.shape[0]):
    for cx in range(g.shape[1]):
        dark = 1 - g[cy, cx] / 255
        if dark > 0.02:
            r = C / 2 * (0.3 + 0.7 * np.sqrt(dark)) * min(1, dark * 4)
            ink[cy * C:(cy + 1) * C, cx * C:(cx + 1) * C] = dist <= r
save("halftone", ink | (gray < 60))          # keep the outlines crisp: cells are wider than lines

# Ascii: the app's 5x7 glyph ramp in 6x8 cells, drawn at 2 px per glyph pixel
GLYPHS = [[0, 0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 4, 0], [0, 4, 0, 0, 4, 0, 0], [0, 0, 0, 14, 0, 0, 0],
          [0, 0, 14, 0, 14, 0, 0], [0, 4, 4, 31, 4, 4, 0], [0, 21, 14, 31, 14, 21, 0],
          [10, 10, 31, 10, 31, 10, 10], [17, 2, 4, 4, 8, 16, 17], [14, 17, 23, 21, 23, 16, 14]]
S = 2
cw, ch = 6 * S, 8 * S
ink = np.zeros((N, N), bool)
for cy in range(N // ch):
    for cx in range(N // cw):
        cell = gray[cy * ch:(cy + 1) * ch, cx * cw:(cx + 1) * cw]
        glyph = GLYPHS[int(np.sqrt(1 - cell.mean() / 255) * 9)]
        for row in range(7):
            for col in range(5):
                if glyph[row] & (1 << (4 - col)):
                    ink[cy * ch + row * S:cy * ch + (row + 1) * S, cx * cw + col * S:cx * cw + (col + 1) * S] = True
save("ascii", ink | (gray < 60))             # keep the outlines crisp

# Scanlines: the drawing thresholded, with every third (scaled) row knocked out of the ink
line = (np.indices((N, N))[0] // 3) % 3 == 0
save("scanlines", (gray < 160) & ~line | (gray < 60))
