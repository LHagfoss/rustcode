#!/usr/bin/env bash
# Archive locally with existing signing. Never install, upload or update signing.
set -euo pipefail

usage() {
  printf '%s\n' 'Usage: Scripts/build-ios-release.sh' \
    'Archive the current Release app using cached signing into build/releases/v<VERSION>/.'
}

if [[ $# -gt 0 ]]; then
  if [[ $# -eq 1 && ( "$1" == '--help' || "$1" == '-h' ) ]]; then usage; exit 0; fi
  usage >&2
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_root"
support="$repo_root/Scripts/ios-release-support.py"
[[ -d RustCode.xcodeproj ]] || { printf '%s\n' 'Error: RustCode.xcodeproj is missing.' >&2; exit 1; }
for tool in xcodebuild python3 codesign; do
  command -v "$tool" >/dev/null || { printf 'Error: %s is required.\n' "$tool" >&2; exit 1; }
done

temporary="$(mktemp -d "${TMPDIR:-/tmp}/rustcode-ios-archive.XXXXXX")"
staging=''
cleanup() {
  rm -rf "$temporary"
  if [[ -n "$staging" ]]; then rm -rf "$staging"; fi
}
trap cleanup EXIT

source_fingerprint="$(python3 "$support" fingerprint "$repo_root")"
xcodebuild -project RustCode.xcodeproj -scheme RustCode -configuration Release \
  -destination 'generic/platform=iOS' -showBuildSettings -json > "$temporary/settings.json"
metadata="$(python3 "$support" metadata "$temporary/settings.json")"
{ read -r version; read -r build; read -r bundle; } <<< "$metadata"
release_dir="$repo_root/build/releases/v$version"
archive_path="$release_dir/RustCode.xcarchive"
app_path="$archive_path/Products/Applications/RustCode.app"
mkdir -p "$release_dir"
staging="$(mktemp -d "$release_dir/.archive.XXXXXX")"
staged_archive="$staging/RustCode.xcarchive"
staged_app="$staged_archive/Products/Applications/RustCode.app"

printf 'Archiving RustCode %s (build %s) with existing signing...\n' "$version" "$build"
if ! xcodebuild -quiet -project RustCode.xcodeproj -scheme RustCode -configuration Release \
  -destination 'generic/platform=iOS' -derivedDataPath "$release_dir/DerivedData" \
  -archivePath "$staged_archive" archive \
  MARKETING_VERSION="$version" CURRENT_PROJECT_VERSION="$build"; then
  printf '%s\n' 'Error: Archive failed. Check existing Xcode signing; no provisioning updates were requested.' >&2
  exit 1
fi
python3 "$support" validate-app "$staged_app" "$version" "$build" "$bundle"
codesign --verify --deep --strict "$staged_app"
if [[ "$(python3 "$support" fingerprint "$repo_root")" != "$source_fingerprint" ]]; then
  printf '%s\n' 'Error: App source or signing configuration changed during the build; rebuild before installing.' >&2
  exit 1
fi
printf '%s\n' "$source_fingerprint" > "$staged_archive/source-fingerprint.sha256"

# Preserve the previous local archive until the new one has built and verified.
backup=''
if [[ -e "$archive_path" ]]; then
  backup="$archive_path.previous.$(date +%Y%m%d%H%M%S).$$"
  mv "$archive_path" "$backup"
fi
if ! mv "$staged_archive" "$archive_path"; then
  if [[ -n "$backup" ]]; then mv "$backup" "$archive_path"; fi
  printf '%s\n' 'Error: Could not publish the verified archive locally.' >&2
  exit 1
fi
printf 'Archive: %s\nApp: %s\n' "$archive_path" "$app_path"
if [[ -n "$backup" ]]; then printf 'Previous archive retained: %s\n' "$backup"; fi
