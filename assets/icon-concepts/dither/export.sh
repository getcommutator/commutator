#!/bin/sh
# Writes every platform's app icon and the welcome-screen marks from ink.png (the dithered drawing:
# black ink on white, 1024², made by icon.sh). Full rebuild, from this folder:
#   python3 scheme_svg.py && rsvg-convert -w 1024 scheme.svg -o scheme-svg.png && sh icon.sh && sh export.sh
set -e
cd "$(dirname "$0")"
ROOT=../../..
IOS=$ROOT/apps/ios/Commutator/Assets.xcassets/AppIcon.appiconset
DRAWABLE=$ROOT/apps/android/app/src/main/res/drawable-nodpi
LIGHT=zeron-plate.png
DARK=zeron-plate-dark.png
T=$(mktemp -d)

# iOS: full bleed, no alpha
magick $LIGHT ink.png -compose multiply -composite -alpha off "$IOS/AppIcon1024.png"
magick $DARK \( ink.png -negate \) -compose screen -composite -alpha off "$IOS/AppIcon1024-dark.png"
# tinted: light drawing on black, grayscale; the system applies the tint
magick ink.png -negate -colorspace gray -alpha off "$IOS/AppIcon1024-tinted.png"

# Android adaptive background: launchers mask it (down to a circle), so the drawing goes in the safe zone
magick ink.png -resize 64% -background white -gravity center -extent 1024x1024 "$T/small.png"
magick $LIGHT "$T/small.png" -compose multiply -composite -alpha off -resize 432x432 \
  "$DRAWABLE/ic_launcher_commutator.png"

# macOS: 824 px rounded square on the 1024 grid, transparent corners, soft drop shadow
magick $LIGHT ink.png -compose multiply -composite -resize 824x824 "$T/mac.png"
magick -size 824x824 xc:none -fill white -draw "roundrectangle 0,0 823,823 185,185" "$T/mask.png"
magick "$T/mac.png" "$T/mask.png" -alpha off -compose CopyOpacity -composite "$T/mac-r.png"
magick "$T/mac-r.png" \( +clone -background black -shadow 35x14+0+10 \) +swap -background none -layers merge +repage \
  -gravity center -extent 1024x1024 "$ROOT/assets/icon.png"

# Welcome screens: just the drawing, no plate, as a mask (ink opaque, the rest transparent) that the
# apps tint with their text colour. Dithered at its own size: scaling the 1024 one down blurs the dots.
welcome() {  # size dot out
  magick scheme-centered.png -resize "$1x$1" "$T/w-src.png"
  python3 dither.py "$T/w-src.png" "$T/w-ink.png" "$2" '#ffffff'
  magick -size "$1x$1" xc:black \( "$T/w-ink.png" -colorspace gray -negate \) \
    -alpha off -compose CopyOpacity -composite "$3"
}
WELCOME=$ROOT/apps/ios/Commutator/Assets.xcassets/welcome-mark.imageset
rm -rf "$WELCOME" "$ROOT/apps/android/app/src/main/res/drawable-night-nodpi"
mkdir -p "$WELCOME"
welcome 360 2 "$WELCOME/welcome-mark.png"                                        # 120 pt @3x
cat > "$WELCOME/Contents.json" <<'EOF'
{
  "images" : [ { "filename" : "welcome-mark.png", "idiom" : "universal", "scale" : "3x" } ],
  "info" : { "author" : "xcode", "version" : 1 },
  "properties" : { "template-rendering-intent" : "template" }
}
EOF
welcome 512 2 "$DRAWABLE/welcome_commutator.png"                                 # 148 dp

rm -rf "$T"
echo "icons written"
