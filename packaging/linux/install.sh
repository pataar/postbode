#!/usr/bin/env bash
# Install Postbode's desktop entry and icons, so it shows up in the app launcher.
# The postbode binary itself must already be on PATH.
#
# Usage: packaging/linux/install.sh [--prefix DIR] [--uninstall]
#   DIR defaults to ${XDG_DATA_HOME:-~/.local/share}; use /usr/local/share for all users.
set -euo pipefail

app_id=io.github.pataar.postbode
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
icons=$here/../../assets/hicolor
prefix=${XDG_DATA_HOME:-$HOME/.local/share}
uninstall=false

while [ $# -gt 0 ]; do
    case $1 in
        --prefix) prefix=${2:?--prefix needs a directory}; shift 2 ;;
        --uninstall) uninstall=true; shift ;;
        -h | --help) sed -n '2,6s/^# \{0,1\}//p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

desktop=$prefix/applications/$app_id.desktop
if $uninstall; then
    rm -f "$desktop"
    rm -f "$prefix"/icons/hicolor/*/apps/"$app_id".png "$prefix/icons/hicolor/scalable/apps/$app_id.svg"
else
    install -Dm644 "$here/$app_id.desktop" "$desktop"
    for icon in "$icons"/*/apps/"$app_id".*; do
        size=$(basename "$(dirname "$(dirname "$icon")")")
        install -Dm644 "$icon" "$prefix/icons/hicolor/$size/apps/$(basename "$icon")"
    done
fi

if command -v update-desktop-database >/dev/null; then
    update-desktop-database -q "$prefix/applications" || true
fi
if command -v gtk-update-icon-cache >/dev/null; then
    gtk-update-icon-cache -q -t "$prefix/icons/hicolor" || true
fi
echo "$($uninstall && echo Removed || echo Installed) $desktop"
