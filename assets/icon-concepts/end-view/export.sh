#!/bin/sh
# Writes every platform's app icon, the welcome-screen marks and the flat logos from G4.svg
# (end view of the commutator under one light), S1.svg (its silhouette) and glyph.svg (flat
# one-colour mark). From this folder:
#   sh export.sh
# Regenerate the SVGs first with `python3 gen.py` only if the geometry changes.
set -e
cd "$(dirname "$0")"
ROOT=../../..
IOS=$ROOT/apps/ios/Commutator/Assets.xcassets/AppIcon.appiconset
DRAWABLE=$ROOT/apps/android/app/src/main/res/drawable-nodpi
# Warm paper; the silhouette gets a faint tone so the lit side of the drawing still reads.
PAPER='#EDEBE5'; TONE='#D6D3CA'; INK='#121211'
NIGHT='#151514'; NTONE='#2B2A27'; NINK='#ECEAE3'
T=$(mktemp -d)

# Masks (white = paint) at $2 px: the silhouette, and the ink: a 1-bit ordered dither of G4 at $1 px,
# scaled up nearest-neighbour and kept inside the silhouette.
masks() {  # src_px out_px
  rsvg-convert -w "$2" -h "$2" S1.svg | magick - -colorspace Gray -negate -threshold 50% "$T/sil.png"
  rsvg-convert -w "$1" -h "$1" G4.svg -o "$T/src.png"
  magick "$T/src.png" -colorspace Gray -ordered-dither o8x8 -filter point -resize "$2x$2" -negate \
    "$T/sil.png" -compose multiply -composite "$T/ink.png"
}
paint() {  # paper tone ink out
  magick -size 1024x1024 xc:"$1" xc:"$2" "$T/sil.png" -composite xc:"$3" "$T/ink.png" -composite PNG24:"$4"
}
masks 128 1024   # 8 px per dot
paint "$PAPER" "$TONE" "$INK" icon-light.png
paint "$NIGHT" "$NTONE" "$NINK" icon-dark.png

# iOS: full bleed, no alpha; tinted = light drawing on black, grayscale, the system applies the tint
cp icon-light.png "$IOS/AppIcon1024.png"
cp icon-dark.png "$IOS/AppIcon1024-dark.png"
magick icon-dark.png -colorspace gray -alpha off "$IOS/AppIcon1024-tinted.png"

# Android adaptive background: launchers mask it (down to a circle), so the drawing goes in the safe zone
magick icon-light.png -resize 64% -background "$PAPER" -gravity center -extent 1024x1024 -resize 432x432 \
  PNG24:"$DRAWABLE/ic_launcher_commutator.png"

# macOS app icon: Icon Composer layers (light + dark) that `make` compiles into Assets.car;
# the system draws the squircle, so the art is a full square with the drawing at 97% (78% of the plate, like Ask).
ICON=$ROOT/dist/macos/Commutator.icon/Assets
magick icon-light.png -resize 97% -background "$PAPER" -gravity center -extent 1024x1024 PNG24:"$ICON/art.png"
magick icon-dark.png -resize 97% -background "$NIGHT" -gravity center -extent 1024x1024 PNG24:"$ICON/art-dark.png"

# README / legacy: 824 px rounded square on the 1024 grid, transparent corners, soft drop shadow
magick "$ICON/art.png" -resize 824x824 "$T/mac.png"
magick -size 824x824 xc:black -fill white -draw "roundrectangle 0,0 823,823 185,185" "$T/mask.png"
magick "$T/mac.png" "$T/mask.png" -alpha off -compose CopyOpacity -composite "$T/mac-r.png"
magick "$T/mac-r.png" \( +clone -background black -shadow 35x14+0+10 \) +swap -background none -layers merge +repage \
  -gravity center -extent 1024x1024 "$ROOT/assets/icon.png"

# Welcome screens: the drawing as a mask the apps tint with their text colour: ink opaque, the
# silhouette's tone at 15%, the rest transparent. Dithered at half its size, so each dot is 2x2 px.
welcome() {  # size out
  masks $(($1 / 2)) "$1"
  magick "$T/sil.png" -evaluate multiply 0.15 "$T/ink.png" -compose lighten -composite "$T/a.png"
  magick -size "$1x$1" xc:black "$T/a.png" -alpha off -compose CopyOpacity -composite "$2"
}
welcome 360 "$ROOT/apps/ios/Commutator/Assets.xcassets/welcome-mark.imageset/welcome-mark.png"  # 120 pt @3x
welcome 512 "$DRAWABLE/welcome_commutator.png"                                                  # 148 dp

# Flat marks: desktop UI logo (tinted via currentColor) and Android's vector glyph (svg2vd recolours it)
sed 's/fill="#000"/fill="currentColor"/g' glyph.svg > "$ROOT/crates/ui/assets/icons/commutator-logo.svg"
cp glyph.svg "$ROOT/apps/android/icons/commutator.svg"

rm -rf "$T"
echo "icons written"
