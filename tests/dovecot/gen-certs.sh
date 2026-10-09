#!/bin/sh
# Regenerates the test-only CA and the localhost certificate for the Dovecot live tests. These keys are public; never trust them elsewhere.
set -eu
cd "$(dirname "$0")"
mkdir -p certs
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
openssl req -x509 -newkey rsa:2048 -nodes -days 36500 -subj "/CN=Postvak test CA" \
  -keyout "$tmp/ca.key" -out certs/ca.pem \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign"
openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" -keyout certs/tls.key -out "$tmp/tls.csr"
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n' > "$tmp/ext"
openssl x509 -req -in "$tmp/tls.csr" -CA certs/ca.pem -CAkey "$tmp/ca.key" -set_serial 1 \
  -days 36500 -extfile "$tmp/ext" -out certs/tls.crt
# Dovecot in the container runs as uid 1000 and must read the key.
chmod 644 certs/tls.key
