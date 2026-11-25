import math, os

D = os.path.dirname(os.path.abspath(__file__))
S, C = 1024, 512
BG, INK, ACC = "#F4F3EE", "#111111", "#E8572A"


def wrap(body, bg=BG, rx=0):
    # rx=0: iOS masks the squircle itself
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {S} {S}">'
            f'<rect width="{S}" height="{S}" fill="{bg}"/>{body}</svg>')


def rects(n, r_in, r_out, w, fill=INK, rx=0, rot0=0):
    h = r_out - r_in
    out = []
    for i in range(n):
        a = rot0 + 360 * i / n
        out.append(f'<rect x="{C - w/2}" y="{C - r_out}" width="{w}" height="{h}" rx="{rx}" '
                   f'fill="{fill}" transform="rotate({a} {C} {C})"/>')
    return "".join(out)


def sectors(n, r_in, r_out, gap_deg, fill=INK, rot0=0):
    out = []
    for i in range(n):
        a0 = math.radians(rot0 + 360 * i / n + gap_deg / 2 - 90)
        a1 = math.radians(rot0 + 360 * (i + 1) / n - gap_deg / 2 - 90)
        p = lambda r, a: f"{C + r*math.cos(a):.2f},{C + r*math.sin(a):.2f}"
        out.append(f'<path fill="{fill}" d="M{p(r_out,a0)} A{r_out},{r_out} 0 0 1 {p(r_out,a1)} '
                   f'L{p(r_in,a1)} A{r_in},{r_in} 0 0 0 {p(r_in,a0)} Z"/>')
    return "".join(out)


def circle(r, fill=INK, stroke=None, sw=0):
    s = f' stroke="{stroke}" stroke-width="{sw}"' if stroke else ""
    return f'<circle cx="{C}" cy="{C}" r="{r}" fill="{fill}"{s}/>'


V = {
    # A: the brief literally — shaft + 12 radial rectangles at a distance
    "A": wrap(circle(110) + rects(12, 250, 390, 70)),
    # B: real commutator bars — annular sectors, thin mica gaps, shaft
    "B": wrap(sectors(12, 220, 390, 7) + circle(120)),
    # C: shaft as a ring (hollow), 8 fat bars
    "C": wrap(circle(105, "none", INK, 50) + rects(8, 230, 390, 110, rx=14)),
    # D: 16 thin rounded bars, small dot shaft
    "D": wrap(circle(80) + rects(16, 230, 400, 42, rx=21)),
    # E: dark, sectors + shaft with one live segment in accent
    "E": wrap(sectors(10, 210, 380, 9, fill="#F4F3EE") + sectors(10, 210, 380, 9, fill=ACC).split("/>")[0] + "/>"
              + circle(110, "#F4F3EE"), bg="#121212"),
    # F: with two brushes touching from the sides
    "F": wrap(sectors(12, 200, 330, 8) + circle(100)
              + f'<rect x="{C+345}" y="{C-60}" width="140" height="120" rx="10" fill="{ACC}"/>'
              + f'<rect x="{C-485}" y="{C-60}" width="140" height="120" rx="10" fill="{ACC}"/>'),
    # G: shaft + insulation ring + bars (three layers like the real cross-section)
    "G": wrap(circle(90) + circle(170, "none", INK, 22) + rects(12, 240, 390, 74, rx=8)),
    # H: minimal — 6 bars, big hub, ring around bars
    "H": wrap(circle(140) + rects(6, 245, 360, 120, rx=18) + circle(420, "none", INK, 26)),
}

for k, v in V.items():
    open(f"{D}/{k}.svg", "w").write(v)


def f_variant(n, gap):
    # ponytail: rot0 = half a step so a gap (not a bar) faces each brush — reads as switching
    return wrap(sectors(n, 200, 330, gap, rot0=180 / n) + circle(100)
                + f'<rect x="{C+345}" y="{C-60}" width="140" height="120" rx="10" fill="{ACC}"/>'
                + f'<rect x="{C-485}" y="{C-60}" width="140" height="120" rx="10" fill="{ACC}"/>')


for n, gap in [(6, 10), (8, 9), (10, 8)]:
    open(f"{D}/F{n}.svg", "w").write(f_variant(n, gap))


# no brushes, fewer bars; gap scales up a bit as bars get fewer
for n, gap in [(3, 14), (4, 12), (5, 11), (6, 10)]:
    open(f"{D}/N{n}.svg", "w").write(wrap(sectors(n, 200, 330, gap, rot0=180 / n) + circle(100)))


# F8 with straight rectangles instead of arc sectors
BR = (f'<rect x="{C+345}" y="{C-60}" width="140" height="120" rx="10" fill="{ACC}"/>'
      f'<rect x="{C-485}" y="{C-60}" width="140" height="120" rx="10" fill="{ACC}"/>')
for w in (110, 150):
    body = rects(8, 200, 330, w, rot0=22.5) + circle(100)
    open(f"{D}/R8w{w}.svg", "w").write(wrap(body))
    open(f"{D}/R8w{w}br.svg", "w").write(wrap(body + BR))


def bars(n, r_in, r_out, gap, outer="arc", inner="arc", rot0=0, fill=INK):
    """Sectors whose gaps have parallel sides (constant width `gap`), not wedge-shaped."""
    h = gap / 2
    out = []
    for i in range(n):
        t0 = math.radians(rot0 + 360 * i / n - 90)
        t1 = math.radians(rot0 + 360 * (i + 1) / n - 90)
        # side edge offset by h from radial line: at radius r its angle shifts by asin(h/r)
        a = lambda t, r, s: t + s * math.asin(h / r)
        p = lambda r, ang: f"{C + r*math.cos(ang):.2f},{C + r*math.sin(ang):.2f}"
        o0, o1 = a(t0, r_out, 1), a(t1, r_out, -1)
        i0, i1 = a(t0, r_in, 1), a(t1, r_in, -1)
        seg_o = f"A{r_out},{r_out} 0 0 1 {p(r_out, o1)}" if outer == "arc" else f"L{p(r_out, o1)}"
        seg_i = f"A{r_in},{r_in} 0 0 0 {p(r_in, i0)}" if inner == "arc" else f"L{p(r_in, i0)}"
        out.append(f'<path fill="{fill}" d="M{p(r_out, o0)} {seg_o} L{p(r_in, i1)} {seg_i} Z"/>')
    return "".join(out)


for name, kw in {"P6arc": dict(outer="arc", inner="arc"),
                 "P6hex": dict(outer="line", inner="arc"),
                 "P6hex2": dict(outer="line", inner="line")}.items():
    for g in (26, 44):
        open(f"{D}/{name}g{g}.svg", "w").write(wrap(bars(6, 200, 330, g, rot0=30, **kw) + circle(100)))


for n in (4, 6):
    open(f"{D}/Q{n}.svg", "w").write(wrap(bars(n, 200, 330, 34, rot0=180 / n) + circle(100)))


# bigger, thicker bars; bars at reduced opacity, shaft solid
for op in (1, 0.7, 0.45):
    tag = str(op).replace(".", "")
    body = (f'<g opacity="{op}">' + bars(6, 225, 410, 40, rot0=30) + '</g>' + circle(120))
    open(f"{D}/Z{tag}.svg", "w").write(wrap(body))
    open(f"{D}/Z{tag}d.svg", "w").write(wrap(
        f'<g opacity="{op}">' + bars(6, 225, 410, 40, rot0=30, fill=BG) + '</g>' + circle(120, BG), bg="#121212"))


# grayscale sources for dithering (bg white, black strokes survive the dither)
def gray(bars_fill, shaft_fill, stroke=None, defs=""):
    s = f' stroke="#000" stroke-width="{stroke}" stroke-linejoin="round"' if stroke else ""
    b = bars(6, 225, 410, 40, rot0=30, fill=bars_fill).replace('<path ', f'<path{s} ')
    c = circle(120, shaft_fill, "#000" if stroke else None, stroke or 0)
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {S} {S}">{defs}<rect width="{S}" height="{S}" fill="#fff"/>{b}{c}</svg>'

LG = ('<defs><linearGradient id="g" x1="0.15" y1="0.1" x2="0.85" y2="0.95">'
      '<stop offset="0" stop-color="#d8d8d8"/><stop offset="1" stop-color="#202020"/></linearGradient>'
      '<radialGradient id="s" cx="0.38" cy="0.35" r="0.7"><stop offset="0" stop-color="#9a9a9a"/>'
      '<stop offset="1" stop-color="#000"/></radialGradient></defs>')
open(f"{D}/G1.svg", "w").write(gray("#c8c8c8", "#555", stroke=18))   # flat tones + outline, like old icon
open(f"{D}/G2.svg", "w").write(gray("url(#g)", "url(#s)", defs=LG))  # lit gradient, no outline
open(f"{D}/G3.svg", "w").write(gray("url(#g)", "#000", stroke=18, defs=LG))  # gradient + outline


# glyph for Live Activity / macOS menu bar: single color, transparent, tight viewBox.
# fill=black works as a macOS template image (alpha only); swap to currentColor where CSS/tint applies.
open(f"{D}/glyph.svg", "w").write(
    f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{C-412} {C-412} 824 824">'
    + bars(6, 225, 410, 64, rot0=30, fill="#000") + circle(120, "#000") + '</svg>')  # wider gap: stays open at 16-22px
