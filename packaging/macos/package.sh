#!/usr/bin/env bash
# Build Postvak.app and a DMG holding it; sign and notarize both when the secrets are there.
#
# Usage: packaging/macos/package.sh [--arch universal|aarch64|x86_64] [--skip-build]
#   --arch        what to build (default universal: both architectures joined with lipo)
#   --skip-build  reuse target/<triple>/release/postvak from an earlier build
#
# Runs on macOS only, with its stock bash 3.2 too (lipo, iconutil, codesign, hdiutil, xcrun). Needs the Rust targets:
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin
#
# Environment, all optional; a missing one gives a warning, never a failed build:
#   MACOS_SIGN_IDENTITY  codesign identity, e.g. "Developer ID Application: Name (TEAMID)"; default "-" (ad hoc)
#   MACOS_KEYCHAIN       keychain holding that identity (import-cert.sh sets both)
#   APPLE_ID             Apple ID for notarization
#   APPLE_PASSWORD       app-specific password of that Apple ID
#   APPLE_TEAM_ID        team id of the Developer ID certificate
#
# Writes dist/macos/Postvak.app and dist/macos/postvak-<version>-macos-<arch>.dmg, and prints the DMG's sha256.
set -euo pipefail

# Keep equal to LSMinimumSystemVersion in Info.plist.in (and the cask's depends_on macos).
export MACOSX_DEPLOYMENT_TARGET=11.0

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
here=$root/packaging/macos
out=$root/dist/macos

arch=universal
skip_build=false
while [ $# -gt 0 ]; do
    case $1 in
        --arch) arch=${2:?--arch needs a value}; shift 2 ;;
        --arch=*) arch=${1#--arch=}; shift ;;
        --skip-build) skip_build=true; shift ;;
        -h|--help) sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
case $arch in
    universal) triples=(aarch64-apple-darwin x86_64-apple-darwin) ;;
    aarch64) triples=(aarch64-apple-darwin) ;;
    x86_64) triples=(x86_64-apple-darwin) ;;
    *) echo "--arch must be universal, aarch64 or x86_64, not $arch" >&2; exit 2 ;;
esac

warn() {
    if [ "${GITHUB_ACTIONS:-}" = true ]; then
        echo "::warning::$*"
    else
        echo "warning: $*" >&2
    fi
}

[ "$(uname -s)" = Darwin ] || { echo "package.sh runs on macOS only" >&2; exit 1; }

version=$(awk -F'"' '/^\[/ { pkg = ($0 == "[package]") } pkg && /^version *=/ { print $2; exit }' "$root/Cargo.toml")
[ -n "$version" ] || { echo "no [package] version in Cargo.toml" >&2; exit 1; }
short_version=${version%%[-+]*}
build_sha=$(git -C "$root" rev-parse --short=12 HEAD 2>/dev/null || echo unknown)

# --- build -----------------------------------------------------------------------------------------------------
bins=()
for triple in "${triples[@]}"; do
    if [ "$skip_build" = false ]; then
        cargo build --manifest-path "$root/Cargo.toml" --release --locked --target "$triple"
    fi
    bin=$root/target/$triple/release/postvak
    [ -x "$bin" ] || { echo "missing $bin; build it or drop --skip-build" >&2; exit 1; }
    bins+=("$bin")
done

# --- bundle ----------------------------------------------------------------------------------------------------
app=$out/Postvak.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
if [ "${#bins[@]}" -gt 1 ]; then
    lipo -create "${bins[@]}" -output "$app/Contents/MacOS/postvak"
else
    cp "${bins[0]}" "$app/Contents/MacOS/postvak"
fi
lipo -info "$app/Contents/MacOS/postvak"
iconutil -c icns "$root/assets/macos.iconset" -o "$app/Contents/Resources/Postvak.icns"
sed -e "s/@VERSION@/$version/g" -e "s/@SHORT_VERSION@/$short_version/g" -e "s/@BUILD_SHA@/$build_sha/g" \
    "$here/Info.plist.in" >"$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
printf 'APPL????' >"$app/Contents/PkgInfo"

# --- signing ---------------------------------------------------------------------------------------------------
identity=${MACOS_SIGN_IDENTITY:--}
sign_args=(--force --options runtime)
if [ "$identity" = - ]; then
    warn "MACOS_SIGN_IDENTITY is not set: signing ad hoc; Gatekeeper will block the app on other Macs"
    sign_args+=(--timestamp=none)
else
    sign_args+=(--timestamp)
fi
keychain_args=()
if [ -n "${MACOS_KEYCHAIN:-}" ]; then
    keychain_args=(--keychain "$MACOS_KEYCHAIN")
fi
sign_args+=(--sign "$identity" ${keychain_args[@]+"${keychain_args[@]}"})

# Inside out: the executable, then the bundle. No --deep: it signs nested code with the wrong options.
codesign "${sign_args[@]}" --entitlements "$here/entitlements.plist" "$app/Contents/MacOS/postvak"
codesign "${sign_args[@]}" --entitlements "$here/entitlements.plist" "$app"
codesign --verify --strict --deep --verbose=2 "$app"

notarize=false
if [ "$identity" = - ]; then
    : # ad hoc code cannot be notarized; warned above
elif [ -z "${APPLE_ID:-}" ] || [ -z "${APPLE_PASSWORD:-}" ] || [ -z "${APPLE_TEAM_ID:-}" ]; then
    warn "APPLE_ID, APPLE_PASSWORD or APPLE_TEAM_ID is not set: skipping notarization"
else
    notarize=true
fi

# notarize <file>: submit, wait, and fail with Apple's log unless it is accepted.
notarize() {
    local file=$1 result id status
    result=$(xcrun notarytool submit "$file" \
        --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" \
        --wait --timeout 1h --output-format json)
    echo "$result"
    id=$(plutil -extract id raw -o - - <<<"$result")
    status=$(plutil -extract status raw -o - - <<<"$result")
    if [ "$status" != Accepted ]; then
        xcrun notarytool log "$id" \
            --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" || true
        echo "notarization of $(basename "$file") ended as $status" >&2
        exit 1
    fi
}

if [ "$notarize" = true ]; then
    zip=$out/Postvak.zip
    rm -f "$zip"
    ditto -c -k --keepParent "$app" "$zip"
    notarize "$zip"
    rm -f "$zip"
    xcrun stapler staple "$app"
    xcrun stapler validate "$app"
    spctl --assess --type execute --verbose=2 "$app"
fi

# --- DMG -------------------------------------------------------------------------------------------------------
dmg=$out/postvak-$version-macos-$arch.dmg
stage=$(mktemp -d)
raw=$(mktemp -u).dmg
trap 'rm -rf "$stage" "$raw"' EXIT
ditto "$app" "$stage/Postvak.app"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
# makehybrid + convert instead of `hdiutil create -srcfolder`, which fails intermittently on CI runners.
hdiutil makehybrid -hfs -hfs-volume-name Postvak -hfs-openfolder "$stage" -o "$raw" "$stage"
hdiutil convert "$raw" -format UDZO -o "$dmg"

if [ "$identity" != - ]; then
    codesign --force --timestamp --sign "$identity" ${keychain_args[@]+"${keychain_args[@]}"} "$dmg"
    codesign --verify --strict --verbose=2 "$dmg"
fi
if [ "$notarize" = true ]; then
    notarize "$dmg"
    xcrun stapler staple "$dmg"
    xcrun stapler validate "$dmg"
    spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
fi

echo "$dmg"
shasum -a 256 "$dmg"
