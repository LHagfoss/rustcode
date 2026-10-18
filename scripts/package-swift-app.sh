#!/usr/bin/env bash
# Assemble the SwiftUI app with the Rust engine built by the CLI release job.
# Usage: scripts/package-swift-app.sh <rustcode-binary> [output-directory]
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
ENGINE="${1:-}"
OUTPUT="${2:-$ROOT/target/swift-app}"
[[ "$OUTPUT" = /* ]] || OUTPUT="$ROOT/$OUTPUT"
if [[ -z "$ENGINE" || ! -f "$ENGINE" || ! -x "$ENGINE" ]]; then
    echo "Usage: $0 <executable-rustcode-binary> [output-directory]" >&2
    exit 2
fi
case "$(uname -s)" in Darwin) ;; *) echo "Swift app packaging requires macOS." >&2; exit 1 ;; esac
VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
[[ -n "$VERSION" ]] || { echo "Could not read RustCode version from Cargo.toml" >&2; exit 1; }
case "$OUTPUT" in
    "$ROOT/target"|"$ROOT/target/"*) ;;
    *) echo "Output must be inside the repository target directory: $ROOT/target" >&2; exit 2 ;;
esac
APP="$OUTPUT/RustCode.app"
mkdir -p "$OUTPUT"
STAGING="$OUTPUT/.RustCode.app.$$"
trap 'rm -rf "$STAGING"' EXIT
rm -rf "$STAGING"
mkdir -p "$STAGING/Contents/MacOS" "$STAGING/Contents/Helpers" "$STAGING/Contents/Resources"
(
    cd "$ROOT/macos"
    swift build --configuration release
    cp .build/release/RustCode "$STAGING/Contents/MacOS/RustCode"
)
install -m 755 "$ENGINE" "$STAGING/Contents/Helpers/rustcode"
# Compile the Icon Composer source (Liquid Glass on macOS 26) with Xcode 26 or
# later; older toolchains fall back to the committed flat AppIcon.icns, which
# macOS 26 shows on a gray plate. REQUIRE_COMPILED_ICON=1 makes that an error.
if ! xcrun actool "$ROOT/macos/Resources/AppIcon.icon" --compile "$STAGING/Contents/Resources" \
    --app-icon AppIcon --platform macosx --minimum-deployment-target 14.0 \
    --output-partial-info-plist "$OUTPUT/.icon-info.plist.$$" >/dev/null 2>&1 \
    || [[ ! -f "$STAGING/Contents/Resources/AppIcon.icns" ]]; then
    if [[ "${REQUIRE_COMPILED_ICON:-}" == 1 ]]; then
        echo "actool could not compile AppIcon.icon; release builds need Xcode 26 or later." >&2
        exit 1
    fi
    echo "warning: actool could not compile AppIcon.icon; using the flat AppIcon.icns." >&2
    rm -f "$STAGING/Contents/Resources/Assets.car"
    cp "$ROOT/macos/Resources/AppIcon.icns" "$STAGING/Contents/Resources/AppIcon.icns"
fi
rm -f "$OUTPUT/.icon-info.plist.$$"
# NSAllowsLocalNetworking lets the localhost preview load plain-http dev servers.
cat > "$STAGING/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleDevelopmentRegion</key><string>en</string>
<key>CFBundleDisplayName</key><string>RustCode</string>
<key>CFBundleExecutable</key><string>RustCode</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>CFBundleIconName</key><string>AppIcon</string>
<key>CFBundleIdentifier</key><string>com.lhagfoss.rustcode.desktop</string>
<key>CFBundleName</key><string>RustCode</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$VERSION</string>
<key>CFBundleVersion</key><string>$VERSION</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSAppTransportSecurity</key><dict><key>NSAllowsLocalNetworking</key><true/></dict>
</dict></plist>
EOF
plutil -lint "$STAGING/Contents/Info.plist"
# Sign inside out so the bundle is sealed. With MACOS_SIGN_IDENTITY (a
# Developer ID Application identity in the keychain, set by
# scripts/macos-release-signing.sh import) both are signed for notarization:
# hardened runtime and a secure timestamp. Neither needs entitlements: the app
# is not sandboxed, and neither executable uses JIT or loads third-party
# libraries. Otherwise the bundle is ad-hoc signed: Apple Silicon refuses to
# run an unsealed quarantined bundle and reports it as damaged; sealed but
# unnotarized, Gatekeeper shows a warning the user can bypass.
if [[ -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
    SIGNING="Developer ID signed"
    SIGN_ARGS=(--force --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY")
else
    SIGNING="ad-hoc signed"
    SIGN_ARGS=(--force --sign -)
fi
codesign "${SIGN_ARGS[@]}" "$STAGING/Contents/Helpers/rustcode"
codesign "${SIGN_ARGS[@]}" "$STAGING"
codesign --verify --deep --strict "$STAGING"
rm -rf "$APP.previous"
if [[ -e "$APP" ]]; then mv "$APP" "$APP.previous"; fi
if mv "$STAGING" "$APP"; then
    rm -rf "$APP.previous"
    trap - EXIT
else
    [[ ! -e "$APP.previous" ]] || mv "$APP.previous" "$APP"
    exit 1
fi
printf 'Built %s %s (version %s)\n' "$SIGNING" "$APP" "$VERSION"
