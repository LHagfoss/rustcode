#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
cd "$scratch"
git init -q
git config user.name CI
git config user.email ci@example.test
mkdir -p rustcode/tui ios/RustCode.xcodeproj
printf '\t\t\t\tCURRENT_PROJECT_VERSION = 1000000;\n\t\t\t\tMARKETING_VERSION = 1.0.0;\n\t\t\t\tSWIFT_VERSION = 6.0;\n' > ios/RustCode.xcodeproj/project.pbxproj
printf 'version = "1.0.0"\n' > Cargo.toml
printf 'version = "1.0.0"\n' > rustcode/tui/Cargo.toml
printf 'version = "1.0.0"\n' > Cargo.lock
printf '# Changelog\n' > CHANGELOG.md
git add .
git commit -qm baseline
base="$(git rev-parse HEAD)"

printf 'version = "1.0.1"\n' > Cargo.toml
printf 'version = "1.0.1"\n' > rustcode/tui/Cargo.toml
printf 'version = "1.0.1"\n' > Cargo.lock
printf '# Changelog\n1.0.1\n' > CHANGELOG.md
printf '\t\t\t\tCURRENT_PROJECT_VERSION = 1000001;\n\t\t\t\tMARKETING_VERSION = 1.0.1;\n\t\t\t\tSWIFT_VERSION = 6.0;\n' > ios/RustCode.xcodeproj/project.pbxproj
git add .
git commit -qm release
bash "$script_dir/ci-version-only.sh" "$base"
echo 'ok - release version edits skip compiled checks'

printf 'dependency = "2"\n' >> Cargo.toml
git add .
git commit -qm dependency
if bash "$script_dir/ci-version-only.sh" "$base"; then exit 1; fi
echo 'ok - dependency edits require compiled checks'

git checkout -q HEAD~1
sed -i.bak 's/SWIFT_VERSION = 6.0/SWIFT_VERSION = 5.0/' ios/RustCode.xcodeproj/project.pbxproj
rm ios/RustCode.xcodeproj/project.pbxproj.bak
git add .
git commit -qm build-setting
if bash "$script_dir/ci-version-only.sh" "$base"; then exit 1; fi
echo 'ok - other iOS build setting edits require compiled checks'

git checkout -q HEAD~1
mkdir -p rustcode/tui/src
printf 'fn main() {}\n' > rustcode/tui/src/main.rs
git add .
git commit -qm source
if bash "$script_dir/ci-version-only.sh" "$base"; then exit 1; fi
echo 'ok - source edits require compiled checks'
