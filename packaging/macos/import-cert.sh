#!/usr/bin/env bash
# Import the Developer ID certificate into a temporary keychain on a CI runner, for package.sh.
#
# Usage: packaging/macos/import-cert.sh
#
# Environment:
#   APPLE_CERTIFICATE           base64 of the .p12 holding the "Developer ID Application" certificate and key
#   APPLE_CERTIFICATE_PASSWORD  password of that .p12
#   KEYCHAIN_PASSWORD           password for the temporary keychain (any random string)
#   GITHUB_ENV                  set by GitHub Actions; receives MACOS_SIGN_IDENTITY and MACOS_KEYCHAIN
#
# Without APPLE_CERTIFICATE it does nothing but warn, and package.sh signs ad hoc.
set -euo pipefail

warn() {
    if [ "${GITHUB_ACTIONS:-}" = true ]; then
        echo "::warning::$*"
    else
        echo "warning: $*" >&2
    fi
}

if [ -z "${APPLE_CERTIFICATE:-}" ]; then
    warn "APPLE_CERTIFICATE is not set: the app will be signed ad hoc and not notarized"
    exit 0
fi
: "${APPLE_CERTIFICATE_PASSWORD:?APPLE_CERTIFICATE_PASSWORD is required with APPLE_CERTIFICATE}"
: "${KEYCHAIN_PASSWORD:?KEYCHAIN_PASSWORD is required with APPLE_CERTIFICATE}"

dir=${RUNNER_TEMP:-$(mktemp -d)}
keychain=$dir/postbode-signing.keychain-db
cert=$dir/postbode-signing.p12
trap 'rm -f "$cert"' EXIT

printf '%s' "$APPLE_CERTIFICATE" | base64 --decode >"$cert"
security create-keychain -p "$KEYCHAIN_PASSWORD" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$keychain"
security import "$cert" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" -f pkcs12 \
    -T /usr/bin/codesign -T /usr/bin/security
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$keychain" >/dev/null
# Add it to the search list, keeping the existing keychains, so codesign finds the certificate chain.
existing=()
while IFS= read -r line; do
    line=${line#"${line%%[![:space:]]*}"}
    line=${line#\"}
    existing+=("${line%\"}")
done < <(security list-keychains -d user)
security list-keychains -d user -s "$keychain" ${existing[@]+"${existing[@]}"}

identity=$(security find-identity -v -p codesigning "$keychain" |
    awk -F'"' '/Developer ID Application/ { print $2; exit }')
if [ -z "$identity" ]; then
    echo "no Developer ID Application identity in APPLE_CERTIFICATE" >&2
    exit 1
fi
echo "signing as: $identity"

if [ -n "${GITHUB_ENV:-}" ]; then
    {
        echo "MACOS_SIGN_IDENTITY=$identity"
        echo "MACOS_KEYCHAIN=$keychain"
    } >>"$GITHUB_ENV"
else
    echo "export MACOS_SIGN_IDENTITY='$identity' MACOS_KEYCHAIN='$keychain'"
fi
