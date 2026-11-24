# ponytail: one-off: the commutator as an SVG technical drawing. A tiny 3D model, projected
# orthographically. Faces facing away are culled, curved surfaces are drawn as strips and outlined
# only along their visible edges, and parts are layered back to front by hand for this one view.
# White faces with black outlines; grey on the cut sections and the mica, which the dither turns
# into hatching.
#   python3 scheme_svg.py && rsvg-convert -w 1024 scheme.svg -o scheme-svg.png
import math

N, L = 12, 0.55                   # bars, drum length
GAP = 0.06                        # radians cut from each side of a bar: the slots read at icon size
A0, A1 = 0.0, math.radians(270)   # kept arc; the lower-front quarter is cut away
STROKE = 7
WHITE, SHAFT, MICA, CUT = "#ffffff", "#d2d2d2", "#5a5a5a", "#8c8c8c"


def norm(v):
    n = math.sqrt(sum(c * c for c in v))
    return tuple(c / n for c in v)


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def dot(a, b):
    return sum(x * y for x, y in zip(a, b))


# 3/4 view: drum axis along world X, clamp ring toward the viewer (same camera as the Blender take)
CAM, TARGET = (-3.0, -3.2, 1.9), (0.3, 0.0, 0.0)
F = norm(tuple(t - c for t, c in zip(TARGET, CAM)))
R = norm(cross(F, (0, 0, 1)))
U = cross(R, F)


def vec(lx, ly, lz):
    return (lz, ly, -lx)          # model (drum axis = local Z) -> world


def pt(r, a, z):
    return vec(r * math.cos(a), r * math.sin(a), z)


def scr(p):
    return dot(p, R), -dot(p, U)


def faces_viewer(n):
    return dot(n, F) < -1e-6


LAYERS = []   # each: list of ("poly", pts2d, fill, stroked) or ("line", pts2d)


def poly(layer, pts3, fill, stroked=True):
    layer.append(("poly", [scr(p) for p in pts3], fill, stroked))


def line(layer, pts3):
    layer.append(("line", [scr(p) for p in pts3]))


def planar(layer, pts3, normal, fill):
    if faces_viewer(normal):
        poly(layer, pts3, fill)


def surface(layer, r, a0, a1, z0, z1, outward, fill, steps):
    """Cylindrical patch: visible strips filled, outline along the edges of each visible run."""
    def visible_at(t0, t1):
        m = (t0 + t1) / 2
        return faces_viewer(vec(outward * math.cos(m), outward * math.sin(m), 0))

    if abs(a1 - a0 - 2 * math.pi) < 1e-6:
        # full circle: start the sweep on a hidden strip so a visible run never splits at the seam
        d = (a1 - a0) / steps
        k = next(i for i in range(steps) if not visible_at(a0 + i * d, a0 + (i + 1) * d))
        a0, a1 = a0 + k * d, a1 + k * d
    ts = [a0 + (a1 - a0) * i / steps for i in range(steps + 1)]
    vis = [visible_at(t0, t1) for t0, t1 in zip(ts, ts[1:])]
    for (t0, t1), v in zip(zip(ts, ts[1:]), vis):
        if v:
            poly(layer, [pt(r, t0, z0), pt(r, t1, z0), pt(r, t1, z1), pt(r, t0, z1)], fill, stroked=False)
    i = 0
    while i < len(vis):
        if not vis[i]:
            i += 1
            continue
        j = i
        while j < len(vis) and vis[j]:
            j += 1
        run = ts[i:j + 1]
        line(layer, [pt(r, t, z0) for t in run])
        line(layer, [pt(r, t, z1) for t in run])
        line(layer, [pt(r, run[0], z0), pt(r, run[0], z1)])
        line(layer, [pt(r, run[-1], z0), pt(r, run[-1], z1)])
        i = j


def ring_end(layer, a0, a1, r0, r1, z, normal_z, fill, steps):
    pts = [pt(r1, a0 + (a1 - a0) * i / steps, z) for i in range(steps + 1)] + \
          [pt(r0, a1 - (a1 - a0) * i / steps, z) for i in range(steps + 1)]
    planar(layer, pts, vec(0, 0, normal_z), fill)


def solid(a0, a1, r0, r1, z0, z1, fill, steps, side_fill=None):
    layer = []
    surface(layer, r0, a0, a1, z0, z1, -1, fill, steps)                     # inner
    surface(layer, r1, a0, a1, z0, z1, +1, fill, steps)                     # outer
    for a, n in ((a0, (math.sin(a0), -math.cos(a0), 0)), (a1, (-math.sin(a1), math.cos(a1), 0))):
        planar(layer, [pt(r0, a, z0), pt(r1, a, z0), pt(r1, a, z1), pt(r0, a, z1)], vec(*n), side_fill or fill)
    ring_end(layer, a0, a1, r0, r1, z1, +1, fill, steps)                    # back end
    ring_end(layer, a0, a1, r0, r1, z0, -1, fill, steps)                    # front end
    return layer


def shaft(z0, z1):
    layer = []
    surface(layer, 0.14, 0, 2 * math.pi, z0, z1, +1, SHAFT, 64)
    planar(layer, [pt(0.14, 2 * math.pi * i / 64, z0) for i in range(64)], vec(0, 0, -1), SHAFT)   # end cap
    return layer


step = 2 * math.pi / N
LAYERS.append(solid(A0, A1, 0.16, 0.34, 0.08, L, MICA, 48, side_fill=CUT))             # core
LAYERS.append(shaft(0.0, L + 0.12))                                                      # shaft, inside
bars = []
for i in range(N):
    a = i * step
    if (a + step / 2 - A0) % (2 * math.pi) > A1 - A0:
        continue                                                                          # cut quarter
    mid = a + step / 2
    depth = dot(pt(0.5, mid, L / 2), F)
    bars.append((depth, solid(a + GAP, a + step - GAP, 0.34, 0.5, 0.1, L, WHITE, 8) +
                 solid(a + GAP, a + step - GAP, 0.5, 0.6, L - 0.12, L, WHITE, 8)))        # bar + riser (a low flange)
LAYERS += [layer for _, layer in sorted(bars, key=lambda b: -b[0])]
LAYERS.append(solid(A0, A1, 0.24, 0.42, 0.0, 0.1, WHITE, 48, side_fill=CUT))            # clamp ring
LAYERS.append(shaft(-0.18, 0.0))                                                          # shaft tip

# fit to 1024 with margins and write
xs = [x for layer in LAYERS for item in layer for x, _ in item[1]]
ys = [y for layer in LAYERS for item in layer for _, y in item[1]]
scale = 900 / max(max(xs) - min(xs), max(ys) - min(ys))
ox = 512 - (max(xs) + min(xs)) / 2 * scale
oy = 512 - (max(ys) + min(ys)) / 2 * scale
P = lambda pts: " ".join(f"{x * scale + ox:.1f},{y * scale + oy:.1f}" for x, y in pts)
out = ['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">']
for layer in LAYERS:
    for item in layer:          # fills first, then this layer's outlines on top of them
        if item[0] == "poly":
            stroke = f'stroke="#000" stroke-width="{STROKE}"' if item[3] else f'stroke="{item[2]}" stroke-width="1.5"'
            out.append(f'<polygon points="{P(item[1])}" fill="{item[2]}" {stroke} stroke-linejoin="round"/>')
    for item in layer:
        if item[0] == "line":
            out.append(f'<polyline points="{P(item[1])}" fill="none" stroke="#000" stroke-width="{STROKE}" '
                       f'stroke-linejoin="round" stroke-linecap="round"/>')
out.append("</svg>")
open("scheme.svg", "w").write("\n".join(out))
