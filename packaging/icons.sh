#!/usr/bin/env bash
# Render every app icon from assets/icon.svg. Run it after changing the SVG
# and commit the outputs, so builds never need resvg.
#
# Usage: packaging/icons.sh            (from anywhere in the repo)
# Needs: resvg (cargo install --locked resvg); iconutil (macOS only) for the .icns
#
# Writes:
#   assets/icon.png                     512x512 window icon, transparent background
#   assets/hicolor/<s>x<s>/apps/<id>.png  Linux icon theme sizes 16..512
#   assets/hicolor/scalable/apps/<id>.svg a copy of icon.svg
#   assets/macos.iconset/icon_*.png     Apple's icon grid: the tile is 824/1024 of
#                                       the canvas, centred, with a transparent
#                                       margin. Committed so a macOS runner can
#                                       run `iconutil -c icns` without resvg.
#   assets/postbode.icns                only where iconutil exists (macOS)
set -euo pipefail

app_id=io.github.pataar.postbode
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
assets=$root/assets
svg=$assets/icon.svg

command -v resvg >/dev/null || { echo "resvg not found: cargo install --locked resvg" >&2; exit 1; }

render() { # render <svg> <size> <out.png>
    mkdir -p "$(dirname "$3")"
    resvg --width "$2" --height "$2" "$1" "$3"
}

render "$svg" 512 "$assets/icon.png"

rm -rf "$assets/hicolor"
for s in 16 24 32 48 64 128 256 512; do
    render "$svg" "$s" "$assets/hicolor/${s}x${s}/apps/$app_id.png"
done
mkdir -p "$assets/hicolor/scalable/apps"
cp "$svg" "$assets/hicolor/scalable/apps/$app_id.svg"

# Apple grid: widen the 120-unit viewBox so the tile is 824/1024 of it.
# side = 120 * 1024 / 824 = 15360/103, margin = (side - 120) / 2 = 1500/103.
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
read -r margin side < <(awk 'BEGIN { s = 120 * 1024 / 824; printf "%.6f %.6f\n", (s - 120) / 2, s }')
viewbox="-$margin -$margin $side $side"
sed "s/viewBox=\"0 0 120 120\"/viewBox=\"$viewbox\"/" "$svg" >"$tmp/mac.svg"
grep -q "viewBox=\"$viewbox\"" "$tmp/mac.svg" || {
    echo "icon.svg has no viewBox=\"0 0 120 120\"; update the Apple grid maths" >&2
    exit 1
}

iconset=$assets/macos.iconset
rm -rf "$iconset"
for s in 16 32 128 256 512; do
    render "$tmp/mac.svg" "$s" "$iconset/icon_${s}x${s}.png"
    render "$tmp/mac.svg" $((s * 2)) "$iconset/icon_${s}x${s}@2x.png"
done

if command -v iconutil >/dev/null; then
    iconutil -c icns -o "$assets/postbode.icns" "$iconset"
else
    echo "warning: iconutil not found (macOS only); skipped assets/postbode.icns" >&2
fi
