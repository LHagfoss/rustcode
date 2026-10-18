#!/usr/bin/env bash
# Developer ID signing and notarization for the macOS release assets.
# Used by .github/workflows/build.yml; see docs/macos-release-signing.md.
#
# Usage:
#   scripts/macos-release-signing.sh check
#       Exit 0 and print `enabled=true|false` (also to $GITHUB_OUTPUT) after
#       checking which signing secrets are set. Fails when only some are.
#   scripts/macos-release-signing.sh import
#       Import the Developer ID certificate into a temporary keychain and
#       export MACOS_SIGN_IDENTITY (to $GITHUB_ENV when set).
#   scripts/macos-release-signing.sh sign <binary>
#       Sign one executable with the hardened runtime and a secure timestamp.
#   scripts/macos-release-signing.sh notarize <file.zip|App.app>
#       Submit to the notary service and wait. An app is stapled and checked
#       with spctl; a zip (a bare binary) cannot be stapled and is only
#       submitted.
#   scripts/macos-release-signing.sh cleanup
#       Delete the temporary keychain.
#
# Secrets (environment): MACOS_DEVELOPER_ID_P12_BASE64,
# MACOS_DEVELOPER_ID_P12_PASSWORD, MACOS_NOTARY_APPLE_ID,
# MACOS_NOTARY_TEAM_ID, MACOS_NOTARY_PASSWORD (an app-specific password).
set -euo pipefail

SECRETS=(
    MACOS_DEVELOPER_ID_P12_BASE64
    MACOS_DEVELOPER_ID_P12_PASSWORD
    MACOS_NOTARY_APPLE_ID
    MACOS_NOTARY_TEAM_ID
    MACOS_NOTARY_PASSWORD
)
KEYCHAIN="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/rustcode-signing.keychain-db"

usage() {
    sed -n '2,24p' "$0" >&2
    exit 2
}

check() {
    local present=() missing=() name
    for name in "${SECRETS[@]}"; do
        if [[ -n "${!name:-}" ]]; then present+=("$name"); else missing+=("$name"); fi
    done
    local enabled=false
    if ((${#missing[@]} == 0)); then
        enabled=true
    elif ((${#present[@]} > 0)); then
        echo "::error::Developer ID signing is half configured; missing secrets: ${missing[*]}"
        exit 1
    else
        echo "::notice::Developer ID signing secrets are not set; macOS assets stay ad-hoc signed and are not notarized. See docs/macos-release-signing.md."
    fi
    echo "enabled=$enabled"
    if [[ -n "${GITHUB_OUTPUT:-}" ]]; then echo "enabled=$enabled" >>"$GITHUB_OUTPUT"; fi
}

require_secrets() {
    local name
    for name in "${SECRETS[@]}"; do
        [[ -n "${!name:-}" ]] || { echo "$name is not set" >&2; exit 1; }
    done
}

import() {
    require_secrets
    local password certificate
    password="$(openssl rand -base64 24)"
    certificate="$(mktemp "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/developer-id.XXXXXX")"
    printf '%s' "$MACOS_DEVELOPER_ID_P12_BASE64" | base64 --decode >"$certificate"
    security delete-keychain "$KEYCHAIN" 2>/dev/null || true
    security create-keychain -p "$password" "$KEYCHAIN"
    security set-keychain-settings -lut 21600 "$KEYCHAIN"
    security unlock-keychain -p "$password" "$KEYCHAIN"
    security import "$certificate" -k "$KEYCHAIN" -P "$MACOS_DEVELOPER_ID_P12_PASSWORD" \
        -f pkcs12 -T /usr/bin/codesign -T /usr/bin/security
    rm -f "$certificate"
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$KEYCHAIN" >/dev/null
    # Put the temporary keychain first in the search list, keeping the others.
    local existing=()
    while IFS= read -r line; do
        line="${line//\"/}"
        line="${line#"${line%%[![:space:]]*}"}"
        [[ -n "$line" ]] && existing+=("$line")
    done < <(security list-keychains -d user)
    security list-keychains -d user -s "$KEYCHAIN" ${existing[@]+"${existing[@]}"}
    local identity
    identity="$(security find-identity -v -p codesigning "$KEYCHAIN" \
        | awk -F'"' '/Developer ID Application/ {print $2; exit}')"
    [[ -n "$identity" ]] || { echo "No Developer ID Application identity in the certificate" >&2; exit 1; }
    echo "Signing identity: $identity"
    if [[ -n "${GITHUB_ENV:-}" ]]; then echo "MACOS_SIGN_IDENTITY=$identity" >>"$GITHUB_ENV"; fi
}

sign() {
    local binary="${1:?binary}"
    : "${MACOS_SIGN_IDENTITY:?run import first}"
    codesign --force --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$binary"
    codesign --verify --strict --verbose=2 "$binary"
}

notarize() {
    local target="${1:?file.zip or App.app}" upload result status id
    require_secrets
    upload="$target"
    if [[ "$target" == *.app ]]; then
        upload="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/$(basename "$target" .app)-notarize.zip"
        ditto -c -k --sequesterRsrc --keepParent "$target" "$upload"
    fi
    result="$(xcrun notarytool submit "$upload" \
        --apple-id "$MACOS_NOTARY_APPLE_ID" \
        --team-id "$MACOS_NOTARY_TEAM_ID" \
        --password "$MACOS_NOTARY_PASSWORD" \
        --wait --timeout 30m --output-format json)"
    status="$(printf '%s' "$result" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))')"
    id="$(printf '%s' "$result" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))')"
    echo "Notarization $id: $status"
    if [[ "$status" != "Accepted" ]]; then
        [[ -z "$id" ]] || xcrun notarytool log "$id" \
            --apple-id "$MACOS_NOTARY_APPLE_ID" \
            --team-id "$MACOS_NOTARY_TEAM_ID" \
            --password "$MACOS_NOTARY_PASSWORD" || true
        exit 1
    fi
    if [[ "$target" == *.app ]]; then
        rm -f "$upload"
        xcrun stapler staple "$target"
        xcrun stapler validate "$target"
        spctl --assess --type execute --verbose=4 "$target"
    fi
}

cleanup() {
    security delete-keychain "$KEYCHAIN" 2>/dev/null || true
}

case "${1:-}" in
    check) check ;;
    import) import ;;
    sign) shift; sign "$@" ;;
    notarize) shift; notarize "$@" ;;
    cleanup) cleanup ;;
    *) usage ;;
esac
