#!/usr/bin/env bash
# Build Postvak's AppImage from a release binary.
#
# Usage: packaging/linux/appimage.sh BINARY ARCH OUTPUT
#   ARCH is x86_64 or aarch64. Needs appimagetool (or $APPIMAGETOOL) and the AppImage runtime for ARCH in
#   $RUNTIME_FILE; without it appimagetool downloads an unpinned one.
set -euo pipefail

binary=${1:?usage: appimage.sh BINARY ARCH OUTPUT}
arch=${2:?usage: appimage.sh BINARY ARCH OUTPUT}
output=${3:?usage: appimage.sh BINARY ARCH OUTPUT}
app_id=io.github.postvak_app.postvak
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

appdir=$(mktemp -d)/Postvak.AppDir
trap 'rm -rf "$(dirname "$appdir")"' EXIT

install -Dm755 "$binary" "$appdir/usr/bin/postvak"
install -Dm755 "$here/AppRun" "$appdir/AppRun"
"$here/install.sh" --prefix "$appdir/usr/share" >/dev/null
# appimagetool wants the desktop entry and its icon at the AppDir's root.
cp "$appdir/usr/share/applications/$app_id.desktop" "$appdir/"
cp "$appdir/usr/share/icons/hicolor/256x256/apps/$app_id.png" "$appdir/"
ln -s "$app_id.png" "$appdir/.DirIcon"

args=()
if [ -n "${RUNTIME_FILE:-}" ]; then
    args+=(--runtime-file "$RUNTIME_FILE")
fi
ARCH=$arch "${APPIMAGETOOL:-appimagetool}" "${args[@]}" "$appdir" "$output"
