#!/usr/bin/env bash
# Build the macOS GPUI app bundle with its Icon Composer app icon.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "The native app bundle can only be built on macOS." >&2
    exit 1
fi

PROFILE=debug
case "${1:-}" in
    "") ;;
    --release)
        PROFILE=release
        shift
        ;;
    *)
        echo "Usage: scripts/build-native-app.sh [--release]" >&2
        exit 2
        ;;
esac
if [[ $# -ne 0 ]]; then
    echo "Usage: scripts/build-native-app.sh [--release]" >&2
    exit 2
fi

if ! xcrun --find actool >/dev/null 2>&1; then
    echo "Xcode with Icon Composer support is required (Xcode 26 or later)." >&2
    exit 1
fi
XCODE_VERSION="$(xcodebuild -version | sed -n 's/^Xcode \([0-9]*\).*/\1/p')"
if [[ -z "$XCODE_VERSION" || "$XCODE_VERSION" -lt 26 ]]; then
    echo "Icon Composer app bundles require Xcode 26 or later." >&2
    exit 1
fi

TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
if [[ "$TARGET_DIR" != /* ]]; then
    TARGET_DIR="$REPO_ROOT/$TARGET_DIR"
fi
APP_DIR="$TARGET_DIR/$PROFILE/RustCode.app"
ICON_SOURCE="$REPO_ROOT/images/AppIcon.icon"
VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$REPO_ROOT/crates/rustcode-app/Cargo.toml" | head -n 1)"
if [[ ! -d "$ICON_SOURCE" ]]; then
    echo "Icon Composer source missing: $ICON_SOURCE" >&2
    exit 1
fi

cd "$REPO_ROOT"
if [[ "$PROFILE" == release ]]; then
    cargo build --locked -p rustcode-app --release
else
    cargo build --locked -p rustcode-app
fi

mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
cp "$TARGET_DIR/$PROFILE/rustcode-app" "$APP_DIR/Contents/MacOS/rustcode-app"
xcrun actool "$ICON_SOURCE" \
    --compile "$APP_DIR/Contents/Resources" \
    --platform macosx \
    --target-device mac \
    --minimum-deployment-target 14.0 \
    --app-icon AppIcon \
    --output-partial-info-plist "$TARGET_DIR/$PROFILE/rustcode-app-icon.plist" \
    --output-format human-readable-text

cat > "$APP_DIR/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleDisplayName</key><string>RustCode</string>
    <key>CFBundleExecutable</key><string>rustcode-app</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundleIconName</key><string>AppIcon</string>
    <key>CFBundleIdentifier</key><string>com.lhagfoss.rustcode.app</string>
    <key>CFBundleName</key><string>RustCode</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF

touch "$APP_DIR"
echo "Built $APP_DIR"
