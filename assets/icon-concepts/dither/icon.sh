#!/bin/sh
# scheme-svg.png (the drawing, transparent) -> centred, dithered, on the Zeron plates.
#   sh icon.sh [dot=4] [fill=800] [src=scheme-svg.png]
set -e
cd "$(dirname "$0")"
DOT=${1:-4}
FILL=${2:-800}
SRC=${3:-scheme-svg.png}
magick "$SRC" -colorspace gray -colorspace sRGB -trim +repage -resize ${FILL}x${FILL} \
  -background none -gravity center -extent 1024x1024 scheme-centered.png
python3 dither.py scheme-centered.png ink.png "$DOT" '#ffffff'
# light: black ink on the cream plate; dark: the same ink inverted, light on the dark plate
magick zeron-plate.png ink.png -compose multiply -composite icon-light.png
magick zeron-plate-dark.png \( ink.png -negate \) -compose screen -composite icon-dark.png
python3 - <<'EOF'
from PIL import Image
B, G = 460, 24
sheet = Image.new("RGB", (G + 2 * (B + G) + 128 + G + 64 + G + 32 + G + 16 + G, B + 2 * G), (127, 138, 148))
for i, n in enumerate(("icon-light.png", "icon-dark.png")):
    sheet.paste(Image.open(n).convert("RGB").resize((B, B), Image.LANCZOS), (G + i * (B + G), G))
img = Image.open("icon-light.png").convert("RGB")
x = G + 2 * (B + G)
for s in (128, 64, 32, 16):
    sheet.paste(img.resize((s, s), Image.LANCZOS), (x, G + (B - s) // 2))
    x += s + G
sheet.save("icon-sheet.png")
EOF
echo ok
