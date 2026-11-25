#!/bin/sh
# Writes every platform's app icon, the welcome-screen marks and the flat logos from G3.svg
# (grayscale end view of the commutator) and glyph.svg (flat one-colour mark). From this folder:
#   sh export.sh
# Regenerate the SVGs first with `python3 gen.py` only if the geometry changes.
set -e
cd "$(dirname "$0")"
ROOT=../../..
IOS=$ROOT/apps/ios/Commutator/Assets.xcassets/AppIcon.appiconset
DRAWABLE=$ROOT/apps/android/app/src/main/res/drawable-nodpi
# Neutral greys: the dithered ink stays in Gray colorspace, so a warm paper would collapse to grey anyway.
PAPER='#EEEEEE'; INK='#111111'; NIGHT='#121212'
T=$(mktemp -d)

# 1-bit ordered dither at $1 px, scaled up nearest-neighbour to $2 px: black ink on white.
ink() {  # src_px out_px out
  rsvg-convert -w "$1" -h "$1" G3.svg -o "$T/src.png"
  magick "$T/src.png" -colorspace Gray -ordered-dither o8x8 -filter point -resize "$2x$2" -type TrueColor "$3"
}
ink 160 1024 "$T/ink.png"   # chunky pixels: 1024/160 = 6.4 px per dot

magick "$T/ink.png" -fill "$INK" -opaque black -fill "$PAPER" -opaque white PNG24:icon-light.png
magick "$T/ink.png" -negate -fill "$PAPER" -opaque white -fill "$NIGHT" -opaque black PNG24:icon-dark.png

# iOS: full bleed, no alpha; tinted = light drawing on black, grayscale, the system applies the tint
cp icon-light.png "$IOS/AppIcon1024.png"
cp icon-dark.png "$IOS/AppIcon1024-dark.png"
magick "$T/ink.png" -negate -colorspace gray -alpha off "$IOS/AppIcon1024-tinted.png"

# Android adaptive background: launchers mask it (down to a circle), so the drawing goes in the safe zone
magick icon-light.png -resize 64% -background "$PAPER" -gravity center -extent 1024x1024 -resize 432x432 \
  PNG24:"$DRAWABLE/ic_launcher_commutator.png"

# macOS: 824 px rounded square on the 1024 grid, transparent corners, soft drop shadow
magick icon-light.png -resize 824x824 "$T/mac.png"
magick -size 824x824 xc:none -fill white -draw "roundrectangle 0,0 823,823 185,185" "$T/mask.png"
magick "$T/mac.png" "$T/mask.png" -alpha off -compose CopyOpacity -composite "$T/mac-r.png"
magick "$T/mac-r.png" \( +clone -background black -shadow 35x14+0+10 \) +swap -background none -layers merge +repage \
  -gravity center -extent 1024x1024 "$ROOT/assets/icon.png"

# Welcome screens: the drawing as a mask (ink opaque, the rest transparent) that the apps tint with
# their text colour. Dithered at half its size, so each dot is 2x2 px rather than a blurred 6.4.
welcome() {  # size out
  ink $(($1 / 2)) "$1" "$T/w.png"
  magick -size "$1x$1" xc:black \( "$T/w.png" -colorspace gray -negate \) \
    -alpha off -compose CopyOpacity -composite "$2"
}
welcome 360 "$ROOT/apps/ios/Commutator/Assets.xcassets/welcome-mark.imageset/welcome-mark.png"  # 120 pt @3x
welcome 512 "$DRAWABLE/welcome_commutator.png"                                                  # 148 dp

# Flat marks: desktop UI logo (tinted via currentColor) and Android's vector glyph (svg2vd recolours it)
sed 's/fill="#000"/fill="currentColor"/g' glyph.svg > "$ROOT/crates/ui/assets/icons/commutator-logo.svg"
cp glyph.svg "$ROOT/apps/android/icons/commutator.svg"

rm -rf "$T"
echo "icons written"
