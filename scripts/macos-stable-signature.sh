#!/usr/bin/env bash
# Give the local rustcode binary a signature that stays the same across
# updates, so macOS Keychain stops asking for the login password after each one.
#
# A downloaded or locally built binary is ad-hoc signed: its identity is the
# hash of that exact build. Keychain grants "Always Allow" to one identity, so
# every update is a stranger and prompts again for each stored credential.
# Signing with one local certificate makes the identity
# `org.rustcode.cli` + that certificate, which survives updates.
#
# Usage: scripts/macos-stable-signature.sh [path-to-rustcode]
#
# Run it once. It creates a self-signed code-signing certificate named
# "RustCode Local Signing" in the login keychain (macOS may ask for the login
# password to store it and once more when codesign first uses it), then signs
# the binary. `rustcode update` and scripts/install.sh sign with the same
# certificate afterwards. The first run after signing prompts one last time
# per stored credential; choose "Always Allow".
#
# To undo: delete "RustCode Local Signing" in Keychain Access.
set -euo pipefail

IDENTITY="RustCode Local Signing"
IDENTIFIER="org.rustcode.cli"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "This script is only needed on macOS." >&2
    exit 1
fi

TARGET="${1:-$(command -v rustcode || true)}"
if [ -z "$TARGET" ] || [ ! -f "$TARGET" ]; then
    echo "Could not find rustcode; pass its path as the first argument." >&2
    exit 1
fi

KEYCHAIN="$(security login-keychain | sed -E 's/^[[:space:]]*"(.*)"$/\1/')"

if ! security find-certificate -c "$IDENTITY" "$KEYCHAIN" >/dev/null 2>&1; then
    WORK="$(mktemp -d)"
    trap 'rm -rf "$WORK"' EXIT
    cat >"$WORK/cert.cnf" <<CONFIG
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $IDENTITY
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CONFIG
    PASSPHRASE="$(openssl rand -hex 16)"
    openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$WORK/cert.cnf" \
        -keyout "$WORK/key.pem" -out "$WORK/cert.pem" >/dev/null 2>&1
    openssl pkcs12 -export -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
        -name "$IDENTITY" -out "$WORK/identity.p12" -passout "pass:$PASSPHRASE"
    security import "$WORK/identity.p12" -k "$KEYCHAIN" -P "$PASSPHRASE" -T /usr/bin/codesign
    echo "Created the \"$IDENTITY\" certificate in the login keychain."
fi

codesign --force --sign "$IDENTITY" --identifier "$IDENTIFIER" "$TARGET"
echo "Signed $TARGET:"
codesign -d -r- "$TARGET" 2>&1 | sed -n 's/^designated => /  /p'
