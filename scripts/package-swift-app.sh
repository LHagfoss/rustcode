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
mkdir -p "$STAGING/Contents/MacOS" "$STAGING/Contents/Helpers"
(
    cd "$ROOT/macos"
    swift build --configuration release
    cp .build/release/RustCode "$STAGING/Contents/MacOS/RustCode"
)
install -m 755 "$ENGINE" "$STAGING/Contents/Helpers/rustcode"
cat > "$STAGING/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleDevelopmentRegion</key><string>en</string>
<key>CFBundleDisplayName</key><string>RustCode</string>
<key>CFBundleExecutable</key><string>RustCode</string>
<key>CFBundleIdentifier</key><string>com.lhagfoss.rustcode.desktop</string>
<key>CFBundleName</key><string>RustCode</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$VERSION</string>
<key>CFBundleVersion</key><string>$VERSION</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF
plutil -lint "$STAGING/Contents/Info.plist"
# Ad-hoc sign inside out so the bundle is sealed. Apple Silicon refuses to run
# an unsealed quarantined bundle and reports it as damaged; sealed but
# unnotarized, Gatekeeper shows a warning the user can bypass.
codesign --force --sign - "$STAGING/Contents/Helpers/rustcode"
codesign --force --sign - "$STAGING"
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
printf 'Built ad-hoc signed %s (version %s)\n' "$APP" "$VERSION"
