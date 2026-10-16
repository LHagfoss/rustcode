#!/usr/bin/env bash
# Run deliberately when ready to install on the connected physical iPhone.
set -euo pipefail

usage() {
  printf '%s\n' 'Usage: Scripts/install-on-iphone.sh --local [--device NAME/ID] [--no-launch]' \
    'Install the current locally signed Release app over the existing iPhone app.' \
    'Auto-selects one connected, paired physical iPhone; builds only if its archive is missing or stale.'
}

device_selector=''
launch=yes
while [[ $# -gt 0 ]]; do
  case "$1" in
    --device)
      if [[ $# -lt 2 || -z "$2" || "$2" == --* ]]; then
        printf '%s\n' 'Error: --device needs a name or ID.' >&2; exit 2
      fi
      device_selector="$2"; shift 2 ;;
    --no-launch) launch=no; shift ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; printf 'Error: Unknown argument: %s\n' "$1" >&2; exit 2 ;;
  esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_root"
support="$repo_root/Scripts/ios-release-support.py"
[[ -d RustCode.xcodeproj ]] || { printf '%s\n' 'Error: RustCode.xcodeproj is missing.' >&2; exit 1; }
for tool in xcodebuild xcrun python3 codesign; do
  command -v "$tool" >/dev/null || { printf 'Error: %s is required.\n' "$tool" >&2; exit 1; }
done
temporary="$(mktemp -d "${TMPDIR:-/tmp}/rustcode-iphone-install.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT

# Discovery happens before any build or device mutation.
xcrun devicectl list devices --timeout 15 --json-output "$temporary/devices.json" \
  > "$temporary/device-list.log"
device_id="$(python3 "$support" select-device "$temporary/devices.json" "$device_selector")"
source_fingerprint="$(python3 "$support" fingerprint "$repo_root")"
xcodebuild -project RustCode.xcodeproj -scheme RustCode -configuration Release \
  -destination 'generic/platform=iOS' -showBuildSettings -json > "$temporary/settings.json"
metadata="$(python3 "$support" metadata "$temporary/settings.json")"
{ read -r version; read -r build; read -r bundle; } <<< "$metadata"
archive_path="$repo_root/build/releases/v$version/RustCode.xcarchive"
app_path="$archive_path/Products/Applications/RustCode.app"

if ! python3 "$support" validate-app "$app_path" "$version" "$build" "$bundle" \
    > /dev/null 2>&1 || ! python3 "$support" validate-fingerprint "$archive_path" "$source_fingerprint" \
    > /dev/null 2>&1 || ! codesign --verify --deep --strict "$app_path" > /dev/null 2>&1; then
  printf '%s\n' 'No matching signed archive; building the current Release app.'
  "$repo_root/Scripts/build-ios-release.sh"
fi
# Recheck even after the build helper, so an unsigned or stale app never reaches install.
python3 "$support" validate-app "$app_path" "$version" "$build" "$bundle"
python3 "$support" validate-fingerprint "$archive_path" "$(python3 "$support" fingerprint "$repo_root")"
codesign --verify --deep --strict "$app_path"

printf 'Installing RustCode %s (build %s) over the existing app...\n' "$version" "$build"
if ! xcrun devicectl device install app --device "$device_id" "$app_path"; then
  printf '%s\n' 'Install failed. Unlock the iPhone, keep its screen on, and reconnect it directly by USB.' \
    'If Xcode also cannot connect, restart the phone and Xcode, then try again.' \
    'The installer has not uninstalled the existing app.' >&2
  exit 1
fi
if [[ "$launch" == yes ]]; then
  xcrun devicectl device process launch --device "$device_id" "$bundle"
fi
printf 'Installed RustCode %s (build %s).\n' "$version" "$build"
